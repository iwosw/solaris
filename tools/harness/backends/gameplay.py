"""Shared server and MCP lifecycle for Python-driven client gameplay."""
from __future__ import annotations

import hashlib
import json
import os
import secrets
import subprocess
import uuid
from pathlib import Path
from typing import Any

from .. import runtime
from ..mcp import McpClient
from ..profiles import ProfileContext
from . import survival


def require_tools(tools: list[dict[str, Any]], required: set[str], scenario: str) -> None:
    missing = sorted(required - {tool["name"] for tool in tools})
    if missing:
        raise RuntimeError(f"{scenario}: missing MCP tool(s): {', '.join(missing)}")


def peak_rss_kib(pid: int) -> int | None:
    """The kernel's lifetime resident high-water mark, not a sampled estimate."""
    try:
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith("VmHWM:"):
                return int(line.split()[1])
    except (OSError, ValueError):
        pass
    return None


def cpu_usage(pid: int) -> dict[str, float] | None:
    """Lifetime user+system CPU and process age from /proc, read once (no sampling)."""
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
        uptime = float(Path("/proc/uptime").read_text().split()[0])
    except (OSError, ValueError):
        return None
    fields = stat[stat.rindex(")") + 2:].split()  # fields[0] is stat field 3 (state)
    ticks = os.sysconf("SC_CLK_TCK")
    cpu = (int(fields[11]) + int(fields[12])) / ticks  # utime (14) + stime (15)
    age = uptime - int(fields[19]) / ticks  # starttime (22)
    return {"cpu_seconds": round(cpu, 3), "age_seconds": round(age, 3),
            "average_cores": round(cpu / age, 3) if age > 0 else 0.0}


def plugin_directory(ctx: ProfileContext) -> Path:
    """Use an explicit candidate read-only, or create the isolated empty default."""
    if ctx.plugins_dir is None:
        directory = ctx.artifact_dir / "plugins"
        directory.mkdir()
    else:
        directory = ctx.plugins_dir.expanduser().resolve()
        if not directory.is_dir():
            raise ValueError(f"--plugins-dir is not an existing directory: {directory}")
    ctx.client_evidence["plugins_directory"] = str(directory)
    return directory


