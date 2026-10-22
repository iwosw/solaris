"""Empty-server pause gate (vanilla ``pause-when-empty-seconds``).

Real server listener, real protocol client, exact events only:

* wait for the pause log line (inotify-driven log condition),
* prove the world tick is frozen across the console ``save-all`` completion,
* answer a real 26.1.2 Status probe while paused,
* complete an offline Login + Configuration + Play join and wait for the
  ``LoginPlay`` packet, which the edge sends only after the core processed
  the join; the join therefore also proves ticking resumed,
* close the client and wait for the second pause log line.

No sleeps: every wait is an inotify log condition, a port readiness event or
a blocking socket read. Wire framing/decoding lives in the tracked
``tools/harness/wire.py`` helper.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import urllib.request
from pathlib import Path
from typing import Any

from .. import runtime, wire
from ..profiles import ProfileContext

PAUSE_LOG = "server empty, pausing world"


def _server_stats(dashboard_port: int) -> dict[str, Any]:
    body = urllib.request.urlopen(
        f"http://127.0.0.1:{dashboard_port}/stats", timeout=10
    ).read()
    data = json.loads(body)
    players = data.get("players") or {"count": 0}
    return {
        "simulation_tick": data["simulation_tick"],
        "players": players["count"],
        "uptime_secs": data["uptime_secs"],
    }


def _wait_pause(log_path: Path, occurrences: int, timeout: float) -> str:
    def reached() -> tuple[bool, str]:
        text = log_path.read_text(errors="replace")
        return text.count(PAUSE_LOG) >= occurrences, text

    text = runtime.wait_for_path_condition(log_path, timeout, reached)
    lines = [line for line in text.splitlines() if PAUSE_LOG in line]
    return lines[occurrences - 1]


def _wait_log(
    log_path: Path, needle: str, timeout: float, *, occurrences: int = 1
) -> str:
    def reached() -> tuple[bool, str]:
        text = log_path.read_text(errors="replace")
        return text.count(needle) >= occurrences, text

    text = runtime.wait_for_path_condition(log_path, timeout, reached)
    lines = [line for line in text.splitlines() if needle in line]
    return lines[occurrences - 1]


def _check(checks: dict[str, Any], name: str, condition: bool, detail: Any = None) -> None:
    checks[name] = {"passed": bool(condition), "detail": detail}
    if not condition:
        raise RuntimeError(f"check failed: {name}: {detail!r}")


def run(ctx: ProfileContext) -> dict[str, Any]:
    parser = argparse.ArgumentParser(prog="pause-empty")
    parser.add_argument("--pause-seconds", type=int, default=2)
    parser.add_argument("--join-name", default="PauseProbe")
    parser.add_argument("--command-timeout-seconds", type=float, default=120.0)
    args = parser.parse_args(ctx.extra_args)
    if args.pause_seconds <= 0:
        raise ValueError("--pause-seconds must be positive")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else 300.0
    if not (timeout > 0):
        raise ValueError("timeout must be positive")
    if ctx.platform is not None or ctx.plugins_dir is not None:
        raise ValueError("pause-empty runs the bare server without Loader/plugins")

    artifact = ctx.artifact_dir
    artifact.mkdir(parents=True, exist_ok=True)
    from ..profiles import _server_binary

    binary = _server_binary(ctx, "server-build.log")
    port = runtime.reserve_port()
    dashboard_port = runtime.reserve_port()
    world = artifact / "world"
    world.mkdir(parents=True, exist_ok=True)
    config_path = artifact / "pause-empty.toml"
    log_path = artifact / "server.log"
    config_path.write_text(
        f"""[server]
name = "pause-empty-gate"
view_distance = 2
simulation_distance = 2
pause_when_empty_seconds = {args.pause_seconds}

[network]
bind_address = "127.0.0.1"
port = {port}

[auth]
online_mode = false

[mob_griefing]
creeper_block_damage = false
ghast_block_damage = false
wither_block_damage = false
villager_farming = false
sheep_grazing = false

[dashboard]
enabled = true
bind_address = "127.0.0.1"
port = {dashboard_port}

