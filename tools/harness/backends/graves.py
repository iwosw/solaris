"""Death-graves custody gate on a real server, read back from the saved world.

One tracked scenario, no hand-rolled lifecycle:

* launch the real ``solaris-v2`` with the graves package directory,
* join a real Rust protocol bot (``player-scale-bots``, unassisted login),
* console ``/give`` the bot stacks and ``/kill`` it, so native custody runs,
* wait for the package's own journaled ``grave-<x>-<y>-<z>-<hash>`` upsert,
  which names the exact grave cell,
* console ``stop`` for a clean shutdown (exit 0),
* read the *saved* state back through ``world probe``
  (``NativeReadOnlyStore``; no server start, no journal replay) and assert,
  fail-closed, that the barrel at that cell holds exactly the given stacks and
  that no copy exists in the player's stored inventory or as an item entity.

Every wait is a port readiness event, an inotify log/file condition or a
blocking process wait; nothing polls on a sleep.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
from pathlib import Path
from typing import Any

from .. import runtime
from ..profiles import ProfileContext, _run_command, _server_binary

GRAVE_REQUEST = re.compile(r"grave-(-?\d+)-(-?\d+)-(-?\d+)-[0-9a-f]{4}")
LOADED_PACKAGES = re.compile(r"loaded_packages=(\d+)")


def _check(checks: dict[str, Any], name: str, condition: bool, detail: Any = None) -> None:
    checks[name] = {"passed": bool(condition), "detail": detail}
    if not condition:
        raise RuntimeError(f"check failed: {name}: {detail!r}")


def _wait_log(
    log_path: Path,
    needle: str,
    timeout: float,
    *,
    occurrences: int = 1,
    process: subprocess.Popen[Any] | None = None,
) -> str:
    def reached() -> tuple[bool, str]:
        if process is not None and process.poll() is not None:
            raise RuntimeError(
                f"server exited {process.returncode} while waiting for {needle!r}; "
                f"log={log_path}"
            )
        text = log_path.read_text(errors="replace")
        return text.count(needle) >= occurrences, text

    text = runtime.wait_for_path_condition(log_path, timeout, reached)
    lines = [line for line in text.splitlines() if needle in line]
    return lines[occurrences - 1]


def _bot_binary() -> Path:
    override = os.environ.get("SOLARIS_BOTS_BINARY")
    candidates = (
        [Path(override).expanduser()] if override else []
    ) + [
        runtime.REPO_ROOT / "target/release/player-scale-bots",
        runtime.REPO_ROOT / "target/debug/player-scale-bots",
    ]
    for candidate in candidates:
        if candidate.is_file() and candidate.resolve().stat().st_mode & 0o111:
            return candidate.resolve()
    raise RuntimeError(
        "missing player-scale-bots executable; build it or set SOLARIS_BOTS_BINARY"
    )


def check_custody(
    checks: dict[str, Any], receipt: dict[str, Any], *, item: str, count: int
) -> None:
    """Fail-closed custody assertions over a ``world probe`` receipt.

    The barrel at the journalled grave cell must hold exactly the given stack,
    the stored player must keep no copy and no matching item entity may exist.
    """
    _check(checks, "probe_pass", receipt.get("status") == "PASS", receipt.get("error"))
    _check(checks, "probe_source_unchanged", receipt.get("source_unchanged") is True, None)
    blocks = receipt.get("blocks", [])
    _check(checks, "probe_reported_grave_cell", len(blocks) == 1, blocks)
    block = blocks[0]
    _check(
        checks, "grave_is_barrel", block.get("block") == "minecraft:barrel", block.get("block")
    )
    held = [
        {"item": stack["item"], "count": stack["count"]}
        for stack in block.get("items", [])
        if stack.get("place") == "container"
    ]
    _check(checks, "barrel_holds_exactly_given_stacks", held == [{"item": item, "count": count}], held)
    players = receipt.get("players", [])
    _check(checks, "stored_player_present", len(players) == 1, [p["uuid"] for p in players])
    carried = [
        stack
        for stack in players[0].get("items", [])
        if stack.get("place") in ("inventory", "cursor", "grid")
    ]
    _check(
        checks,
        "player_inventory_holds_no_copy",
        all(stack["item"] != item for stack in carried),
        carried,
    )
    item_entities = receipt.get("item_entities", [])
    _check(
        checks,
        "no_dropped_copy",
        all(entity.get("item") != item for entity in item_entities),
        item_entities,
    )


def run(ctx: ProfileContext) -> dict[str, Any]:
    parser = argparse.ArgumentParser(prog="graves")
    parser.add_argument("--bot-name", default="scale_0000")
    parser.add_argument("--give-item", default="minecraft:dirt")
    parser.add_argument("--give-count", type=int, default=7)
    parser.add_argument("--command-timeout-seconds", type=float, default=120.0)
    parser.add_argument("--view-distance", type=int, default=2)
    args = parser.parse_args(ctx.extra_args)
    if ctx.platform is not None:
        raise ValueError("graves runs the bare native platform without Loader")
    if ctx.plugins_dir is None:
        raise ValueError(
            "graves requires --plugins-dir pointing at the deployed graves package"
        )
    plugin_dir = ctx.plugins_dir.expanduser().resolve()
    package = plugin_dir / "graves"
    if not (package / "plugin.toml").is_file() or not (package / "plugin.wasm").is_file():
        raise ValueError(f"{package} is not a deployed graves package")
    package_sha256 = hashlib.file_digest(
        (package / "plugin.wasm").open("rb"), "sha256"
    ).hexdigest()
    if not 0 < args.give_count:
        raise ValueError("--give-count must be positive")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else 600.0
    if not timeout > 0:
        raise ValueError("timeout must be positive")

    artifact = ctx.artifact_dir
    artifact.mkdir(parents=True, exist_ok=True)
    binary = _server_binary(ctx, "server-build.log")
    bot_binary = _bot_binary()
    port = runtime.reserve_port()
    dashboard_port = runtime.reserve_port()
    world = artifact / "world"
    world.mkdir(parents=True, exist_ok=True)
    # Copy the deployed pack so the run stays reproducible and self-contained.
    # Every sibling package is kept: the gate must prove graves beside the rest
    # of the pack, not in an artificial single-package host.
    packages = artifact / "packages"
    packages.mkdir(parents=True)
    copied = []
    for child in sorted(plugin_dir.iterdir()):
        if not child.is_dir() or not (child / "plugin.toml").is_file():
            continue
        if not (child / "plugin.wasm").is_file():
            continue
        shutil.copytree(child, packages / child.name)
        copied.append(child.name)
    if "graves" not in copied:
        raise ValueError(f"{plugin_dir} holds no graves package with an encoded component")
    config_path = artifact / "graves.toml"
    log_path = artifact / "server.log"
    config_path.write_text(
        f"""[server]
