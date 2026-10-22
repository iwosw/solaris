"""Real-client proof for the vanilla admin commands: /tellraw, /title, /bossbar, /tick, /locate.

An operator joins the native client through the ordinary connect path and issues each command as
a player chat command. Every effect is read back from the client's own received state
(`recent_chat`, the HUD `ui_nodes` the client builds from the received title/boss-bar packets and
its translated feedback), never from a server-side shortcut:

* `/tellraw`: the raw component arrives as a chat line (colour component, not translated text).
* `/title`: `hud/title` + `hud/subtitle` nodes appear with the sent text and disappear on `clear`.
* `/bossbar`: `boss/*` nodes appear for a listed player and are gone after `remove`.
* `/tick`: `freeze` stops the simulated game clock (`time query gametime` stays constant while
  the client keeps ticking), `step` advances it by exactly the stepped ticks, `sprint` reports an
  achieved tick rate, `rate` and `query` are reported.
* `/locate`: biome, poi (a lectern placed beside the player) and structure searches answer
  asynchronously while the game clock keeps advancing.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import time
from pathlib import Path
from typing import Any

from .. import runtime
from ..profiles import ProfileContext
from . import gameplay, native_client
from .gameplay import peak_rss_kib
from .probe_steps import ProbeSteps

USERNAME = "AdminProof"


def _chat(state: dict[str, Any]) -> list[str]:
    return [line for line in state.get("recent_chat", []) if isinstance(line, str)]


def _nodes(state: dict[str, Any], prefix: str) -> list[dict[str, Any]]:
    return [n for n in state.get("ui_nodes", []) if str(n.get("id", "")).startswith(prefix)]


def _label(state: dict[str, Any], node_id: str) -> str | None:
    node = next((n for n in state.get("ui_nodes", []) if n.get("id") == node_id), None)
    return None if node is None else node.get("label")


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.client != "native":
        raise ValueError("admin-commands requires --client native")
    if ctx.extra_args or ctx.platform is not None:
        raise ValueError("admin-commands accepts only --client, --windowed and --timeout-seconds")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else 240.0
    if not 0 < timeout <= 600:
        raise ValueError("admin-commands --timeout-seconds must be in (0, 600]")
    artifact = ctx.artifact_dir
    override = os.environ.get("SOLARIS_SERVER_BINARY")
    if not override:
        raise RuntimeError("admin-commands requires SOLARIS_SERVER_BINARY (a solaris-v2 built from this tree)")
    binary = Path(override).expanduser().resolve()
    if not binary.is_file():
        raise RuntimeError(f"missing executable server binary: {binary}")
    server_port = runtime.reserve_port()
    address = f"127.0.0.1:{server_port}"
    config = artifact / "admin-commands.toml"
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
            raise RuntimeError(f"expected one admin-commands config token: {old_tok}")
        source = source.replace(old_tok, new_tok, 1)
    config.write_text(source)
    (artifact / "plugins").mkdir(parents=True, exist_ok=True)
    gameplay.prepare_operator_fixture(ctx, [USERNAME])
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
        with ProbeSteps(mcp, artifact / "admin-commands", timeout, "native-admin-commands") as probe:
            probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})

            def shot(name: str) -> None:
                if ctx.windowed:
                    native_client.capture(mcp, artifact, name)

            marks = [0]

            def say(command: str, expect: str, label: str) -> list[str]:
                """Run one player command; wait for a NEW chat line containing `expect`."""
                marks[0] += 1
                probe.call("minecraft_send_chat",
                           {"message": f'tellraw @s {{"text":"MARK-{marks[0]}"}}', "command": True})
                probe.wait(lambda s: any(f"MARK-{marks[0]}" in l for l in _chat(s)), "marker_arrives")
                probe.call("minecraft_send_chat", {"message": command, "command": True})
                return after_mark_pending(expect, label)

            def after_mark_pending(expect: str, label: str) -> list[str]:
                marker = f"MARK-{marks[0]}"

                def newer(state: dict[str, Any]) -> bool:
                    chat = _chat(state)
                    cut = next((i for i, line in enumerate(chat) if marker in line), None)
                    return cut is not None and any(expect in line for line in chat[:cut])
                state = probe.wait(newer, label)
                chat = _chat(state)
                cut = next(i for i, line in enumerate(chat) if marker in line)
                return [line for line in chat[:cut] if expect in line]

            def game_time(label: str) -> int:
                lines = say("time query gametime", "The time is", label)
                match = re.search(r"The time is (\d+)", lines[0])
                if not match:
                    raise RuntimeError(f"time query reply not numeric: {lines[0]!r}")
                return int(match.group(1))

            def client_ticks(count: int, label: str) -> None:
                """Let the client run `count` more ticks (event-driven on its own tick counter)."""
                start = probe.observe()["client_tick"]
                probe.wait(lambda s: s["client_tick"] >= start + count, label)

            # --- /tellraw ---------------------------------------------------------------
            probe.fixture("gamemode creative @s")
            lines = say('tellraw @s {"text":"RAW-OK-7f3","color":"green"}', "RAW-OK-7f3", "tellraw_arrives")
            probe.report["tellraw_line"] = lines[:1]

            # --- /title -----------------------------------------------------------------
            probe.call("minecraft_send_chat", {"message": "title @s times 2 200 2", "command": True})
            probe.call("minecraft_send_chat", {"message": 'title @s subtitle {"text":"SUB-LINE-3a9"}', "command": True})
            probe.call("minecraft_send_chat", {"message": 'title @s title {"text":"HEAD-LINE-3a9","color":"gold"}', "command": True})
            shown = probe.wait(lambda s: _label(s, "hud/title") == "HEAD-LINE-3a9", "title_node_shows_text")
            probe.check("subtitle_node_shows_text", _label(shown, "hud/subtitle") == "SUB-LINE-3a9")
            shot("title-shown")
            probe.call("minecraft_send_chat", {"message": "title @s clear", "command": True})
            probe.wait(lambda s: _label(s, "hud/title") is None and _label(s, "hud/subtitle") is None,
                       "title_cleared")

            # --- /bossbar ---------------------------------------------------------------
            probe.fixture('bossbar add proof:bar {"text":"Proof Bar","color":"red"}')
            probe.fixture("bossbar set proof:bar color red")
            probe.fixture("bossbar set proof:bar max 10")
            probe.fixture("bossbar set proof:bar value 7")
            probe.fixture("bossbar set proof:bar players @s")
            bar = probe.wait(lambda s: bool(_nodes(s, "boss/")), "bossbar_nodes_appear")
            probe.report["bossbar_nodes"] = [n["id"] for n in _nodes(bar, "boss/")]
            shot("bossbar-shown")
            lines = say("bossbar get proof:bar value", "7", "bossbar_get_value")
            probe.fixture("bossbar remove proof:bar")
            probe.wait(lambda s: not _nodes(s, "boss/"), "bossbar_removed")

            # --- /tick ------------------------------------------------------------------
            say("tick query", "Target tick rate", "tick_query_reports_rate")
            probe.fixture("tick freeze")
            probe.wait(lambda s: any("frozen" in l.lower() for l in _chat(s)), "tick_freeze_feedback")
            held = game_time("gametime_frozen_a")
            # A frozen server tells the client (ClientboundTickingState) to stop its own ticking.
            frozen_client = probe.observe()["client_tick"]
            time.sleep(3.0)
            held_after = game_time("gametime_frozen_b")
            probe.check("frozen_game_time_does_not_advance", held_after == held)
            probe.check("client_halts_ticks_while_frozen",
                        probe.observe()["client_tick"] == frozen_client)
            probe.fixture("tick step 5")
            probe.wait(lambda s: s["client_tick"] >= frozen_client + 5, "client_steps_five_ticks")
            time.sleep(1.0)
            probe.check("client_ticks_exactly_the_stepped_count",
                        probe.observe()["client_tick"] == frozen_client + 5)
            stepped = game_time("gametime_after_step")
            probe.check("step_advances_exactly_five_ticks", stepped - held == 5)
            probe.report["tick_game_time"] = {"frozen": held, "after_wait": held_after, "after_step": stepped}
            shot("tick-frozen")
            probe.fixture("tick unfreeze")
            probe.wait(lambda s: any("running normally" in l.lower() for l in _chat(s)), "tick_unfreeze_feedback")
            running_a = game_time("gametime_running_a")
            client_ticks(60, "client_ticks_running")
            running_b = game_time("gametime_running_b")
            probe.check("running_game_time_advances", running_b > running_a)
            sprint = say("tick sprint 200", "Sprint completed with", "tick_sprint_reports_rate")
            probe.report["tick_sprint_report"] = sprint[:1]
            match = re.search(r"Sprint completed with (\d+) ticks per second", sprint[0])
            probe.check("sprint_exceeds_default_rate", bool(match) and int(match.group(1)) > 20)
            # The native client only supports the 20 TPS simulation rate (it disconnects on any
            # other ticking-state rate), so the client gate sets the default rate explicitly and the
            # non-default rate is covered by the console run + tick_control unit tests.
            say("tick rate 20", "20.0", "tick_rate_set")
            say("tick query", "Target tick rate: 20.0", "tick_query_shows_rate")

            # --- /locate ----------------------------------------------------------------
            before = game_time("gametime_before_locate")
            probe.call("minecraft_send_chat", {"message": "locate biome #minecraft:is_overworld", "command": True})
            found = probe.wait(
                lambda s: any("The nearest" in l or "Could not find" in l for l in _chat(s)),
                "locate_biome_answers")
            probe.report["locate_biome"] = [l for l in _chat(found) if "nearest" in l or "Could not" in l][:1]
            probe.fixture("setblock ~ ~ ~3 minecraft:lectern")
            poi = say("locate poi minecraft:librarian", "The nearest", "locate_poi_finds_lectern")
            probe.report["locate_poi"] = poi[:1]
            probe.call("minecraft_send_chat", {"message": "locate structure #minecraft:village", "command": True})
            village = probe.wait(
                lambda s: any("The nearest" in l or "Could not find" in l for l in _chat(s)
                              if "village" in l.lower()),
                "locate_structure_answers")
            probe.report["locate_structure"] = [l for l in _chat(village) if "illage" in l][:1]
            after = game_time("gametime_after_locate")
            probe.check("game_clock_advanced_during_locate", after > before)
            shot("locate-results")
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
