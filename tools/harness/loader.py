#!/usr/bin/env python3
"""Real Solaris Loader compatibility gate for one client platform.

The caller creates ``artifact_dir`` and supplies an immutable solaris-v2
candidate and plugin deployment. This module owns lifecycle and cleanup, never the
canonical top-level ``result.json``.
"""

from __future__ import annotations

import json
import subprocess
import time
import tomllib
import hashlib
import os
from pathlib import Path
from typing import Any

from . import runtime
from .mcp import McpClient

REPO_ROOT = runtime.REPO_ROOT
SERVER_ADDRESS = "127.0.0.1:25567"
CONFIRMATION_TITLES = (
    f"Allow Solaris content from {SERVER_ADDRESS}?",
    "Allow Solaris content from localhost:25567?",
)
PLATFORMS = ("fabric", "neoforge", "forge")
USERNAMES = {"fabric": "GateFabric", "neoforge": "GateNeoForge", "forge": "GateForge"}


def confirm_loader_permission(client: Any, timeout_seconds: float) -> str:
    deadline = time.monotonic() + timeout_seconds
    last_error: Exception | None = None
    observed: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        for title in CONFIRMATION_TITLES:
            try:
                client.call_tool(
                    "minecraft_click_confirmation_button",
                    {"expected_title": title, "button_label": "Allow"},
                )
                return title
            except Exception as error:
                last_error = error
        if observed is None:
            observed = runtime.current_client_state(client, min(timeout_seconds, 120.0))
        else:
            observed = runtime.next_client_state(client, observed, deadline)
    raise RuntimeError(
        f"minecraft_click_confirmation_button did not succeed within {timeout_seconds:.1f}s: {last_error}"
    )


def capture_ui(client: Any, artifact_dir: Path, name: str) -> dict[str, Any]:
    observed = runtime.current_client_state(client, 30.0)
    deadline = time.monotonic() + 30.0
    while observed["overlay"]["open"]:
        observed = runtime.next_client_state(client, observed, deadline)
    client.call_tool("minecraft_wait_ticks", {"ticks": 1})
    path = artifact_dir / "screenshots" / f"{name}.png"
    return client.call_tool("minecraft_screenshot", {"path": str(path)})