name = "graves-gate"
view_distance = {args.view_distance}
simulation_distance = {args.view_distance}
pause_when_empty_seconds = 0

[network]
bind_address = "127.0.0.1"
port = {port}

[auth]
online_mode = false

[dashboard]
enabled = true
bind_address = "127.0.0.1"
port = {dashboard_port}

[plugins]
directory = {json.dumps(str(packages))}

[anti_cheat]
# The load bot's client-side ground model disagrees with the server's on a
# fresh spawn; a grave-custody gate must not be an anti-flight gate.
packet_rate_per_second = 0
packet_burst = 0
max_airtime_ticks = 0

[data]
world_dir = {json.dumps(str(world))}
vanilla_data_dir = "data/vanilla"
seed = 11
"""
    )
    checks: dict[str, Any] = {}
    report: dict[str, Any] = {
        "scope": "native death custody: real server, real protocol death, clean stop, saved-state probe",
        "server_binary": str(binary),
        "bot_binary": str(bot_binary),
        "plugins_directory": str(packages),
        "plugin_source": str(package),
        "plugin_wasm_sha256": package_sha256,
        "packages_copied": copied,
        "bot_name": args.bot_name,
        "give": {"item": args.give_item, "count": args.give_count},
        "checks": checks,
    }
    ctx.client_evidence["graves"] = report

    argv = [str(binary), "--config", str(config_path)]
    log_handle = log_path.open("wb")
    ctx.commands.append(argv)
    ctx.cwds.append(str(runtime.REPO_ROOT))
    server_exit = len(ctx.exits)
    ctx.exits.append(None)
    ctx.logs.append(str(log_path.relative_to(runtime.REPO_ROOT)))
    bot_exit = len(ctx.exits)
    bot_log_path = artifact / "bots.jsonl"
    bot_log_handle = bot_log_path.open("wb")
    server: subprocess.Popen[Any] | None = None
    bot: subprocess.Popen[Any] | None = None
    try:
        server = subprocess.Popen(
            argv,
            cwd=runtime.REPO_ROOT,
            stdin=subprocess.PIPE,
            stdout=log_handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        runtime.wait_port(port, min(timeout, 180.0), server, log_path=log_path)
        loaded = _wait_log(
            log_path, "plugin.runtime_ready", min(timeout, 120.0), process=server
        )
        report["plugin_ready_log"] = loaded
        loaded_count = LOADED_PACKAGES.search(loaded)
        report["loaded_packages"] = int(loaded_count.group(1)) if loaded_count else None
        _check(
            checks,
            "plugin_host_loaded_packages",
            loaded_count is not None and int(loaded_count.group(1)) >= 1,
            {"loaded": report["loaded_packages"], "copied": len(copied)},
        )

        bot_argv = [
            str(bot_binary),
            "--addr",
            f"127.0.0.1:{port}",
            "--clients",
            "1",
            "--view-distance",
            str(args.view_distance),
            "--unassisted-login",
            "--prepare-seconds",
            str(int(max(timeout, 600.0))),
            "--report",
            str(artifact / "bots-report.json"),
        ]
        ctx.commands.append(bot_argv)
        ctx.cwds.append(str(runtime.REPO_ROOT))
        ctx.exits.append(None)
        ctx.logs.append(str(bot_log_path.relative_to(runtime.REPO_ROOT)))
        bot = subprocess.Popen(
            bot_argv,
            cwd=runtime.REPO_ROOT,
            stdout=bot_log_handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        ready = _wait_log(bot_log_path, '"event":"ready"', min(timeout, 300.0))
        report["bot_ready"] = ready
        assert server.stdin is not None

        def console(command: str) -> None:
            assert server is not None and server.stdin is not None
            server.stdin.write(command.encode() + b"\n")
            server.stdin.flush()

        console("/list")
        roster = _wait_log(
            log_path, args.bot_name, args.command_timeout_seconds, process=server
        )
        report["roster"] = roster
        _check(checks, "player_joined_before_custody", True, roster)

        console(f"/give {args.bot_name} {args.give_item} {args.give_count}")
        give_line = _wait_log(
            log_path, "commands.give.success", args.command_timeout_seconds, process=server
        )
        report["give_log"] = give_line
        _check(
            checks,
            "given_before_death",
            f"{args.give_count}" in give_line,
            give_line,
        )

        console(f"/kill {args.bot_name}")
        commit_line = _wait_log(
            log_path, "request=grave-", args.command_timeout_seconds, process=server
        )
        report["grave_commit_log"] = commit_line
        _check(checks, "grave_upsert_committed", '"committed"' in commit_line, commit_line)
        match = GRAVE_REQUEST.search(commit_line)
        if match is None:
            raise RuntimeError(f"no grave coordinates in {commit_line!r}")
        grave = [int(match.group(index)) for index in (1, 2, 3)]
        report["grave_cell"] = grave

        # Clean shutdown: the operator console path, not a signal.
        console("stop")
        try:
            server.wait(timeout=args.command_timeout_seconds)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError(
                f"server did not exit after console stop within "
                f"{args.command_timeout_seconds}s"
            ) from error
        _check(checks, "server_clean_exit", server.returncode == 0, server.returncode)
    finally:
        if bot is not None:
            runtime.stop_process(bot)
            ctx.exits[bot_exit] = bot.returncode
        if server is not None:
            runtime.stop_process(server, interrupt=True)
            ctx.exits[server_exit] = server.returncode
            if server.stdin is not None:
                server.stdin.close()
        log_handle.close()
        bot_log_handle.close()

    if server is None or server.returncode != 0:
        raise RuntimeError("server did not stop cleanly; saved state is not evidence")

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
        ",".join(str(axis) for axis in grave),
    ]
    try:
        probe_result = _run_command(probe_argv, ctx, "probe.log")
    except RuntimeError as error:
        probe_result = {"failed": str(error)}
    report["probe"] = probe_result
    if not receipt_path.is_file():
        raise RuntimeError(f"probe wrote no receipt at {receipt_path}")
    receipt = json.loads(receipt_path.read_text())
    report["probe_receipt"] = str(receipt_path)
    check_custody(checks, receipt, item=args.give_item, count=args.give_count)
    report["grave_block"] = receipt["blocks"][0]
    report["stored_player"] = receipt["players"][0]
    report["item_entities"] = receipt["item_entities"]
    return report
