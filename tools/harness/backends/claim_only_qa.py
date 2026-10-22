"""Claim/fencing/restart checklist on a settled autonomous world (native).

Resumes an existing world dir (VILLAGE_AUTONOMY_RESUME_WORLD, required):
ownerless Storage-L1 settlement already present. Runs ONLY:
1. planner_status observation (projects dialog, chest, building list)
2. claim via dialog Claim button (chat fallback recorded)
3. second player claim refused
4. owner orders a building (live project verified)
5. real process restart on same world; owner persists; project resumes
   (or a completed building client-verifies)
No growth watches, no builder waits: fits the 3600 s cap.
"""

from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any

from .. import runtime
from ..profiles import ProfileContext
from . import native_client
from .gameplay import peak_rss_kib, prepare_operator_fixture
from .probe_steps import ProbeSteps
from . import village_autonomy_qa as va

USERNAME = "VillageAuto"
SECOND_USERNAME = "VillageAutoB"


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.client != "native":
        raise ValueError("claim-only-qa requires --client native")
    if not ctx.windowed:
        raise ValueError("claim-only-qa requires --windowed")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else 600.0
    if not 0 < timeout <= 3600:
        raise ValueError("claim-only-qa --timeout-seconds must be in (0, 3600]")
    artifact = ctx.artifact_dir
    resume_world = os.environ.get("VILLAGE_AUTONOMY_RESUME_WORLD", "")
    if not resume_world:
        raise RuntimeError("claim-only-qa requires VILLAGE_AUTONOMY_RESUME_WORLD")
    override = os.environ.get("SOLARIS_SERVER_BINARY")
    if not override:
        raise RuntimeError("claim-only-qa requires SOLARIS_SERVER_BINARY")
    binary = Path(override).expanduser().resolve()
    if not binary.is_file():
        raise RuntimeError(f"missing executable server binary: {binary}")
    server_port = runtime.reserve_port()
    address = f"127.0.0.1:{server_port}"
    config = artifact / "claim-only-qa.toml"
    source = (runtime.REPO_ROOT / "playable.toml").read_text()
    replacements = (
        ("port = 25565", f"port = {server_port}"),
        (
            'world_dir = ".analysis/test-world-v12"',
            f'world_dir = "{(artifact / "world").relative_to(runtime.REPO_ROOT).as_posix()}"',
        ),
        ('directory = "plugins"', f"directory = {__import__('json').dumps(str(artifact / 'plugins'))}"),
        ("seed = 0", f"seed = {va.SEED}"),
    )
    for old_tok, new_tok in replacements:
        if source.count(old_tok) != 1:
            raise RuntimeError(f"expected one claim-only-qa config token: {old_tok}")
        source = source.replace(old_tok, new_tok, 1)
    config.write_text(source)
    _src = Path(resume_world).expanduser().resolve()
    _dst = artifact / "world"
    if _dst.exists():
        shutil.rmtree(_dst)
    shutil.copytree(_src, _dst)
    (artifact / "plugins").mkdir(parents=True, exist_ok=True)
    # Operators only so they can teleport (the owner next to the build site, the
    # second player into the village); claim and orders stay ordinary commands.
    prepare_operator_fixture(ctx, [USERNAME, SECOND_USERNAME])
    evidence = ctx.client_evidence
    with binary.open("rb") as server_executable:
        server_hash = hashlib.file_digest(server_executable, "sha256").hexdigest()
    evidence.update(
        server_address=address,
        server_binary_path=str(binary),
        server_binary_sha256=server_hash,
        world_dir=str(artifact / "world"),
        seed=va.SEED,
        village_center=list(va.VILLAGE_CENTER),
        resumed_from=str(_src),
    )
    server = client_process = xvfb = mcp = None
    peer_process = peer_mcp = None
    peer_log = None
    peer_command_index = None
    active_client_evidence = evidence
    server_log = None
    server_log_path = artifact / "server.log"

    def capture_peer(session: tuple[Any, Any, Any], command_index: int) -> None:
        nonlocal peer_process, peer_log, peer_mcp, peer_command_index
        peer_process, peer_log, peer_mcp = session
        peer_command_index = command_index

    try:
        server_log = server_log_path.open("wb")
        env = os.environ.copy()
        env["SOLARIS_HARNESS_LOG_STDOUT"] = "1"
        server = subprocess.Popen(
            [str(binary), "--config", str(config), "--no-console"],
            cwd=runtime.REPO_ROOT,
            stdout=server_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            env=env,
        )
        ctx.logs.append(str(server_log_path))
        runtime.wait_port(server_port, timeout, server, log_path=server_log_path)
        xvfb, display, _ = runtime.start_xvfb(artifact)
        ctx.logs.append(str(artifact / "xvfb.log"))
        client_process, _client_log, mcp = native_client.start(
            ctx, native_client.resolve_binary(ctx), address, USERNAME, timeout, display=display
        )
        mcp.request_timeout_seconds = min(timeout + 10, 130.0)
        with ProbeSteps(mcp, artifact / "claim-only-qa", timeout, "claim-only") as probe:
            probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
            # Hostile mobs killed the idle owner in an earlier run; the watches are
            # about the builder, not survival.
            probe.fixture("difficulty peaceful")
            detail: dict[str, Any] = {"behaviour": {}, "evidence": {}, "repro": {}}
            probe.report["claim_acceptance"] = detail
            # Settle: player present, then find the autonomous settlement.
            rooted = probe.wait(
                lambda s: isinstance((s.get("player") or {}).get("x"), float), "player_present"
            )
            detail["player_at_start"] = [
                rooted["player"].get("x"), rooted["player"].get("y"), rooted["player"].get("z")]
            detail["start_tick"] = int(rooted.get("game_time") or rooted.get("tick") or 0)
            settlement_name: str | None = None
            deadline = time.monotonic() + 180.0
            info_lines: list[str] = []
            while time.monotonic() < deadline:
                state = va._send(probe, "settlement list")
                info_lines = va._recent_chat(state)
                joined = " ".join(info_lines)
                if "Settlements:" in joined and "No settlements" not in joined:
                    for line in info_lines:
                        if line.startswith("Settlements:"):
                            names = [n.strip().rstrip(".") for n in line.split(":", 1)[1].split(",")]
                            settlement_name = next((n for n in names if n), None)
                            break
                    if settlement_name:
                        break
                probe.call("minecraft_wait_ticks", {"ticks": 100})
            detail["settlement_list_chat"] = info_lines
            detail["settlement_name"] = settlement_name
            if not settlement_name:
                va._verdict(detail, "ownerless_settlement", "FAIL", f"no settlement: {info_lines}")
                probe.check("ownerless_settlement_listed", False)
                return {"scenario": probe.report}
            state = va._send(probe, f"settlement info {settlement_name}")
            info_lines = va._recent_chat(state, 20)
            detail["settlement_info_chat"] = info_lines
            joined = " ".join(info_lines)
            ownerless = "owner=autonomous" in joined
            va._verdict(detail, "ownerless_settlement", "PASS" if ownerless else "FAIL",
                        f"name={settlement_name} chat={info_lines}")
            probe.check("settlement_is_ownerless", ownerless)
            # Planner observation (non-gating): projects dialog + buildings.
            try:
                detail["planner_projects"] = va._recent_chat(
                    va._send(probe, "settlement menu projects"), 25)
            except Exception as exc:
                detail["planner_projects_error"] = str(exc)[:300]
            state = va._send(probe, "settlement building list")
            building_lines = va._recent_chat(state, 25)
            detail["building_list_chat"] = building_lines
            # Chest census (best effort; never gates claim).
            try:
                anchor = detail.get("settlement_anchor")
                chest = va._find_nearby_chest(probe, {"settlement_anchor": anchor} if anchor else {})
                if chest is not None:
                    detail["founding_chest"] = list(chest)
                    detail["chest_now"] = va._chest_totals(va._open_chest(probe, chest))
            except Exception as exc:
                detail["chest_error"] = str(exc)[:300]
            # Claim via dialog (same steps as the full scenario).
            from dataclasses import replace
            claimed_ok = _claim(probe, detail, ctx, artifact, settlement_name,
                                lambda s, i: capture_peer(s, i), address, timeout)
            if not claimed_ok:
                probe.check("claimed_at_all", False)
                return {"scenario": probe.report}
            # Second player refused.
            peer_ctx = replace(ctx, artifact_dir=artifact / "peer-client",
                               client_evidence={}, windowed=False)
            peer_process, peer_log, peer_mcp = native_client.start(
                peer_ctx, native_client.resolve_binary(peer_ctx), address, SECOND_USERNAME, timeout)
            detail["second_client"] = peer_ctx.client_evidence
            capture_peer((peer_process, peer_log, peer_mcp), peer_ctx.client_evidence["command_index"])
            (artifact / "peer-client" / "peer").mkdir(parents=True, exist_ok=True)
            with ProbeSteps(peer_mcp, artifact / "peer-client" / "peer", timeout, "second-player") as peer_probe:
                peer_probe.call("minecraft_wait_for_play", {"timeout_seconds": peer_probe.remaining()})
                x, y, z = detail["player_at_start"]
                peer_probe.fixture(f"tp @s {x:.1f} {y:.1f} {z:.1f}")
                peer_probe.call("minecraft_wait_ticks", {"ticks": 200})
                peer_probe.call("minecraft_send_chat", {"message": "settlement claim", "command": True})
                _before = " ".join(va._recent_chat(peer_probe.observe(), 20))
                peer_state = peer_probe.observe()
                for _ in range(6):
                    peer_probe.call("minecraft_wait_ticks", {"ticks": 20})
                    peer_state = peer_probe.observe()
                    if " ".join(va._recent_chat(peer_state, 20)) != _before:
                        break
                peer_chat = va._recent_chat(peer_state, 8)
            detail["second_claim_chat"] = peer_chat
            second_refused = any("already has an owner" in line for line in peer_chat)
            va._verdict(detail, "second_claim_refused", "PASS" if second_refused else "FAIL",
                        f"second player chat={peer_chat}")
            probe.check("second_claim_refused", second_refused)
            # Owner orders a building.
            order_ok = va._wait_owner_order(probe, detail, va.OWNER_ORDER_BLUEPRINTS)
            va._verdict(detail, "owner_orders_building", "PASS" if order_ok else "FAIL",
                        f"chat={detail.get('post_claim_order_chat')}")
            probe.check("owner_can_order", order_ok)
            # Restart resume (same as full scenario tail).
            state = va._send(probe, f"settlement info {settlement_name}")
            detail["final_info_chat"] = va._recent_chat(state, 10)
            detail["fixture_commands"] = list(probe.report.get("fixture_commands", []))
            va._resident_census(probe, detail)
            _stand_near_site(probe, detail)
            before = va._wait_project_started(probe)
            detail["restart_before"] = before
            if peer_mcp is not None:
                peer_mcp.close()
            runtime.stop_process(peer_process)
            if peer_process is not None and peer_command_index is not None:
                ctx.exits[peer_command_index] = peer_process.returncode
            if peer_log is not None:
                peer_log.close()
            peer_process = peer_mcp = peer_log = None
            mcp.close()
            runtime.stop_process(client_process)
            ctx.exits[active_client_evidence["command_index"]] = client_process.returncode
            _client_log.close()
            before_pid = server.pid
            runtime.stop_process(server, interrupt=True)
            probe.check("restart_server_cleanly_stopped", server.returncode == 0)
            server_log.close()
            server_log_path = artifact / "server-restart.log"
            server_log = server_log_path.open("wb")
            server = subprocess.Popen(
                [str(binary), "--config", str(config), "--no-console"],
                cwd=runtime.REPO_ROOT, stdout=server_log, stderr=subprocess.STDOUT,
                start_new_session=True, env=env,
            )
            ctx.logs.append(str(server_log_path))
            runtime.wait_port(server_port, timeout, server, log_path=server_log_path)
            restart_ctx = replace(ctx, artifact_dir=artifact / "restart-client", client_evidence={})
            client_process, _client_log, mcp = native_client.start(
                restart_ctx, native_client.resolve_binary(restart_ctx), address, USERNAME,
                timeout, display=display,
            )
            active_client_evidence = restart_ctx.client_evidence
            mcp.request_timeout_seconds = min(timeout + 10, 130.0)
            evidence["restart"] = {
                "before_server_pid": before_pid, "after_server_pid": server.pid,
                "same_world": str(artifact / "world"), "client": restart_ctx.client_evidence,
            }
            probe.client = mcp
            probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
            probe.fixture("difficulty peaceful")
            wx, wy, wz = detail["watch_position"]
            probe.fixture(f"tp @s {wx:.1f} {wy + 0.5:.1f} {wz:.1f}")
            probe.call("minecraft_wait_ticks", {"ticks": 100})
            resumed = va._wait_project_resumed(probe, detail, before)
            va._verdict(detail, "restart_resume", "PASS" if resumed else "FAIL",
                        str(detail.get("restart_after")))
            probe.check("project_resumes_after_process_restart", resumed)
            native_client.capture(mcp, artifact, "claim-restarted-resumed")
            detail["total_game_ticks"] = va._gametime(probe) - detail["start_tick"]
        if ctx.windowed:
            evidence["capture"] = native_client.capture(mcp, artifact)
        if client_process is not None:
            rss = peak_rss_kib(client_process.pid)
            if rss is not None:
                evidence["peak_client_rss_kib"] = rss
        return {"scenario": probe.report}
    finally:
        for session in (mcp, peer_mcp):
            if session is not None:
                try:
                    session.close()
                except Exception:
                    pass
        runtime.stop_process(peer_process)
        if peer_process is not None and peer_command_index is not None:
            ctx.exits[peer_command_index] = peer_process.returncode
        runtime.stop_process(client_process)
        if client_process is not None:
            ctx.exits[active_client_evidence["command_index"]] = client_process.returncode
            try:
                _client_log.close()
            except Exception:
                pass
        runtime.stop_process(server, interrupt=True)
        runtime.stop_process(xvfb)
        if peer_log is not None:
            try:
                peer_log.close()
            except Exception:
                pass
        if server_log is not None:
            server_log.close()


