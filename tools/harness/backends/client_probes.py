"""Cheap MCP client probes; the broader Java compatibility default stays separate."""
from __future__ import annotations

import hashlib
import json
import math
import os
import secrets
import subprocess
import uuid
from dataclasses import replace
from pathlib import Path
from typing import Any

from .. import compatibility, runtime
from ..mcp import McpClient
from ..profiles import ProfileContext, _server_binary
from . import entities_probe, java_p5, light_wire, native_client, native_lifecycle, native_light, native_v2
from .gameplay import cpu_usage, peak_rss_kib, plugin_directory, prepare_operator_fixture, require_tools
from .probe_steps import ProbeSteps

CORE_TOOLS = frozenset({"minecraft_observe", "minecraft_wait_for_state_change", "minecraft_send_chat"})


def run_core_subset(client: Any, artifact_dir: Path, timeout: float) -> dict[str, Any]:
    with ProbeSteps(client, artifact_dir / "core-probe", timeout, "core-client-chat-subset") as steps:
        before = steps.observe()
        marker = f"Solaris core-client MCP {uuid.uuid4().hex[:12]}"
        steps.call("minecraft_send_chat", {"message": marker, "command": False})
        after = steps.wait(lambda state: any(marker in line for line in state["recent_chat"]),
                           "own_chat_echo_observed")
        steps.check("same_player_connection", before["player"]["entity_uuid"] == after["player"]["entity_uuid"])
        steps.check("same_dimension", before["dimension"] == after["dimension"])
        steps.report.update(scope="server-chat-only subset", marker=marker,
                            excludes=["Wasm plugin checks", "economy purchase/refund", "Loader-required rejection"])
    return steps.report




