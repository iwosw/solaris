"""Server-driven dialog proof for the native client (`show_dialog` -> `custom_click_action`).

Starts the server with `SOLARIS_DIALOG_PROBE=1` (core `dialog::probe`: every joining player is
shown a `multi_action` dialog with a text input, a number slider and a checkbox, and a click is
logged and echoed), launches the native client through the ordinary connect path, then through
the ordinary screen tools: matches the dialog screen class + title, types the text input,
steps the slider, flips the checkbox, screenshots, presses Submit, and requires the server to log
the exact typed payload and the client to receive the echo.

No privileged fixtures: the dialog arrives from the server like any production dialog.
"""
from __future__ import annotations

import hashlib
import json
import os
import subprocess
from pathlib import Path
from typing import Any

from .. import runtime
from ..profiles import ProfileContext
from . import native_client
from .gameplay import peak_rss_kib
from .probe_steps import ProbeSteps

DIALOG_CLASS = "net.minecraft.client.gui.screens.dialog.MultiButtonDialogScreen"
DIALOG_TITLE = "Dialog probe"
USERNAME = "DialogProof"
TYPED_NAME = "Solaris"


def _node(state: dict[str, Any], node_id: str) -> dict[str, Any] | None:
    return next((n for n in state.get("ui_nodes", []) if n["id"] == node_id), None)


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.client != "native":
        raise ValueError("dialogs requires --client native")
    if ctx.extra_args or ctx.platform is not None:
        raise ValueError("dialogs accepts only --client, --windowed and --timeout-seconds")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else 120.0
    if not 0 < timeout <= 120:
        raise ValueError("dialogs --timeout-seconds must be in (0, 120]")
    artifact = ctx.artifact_dir
    override = os.environ.get("SOLARIS_SERVER_BINARY")
    if not override:
        raise RuntimeError("dialogs requires SOLARIS_SERVER_BINARY (a solaris-v2 built from this tree)")
    binary = Path(override).expanduser().resolve()
    if not binary.is_file():
        raise RuntimeError(f"missing executable server binary: {binary}")
    server_port = runtime.reserve_port()
    address = f"127.0.0.1:{server_port}"
    config = artifact / "dialogs.toml"
    source = (runtime.REPO_ROOT / "playable.toml").read_text()
    replacements = (
        ("port = 25565", f"port = {server_port}"),
        ('world_dir = ".analysis/test-world-v12"',
         f'world_dir = "{(artifact / "world").relative_to(runtime.REPO_ROOT).as_posix()}"'),
        ('directory = "plugins"', f"directory = {json.dumps(str(artifact / 'plugins'))}"),
        ("seed = 0", "seed = 81"),
    )
    for old_tok, new_tok in replacements:
        if source.count(old_tok) != 1:
            raise RuntimeError(f"expected one dialogs config token: {old_tok}")
        source = source.replace(old_tok, new_tok, 1)
    config.write_text(source)
    (artifact / "plugins").mkdir(parents=True, exist_ok=True)
    evidence = ctx.client_evidence
    with binary.open("rb") as server_executable:
        server_hash = hashlib.file_digest(server_executable, "sha256").hexdigest()
    evidence.update(server_address=address, server_binary_path=str(binary),
                    server_binary_sha256=server_hash, world_dir=str(artifact / "world"), seed=81)
    server = client_process = xvfb = mcp = None
    server_log = None
    server_log_path = artifact / "server.log"
    try:
        server_log = server_log_path.open("wb")
        env = os.environ.copy()
        env["SOLARIS_DIALOG_PROBE"] = "1"
        env["SOLARIS_HARNESS_LOG_STDOUT"] = "1"
        server = subprocess.Popen([str(binary), "--config", str(config), "--no-console"],
                                  cwd=runtime.REPO_ROOT, stdout=server_log,
                                  stderr=subprocess.STDOUT, start_new_session=True, env=env)
        ctx.logs.append(str(server_log_path))
        runtime.wait_port(server_port, timeout, server, log_path=server_log_path)
        display = None
        if ctx.windowed:
            xvfb, display, _ = runtime.start_xvfb(artifact)
            ctx.logs.append(str(artifact / "xvfb.log"))
        client_process, _client_log, mcp = native_client.start(
            ctx, native_client.resolve_binary(ctx), address, USERNAME, timeout, display=display)
        mcp.request_timeout_seconds = timeout + 10
        with ProbeSteps(mcp, artifact / "dialogs", timeout, "native-dialogs") as probe:
            probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
            opened = probe.wait(
                lambda s: s["screen"].get("class") == DIALOG_CLASS, "dialog_screen_opened")
            probe.check("dialog_title_matches", opened["screen"].get("title") == DIALOG_TITLE)
            probe.check("dialog_reports_inputs", opened["dialog"]["inputs"] == ["name", "radius", "walls"])
            for node_id in ("dialog/title", "dialog/body/0", "dialog/input/name",
                            "dialog/input/radius", "dialog/input/walls",
                            "dialog/action/0", "dialog/action/1", "dialog/action/2"):
                probe.check(f"node_{node_id.replace('/', '_')}", _node(opened, node_id) is not None)
            probe.check("slider_starts_at_initial", _node(opened, "dialog/input/radius")["label"] == "Radius: 8")
            probe.check("checkbox_starts_off", _node(opened, "dialog/input/walls")["label"] == "Walls: OFF")
            if ctx.windowed:
                native_client.capture(mcp, artifact, "dialog-open")
            screen = {"expected_screen_class": DIALOG_CLASS, "expected_title": DIALOG_TITLE}
            probe.call("minecraft_set_screen_input", {**screen, "input_label": "Name", "value": TYPED_NAME})
            probe.call("minecraft_click_screen_button", {**screen, "button_label": "Radius: 8"})
            probe.call("minecraft_click_screen_button", {**screen, "button_label": "Walls: OFF"})
            filled = probe.observe()
            probe.check("slider_stepped", _node(filled, "dialog/input/radius")["label"] == "Radius: 9")
            probe.check("checkbox_on", _node(filled, "dialog/input/walls")["label"] == "Walls: ON")
            probe.check("dialog_still_open", filled["screen"].get("class") == DIALOG_CLASS)
            if ctx.windowed:
                native_client.capture(mcp, artifact, "dialog-filled")
            probe.call("minecraft_click_screen_button", {**screen, "button_label": "Submit"})
            closed = probe.wait(lambda s: s["screen"].get("class") in (None, "none"), "dialog_closed_after_action")
            probe.check("dialog_state_closed", closed["dialog"]["open"] is False)
            expected = ('dialog probe: action=submit name=Some("Solaris") radius=Some(9.0) '
                        'walls=Some(true) source=Some("probe")')

            def logged() -> tuple[bool, str]:
                text = server_log_path.read_text(errors="replace")
                return expected in text, text

            server_text = runtime.wait_for_path_condition(server_log_path, probe.remaining(), logged)
            probe.report["server_log_line"] = next(
                line for line in server_text.splitlines() if expected in line)
            probe.check("server_logged_exact_payload", True)
            echoed = probe.wait(
                lambda s: any(expected in line for line in s.get("recent_chat", [])),
                "client_received_server_echo")
            probe.report["client_chat_echo"] = [line for line in echoed["recent_chat"] if expected in line]
            if ctx.windowed:
                native_client.capture(mcp, artifact, "dialog-submitted")
            probe.report.update(address=address, username=USERNAME)
        if ctx.windowed:
            evidence["capture"] = native_client.capture(mcp, artifact)
        if client_process is not None:
            rss = peak_rss_kib(client_process.pid)
            if rss is not None:
                evidence["peak_client_rss_kib"] = rss
        return {"scenario": probe.report}
    finally:
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        runtime.stop_process(client_process)
        runtime.stop_process(server, interrupt=True)
        runtime.stop_process(xvfb)
        if server_log is not None:
            server_log.close()