def prepare_operator_fixture(ctx: ProfileContext, names: list[str]) -> None:
    """Write vanilla access lists beside the solaris-v2 config."""
    entries = [{
        "uuid": str(uuid.UUID(bytes=hashlib.md5(f"OfflinePlayer:{name}".encode(),
                                               usedforsecurity=False).digest(), version=3)),
        "name": name, "level": 4, "bypassesPlayerLimit": False,
    } for name in names]
    path = ctx.artifact_dir / "ops.json"
    path.write_text(json.dumps(entries, indent=2) + "\n")
    ctx.client_evidence.update(operator_fixture_path=str(path), operator_fixture=entries)


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.extra_args or ctx.platform is not None:
        raise ValueError("survival accepts only --client and --timeout-seconds (debug, no Loader)")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else 120.0
    if not 0 < timeout <= 120:
        raise ValueError("survival --timeout-seconds must be in (0, 120]")
    artifact = ctx.artifact_dir
    plugin_dir = plugin_directory(ctx)
    from .client_probes import _server_binary

    binary = _server_binary(ctx)
    server_port = runtime.reserve_port()
    address = f"127.0.0.1:{server_port}"
    config = artifact / "playable.toml"
    source = (runtime.REPO_ROOT / "playable.toml").read_text()
    operators = ["HarnessSurvival"] if ctx.windowed else []
    replacements = (
        ("port = 25565", f"port = {server_port}"),
        ('world_dir = ".analysis/test-world-v12"',
         f'world_dir = "{(artifact / "world").relative_to(runtime.REPO_ROOT).as_posix()}"'),
        ('directory = "plugins"',
         f"directory = {json.dumps(str(plugin_dir))}"),
        ("seed = 0", "seed = 81"),
    )
    for old, new in replacements:
        if source.count(old) != 1:
            raise RuntimeError(f"expected one survival config token: {old}")
        source = source.replace(old, new, 1)
    config.write_text(source)
    prepare_operator_fixture(ctx, operators)
    evidence = ctx.client_evidence
    with binary.open("rb") as server_executable:
        server_hash = hashlib.file_digest(server_executable, "sha256").hexdigest()
    evidence.update(server_address=address, server_binary_path=str(binary),
                    server_binary_sha256=server_hash,
                    world_dir=str(artifact / "world"), seed=81, operators=operators,
                    peak_client_rss_kib=None, client_rss_source="Linux /proc/PID/status VmHWM",
                    client_cpu=None, client_cpu_source="Linux /proc/PID/stat utime+stime, starttime")
    server = client_process = xvfb = mcp = None
    server_log = client_log = display_log = None
    try:
        server_log = (artifact / "server.log").open("wb")
        server = subprocess.Popen([str(binary), "--config", str(config), "--no-console"],
                                  cwd=runtime.REPO_ROOT, stdout=server_log,
                                  stderr=subprocess.STDOUT, start_new_session=True)
        ctx.logs.append(str(artifact / "server.log"))
        runtime.wait_port(server_port, timeout, server, log_path=artifact / "server.log")
        if ctx.client == "native":
            from . import native_client

            display = None
            if ctx.windowed:
                xvfb, display, display_log = runtime.start_xvfb(artifact)
                ctx.logs.append(str(artifact / "xvfb.log"))
            native_binary = native_client.resolve_binary(ctx)
            client_process, client_log, mcp = native_client.start(
                ctx, native_binary, address, "HarnessSurvival", timeout, display=display)
        else:
            xvfb, display, display_log = runtime.start_xvfb(artifact)
            token = secrets.token_urlsafe(32)
            mcp_port = runtime.reserve_port()
            client_process, client_log = runtime.start_client(
                game_dir=artifact / "game", log_path=artifact / "client.log",
                token=token, mcp_port=mcp_port, username="HarnessSurvival", display=display)
            mcp = McpClient(f"http://127.0.0.1:{mcp_port}/mcp", token,
                            request_timeout_seconds=timeout + 10)
            mcp.initialize()
        # Startup's remaining deadline must not become the gameplay HTTP budget.
        mcp.request_timeout_seconds = timeout + 10
        tools = mcp.list_tools()
        evidence["available_tools"] = sorted(tool["name"] for tool in tools)
        require_tools(tools, survival.REQUIRED_TOOLS | {"minecraft_wait_for_play", "minecraft_disconnect"},
                      "survival")
        if ctx.client == "java":
            require_tools(tools, {"minecraft_connect", "minecraft_wait_for_state_change"}, "survival")
            runtime.wait_client_ready_for_connect(mcp, timeout)
            mcp.call_tool("minecraft_connect", {"server_addr": address})
        play = mcp.call_tool("minecraft_wait_for_play", {"timeout_seconds": timeout})
        if not play.get("in_play"):
            raise RuntimeError(f"survival client did not enter Play: {play}")
        result = survival.run(mcp, artifact, timeout)
        if result.get("passed") is not True:
            raise RuntimeError(f"survival scenario failed: {result}")
        if ctx.windowed:
            evidence["capture"] = native_client.capture(mcp, artifact)
            # Human-playthrough step screenshots on the same live session
            # (title N/A: harness always passes --server; HUD/inventory/chat/
            # death+Respawn/final). Failure here fails the run.
            from . import native_playthrough

            evidence["playthrough"] = native_playthrough.run(
                mcp, artifact, timeout, owned_window=evidence["window_focus"])
        if ctx.client == "native":
            rss = peak_rss_kib(client_process.pid)
            if rss is None:
                raise RuntimeError("native client peak RSS unavailable after gameplay")
            evidence["peak_client_rss_kib"] = rss
            evidence["client_cpu"] = cpu_usage(client_process.pid)
        mcp.call_tool("minecraft_disconnect")
        return {"scenario": result}
    finally:
        if ctx.client == "native" and client_process is not None:
            rss = peak_rss_kib(client_process.pid)
            if rss is not None:
                evidence["peak_client_rss_kib"] = rss
            cpu = cpu_usage(client_process.pid)
            if cpu is not None:
                evidence["client_cpu"] = cpu
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        runtime.stop_process(client_process)
        if ctx.client == "native" and client_process is not None:
            ctx.exits[ctx.client_evidence["command_index"]] = client_process.returncode
        runtime.stop_process(server, interrupt=True)
        runtime.stop_process(xvfb)
        for log in (server_log, client_log, display_log):
            if log is not None:
                log.close()