def run(ctx: ProfileContext, kind: str) -> dict[str, Any]:
    if kind not in {"inventory", "entities", "core-client", "native-lifecycle"}:
        raise ValueError(f"unknown shared client probe: {kind}")
    v2_scenario = ctx.scenario_id in native_v2.NATIVE_SCENARIO_IDS
    p5_scenario = ctx.scenario_id in java_p5.SCENARIO_IDS
    if p5_scenario:
        if ctx.client != "java" or kind != "entities":
            raise ValueError("java-v2-p5 scenarios require --client java and entities")
    if v2_scenario:
        if ctx.client != "native" or kind != "entities":
            raise ValueError("native-v2 scenarios require --client native and entities")
    if ctx.scenario_id in ("native-v2-p5-humanoid-armor", native_light.SCENARIO_ID,
                           "native-v2-radius-full-view") and not ctx.windowed:
        raise ValueError(f"{ctx.scenario_id} requires --windowed")
    soak_seconds = 1800.0
    check = kind in {"inventory", "core-client"} and ctx.extra_args == ["--check"]
    if kind == "native-lifecycle":
        if ctx.extra_args:
            if len(ctx.extra_args) != 2 or ctx.extra_args[0] != "--soak-seconds":
                raise ValueError("native-lifecycle accepts only --soak-seconds N")
            soak_seconds = float(ctx.extra_args[1])
        if not math.isfinite(soak_seconds) or soak_seconds < 0:
            raise ValueError("--soak-seconds must be finite and nonnegative")
    if (ctx.extra_args and kind != "native-lifecycle" and not check) or ctx.platform is not None:
        raise ValueError(f"{kind} probe accepts only --client, --timeout-seconds and --check, without Loader")
    default_timeout = (soak_seconds + 180.0 if kind == "native-lifecycle"
                       else 600.0 if ctx.scenario_id in (native_light.SCENARIO_ID,
                                                        "native-v2-radius-full-view")
                       else (180.0 if v2_scenario else 120.0))
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else default_timeout
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("probe --timeout-seconds must be finite and positive")
    startup_timeout = min(timeout, 120.0)
    required = {"inventory": compatibility.INVENTORY_TOOLS,
                "entities": entities_probe.REQUIRED_TOOLS,
                "core-client": CORE_TOOLS,
                "native-lifecycle": native_lifecycle.REQUIRED_TOOLS}[kind]
    if v2_scenario:
        required = native_v2.REQUIRED_TOOLS
    if p5_scenario:
        required = java_p5.REQUIRED_TOOLS
    artifact = ctx.artifact_dir
    plugin_dir = plugin_directory(ctx)
    binary = _server_binary(ctx)
    port = runtime.reserve_port()
    address = f"127.0.0.1:{port}"
    username = {"inventory": "HarnessInventory", "entities": "HarnessEntities",
                "core-client": "HarnessCore", "native-lifecycle": "HarnessLifecycle"}[kind]
    if ctx.scenario_id == "java-v2-p5-towns":
        username = "P5Setup"
    operators = [username] if kind != "core-client" else []
    if ctx.scenario_id == "native-v2-p5-humanoid-armor":
        operators.append("P5ArmorActor")
    config = artifact / "server.toml"
    view_distance, simulation_distance = 4, 4
    if ctx.scenario_id == "native-v2-radius-full-view":
        # G5 radius split: full client view square with a smaller sim radius.
        view_distance, simulation_distance = 8, 4
    config.write_text(f'''[server]
name = "shared-{kind}-probe"
motd = "Shared Python MCP gameplay probe"
view_distance = {view_distance}
simulation_distance = {simulation_distance}
[network]
bind_address = "127.0.0.1"
port = {port}
[auth]
online_mode = false
[plugins]
directory = {json.dumps(str(plugin_dir))}
[data]
world_dir = "{(artifact / "world").relative_to(runtime.REPO_ROOT).as_posix()}"
vanilla_data_dir = "data/vanilla"
seed = 81
''')
    prepare_operator_fixture(ctx, operators)
    with binary.open("rb") as executable:
        digest = hashlib.file_digest(executable, "sha256").hexdigest()
    evidence = ctx.client_evidence
    evidence.update(server_address=address, server_binary_path=str(binary), server_binary_sha256=digest,
                    world_dir=str(artifact / "world"), seed=81, operators=operators,
                    probe_scope=kind if kind != "core-client" else "server-chat-only subset",
                    peak_client_rss_kib=None, client_rss_source="Linux /proc/PID/status VmHWM",
                    client_cpu=None, client_cpu_source="Linux /proc/PID/stat utime+stime, starttime")
    if check:
        return {"mode": "prepared", "client_evidence": evidence}
    server = client_process = xvfb = mcp = wire_trace = None
    peer_process = peer_mcp = peer_log = None
    peer_evidence: dict[str, Any] = {}
    server_log = client_log = display_log = None
    try:
        server_log_path = artifact / "server.log"
        server_log = server_log_path.open("wb")
        server = subprocess.Popen([str(binary), "--config", str(config), "--no-console"],
                                  cwd=runtime.REPO_ROOT, stdout=server_log,
                                  stderr=subprocess.STDOUT, start_new_session=True)
        ctx.logs.append(str(server_log_path))
        runtime.wait_port(port, startup_timeout, server, log_path=server_log_path)
        if ctx.client == "native":
            display = None
            client_address = address
            if (ctx.scenario_id == "native-v2-p1-signs" and os.environ.get("SOLARIS_P1_SIGN_WIRE_TRACE") == "1"
                    or ctx.scenario_id == "native-v2-p2-fishing"
                    or ctx.scenario_id in ("native-v2-p1-armor-stand", "native-v2-p1-helmet-equip",
                                          "native-v2-p1-player-observer")):
                from .native_p1 import SignWireTrace

                wire_trace = SignWireTrace(port, artifact, fishing=ctx.scenario_id == "native-v2-p2-fishing")
                client_address = wire_trace.address
                evidence["native_wire_trace"] = str(wire_trace.path)
            elif ctx.scenario_id in (native_light.SCENARIO_ID, "native-v2-radius-full-view"):
                wire_trace = light_wire.LightWireProxy(port, artifact)
                client_address = wire_trace.address
                evidence["native_wire_trace"] = str(wire_trace.path)
            if ctx.windowed:
                xvfb, display, display_log = runtime.start_xvfb(artifact)
                ctx.logs.append(str(artifact / "xvfb.log"))
            client_process, client_log, mcp = native_client.start(
                ctx, native_client.resolve_binary(ctx), client_address, username, startup_timeout,
                display=display)
            if ctx.scenario_id == "native-v2-p2-fishing":
                mcp.p2_wire_trace = wire_trace
            if ctx.scenario_id in (native_light.SCENARIO_ID, "native-v2-radius-full-view"):
                mcp.light_wire = wire_trace
            if ctx.scenario_id == "native-v2-p1-player-observer":
                peer_ctx = replace(ctx, artifact_dir=artifact / "observer-client", client_evidence=peer_evidence)
                evidence["observer_client"] = peer_evidence
                peer_process, peer_log, peer_mcp = native_client.start(
                    peer_ctx, native_client.resolve_binary(peer_ctx), client_address, "HarnessObserver",
                    startup_timeout, display=display)
                peer_mcp.request_timeout_seconds = 130.0
                peer_mcp.call_tool("minecraft_wait_for_play", {"timeout_seconds": startup_timeout})
                mcp.p1_observer = (peer_mcp, display, client_process.pid, peer_process.pid)
                mcp.p1_wire_trace = wire_trace
                from . import native_focus

                native_focus.acquire(display, client_process.pid, startup_timeout)
            if ctx.scenario_id == "native-v2-p5-humanoid-armor":
                peer_ctx = replace(ctx, artifact_dir=artifact / "headless-actor",
                                   client_evidence=peer_evidence, windowed=False)
                evidence["headless_armor_actor"] = peer_evidence
                peer_process, peer_log, peer_mcp = native_client.start(
                    peer_ctx, native_client.resolve_binary(peer_ctx), client_address, "P5ArmorActor",
                    startup_timeout)
                peer_mcp.request_timeout_seconds = 130.0
                peer_mcp.call_tool("minecraft_wait_for_play", {"timeout_seconds": startup_timeout})
                mcp.p5_armor_actor = peer_mcp
        else:
            xvfb, display, display_log = runtime.start_xvfb(artifact)
            token, mcp_port = secrets.token_urlsafe(32), runtime.reserve_port()
            if p5_scenario:
                game_dir = artifact / "game"
                game_dir.mkdir(parents=True, exist_ok=True)
                (game_dir / "options.txt").write_text(
                    "version:4790\nonboardAccessibility:false\nguiScale:2\nrenderDistance:4\n"
                    "simulationDistance:5\nmaxFps:30\npauseOnLostFocus:false\n")
            client_process, client_log = runtime.start_client(
                game_dir=artifact / "game", log_path=artifact / "client.log", token=token,
                mcp_port=mcp_port, username=username, display=display)
            mcp = McpClient(f"http://127.0.0.1:{mcp_port}/mcp", token, startup_timeout + 10)
            mcp.initialize()
            ctx.logs.extend([str(artifact / "client.log"), str(artifact / "xvfb.log")])
        mcp.request_timeout_seconds = 130.0
        tools = mcp.list_tools()
        evidence["available_tools"] = sorted(tool["name"] for tool in tools)
        require_tools(tools, required | {"minecraft_wait_for_play", "minecraft_disconnect"}, kind)
        if ctx.client == "java":
            require_tools(tools, {"minecraft_connect"}, kind)
            runtime.wait_client_ready_for_connect(mcp, startup_timeout)
            mcp.call_tool("minecraft_connect", {"server_addr": address})
        play = mcp.call_tool("minecraft_wait_for_play", {"timeout_seconds": startup_timeout})
        if not play.get("in_play"):
            raise RuntimeError(f"{kind} client did not enter Play: {play}")
        if p5_scenario:
            def restart_p5_server(remaining: float) -> str:
                nonlocal server, server_log
                old_pid = server.pid
                runtime.stop_process(server)
                if server.returncode != 0:
                    raise RuntimeError(f"P5 server did not stop cleanly: {server.returncode}")
                server_log.close()
                server_log = server_log_path.open("ab")
                server = subprocess.Popen([str(binary), "--config", str(config), "--no-console"],
                                          cwd=runtime.REPO_ROOT, stdout=server_log,
                                          stderr=subprocess.STDOUT, start_new_session=True)
                runtime.wait_port(port, remaining, server, log_path=server_log_path)
                evidence.setdefault("server_restarts", []).append({
                    "old_pid": old_pid, "new_pid": server.pid,
                    "same_world_dir": str(artifact / "world"), "same_binary_sha256": digest,
                })
                return address

            def attach_p5_client() -> None:
                mcp.p4_restart_server = restart_p5_server
                mcp.p5_switch_identity = switch_p5_identity
                mcp.p4_window = java_p5.acquire_window(display, startup_timeout)
                evidence.setdefault("java_windows", []).append(mcp.p4_window)

            def switch_p5_identity(name: str, remaining: float) -> Any:
                nonlocal mcp, client_process, client_log
                mcp.call_tool("minecraft_disconnect")
                mcp.close()
                runtime.stop_process(client_process)
                old_returncode = client_process.returncode
                client_log.close()
                session_dir = artifact / ("identity-" + name)
                game_dir = session_dir / "game"
                game_dir.mkdir(parents=True, exist_ok=True)
                (game_dir / "options.txt").write_text(
                    "version:4790\nonboardAccessibility:false\nguiScale:2\nrenderDistance:4\n"
                    "simulationDistance:5\nmaxFps:30\npauseOnLostFocus:false\n")
                token, mcp_port = secrets.token_urlsafe(32), runtime.reserve_port()
                log_path = session_dir / "client.log"
                client_process, client_log = runtime.start_client(
                    game_dir=game_dir, log_path=log_path, token=token, mcp_port=mcp_port,
                    username=name, display=display)
                ctx.logs.append(str(log_path))
                mcp = McpClient(f"http://127.0.0.1:{mcp_port}/mcp", token, remaining + 10)
                mcp.initialize()
                mcp.request_timeout_seconds = 130.0
                require_tools(mcp.list_tools(), required | {"minecraft_connect", "minecraft_wait_for_play"}, kind)
                runtime.wait_client_ready_for_connect(mcp, remaining)
                mcp.call_tool("minecraft_connect", {"server_addr": address})
                entered = mcp.call_tool("minecraft_wait_for_play", {"timeout_seconds": remaining})
                if entered.get("in_play") is not True:
                    raise RuntimeError(f"P5 identity {name} did not enter Play: {entered}")
                evidence.setdefault("identity_switches", []).append({
                    "username": name, "previous_launcher_exit": old_returncode,
                    "new_launcher_pid": client_process.pid, "previous_stopped_before_next_start": True,
                })
                attach_p5_client()
                return mcp

            attach_p5_client()
        if kind == "inventory":
            with ProbeSteps(mcp, artifact / "inventory-fixture", startup_timeout, "inventory-operator-fixture") as setup:
                setup.fixture("debug give minecraft:apple 2 1")
                setup.call("minecraft_wait_for_inventory", {
                    "item_id": "minecraft:apple", "count": 2, "timeout_seconds": setup.remaining(),
                })
            result = compatibility.run_inventory_probe(mcp, artifact, timeout)
        elif v2_scenario:
            from . import native_p4
            mcp.p4_window = evidence.get("window_focus")

            if ctx.scenario_id in native_p4.RESTART_SCENARIOS or ctx.scenario_id == native_light.SCENARIO_ID:
                def restart_server(remaining: float) -> str:
                    nonlocal server, server_log
                    old_pid = server.pid
                    runtime.stop_process(server)
                    if server.returncode != 0:
                        raise RuntimeError(f"P4 server did not stop cleanly: {server.returncode}")
                    server_log.close()
                    server_log = server_log_path.open("ab")
                    server = subprocess.Popen([str(binary), "--config", str(config), "--no-console"],
                                              cwd=runtime.REPO_ROOT, stdout=server_log,
                                              stderr=subprocess.STDOUT, start_new_session=True)
                    runtime.wait_port(port, remaining, server, log_path=server_log_path)
                    evidence.setdefault("server_restarts", []).append({
                        "old_pid": old_pid, "new_pid": server.pid, "same_world_dir": str(artifact / "world"),
                        "same_binary_sha256": digest,
                    })
                    return address

                mcp.p4_restart_server = restart_server
            result = native_v2.run(mcp, artifact, timeout, ctx.scenario_id)
        elif p5_scenario:
            runners = {
                "java-v2-p5-towns": java_p5.run_towns,
                "java-v2-p5-parity": java_p5.run_parity,
                "java-v2-p5-cursor": java_p5.run_cursor,
            }
            result = runners[ctx.scenario_id](mcp, artifact, timeout)
        elif kind == "entities":
            result = entities_probe.run(mcp, artifact / "entities-probe", timeout)
        elif kind == "native-lifecycle":
            result = native_lifecycle.run(mcp, artifact, timeout, address, client_process.pid, soak_seconds)
        else:
            result = run_core_subset(mcp, artifact, timeout)
        if result.get("passed") is not True:
            raise RuntimeError(f"{kind} probe failed: {result}")
        if ctx.windowed and ctx.client == "native":
            evidence["capture"] = native_client.capture(mcp, artifact)
        if ctx.client == "native":
            rss = peak_rss_kib(client_process.pid)
            if rss is None:
                raise RuntimeError("native client peak RSS unavailable after probe")
            evidence["peak_client_rss_kib"] = rss
        mcp.call_tool("minecraft_disconnect")
        return {"scenario": result}
    finally:
        if ctx.client == "native" and client_process is not None:
            rss, cpu = peak_rss_kib(client_process.pid), cpu_usage(client_process.pid)
            if rss is not None:
                evidence["peak_client_rss_kib"] = rss
            if cpu is not None:
                evidence["client_cpu"] = cpu
        if peer_mcp is not None:
            try:
                peer_mcp.close()
            except Exception:
                pass
        runtime.stop_process(peer_process)
        if peer_process is not None:
            ctx.exits[peer_evidence["command_index"]] = peer_process.returncode
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        runtime.stop_process(client_process)
        if ctx.client == "native" and client_process is not None:
            ctx.exits[evidence["command_index"]] = client_process.returncode
        if wire_trace is not None:
            wire_trace.close()
        runtime.stop_process(server, interrupt=True)
        runtime.stop_process(xvfb)
        for log in (server_log, client_log, peer_log, display_log):
            if log is not None:
                log.close()