def _claim(probe: ProbeSteps, detail: dict[str, Any], ctx: ProfileContext,
           artifact: Path, settlement_name: str, peer_ready: Any,
           address: str, timeout: float) -> bool:
    from . import native_client as _nc
    state = va._send(probe, "settlement")
    opened = probe.wait(lambda s: s["screen"].get("class") == va.DIALOG_CLASS, "claim_dialog_opened")
    title = opened["screen"].get("title", "")
    detail["claim_dialog_title"] = title
    if settlement_name not in title:
        va._verdict(detail, "claim_via_dialog_button", "FAIL", f"dialog title {title!r} misses {settlement_name}")
        probe.check("claim_dialog_opened", False)
        return False
    claim_ok = va._claim_via_dialog(probe, detail, settlement_name, title)
    chat_after = detail.get("claim_chat", [])
    if not claim_ok:
        detail["claim_button_unreachable"] = True
        state = va._send(probe, "settlement claim")
        chat_after = va._recent_chat(state, 8)
        detail["claim_chat_fallback"] = chat_after
        claim_ok = any("Claimed " in line for line in chat_after)
    va._verdict(detail, "claim_via_dialog_button",
                "PASS" if claim_ok and not detail.get("claim_button_unreachable") else "FAIL",
                f"chat={chat_after} button_unreachable={detail.get('claim_button_unreachable', False)}")
    detail["claimed_ok"] = claim_ok
    if ctx.windowed:
        _nc.capture(probe.client, artifact, "claim-only-claimed")
    return claim_ok


def _stand_near_site(probe: ProbeSteps, detail: dict[str, Any]) -> None:
    """Teleport the owner between the build site and the storage chest.

    The site is ~100 blocks from the founding chest, outside simulation distance
    of a player standing at the chest, so the builder would not tick (vanilla-like).
    The midpoint keeps both within simulation distance.
    """
    site = detail["owner_order_project"]["origin"]
    chest = detail["founding_chest"]
    x = (site[0] + chest[0]) / 2 + 0.5
    z = (site[2] + chest[2]) / 2 + 0.5
    y = max(site[1], chest[1]) + 12
    probe.fixture(f"tp @s {x:.1f} {y:.1f} {z:.1f}")
    probe.call("minecraft_wait_ticks", {"ticks": 100})
    player = probe.observe().get("player") or {}
    detail["watch_position"] = [float(player["x"]), float(player["y"]), float(player["z"])]