def run(
    platform: str,
    timeout_seconds: float,
    artifact_dir: Path,
    scenario: dict[str, Any],
    *,
    server_bin: Path,
    plugins_dir: Path,
) -> dict[str, Any]:
    """Run a named scenario against an immutable server and plugin deployment.

    The scenario supplies ``run`` and may supply ``configure`` for ordinary
    server settings. Deployment is owned by the caller.
    """
    if platform not in PLATFORMS:
        raise RuntimeError(f"unsupported platform {platform!r}")
    server_bin = server_bin.expanduser().resolve()
    plugins_dir = plugins_dir.expanduser().resolve()
    if not server_bin.is_file() or not os.access(server_bin, os.X_OK):
        raise ValueError(f"server candidate is not executable: {server_bin}")
    if not plugins_dir.is_dir():
        raise ValueError(f"plugin deployment is not a directory: {plugins_dir}")
    if runtime.port_open(25567):
        raise RuntimeError("loader live-gate server port 25567 is already in use")

    game_dir = artifact_dir / "game"
    game_dir.mkdir(parents=True, exist_ok=True)
    if platform == "forge":
        # Avoid FML's failing early-window GL-context handoff on this Xvfb host.
        config_dir = game_dir / "config"
        config_dir.mkdir(exist_ok=True)
        (config_dir / "fml.toml").write_text("earlyWindowControl = false\n")
    # Keep the gate focused on Loader compatibility instead of Minecraft's first-run
    # accessibility onboarding. Vanilla's 26.1.2 options datafix uses false for an
    # already-onboarded profile. Isolate Loader audio from vanilla background music.
    (game_dir / "options.txt").write_text(
        "version:4790\nonboardAccessibility:false\nsoundDevice:SolarisLoaderGate\n"
        "soundCategory_music:0.0\n"
    )
    with (game_dir / "options.txt").open("a") as options:
        options.write("maxFps:30\nrenderDistance:4\nsimulationDistance:5\nguiScale:2\npauseOnLostFocus:false\n")
    (artifact_dir / "screenshots").mkdir(parents=True, exist_ok=True)
    world_dir = artifact_dir / "world"
    world_dir.mkdir(exist_ok=True)
    run_config = artifact_dir / "playable.toml"
    config_text = (REPO_ROOT / "examples" / "loader-live-gate" / "playable.toml").read_text()
    config_text = config_text.replace(
        'world_dir = ".analysis/loader-live-gate/world-v26"',
        f'world_dir = "{world_dir.relative_to(REPO_ROOT).as_posix()}"',
    )
    username = USERNAMES[platform]
    owners = tuple(sorted(path.parent.name for path in plugins_dir.glob("*/plugin.toml")))
    config_text = config_text.replace(
        'directory = ".analysis/loader-live-gate/plugins"',
        f"directory = {json.dumps(str(plugins_dir))}",
    )
    if configure := scenario.get("configure"):
        config_text = configure(config_text)
    run_config.write_text(config_text)
    from types import SimpleNamespace
    from .backends.gameplay import prepare_operator_fixture

    prepare_operator_fixture(SimpleNamespace(artifact_dir=artifact_dir, client_evidence={}), [username])
    fixture_root = REPO_ROOT / tomllib.loads(config_text)["plugins"]["directory"]
    expected_bundle_cache_files = []
    for owner in owners:
        manifest = tomllib.loads((fixture_root / owner / "plugin.toml").read_text())
        if manifest["id"] != owner:
            raise RuntimeError(f"Loader fixture directory {owner} declares a different owner")
        expected_bundle_cache_files.extend(
            f"{owner}/{bundle['id']}/{bundle['version']}/{bundle['sha256']}.bundle"
            for bundle in manifest.get("client", {}).get("bundles", [])
        )
    expected_bundle_cache_files.sort()
    token = f"solaris-loader-{platform}-{time.time_ns()}"
    mcp_port = runtime.reserve_port()

    xvfb = server = client_process = None
    xvfb_log = server_log_handle = client_log_handle = None
    mcp = None
    result: dict[str, Any] = {
        "platform": platform,
        "artifact_dir": str(artifact_dir),
        "game_dir": str(game_dir.relative_to(REPO_ROOT)),
        "world_dir": str(world_dir.relative_to(REPO_ROOT)),
        "run_config": str(run_config.relative_to(REPO_ROOT)),
        "server_address": SERVER_ADDRESS,
        "mcp_port": mcp_port,
        "forge_early_window_control": False if platform == "forge" else None,
        "server_binary": str(server_bin),
        "server_binary_sha256": hashlib.sha256(server_bin.read_bytes()).hexdigest(),
        "plugins_directory": str(fixture_root),
        "expected_bundle_cache_files": expected_bundle_cache_files,
    }
    try:
        xvfb, display, xvfb_log = runtime.start_xvfb(artifact_dir)

        server_log_path = artifact_dir / "server.log"
        server_log_handle = server_log_path.open("wb")
        server = subprocess.Popen(
            [str(server_bin), "--config", str(run_config)],
            cwd=REPO_ROOT,
            stdout=server_log_handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        runtime.wait_port(25567, min(timeout_seconds, 90.0), server, log_path=server_log_path)
        result["server_ready"] = True

        client_process, client_log_handle = runtime.start_client(
            game_dir=game_dir,
            log_path=artifact_dir / "client.log",
            token=token,
            mcp_port=mcp_port,
            username=username,
            display=display,
            platform=platform,
        )
        result["mcp_ready"] = True

        mcp = McpClient(
            f"http://127.0.0.1:{mcp_port}/mcp",
            token,
            request_timeout_seconds=max(30.0, min(timeout_seconds, 240.0) + 10.0),
        )
        initialized = mcp.initialize()
        tools = mcp.list_tools()
        result["protocol_version"] = initialized["protocolVersion"]
        result["tool_count"] = len(tools)

        before = runtime.wait_client_ready_for_connect(mcp, min(timeout_seconds, 120.0))
        before_screen = before.get("screen")
        result["client_ready_screen"] = (
            before_screen.get("class") if isinstance(before_screen, dict) else before.get("current_screen")
        )
        result["in_play_before_connect"] = bool(before.get("in_play"))
        mcp.call_tool("minecraft_connect", {"server_addr": SERVER_ADDRESS})
        result["permission_title"] = confirm_loader_permission(
            mcp, min(timeout_seconds, 90.0)
        )
        result["permission_confirmed"] = True

        push_wait_seconds = 5.0 if platform == "forge" else min(timeout_seconds, 120.0)
        play = mcp.call_tool(
            "minecraft_wait_for_play",
            {"timeout_seconds": push_wait_seconds},
        )
        if not play.get("in_play") and platform == "forge":
            result["forge_push_wait_snapshot"] = play
            play = mcp.call_tool(
                "minecraft_wait_for_play",
                {"timeout_seconds": min(timeout_seconds, 30.0)},
            )
            if play.get("in_play"):
                result["forge_direct_observe_recovered_play"] = True
        if not play.get("in_play"):
            raise RuntimeError(f"real {platform} client did not reach Play: {play}")
        result["in_play"] = True
        result["named_scenario"] = scenario["run"](mcp, runtime, artifact_dir, server_log_path)
        mcp.call_tool("minecraft_disconnect")
        result["passed"] = True
        (artifact_dir / "scenario.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
        return result
    finally:
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        runtime.stop_process(client_process)
        runtime.stop_process(server, interrupt=True)
        runtime.stop_process(xvfb)
        for handle in (client_log_handle, server_log_handle, xvfb_log):
            if handle is not None:
                handle.close()
        if not result.get("passed"):
            (artifact_dir / "scenario.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