[data]
world_dir = {json.dumps(str(world))}
vanilla_data_dir = "data/vanilla"
seed = 81
"""
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
        "scope": "empty-server pause: log event, frozen tick, console save, status, protocol join resume",
        "server_binary": str(binary),
        "server_port": port,
        "dashboard_port": dashboard_port,
        "pause_when_empty_seconds": args.pause_seconds,
        "mob_griefing_switches": {
            "creeper_block_damage": False,
            "ghast_block_damage": False,
            "wither_block_damage": False,
            "villager_farming": False,
            "sheep_grazing": False,
        },
        "checks": checks,
    }
    ctx.client_evidence["pause_empty"] = report
    try:
        runtime.wait_port(port, min(timeout, 120.0), server, log_path=log_path)
        report["startup_pause_log"] = _wait_pause(log_path, 1, min(timeout, 120.0))
        _check(checks, "pause_logged_from_startup", True, report["startup_pause_log"])

        paused = _server_stats(dashboard_port)
        assert server.stdin is not None
        server.stdin.write(b"save-all\n")
        server.stdin.flush()
        save_line = _wait_log(log_path, "Saved the game", args.command_timeout_seconds)
        after_save = _server_stats(dashboard_port)
        report["tick_samples"] = {"paused": paused, "after_console_save": after_save}
        _check(checks, "console_save_while_paused", True, save_line)
        _check(
            checks,
            "world_tick_frozen_across_console_save",
            paused["simulation_tick"] == after_save["simulation_tick"],
            report["tick_samples"],
        )
        _check(checks, "no_players_while_paused", after_save["players"] == 0, after_save["players"])

        status = wire.probe_status(("127.0.0.1", port))
        report["status"] = status
        _check(
            checks,
            "status_answers_while_paused",
            status["version"] == {"name": wire.RELEASE, "protocol": wire.PROTOCOL},
            status["version"],
        )

        joined_sock, joined = wire.join_offline_socket(
            ("127.0.0.1", port),
            args.join_name,
            timeout=args.command_timeout_seconds,
        )
        try:
            # LoginPlay proves the core processed Join; the first KeepAlive
            # proves the join burst finished and the play loop owns the
            # socket, so closing it below takes the ordinary leave path.
            wire.wait_play_loop(joined_sock, timeout=args.command_timeout_seconds)
        finally:
            joined_sock.close()
        after_join = _server_stats(dashboard_port)
        report["join"] = {
            "client": joined,
            "stats_after_join": after_join,
            "paused_tick": paused["simulation_tick"],
        }
        _check(checks, "protocol_join_reached_login_play", True, joined)
        _check(
            checks,
            "tick_advanced_after_join",
            after_join["simulation_tick"] > paused["simulation_tick"],
            report["join"],
        )
        _check(
            checks,
            "player_online_after_join",
            after_join["players"] == 1,
            after_join["players"],
        )

        # The client socket closed after `join_offline` returned: the core must
        # see the leave and re-enter the empty pause.
        repause_line = _wait_pause(log_path, 2, args.command_timeout_seconds)
        report["repause_log"] = repause_line
        _check(checks, "pause_rearmed_after_leave", True, repause_line)
        repaused = _server_stats(dashboard_port)
        server.stdin.write(b"save-all\n")
        server.stdin.flush()
        _wait_log(log_path, "Saved the game", args.command_timeout_seconds, occurrences=2)
        repaused_after_save = _server_stats(dashboard_port)
        report["tick_samples"]["repaused"] = repaused
        report["tick_samples"]["repaused_after_save"] = repaused_after_save
        _check(checks, "no_players_after_leave", repaused["players"] == 0, repaused["players"])
        _check(
            checks,
            "final_world_tick_frozen",
            repaused["simulation_tick"] == repaused_after_save["simulation_tick"],
            report["tick_samples"],
        )
    finally:
        runtime.stop_process(server)
        log_handle.close()
        ctx.exits[-1] = server.returncode
    _check(checks, "server_clean_exit", server.returncode == 0, server.returncode)
    return report
