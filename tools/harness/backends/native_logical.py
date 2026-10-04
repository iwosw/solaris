"""Functional native movement/look and rejected-use subset; not graphical QA."""
from __future__ import annotations

import hashlib
import json
import math
import subprocess
import uuid
from typing import Any

from .. import runtime
from ..profiles import ProfileContext
from . import native_client
from .client_probes import _server_binary
from .gameplay import cpu_usage, peak_rss_kib, plugin_directory, require_tools
from .probe_steps import ProbeSteps, item_count

REQUIRED_TOOLS = {
    "minecraft_observe", "minecraft_wait_for_play", "minecraft_disconnect",
    "minecraft_wait_for_state_change", "minecraft_send_chat", "minecraft_look",
    "minecraft_press_inputs", "minecraft_wait_ticks", "minecraft_read_block",
    "minecraft_use_item_on", "minecraft_wait_for_inventory", "minecraft_set_hotbar_slot",
}
EXCLUDES = [
    "Original playable twenty-minute natural survival, day/night mobs and restart",
    "Original regression pack and 900ms stable-block window",
    "Original replay move_by centimetres, 5cm convergence, Rust validator and screenshots",
    "GPU/window/audio evidence and authoritative tracking of each ordinary movement",
    "Direct rejected-action acknowledgement identity (not exposed by minecraft_use_item_on)",
]


def _echo(steps: ProbeSteps, label: str) -> dict[str, Any]:
    marker = f"logical {uuid.uuid4().hex}"
    steps.call("minecraft_send_chat", {"message": marker, "command": False})
    return steps.wait(lambda state: any(marker in row for row in state["recent_chat"]), label)


def _block(steps: ProbeSteps, position: tuple[int, int, int]) -> dict[str, Any]:
    return steps.call("minecraft_read_block", dict(zip(("x", "y", "z"), position)))


def _identity(block: dict[str, Any]) -> tuple[Any, Any, Any]:
    return block["block_id"], block["properties"], block["fluid"]


def _scenario(client: Any, artifact: Any, timeout: float) -> dict[str, Any]:
    with ProbeSteps(client, artifact / "logical-probe", timeout, "logical-native-subset") as steps:
        steps.report.update(evidence_domain="functional_headless_native", excludes=EXCLUDES,
                            setup="seed-81 generated world; explicit operator debug grants only",
                            movement_origin="locally_predicted_reconciled_by_server")
        initial = steps.wait(lambda state: state["player"]["health"] is not None
                             and state["player"]["on_ground"], "grounded_initial_pose")
        identity, dimension = initial["player"]["entity_uuid"], initial["dimension"]
        steps.call("minecraft_wait_ticks", {"ticks": 2})
        steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 0})
        baseline = steps.observe()
        steps.call("minecraft_press_inputs", {"keys": ["forward", "jump"], "ticks": 8})
        moved = _echo(steps, "movement_server_echo")
        dx = moved["player"]["x"] - baseline["player"]["x"]
        dz = moved["player"]["z"] - baseline["player"]["z"]
        steps.report["movement"] = {"before": baseline["player"], "after": moved["player"],
                                    "dx": dx, "dz": dz, "input_ticks": 8,
                                    "contract": "ordinary forward+jump, positive Z progress; no cm target"}
        steps.check("forward_positive_z_progress", 0.1 < dz < 4.0 and abs(dx) < 0.05)
        steps.call("minecraft_look", {"yaw_deg": 90, "pitch_deg": 0})
        steps.call("minecraft_wait_ticks", {"ticks": 4})
        looked = steps.observe()
        steps.check("look_exact_local_angles", looked["player"]["yaw"] == 90
                    and looked["player"]["pitch"] == 0)
        steps.check("movement_identity_dimension_fenced", looked["player"]["entity_uuid"] == identity
                    and looked["dimension"] == dimension)
        grounded = steps.wait(lambda state: state["player"]["on_ground"], "grounded_after_jump")
        p = grounded["player"]
        x, y, z = math.floor(p["x"]), math.floor(p["y"]) - 2, math.floor(p["z"])
        near, far = (x, y, z), (x + 8, y, z)
        steps.report["rejected_probes"] = []
        for label, item, clicked in (("occupied_dirt", "minecraft:dirt", near),
                                     ("out_of_reach_dirt", "minecraft:dirt", far),
                                     ("occupied_bucket", "minecraft:water_bucket", near)):
            target = (clicked[0], clicked[1] - 1, clicked[2])
            steps.fixture(f"debug give {item} 1 0")
            steps.call("minecraft_wait_for_inventory", {"item_id": item, "count": 1,
                                                        "timeout_seconds": steps.remaining()})
            steps.call("minecraft_set_hotbar_slot", {"slot": 0})
            held = steps.observe()
            before = [_block(steps, clicked), _block(steps, target)]
            steps.check(f"{label}_occupied_precondition", all(not b["is_air"] and not b["collision_empty"]
                                                               and b["fluid"]["empty"] for b in before))
            steps.check(f"{label}_held_precondition", held["selected_item"]["item_id"] == item
                        and held["selected_item"]["count"] == 1)
            result = steps.call("minecraft_use_item_on", {**dict(zip(("x", "y", "z"), clicked)), "face": "down"})
            # Same-connection chat echo fences processing of preceding use packets.
            # Native tick completion bounds a second read; neither is a 900ms promise.
            fenced = _echo(steps, f"{label}_server_echo")
            first = [_block(steps, clicked), _block(steps, target)]
            steps.call("minecraft_wait_ticks", {"ticks": 20})
            after = [_block(steps, clicked), _block(steps, target)]
            final = steps.observe()
            steps.check(f"{label}_received_blocks_unchanged", all(_identity(a) == _identity(b) == _identity(c)
                        for a, b, c in zip(before, first, after)))
            steps.check(f"{label}_no_fluid", all(b["fluid"]["empty"] for b in after))
            steps.check(f"{label}_held_unchanged", final["selected_item"] == held["selected_item"]
                        and item_count(final, item) == item_count(held, item))
            steps.check(f"{label}_context_fenced", final["player"]["entity_uuid"] == identity
                        and final["dimension"] == dimension)
            steps.report["rejected_probes"].append({"id": label, "clicked": clicked, "target": target,
                "use_result": result, "before": before, "after_echo": first, "after_20_ticks": after,
                "fence_state_version": fenced["state_version"], "final_state_version": final["state_version"],
                "scope": "received state stable after ordered server echo and 20 native ticks; ack identity unavailable"})
    return steps.report


