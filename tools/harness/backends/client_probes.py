"""Cheap MCP client probes; the broader Java compatibility default stays separate."""
from __future__ import annotations

import hashlib
import json
import math
import os
import secrets
import subprocess
import uuid
from pathlib import Path
from typing import Any

from .. import compatibility, runtime
from ..mcp import McpClient
from ..profiles import ProfileContext, _run_command
from . import entities_probe, native_client, native_lifecycle
from .gameplay import cpu_usage, peak_rss_kib, plugin_directory, require_tools
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


def _server_binary(ctx: ProfileContext) -> Path:
    override = os.environ.get("SOLARIS_SERVER_BINARY")
    if override:
        binary = Path(override).expanduser().resolve()
    else:
        env = dict(os.environ, CARGO_PROFILE_DEV_DEBUG="0", CARGO_PROFILE_TEST_DEBUG="0")
        _run_command(["cargo", "build", "--locked", "-j", "4", "--bin", "mc-server"],
                     ctx, "server-build.log", env=env)
        target = Path(env.get("CARGO_TARGET_DIR", str(runtime.REPO_ROOT / "target")))
        binary = (target if target.is_absolute() else runtime.REPO_ROOT / target) / "debug" / "mc-server"
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError(f"missing executable server binary: {binary}")
    return binary


def run(ctx: ProfileContext, kind: str) -> dict[str, Any]:
    if kind not in {"inventory", "entities", "core-client", "native-lifecycle"}:
        raise ValueError(f"unknown shared client probe: {kind}")
    soak_seconds = 1800.0
    if kind == "native-lifecycle":
        if ctx.client != "native":
            raise ValueError("native-lifecycle requires --client native")
        if ctx.extra_args:
            if len(ctx.extra_args) != 2 or ctx.extra_args[0] != "--soak-seconds":
                raise ValueError("native-lifecycle accepts only --soak-seconds N")
            soak_seconds = float(ctx.extra_args[1])
        if not math.isfinite(soak_seconds) or soak_seconds < 0:
            raise ValueError("--soak-seconds must be finite and nonnegative")
    if (ctx.extra_args and kind != "native-lifecycle") or ctx.platform is not None:
        raise ValueError(f"{kind} probe accepts only --client and --timeout-seconds, without Loader")
    default_timeout = soak_seconds + 180.0 if kind == "native-lifecycle" else 120.0
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else default_timeout
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("probe --timeout-seconds must be finite and positive")
    startup_timeout = min(timeout, 120.0)
    required = {"inventory": compatibility.INVENTORY_TOOLS,
                "entities": entities_probe.REQUIRED_TOOLS,
                "core-client": CORE_TOOLS,
                "native-lifecycle": native_lifecycle.REQUIRED_TOOLS}[kind]
    artifact = ctx.artifact_dir
    plugin_dir = plugin_directory(ctx)
    binary = _server_binary(ctx)
    port = runtime.reserve_port()
    address = f"127.0.0.1:{port}"
    username = {"inventory": "HarnessInventory", "entities": "HarnessEntities",
                "core-client": "HarnessCore", "native-lifecycle": "HarnessLifecycle"}[kind]
    operators = [username] if kind != "core-client" else []
    config = artifact / "server.toml"
    config.write_text(f'''[server]
name = "shared-{kind}-probe"
motd = "Shared Python MCP gameplay probe"
view_distance = 4
simulation_distance = 4
[network]
bind_address = "127.0.0.1"
port = {port}
[auth]
online_mode = false
[admin]
operators = {operators!r}
allow_local_dev_operators = false
[plugins]
directory = {json.dumps(str(plugin_dir))}
[data]
world_dir = "{(artifact / "world").relative_to(runtime.REPO_ROOT).as_posix()}"
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
    with binary.open("rb") as executable:
        digest = hashlib.file_digest(executable, "sha256").hexdigest()
    evidence = ctx.client_evidence
    evidence.update(server_address=address, server_binary_path=str(binary), server_binary_sha256=digest,
                    world_dir=str(artifact / "world"), seed=81, operators=operators,
                    probe_scope=kind if kind != "core-client" else "server-chat-only subset",
                    peak_client_rss_kib=None, client_rss_source="Linux /proc/PID/status VmHWM",
                    client_cpu=None, client_cpu_source="Linux /proc/PID/stat utime+stime, starttime")
    server = client_process = xvfb = mcp = None
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
            if ctx.windowed:
                xvfb, display, display_log = runtime.start_xvfb(artifact)
                ctx.logs.append(str(artifact / "xvfb.log"))
            client_process, client_log, mcp = native_client.start(
                ctx, native_client.resolve_binary(ctx), address, username, startup_timeout,
                display=display)
        else:
            xvfb, display, display_log = runtime.start_xvfb(artifact)
            token, mcp_port = secrets.token_urlsafe(32), runtime.reserve_port()
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
        if kind == "inventory":
            with ProbeSteps(mcp, artifact / "inventory-fixture", startup_timeout, "inventory-operator-fixture") as setup:
                setup.fixture("debug give minecraft:apple 2 1")
                setup.call("minecraft_wait_for_inventory", {
                    "item_id": "minecraft:apple", "count": 2, "timeout_seconds": setup.remaining(),
                })
            result = compatibility.run_inventory_probe(mcp, artifact, timeout)
        elif kind == "entities":
            result = entities_probe.run(mcp, artifact / "entities-probe", timeout)
        elif kind == "native-lifecycle":
            result = native_lifecycle.run(mcp, artifact, timeout, address, client_process.pid, soak_seconds)
        else:
            result = run_core_subset(mcp, artifact, timeout)
        if result.get("passed") is not True:
            raise RuntimeError(f"{kind} probe failed: {result}")
        if ctx.windowed:
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
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        runtime.stop_process(client_process)
        if ctx.client == "native" and client_process is not None:
            ctx.exits[evidence["command_index"]] = client_process.returncode
        runtime.stop_process(server, interrupt=True)
        runtime.stop_process(xvfb)
        for log in (server_log, client_log, display_log):
            if log is not None:
                log.close()
