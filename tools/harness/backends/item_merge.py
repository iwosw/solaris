"""Live item-entity merge and hopper transfer gate.

The fixture drives a real server console plus a real offline 26.1.2 protocol
client. It asserts the public effects rather than relying on internal state:
identical loose items become one entity, then a hopper transfers that stack to
a chest whose persisted container contents are observed through the read-only
``world probe`` command after a clean console stop.
"""

from __future__ import annotations

import argparse
import json
import math
import struct
import subprocess
import time
from pathlib import Path
from typing import Any

from .. import runtime, wire
from ..profiles import ProfileContext

def _check(checks: dict[str, Any], name: str, condition: bool, detail: Any) -> None:
    checks[name] = {"passed": bool(condition), "detail": detail}
    if not condition:
        raise RuntimeError(f"check failed: {name}: {detail!r}")


def _wait_log(log_path: Path, needle: str, occurrences: int, timeout: float) -> str:
    """Block for one exact server event recorded in its operator log."""

    def reached() -> tuple[bool, str]:
        text = log_path.read_text(errors="replace")
        return text.count(needle) >= occurrences, text

    text = runtime.wait_for_path_condition(log_path, timeout, reached)
    return [line for line in text.splitlines() if needle in line][occurrences - 1]


def _console(server: subprocess.Popen[Any], command: str) -> None:
    if server.stdin is None:
        raise RuntimeError("server console is unavailable")
    server.stdin.write((command + "\n").encode())
    server.stdin.flush()


def _sprint(server: subprocess.Popen[Any], log_path: Path, ticks: int, timeout: float) -> str:
    """Advance an exact simulation budget, waiting for its logged completion."""
    completed = log_path.read_text(errors="replace").count("tick sprint finished")
    _console(server, f"tick sprint {ticks}")
    return _wait_log(log_path, "tick sprint finished", completed + 1, timeout)

def _setblock(
    server: subprocess.Popen[Any],
    log_path: Path,
    command: str,
    timeout: float,
) -> str:
    """Issue one placement and fail closed on its exact console result."""
    before = len(log_path.read_text(errors="replace").splitlines())
    _console(server, command)

    def replied() -> tuple[bool, str]:
        text = log_path.read_text(errors="replace")
        fresh = text.splitlines()[before:]
        return (
            any(
                "Changed 1 block(s)" in line
                or "Could not set" in line
                or "Position is not loaded" in line
                for line in fresh
            ),
            text,
        )

    text = runtime.wait_for_path_condition(log_path, timeout, replied)
    answers = text.splitlines()[before:]
    negative = next(
        (line for line in answers if "Could not set" in line or "Position is not loaded" in line),
        None,
    )
    if negative is not None:
        raise RuntimeError(f"setblock refused {command!r}: {negative}")
    changed = next((line for line in answers if "Changed 1 block(s)" in line), None)
    if changed is None:
        raise RuntimeError(f"setblock produced no changed-block answer: {command!r}")
    return changed

def _initial_position(sock: Any, timeout: float) -> tuple[float, float, float]:
    """Read the initial position correction and acknowledge its teleport id."""
    deadline = time.monotonic() + timeout
    frames: list[int] = []
    while time.monotonic() < deadline:
        packet_id, body = wire.read_compressed_frame(sock)
        frames.append(packet_id)
        if packet_id != 0x48:
            continue
        teleport_id, offset = wire.read_varint_bytes(body)
        if len(body) < offset + 24:
            raise RuntimeError("truncated initial position packet")
        x, y, z = struct.unpack_from(">ddd", body, offset)
        wire.send_compressed_frame(sock, 0x00, wire.write_varint(teleport_id))
        return x, y, z
    raise TimeoutError(f"no initial player position; frames={frames!r}")