def run(ctx: ProfileContext) -> dict[str, Any]:
    if getattr(ctx, "windowed", False):
        raise ValueError("logical-native is functional headless-only; --windowed is not supported")
    if ctx.client != "native" or ctx.extra_args or ctx.platform is not None:
        raise ValueError("logical-native requires --client native; only --timeout-seconds and --plugins-dir are accepted")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else 120.0
    if not math.isfinite(timeout) or not 0 < timeout <= 120:
        raise ValueError("logical-native --timeout-seconds must be finite and in (0, 120]")
    artifact = ctx.artifact_dir
    binary, plugins = _server_binary(ctx), plugin_directory(ctx)
    port = runtime.reserve_port()
    address, username = f"127.0.0.1:{port}", "HarnessLogical"
    config = artifact / "server.toml"
    config.write_text(f'''[server]
name = "logical-native-subset"
motd = "Functional native logical subset"
view_distance = 4
simulation_distance = 4
[network]
bind_address = "127.0.0.1"
port = {port}
[auth]
online_mode = false
[admin]
operators = ["{username}"]
allow_local_dev_operators = false
[plugins]
directory = {json.dumps(str(plugins))}
[data]
world_dir = {json.dumps(str(artifact / "world"))}
vanilla_data_dir = "data/vanilla"
seed = 81
[simulation]
random_tick_speed = 0
friendly_spawn_interval_ticks = 0
hostile_spawn_interval_ticks = 0
[chunk_pipeline]
worker_threads = 1
[autoscale]
enabled = false
''')
    evidence = ctx.client_evidence
    with binary.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    evidence.update(server_binary_path=str(binary), server_binary_sha256=digest, server_address=address,
                    world_dir=str(artifact / "world"), seed=81, operators=[username],
                    probe_scope="explicit cheap logical subset", graphical=False)
    server = process = client = server_log = client_log = None
    server_index = None
    try:
        argv = [str(binary), "--config", str(config), "--no-console"]
        server_index = len(ctx.commands)
        ctx.commands.append(argv)
        ctx.cwds.append(str(runtime.REPO_ROOT))
        ctx.exits.append(None)
        server_log_path = artifact / "server.log"
        ctx.logs.append(str(server_log_path))
        server_log = server_log_path.open("wb")
        server = subprocess.Popen(argv, cwd=runtime.REPO_ROOT, stdout=server_log,
                                  stderr=subprocess.STDOUT, start_new_session=True)
        runtime.wait_port(port, timeout, server, log_path=server_log_path)
        process, client_log, client = native_client.start(ctx, native_client.resolve_binary(ctx), address, username, timeout)
        client.request_timeout_seconds = 130.0
        tools = client.list_tools()
        evidence["available_tools"] = sorted(t["name"] for t in tools)
        (artifact / "live-tool-schemas.json").write_text(json.dumps(tools, indent=2) + "\n")
        require_tools(tools, REQUIRED_TOOLS, "logical-native")
        play = client.call_tool("minecraft_wait_for_play", {"timeout_seconds": timeout})
        if not play.get("in_play"):
            raise RuntimeError("logical-native client did not enter Play")
        result = _scenario(client, artifact, timeout)
        client.call_tool("minecraft_disconnect")
        disconnected = client.call_tool("minecraft_observe")
        if disconnected.get("in_play") is not False:
            raise RuntimeError("logical-native cleanup did not leave Play")
        evidence["disconnect_left_play"] = True
        return {"scenario": result, "evidence_domain": "functional_headless_native", "excludes": EXCLUDES}
    finally:
        if process is not None:
            evidence["peak_client_rss_kib"] = peak_rss_kib(process.pid)
            evidence["client_cpu"] = cpu_usage(process.pid)
        if client is not None:
            try:
                client.close()
            except Exception as error:
                evidence["mcp_close_error"] = str(error)
        runtime.stop_process(process)
        if process is not None:
            ctx.exits[evidence["command_index"]] = process.returncode
        runtime.stop_process(server, interrupt=True)
        if server is not None and server_index is not None:
            ctx.exits[server_index] = server.returncode
        for handle in (server_log, client_log):
            if handle is not None:
                handle.close()