def run(ctx: ProfileContext) -> dict[str, Any]:
    parser = argparse.ArgumentParser(prog="item-merge")
    parser.add_argument("--items", type=int, default=2)
    parser.add_argument("--command-timeout-seconds", type=float, default=30.0)
    args = parser.parse_args(ctx.extra_args)
    if not 2 <= args.items <= 64:
        raise ValueError("--items must be in 2..=64")
    if ctx.platform is not None or ctx.plugins_dir is not None:
        raise ValueError("item-merge runs the bare server without Loader/plugins")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else 180.0
    if timeout <= 0:
        raise ValueError("timeout must be positive")

    artifact = ctx.artifact_dir
    artifact.mkdir(parents=True, exist_ok=True)
    from ..profiles import _run_command, _server_binary

    binary = _server_binary(ctx, "server-build.log")
    port = runtime.reserve_port()
    world = artifact / "world"
    world.mkdir(parents=True, exist_ok=True)
    config_path = artifact / "item-merge.toml"
    log_path = artifact / "server.log"
    config_path.write_text(
        f'''[server]
name = "item-merge-gate"
view_distance = 2
simulation_distance = 2

[network]
bind_address = "127.0.0.1"
port = {port}

[auth]
online_mode = false


[data]
world_dir = {json.dumps(str(world))}
vanilla_data_dir = "data/vanilla"
seed = 81
'''
    )
    argv = [str(binary), "--config", str(config_path)]
    log_handle = log_path.open("wb")
    ctx.commands.append(argv)
    ctx.cwds.append(str(runtime.REPO_ROOT))
    ctx.exits.append(None)
    ctx.logs.append(str(log_path.relative_to(runtime.REPO_ROOT)))
    server = subprocess.Popen(
        argv,
        cwd=runtime.REPO_ROOT,
        stdin=subprocess.PIPE,
        stdout=log_handle,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    checks: dict[str, Any] = {}
    report: dict[str, Any] = {
        "scope": "live item merge followed by hopper-to-chest transfer",
        "server_binary": str(binary),
        "server_port": port,
        "items_spawned": args.items,
        "checks": checks,
    }
    ctx.client_evidence["item_merge"] = report
    sock = None
    server_stopped = False
    try:
        runtime.wait_port(port, min(timeout, 120.0), server, log_path=log_path)
        _console(server, "gamerule doMobSpawning false")
        report["initial_sprint"] = _sprint(
            server, log_path, 40, args.command_timeout_seconds
        )
        sock, joined = wire.join_offline_socket(
            ("127.0.0.1", port), "ItemMergeProbe", timeout=args.command_timeout_seconds
        )
        report["join"] = joined
        player_x, player_y, player_z = _initial_position(sock, args.command_timeout_seconds)
        report["post_join_sprint"] = _sprint(
            server, log_path, 1, args.command_timeout_seconds
        )
        block_x = math.floor(player_x)
        floor_y = math.floor(player_y) - 1
        block_z = math.floor(player_z)
        chest_x = block_x + 1
        chest_y = floor_y
        report["player_position"] = [player_x, player_y, player_z]
        report["fixture_blocks"] = {
            "hopper_x": block_x,
            "chest_x": chest_x,
            "floor_y": floor_y,
            "z": block_z,
        }

        report["chest_setblock"] = _setblock(
            server,
            log_path,
            f"setblock {chest_x} {chest_y} {block_z} minecraft:chest",
            args.command_timeout_seconds,
        )
        for _ in range(args.items):
            _console(
                server,
                f"summon minecraft:item {block_x + 0.5} {floor_y + 1.1} {block_z + 0.5} "
                "{Item:{id:\"minecraft:cobblestone\",count:1},PickupDelay:32767}",
            )
            _sprint(server, log_path, 1, args.command_timeout_seconds)
        report["merge_sprint"] = _sprint(
            server, log_path, 160, args.command_timeout_seconds
        )
        sock.close()
        sock = None
        _console(server, "stop")
        try:
            server.wait(timeout=args.command_timeout_seconds)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError("server did not exit after merge observation") from error
        server_stopped = True
        log_handle.close()
        ctx.exits[-1] = server.returncode
        _check(
            checks,
            "merge_observation_server_clean_exit",
            server.returncode == 0,
            server.returncode,
        )

        merge_receipt_path = artifact / "merge-probe.json"
        merge_probe_argv = [
            str(binary),
            "--config",
            str(config_path),
            "world",
            "probe",
            "--source",
            str(world),
            "--receipt",
            str(merge_receipt_path),
            "--block",
            f"{block_x},{floor_y},{block_z}",
        ]
        report["merge_probe"] = _run_command(merge_probe_argv, ctx, "merge-probe.log")
        if not merge_receipt_path.is_file():
            raise RuntimeError(f"merge probe wrote no receipt at {merge_receipt_path}")
        merge_receipt = json.loads(merge_receipt_path.read_text())
        report["merge_probe_receipt"] = str(merge_receipt_path)
        _check(
            checks,
            "merge_probe_pass",
            merge_receipt.get("status") == "PASS",
            merge_receipt.get("error"),
        )
        merged_items = [
            {"item": entity.get("item"), "count": entity.get("count")}
            for entity in merge_receipt.get("item_entities", [])
        ]
        _check(
            checks,
            "identical_items_merge_to_one_entity",
            merged_items == [{"item": "minecraft:cobblestone", "count": args.items}],
            merged_items,
        )

        log_handle = log_path.open("ab")
        ctx.commands.append(argv)
        ctx.cwds.append(str(runtime.REPO_ROOT))
        ctx.exits.append(None)
        ctx.logs.append(str(log_path.relative_to(runtime.REPO_ROOT)))
        server = subprocess.Popen(
            argv,
            cwd=runtime.REPO_ROOT,
            stdin=subprocess.PIPE,
            stdout=log_handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        server_stopped = False
        runtime.wait_port(port, min(timeout, 120.0), server, log_path=log_path)
        sock, rejoined = wire.join_offline_socket(
            ("127.0.0.1", port), "ItemMergeProbe", timeout=args.command_timeout_seconds
        )
        report["rejoin"] = rejoined
        _initial_position(sock, args.command_timeout_seconds)
        report["post_rejoin_sprint"] = _sprint(
            server, log_path, 1, args.command_timeout_seconds
        )
        report["hopper_setblock"] = _setblock(
            server,
            log_path,
            f"setblock {block_x} {floor_y} {block_z} minecraft:hopper[facing=east]",
            args.command_timeout_seconds,
        )
        report["hopper_sprint"] = _sprint(
            server, log_path, 40, args.command_timeout_seconds
        )

    finally:
        if sock is not None:
            sock.close()
        if not server_stopped:
            _console(server, "stop")
            try:
                server.wait(timeout=args.command_timeout_seconds)
            except subprocess.TimeoutExpired as error:
                raise RuntimeError("server did not exit after console stop") from error
        log_handle.close()
        ctx.exits[-1] = server.returncode
    _check(checks, "server_clean_exit", server.returncode == 0, server.returncode)

    receipt_path = artifact / "probe.json"
    probe_argv = [
        str(binary),
        "--config",
        str(config_path),
        "world",
        "probe",
        "--source",
        str(world),
        "--receipt",
        str(receipt_path),
        "--block",
        f"{chest_x},{chest_y},{block_z}",
    ]
    report["probe"] = _run_command(probe_argv, ctx, "probe.log")
    if not receipt_path.is_file():
        raise RuntimeError(f"probe wrote no receipt at {receipt_path}")
    receipt = json.loads(receipt_path.read_text())
    report["probe_receipt"] = str(receipt_path)
    _check(checks, "probe_pass", receipt.get("status") == "PASS", receipt.get("error"))
    _check(checks, "probe_source_unchanged", receipt.get("source_unchanged") is True, None)
    blocks = receipt.get("blocks", [])
    _check(checks, "probe_reported_chest_cell", len(blocks) == 1, blocks)
    chest = blocks[0]
    report["chest"] = chest
    _check(checks, "probe_found_chest", chest.get("block") == "minecraft:chest", chest)
    held = [
        {"item": stack["item"], "count": stack["count"]}
        for stack in chest.get("items", [])
        if stack.get("place") == "container"
    ]
    _check(
        checks,
        "hopper_transfers_full_merged_stack",
        held == [{"item": "minecraft:cobblestone", "count": args.items}],
        held,
    )
    return report
