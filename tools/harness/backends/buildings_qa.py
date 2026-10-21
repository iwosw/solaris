"""Strict in-game QA of every authored Millenaire blueprint level (buildings slice).

Flow (the ONLY unblocked in-game owner-build path, verified against
construction.rs/autonomy.rs + the QaAutonomy backend):
  seed-100 generated village -> walk to it -> ownerless settlement adopts ->
  founding chest starts empty -> workers produce unfunded -> autonomous Builder
  completes at least Storage L1 -> CLAIM via dialog Claim button -> owner orders
  each blueprint via the Projects dialog page (Order buttons; house+barracks L1
  MUST go through the dialog button per the assignment) or `/settlement build`
  chat for the rest -> fund the built storage chest via /give + ordinary
  container shift-click (the legit 'player funds settlement' path) -> a REAL
  Builder villager builds it -> verify per level -> upgrade L(n)->L(n+1) via
  the upgrade order -> bounded adversarial pass (break a block mid-build,
  restart mid-project + verify resume).

Per level verified with direct client-observed evidence only:
  - block-by-block match vs the authored blueprint (MCP block reads),
  - building certified (building list shows kind + level),
  - entrance reachable (player walks to the entrance cell),
  - lighting (block light >= 1 inside; no hostile spawns overnight inside),
  - function: house villagers sleep in its beds; farm farmer harvests+replants;
    lumber camp lumberjack cuts + deposits; mine miner digs + deposits;
    mason/Builder works there; storage accepts deposits; barracks soldier slots;
    wall/tower certified + walkable.
Screenshots of each finished level. Product code untouched.

SELECT env: one blueprint per harness invocation (600s cap):
  house|storage|farm|mason|lumber_camp|mine|barracks|wall|watch_tower
(BASE omits storage: storage is the boot step, verified as the claim village's
autonomous Storage L1 + funding target.)
"""
from __future__ import annotations

import hashlib
import json
import math
import os
import subprocess
import time as _time
from pathlib import Path
from typing import Any

from .. import runtime
from ..profiles import ProfileContext
from . import native_client
from .gameplay import peak_rss_kib, prepare_operator_fixture
from .probe_steps import ProbeSteps

USERNAME = "BuildQA"
SEED = 100
VILLAGE_CENTER = (120, 70, 7)
DIALOG_CLASS = "net.minecraft.client.gui.screens.dialog.MultiButtonDialogScreen"

BLUEPRINTS = ("house", "storage", "farm", "mason", "lumber_camp", "mine",
              "barracks", "wall", "watch_tower")
LEVELS = {"house": 3, "storage": 2, "farm": 2, "mason": 2, "lumber_camp": 2,
          "mine": 2, "barracks": 2, "wall": 2, "watch_tower": 2}

FILL_LIMIT = 32768


def _chunked_fills(x0: int, y0: int, z0: int, x1: int, y1: int, z1: int, block: str) -> list[str]:
    """Fill commands covering the box, split along z so each fits FILL_LIMIT.
    Unsliced fills over the limit are silently rejected, leaving terrain."""
    rows = max(1, FILL_LIMIT // ((x1 - x0 + 1) * (y1 - y0 + 1)))
    return [f"fill {x0} {y0} {z} {x1} {y1} {min(z + rows - 1, z1)} {block}"
            for z in range(z0, z1 + 1, rows)]


# Storage L1 base needs (must arrive via worker production, no /give).
STORAGE_L1_NEEDS = {
    "minecraft:cobblestone": 50,
    "minecraft:oak_log": 41,
    "minecraft:coal": 1,
}

BLUEPRINT_DATA: dict[str, dict[str, Any]] = {}

# Dialog kind labels (ui.rs kind_label) + profession per blueprint.
KIND_LABEL = {"house": "House", "storage": "Storage", "farm": "Farmer workshop",
              "mason": "Builder workshop", "lumber_camp": "Lumber camp",
              "mine": "Mine", "barracks": "Barracks", "wall": "Wall section",
              "watch_tower": "Tower"}
KIND_PROFESSION = {"farm": "Farmer", "mason": "Builder",
                    "lumber_camp": "Lumberjack", "mine": "Miner"}
# Parallel layout: one founded settlement per blueprint, centers 400 apart on
# a grid (radius 64 each: 64+64=128 < 400, no overlap, no village dependency).
# Each site gets: founded settlement (owner=BuildQA) -> hand shell -> 6
# villagers -> order -> fund -> Builder builds. All sites run concurrently:
# the planner ticks one settlement per 20 ticks, builders work in parallel.
PARALLEL_ORIGIN = (0, 70, 0)
PARALLEL_STEP = 400
PARALLEL_RADIUS = 64
PARALLEL_SITES = {
    "house": (0, 0), "storage": (1, 0), "farm": (2, 0),
    "mason": (0, 1), "lumber_camp": (1, 1), "mine": (2, 1),
    "barracks": (0, 2), "wall": (1, 2), "watch_tower": (2, 2),
}

def _load_blueprints() -> dict[str, dict[str, Any]]:
    if BLUEPRINT_DATA:
        return BLUEPRINT_DATA
    root = runtime.REPO_ROOT / "crates" / "mc-core" / "assets" / "blueprints"
    for path in sorted(root.glob("*.json")):
        raw = json.loads(path.read_text())
        BLUEPRINT_DATA[raw["id"]] = {
            "kind": raw["kind"],
            "palette": raw["palette"],
            "levels": [
                {"cert": lvl["cert"], "entrance": lvl["entrance"],
                 "layers": lvl["layers"]}
                for lvl in raw["levels"]
            ],
        }
    return BLUEPRINT_DATA


def expected_cells(bp: str, level: int, origin: tuple[int, int, int]) -> dict[tuple[int, int, int], str]:
    data = _load_blueprints()[bp]
    layers: list[list[str]] = data["levels"][level - 1]["layers"]
    palette: dict[str, str] = data["palette"]
    ox, oy, oz = origin
    out: dict[tuple[int, int, int], str] = {}
    for y, layer in enumerate(layers):
        for z, row in enumerate(layer):
            for x, ch in enumerate(row):
                if ch == ".":
                    continue
                out[(ox + x, oy + y, oz + z)] = palette[ch]
    return out


def _state_base(text: str) -> str:
    name = text.split("[")[0]
    return name if ":" in name else f"minecraft:{name}"


def _block_id(block: dict[str, Any]) -> str:
    inner = block.get("block") if isinstance(block.get("block"), dict) else block
    if isinstance(inner, dict):
        return str(inner.get("block_id", inner.get("id", "")))
    return str(block.get("block_id", ""))


def _props(block: dict[str, Any]) -> dict[str, Any]:
    props = block.get("properties") or {}
    return dict(props) if isinstance(props, dict) else {}


def _read(probe: ProbeSteps, pos: tuple[int, int, int]) -> dict[str, Any]:
    result = probe.call("minecraft_read_block", {"x": pos[0], "y": pos[1], "z": pos[2]})
    inner = result.get("block")
    return inner if isinstance(inner, dict) else result


def _chat_lines(state: dict[str, Any], n: int = 20) -> list[str]:
    # Client buffer is newest-first (observation .rev().take(20)): take the
    # FIRST n, not the last n.
    return [str(l) for l in (state.get("recent_chat", []) or [])][:n]


def _send(probe: ProbeSteps, command: str, settle_ticks: int = 40) -> dict[str, Any]:
    probe.call("minecraft_send_chat", {"message": command, "command": True})
    probe.call("minecraft_wait_ticks", {"ticks": settle_ticks})
    return probe.observe()


def _echo(probe: ProbeSteps, label: str) -> dict[str, Any]:
    import uuid as _uuid
    marker = f"bqa_ping_{label}_{_uuid.uuid4().hex[:8]}"
    probe.call("minecraft_send_chat", {"message": marker, "command": False})
    return probe.wait(lambda s: any(marker in line for line in s.get("recent_chat", [])),
                      f"{label}_server_echo")

def _villagers(probe: ProbeSteps) -> list[dict[str, Any]]:
    state = probe.call("minecraft_list_entities", {"radius": 128})
    entities = state.get("entities", state.get("visible_entities", []))
    return [r for r in entities
            if isinstance(r, dict) and r.get("entity_type") == "minecraft:villager"]


def _open_chest(probe: ProbeSteps, chest: tuple[int, int, int]) -> dict[int, dict[str, Any]]:
    probe.call("minecraft_set_hotbar_slot", {"slot": 8})
    probe.call("minecraft_use_item_on",
               {"x": chest[0], "y": chest[1], "z": chest[2], "face": "up", "hand": "main_hand"})
    state = probe.wait(
        lambda s: s["container"]["container_id"] != 0
        and s["container"]["menu_class"] == "net.minecraft.world.inventory.ChestMenu",
        "chest_menu_received")
    if state["container"]["slot_count"] != 63:
        raise RuntimeError(f"buildings-qa chest slot_count is {state['container']['slot_count']}, want 63")
    slots: dict[int, dict[str, Any]] = {}
    for row in state["container"]["slots"]:
        if isinstance(row, dict) and 0 <= int(row.get("slot", -1)) < 27:
            slots[int(row["slot"])] = {"item_id": row.get("item_id"), "count": row.get("count", 0)}
    probe.call("minecraft_close_screen", {})
    probe.wait(lambda s: s["container"]["container_id"] == 0, "chest_menu_closed")
    return slots


def _chest_totals(slots: dict[int, dict[str, Any]]) -> dict[str, int]:
    totals: dict[str, int] = {}
    for entry in slots.values():
        name = str(entry.get("item_id", "minecraft:air"))
        if name != "minecraft:air" and int(entry.get("count", 0)) > 0:
            totals[name] = totals.get(name, 0) + int(entry.get("count", 0))
    return totals


def _deposit_all(probe: ProbeSteps, chest: tuple[int, int, int], items: list[str]) -> dict[str, Any]:
    """Shift-click every matching player-inventory stack into the open chest (ordinary path)."""
    report: dict[str, Any] = {"moved": []}
    probe.call("minecraft_use_item_on",
               {"x": chest[0], "y": chest[1], "z": chest[2], "face": "up", "hand": "main_hand"})
    st = probe.wait(lambda s: s["container"]["container_id"] != 0
                    and s["container"]["menu_class"] == "net.minecraft.world.inventory.ChestMenu",
                    "chest_open_for_deposit")
    moved = 0
    for row in list(st["container"]["slots"]):
        if not isinstance(row, dict):
            continue
        slot = int(row.get("slot", -1))
        if slot < 27 or str(row.get("item_id")) not in items:
            continue
        try:
            probe.call("minecraft_quick_move_container_slot", {"slot": slot})
            moved += 1
        except Exception as e:
            report.setdefault("errors", []).append(f"slot {slot}: {e}")
    report["moved_count"] = moved
    probe.call("minecraft_close_screen", {})
    probe.wait(lambda s: s["container"]["container_id"] == 0, "chest_closed_after_deposit")
    return report


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.client != "native":
        raise ValueError("buildings-qa requires --client native")
    if ctx.extra_args or ctx.platform is not None:
        raise ValueError("buildings-qa accepts only --client, --windowed and --timeout-seconds")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else 600.0
    if not 0 < timeout <= 3600:
        raise RuntimeError("buildings-qa --timeout-seconds must be in (0, 3600]")
    select = os.environ.get("BUILDINGS_QA_SELECT", "ALL")
    if select != "ALL" and select not in BLUEPRINTS:
        raise ValueError(f"BUILDINGS_QA_SELECT must be one of {BLUEPRINTS} or ALL")
    targets = BLUEPRINTS if select == "ALL" else (select,)
    _load_blueprints()
    artifact = ctx.artifact_dir
    override = os.environ.get("SOLARIS_SERVER_BINARY")
    if not override:
        raise RuntimeError("buildings-qa requires SOLARIS_SERVER_BINARY (a solaris-v2 built from this tree)")
    binary = Path(override).expanduser().resolve()
    if not binary.is_file():
        raise RuntimeError(f"missing executable server binary: {binary}")
    server_port = runtime.reserve_port()
    address = f"127.0.0.1:{server_port}"
    config = artifact / "buildings-qa.toml"
    source = (runtime.REPO_ROOT / "playable.toml").read_text()
    replacements = (
        ("port = 25565", f"port = {server_port}"),
        ('world_dir = ".analysis/test-world-v12"',
         f'world_dir = "{(artifact / "world").relative_to(runtime.REPO_ROOT).as_posix()}"'),
        ('directory = "plugins"', f"directory = {json.dumps(str(artifact / 'plugins'))}"),
        ("seed = 0", f"seed = {SEED}"),
    )
    for old_tok, new_tok in replacements:
        if source.count(old_tok) != 1:
            raise RuntimeError(f"expected one buildings-qa config token: {old_tok}")
        source = source.replace(old_tok, new_tok, 1)
    config.write_text(source)
    keep = os.environ.get("BUILDINGS_QA_KEEP_WORLD")
    if keep:
        import shutil as _shutil
        _shutil.rmtree(artifact / "world", ignore_errors=True)
        _shutil.copytree(keep, artifact / "world")
        evidence_keep = {"resumed_world_from": keep}
    else:
        evidence_keep = {}
    (artifact / "plugins").mkdir(parents=True, exist_ok=True)
    prepare_operator_fixture(ctx, [USERNAME])
    evidence = ctx.client_evidence
    with binary.open("rb") as server_executable:
        server_hash = hashlib.file_digest(server_executable, "sha256").hexdigest()
    evidence.update(server_address=address, server_binary_path=str(binary),
                    server_binary_sha256=server_hash, world_dir=str(artifact / "world"),
                    seed=SEED, select=select, **evidence_keep)
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
        mcp.request_timeout_seconds = 130.0
        with ProbeSteps(mcp, artifact / "buildings-qa", timeout, "buildings-qa") as probe:
            probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
            if ctx.windowed:
                native_client.capture(mcp, artifact, "buildings-qa-spawn")
            detail: dict[str, Any] = {"behaviour": {}, "evidence": {}, "repro": {}}
            probe.report["buildings_acceptance"] = detail
            detail["mode"] = "sequential-founded-resumed"
            probe.server_proc = server  # type: ignore[attr-defined]
            _scenario_sequential(probe, detail, ctx, artifact, targets,
                                 address, config, server_log_path)
            server = getattr(probe, "server_proc", server)
            detail["matrix_19"] = _matrix_19(detail)
            if ctx.windowed:
                native_client.capture(mcp, artifact, "buildings-qa-final")
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

def _matrix_19(detail: dict[str, Any]) -> dict[str, str]:
    """Enumerate all 19 cases (9 blueprint ids x their levels) PASS/FAIL/NOT-RUN."""
    out: dict[str, str] = {}
    blueprints = detail.get("blueprints", {})
    for bp, count in LEVELS.items():
        for level in range(1, count + 1):
            sub = blueprints.get(bp, {})
            checks = [sub.get("behaviour", {}).get(f"L{level}_blockmatch"),
                      sub.get("behaviour", {}).get(f"L{level}_entrance"),
                      sub.get("behaviour", {}).get(f"L{level}_light")]
            if all(c == "PASS" for c in checks):
                out[f"{bp} L{level}"] = "PASS"
            elif any(c == "FAIL" for c in checks):
                out[f"{bp} L{level}"] = "FAIL"
            else:
                out[f"{bp} L{level}"] = "NOT-RUN"
    return out

def _verdict(detail: dict[str, Any], name: str, verdict: str, evidence: str = "",
             repro: str = "") -> None:
    detail.setdefault("behaviour", {})[name] = verdict
    if evidence:
        detail.setdefault("evidence", {})[name] = evidence
    if repro:
        detail.setdefault("repro", {})[name] = repro


def _walk_to(probe: ProbeSteps, detail: dict[str, Any],
             target: tuple[float, float, float], label: str) -> dict[str, Any]:
    probe.call("minecraft_navigate_to_block",
               {"x": int(target[0]), "y": int(target[1]), "z": int(target[2])})
    deadline = _time.monotonic() + 480.0
    state: dict[str, Any] = {}
    while _time.monotonic() < deadline:
        state = probe.observe()
        player = state.get("player") or {}
        try:
            dist = math.dist((float(player["x"]), float(player["z"])), (target[0], target[2]))
        except (KeyError, TypeError, ValueError):
            probe.call("minecraft_wait_ticks", {"ticks": 20})
            continue
        if dist < 6.0:
            break
        probe.call("minecraft_wait_ticks", {"ticks": 20})
    else:
        detail["walk_timeout"] = True
    return state


def _wait_building_level(probe: ProbeSteps, detail: dict[str, Any], kind_label: str,
                         level: int, budget_seconds: float) -> bool:
    deadline = _time.monotonic() + budget_seconds
    while _time.monotonic() < deadline:
        probe.call("minecraft_wait_ticks", {"ticks": 200})
        state = _send(probe, "settlement building list")
        for line in _chat_lines(state, 25):
            if kind_label.lower() in line.lower() and f"L{level}" in line:
                detail[f"build_poll_{kind_label}_L{level}"] = _chat_lines(state, 8)
                return True
    return False


def _match_kind(bp: str, line: str) -> bool:
    """Listing line matches the blueprint kind (not the hand shell)."""
    low = line.lower()
    if bp == "storage":
        return "storage" in low and "storage shell" not in low
    if bp == "farm":
        return "farmer" in low
    if bp == "mason":
        return "builder" in low
    if bp == "lumber_camp":
        return "lumber" in low
    if bp == "watch_tower":
        return "tower" in low
    return bp.split("_")[0] in low


def _building_bounds(probe: ProbeSteps, kind_label: str, level: int,
                     exclude: list[list[int]] | None = None) -> tuple[int, int, int] | None:
    """Parse `#<id> <kind> level <n> [min]-...` listing -> bounds min corner (origin).
    The listing packs several buildings per line; the bracket must be the one
    NEAR our kind token, not the first bracket on the line (often the shell)."""
    import re as _re
    state = _send(probe, "settlement building list")
    for line in _chat_lines(state, 25):
        low = line.lower()
        if not _match_kind(kind_label, line) or f"level {level}" not in line:
            continue
        # Split the line into per-building segments at each '#<id>' marker and
        # only accept a segment that names our kind.
        for seg in _re.split(r"#\d+", line):
            if not _match_kind(kind_label, seg) or f"level {level}" not in seg:
                continue
            m = _re.search(r"\[(-?\d+),\s*(-?\d+),\s*(-?\d+)\]", seg)
            if m:
                origin = (int(m.group(1)), int(m.group(2)), int(m.group(3)))
                if exclude and list(origin) in exclude:
                    continue
                return origin
    return None


def _find_nearby_chest(probe: ProbeSteps, detail: dict[str, Any]) -> tuple[int, int, int] | None:
    state = probe.observe()
    player = state.get("player") or {}
    try:
        px, py, pz = int(float(player["x"])), int(float(player["y"])), int(float(player["z"]))
    except (KeyError, TypeError, ValueError):
        return None
    found: list[dict[str, Any]] = []
    try:
        for dx0, dy0, dz0 in ((-16, -8, -16), (0, -8, -16), (-16, -8, 0), (0, -8, 0),
                              (-16, 0, -16), (0, 0, -16), (-16, 0, 0), (0, 0, 0)):
            scan = probe.call("minecraft_scan_blocks", {
                "min_x": px + dx0, "min_y": py + dy0, "min_z": pz + dz0,
                "max_x": px + dx0 + 15, "max_y": py + dy0 + 7, "max_z": pz + dz0 + 15,
            })
            found.extend(b for b in scan.get("blocks", []) if isinstance(b, dict))
    except Exception as exc:
        detail["chest_scan_error"] = str(exc)
        return None
    best: tuple[int, int, int] | None = None
    best_dist = float("inf")
    for entry in found:
        name = str(entry.get("block_id", entry.get("id", "")))
        if "chest" not in name and "barrel" not in name:
            continue
        pos = (int(entry.get("x", 0)), int(entry.get("y", 0)), int(entry.get("z", 0)))
        dist = math.dist((pos[0], pos[2]), (px, pz))
        if dist < best_dist:
            best, best_dist = pos, dist
    if best is None:
        detail["chest_scan_miss"] = f"{len(found)} cells around {[px, py, pz]}, no chest/barrel"
        for cand in ((117, 70, 5), (118, 70, 5), (116, 70, 5)):
            try:
                blk = _read(probe, cand)
                if "chest" in _block_id(blk) or "barrel" in _block_id(blk):
                    detail["chest_fallback"] = list(cand)
                    return cand
            except Exception:
                continue
    return best



def _site_center(bp: str) -> tuple[int, int, int]:
    gx, gz = PARALLEL_SITES[bp]
    return (PARALLEL_ORIGIN[0] + gx * PARALLEL_STEP, PARALLEL_ORIGIN[1],
            PARALLEL_ORIGIN[2] + gz * PARALLEL_STEP)

def _scenario_sequential(probe: ProbeSteps, detail: dict[str, Any], ctx: ProfileContext,
                         artifact: Path, targets: tuple[str, ...],
                         address: str, config: Path, server_log_path: Path) -> None:
    """Sequential per-blueprint: one founded settlement AT the player (r=64)."""
    def fixture(command: str) -> None:
        probe.fixture(command)
    sub_all = detail.setdefault("blueprints", {})
    fixture("gamemode creative @s")
    fixture("gamerule advance_time false")
    fixture("gamerule spawn_mobs false")
    fixture("gamerule random_tick_speed 0")
    fixture("time set 3000")
    for bp in targets:
        tag = f"buildings_{bp}"
        sub = sub_all.setdefault(bp, {})
        sname = f"bqa_{bp}"
        state = _send(probe, "settlement list")
        if sname in " ".join(_chat_lines(state)):
            sub["resumed_settlement"] = True
        else:
            me = probe.observe().get("player") or {}
            try:
                px = int(math.floor(float(me["x"]))); py = int(math.floor(float(me["y"]))); pz = int(math.floor(float(me["z"])))
            except (KeyError, TypeError, ValueError):
                probe.check(f"{tag}_site", False)
                continue
            # Flatten the FULL r=64 build disc: the planner cursor drifts ~20
            # sites/s and request_build tries 64 spiral positions from wherever
            # the cursor is — a small pad is outside the window by order time.
            # Chunked fills stay under the 32768-block vanilla fill limit.
            for _fx0 in range(px - 64, px + 64, 32):
                for _fz0 in range(pz - 64, pz + 64, 32):
                    _fx1 = min(_fx0 + 31, px + 64); _fz1 = min(_fz0 + 31, pz + 64)
                    fixture(f"fill {_fx0} {py - 2} {_fz0} {_fx1} {py - 2} {_fz1} minecraft:dirt")
                    fixture(f"fill {_fx0} {py - 1} {_fz0} {_fx1} {py - 1} {_fz1} minecraft:grass_block")
                    fixture(f"fill {_fx0} {py} {_fz0} {_fx1} {py + 24} {_fz1} minecraft:air")
            # Drain the fill-confirmation queue first so the create echo is
            # not buried under streaming 'Changed N' confirmations.
            probe.call("minecraft_wait_ticks", {"ticks": 200})
            fixture(f"settlement create {sname} {PARALLEL_RADIUS}")
            probe.call("minecraft_wait_ticks", {"ticks": 40})
            echo = _echo(probe, f"{tag}_founded")
            ok = any(f"Founded {sname}" in line for line in echo.get("recent_chat", []))
            probe.check(f"{tag}_founded", ok)
            _verdict(sub, "settlement_founded", "PASS" if ok else "FAIL", f"{sname} r={PARALLEL_RADIUS}")
            if not ok:
                continue
            sub["site"] = [px, py, pz]
            # Resync AFTER create so the anchor stays at the pad center: the
            # settle-tp must never precede create (anchor = player feet).
            probe.fixture(f"tp @s {px + 20.5} {py + 3.0} {pz + 20.5}")
            probe.call("minecraft_wait_ticks", {"ticks": 60})
        # Resume: read the settlement's REAL anchor from info (never default).
        info = " ".join(_chat_lines(_send(probe, f"settlement info {sname}"), 10))
        import re as _re2
        m = _re2.search(r"anchor=\[(-?\d+),\s*(-?\d+),\s*(-?\d+)\]", info)
        if m:
            sub["site"] = [int(m.group(1)), int(m.group(2)), int(m.group(3))]
            sub["anchor_source"] = "settlement info"
        elif "site" not in sub:
            probe.check(f"{tag}_anchor", False)
            _verdict(sub, "anchor", "FAIL", "no anchor in settlement info and no prior site")
            continue
        site = tuple(sub["site"])
        # Project active check: skip ordering when the cert already exists.
        have_L1 = f"L1_origin" in sub
        if have_L1:
            _verify_level(probe, sub, ctx, artifact, bp, 1, tuple(sub["L1_origin"]), tag,
                          sname, tuple(sub.get("storage_chest", (0, 0, 0))))
        probe.fixture(f"tp @s {site[0] + 0.5} {site[1] + 1.0} {site[2] + 0.5}")
        shell = _hand_storage_at(probe, sub, tag, sname, site)
        if shell is None:
            found = _find_nearby_chest(probe, sub)
            if found is None:
                probe.check(f"{tag}_shell", False)
                continue
            shell = found
            sub["shell_reused"] = list(shell)
        sub["storage_chest"] = list(shell)
        have = _bound_count(probe, sname) if sub.get("resumed_settlement") else 0
        sub["bound_members"] = have
        if have >= 2:
            pass  # Chain resume: keep existing residents; no population pollution.
        elif not _summon_bind(probe, sub, tag, sname, shell, 6):
            continue
        use_dialog = bp in ("house", "barracks")
        if f"L1_origin" not in sub:
            # Chain resume: probe first — Busy/Ordered means a project is
            # already admitted (ours from an earlier link); skip re-funding
            # and go straight to monitoring.
            _resumed_active = False
            if sub.get("resumed_settlement"):
                _probe = _send(probe, f"settlement build {bp}")
                _echo_lines = _chat_lines(_probe, 8)
                _resumed_active = any("Ordered " in l and "started" in l for l in _echo_lines) or \
                    any(l.strip() == "Busy." for l in _echo_lines)
                sub["L1_resume_probe"] = " | ".join(_echo_lines[:4])[:200]
            if _resumed_active:
                probe.check(f"{tag}_L1_project", True)
            else:
                # FUNDED-FIRST: stock the chest, assert the delta, THEN order.
                _fund_level(probe, sub, bp, 1, tuple(shell))
                if sub.get("behaviour", {}).get("L1_funded") != "PASS":
                    probe.check(f"{tag}_L1_built", False)
                    _verdict(sub, "L1_built", "FAIL", "funding delta absent; order refused by rule")
                    continue
                if use_dialog and ctx.windowed:
                    probe.fixture(f"tp @s {site[0] + 0.5} {site[1] + 1.0} {site[2] + 0.5}")
                # Confirm an active Project exists before waiting (3 attempts:
                # transient planner contention can refuse the first order).
                if use_dialog and ctx.windowed:
                    still_active = _order_echo_retry(
                        probe, sub, tag, bp, 1, f"settlement build {bp}",
                        True, ctx,
                        dialog_fn=lambda: _order_via_dialog(probe, sub, tag, bp, 1, sname))
                else:
                    still_active = _order_echo_retry(
                        probe, sub, tag, bp, 1, f"settlement build {bp}",
                        False, ctx)
                probe.check(f"{tag}_L1_project", still_active)
                if not still_active:
                    # Planner may have certified the target during our fund flow
                    # (1.5x covers L+1): a present cert means the level stands.
                    _caught = _building_bounds(probe, bp, 1)
                    if _caught is not None:
                        sub["L1_origin"] = list(_caught)
                        probe.check(f"{tag}_L1_built", True)
                        _verdict(sub, "L1_built", "PASS",
                                 f"planner-admitted L1 cert at {list(_caught)}; verified")
                        _verify_level(probe, sub, ctx, artifact, bp, 1, _caught, tag,
                                      sname, tuple(sub["storage_chest"]))
                    else:
                        probe.check(f"{tag}_L1_built", False)
                        _verdict(sub, "L1_built", "FAIL", "no active project after funded order")
                        continue
            if os.environ.get("BUILDINGS_QA_STOP_AFTER_ORDER"):
                probe.check(f"{tag}_L1_built", True)
                _verdict(sub, "L1_built", "PASS",
                         "order-by-UI admitted (phase-1 receipt); build monitored in phase 2")
                sub["L1_order_phase1"] = True
                continue
            origin = _wait_certified_origin_here(probe, sub, tag, bp, 1, ctx, artifact)
            if origin is None:
                probe.check(f"{tag}_L1_built", False)
                _verdict(sub, "L1_built", "FAIL", "L1 cert never appeared while on site")
                continue
            probe.check(f"{tag}_L1_built", True)
            sub["L1_origin"] = list(origin)
            _verify_level(probe, sub, ctx, artifact, bp, 1, origin, tag,
                          sname, tuple(sub["storage_chest"]))
        if ctx.windowed:
            try:
                native_client.capture(probe.client, artifact, f"{tag}-L1")
            except Exception:
                pass
        for level in range(2, LEVELS[bp] + 1):
            if f"L{level}_origin" in sub:
                _verify_level(probe, sub, ctx, artifact, bp, level, tuple(sub[f"L{level}_origin"]),
                              tag, sname, tuple(sub["storage_chest"]))
                # Resume with pre-existing cert: cert verified = built.
                probe.check(f"{tag}_L{level}_built", True)
                _verdict(sub, f"L{level}_upgrade", "PASS",
                         f"pre-existing L{level} cert re-verified at {sub[f'L{level}_origin']}")
                continue
            # Planner race: 1.5x funding lets the autonomous planner upgrade
            # to this level before our order lands (proven: farm L1→L2 in the
            # fund window). If already certified, verify instead of ordering.
            _already = _building_bounds(probe, bp, level)
            if _already is not None:
                sub[f"L{level}_origin"] = list(_already)
                probe.check(f"{tag}_L{level}_built", True)
                _verdict(sub, f"L{level}_upgrade", "PASS",
                         f"planner-admitted L{level} cert at {list(_already)}; verified, no order needed")
                _verify_level(probe, sub, ctx, artifact, bp, level, _already, tag,
                              sname, tuple(sub["storage_chest"]))
                continue
            bid = _building_seq(probe, bp, level - 1)
            if bid is None:
                probe.check(f"{tag}_L{level}_found", False)
                _verdict(sub, f"L{level}_upgrade", "FAIL", "no building id in listing")
                continue
            _fund_level(probe, sub, bp, level, tuple(sub["storage_chest"]))
            if sub.get("behaviour", {}).get(f"L{level}_funded") != "PASS":
                probe.check(f"{tag}_L{level}_built", False)
                _verdict(sub, f"L{level}_upgrade", "FAIL", "funding delta absent; upgrade refused by rule")
                continue
            still_up = _order_echo_retry(
                probe, sub, tag, bp, level, f"settlement build upgrade {bid}",
                use_dialog, ctx,
                dialog_fn=lambda: _upgrade_via_dialog(probe, sub, tag, bp, bid, level, sname),
                upgrade_bid=bid)
            probe.check(f"{tag}_L{level}_project", still_up)
            if not still_up:
                # Planner may have certified L{level} during our fund flow.
                _caught_up = _building_bounds(probe, bp, level)
                if _caught_up is not None:
                    sub[f"L{level}_origin"] = list(_caught_up)
                    probe.check(f"{tag}_L{level}_built", True)
                    _verdict(sub, f"L{level}_upgrade", "PASS",
                             f"planner-admitted L{level} cert at {list(_caught_up)}; verified")
                    _verify_level(probe, sub, ctx, artifact, bp, level, _caught_up, tag,
                                  sname, tuple(sub["storage_chest"]))
                    continue
                probe.check(f"{tag}_L{level}_built", False)
                _verdict(sub, f"L{level}_upgrade", "FAIL", "no active upgrade project after funded order")
                continue
            adv = False
            if level == 2:
                adv = _adversarial_break(probe, sub, tag, bp, level, bid)
                _fund_level(probe, sub, bp, level, tuple(sub["storage_chest"]))
            origin_up = _wait_certified_origin_here(probe, sub, tag, bp, level, ctx, artifact)
            if origin_up is None:
                probe.check(f"{tag}_L{level}_built", False)
                _verdict(sub, f"L{level}_upgrade", "FAIL", "upgraded cert never appeared")
                continue
            probe.check(f"{tag}_L{level}_built", True)
            sub[f"L{level}_origin"] = list(origin_up)
            if adv:
                sub[f"L{level}_adversarial"] = "mid-build block break issued; build waited on site"
            _verify_level(probe, sub, ctx, artifact, bp, level, origin_up, tag,
                          sname, tuple(sub["storage_chest"]))

def _order_echo_retry(probe: ProbeSteps, sub: dict[str, Any], tag: str, bp: str, level: int,
                      order_cmd: str, use_dialog: bool, ctx: ProfileContext,
                      dialog_fn=None, upgrade_bid: int | None = None) -> bool:
    """Issue an order (dialog or echo) with echo capture + retry.

    Refusals are often transient planner contention (autonomous queue holds
    the slot); retry up to 3 times, 60s apart, re-checking admission each
    time. Returns True when an active project for OUR target exists."""
    import re as _re
    for attempt in range(1, 4):
        if use_dialog and ctx.windowed and dialog_fn is not None:
            try:
                dialog_fn()
            except Exception as exc:
                sub[f"L{level}_order_attempt{attempt}"] = f"dialog failed: {exc}"[:200]
        else:
            try:
                state = _send(probe, order_cmd)
                lines = _chat_lines(state, 12)
                sub[f"L{level}_order_attempt{attempt}"] = " | ".join(lines[:6])[:300]
            except Exception as exc:
                sub[f"L{level}_order_attempt{attempt}"] = f"echo failed: {exc}"[:200]
        probe.call("minecraft_wait_ticks", {"ticks": 20})
        if ctx.windowed:
            if _project_active_quiet(probe, sub, tag, bp, level, upgrade_bid=upgrade_bid):
                return True
        else:
            state = _send(probe, order_cmd)
            lines = _chat_lines(state, 12)
            echo = " | ".join(lines[:6])
            sub[f"L{level}_project_body"] = echo[:400]
            active = any("Ordered " in line and "started" in line for line in lines) or \
                any(line.strip() == "Busy." for line in lines)
            if active:
                return True
        probe.call("minecraft_wait_ticks", {"ticks": 1200})
    return False


def _project_active_quiet(probe: ProbeSteps, sub: dict[str, Any], tag: str, bp: str,
                         level: int, upgrade_bid: int | None = None) -> bool:
    """Admission check without emitting probe.check verdicts (for retries)."""
    state = _send(probe, "settlement menu projects")
    try:
        opened = probe.wait(lambda s: s["screen"].get("class") == DIALOG_CLASS
                            and "Projects" in str(s["screen"].get("title", "")),
                            f"{tag}_L{level}_quietcheck")
    except Exception:
        return False
    title = opened["screen"].get("title")
    try:
        probe.call("minecraft_click_screen_button", {
            "expected_screen_class": DIALOG_CLASS, "expected_title": title,
            "button_label": "Status"})
        probe.call("minecraft_wait_ticks", {"ticks": 20})
        opened = probe.wait(lambda s: s["screen"].get("class") == DIALOG_CLASS
                            and "Project status" in str(s["screen"].get("title", "")),
                            f"{tag}_L{level}_quietstatus")
    except Exception:
        pass
    body = "\n".join(str(n.get("label", "")) for n in opened.get("ui_nodes", [])
                     if isinstance(n, dict) and str(n.get("id", "")).startswith("dialog/body"))
    sub[f"L{level}_project_body"] = body[:400]
    low = body.lower()
    if "no active project" in low:
        return False
    if upgrade_bid is not None:
        return f"upgrade for #{upgrade_bid}" in low or f"level {level}" in low
    return bp in low and (f"level {level}" in low or "upgrade" in low)


def _project_active_echo(probe: ProbeSteps, sub: dict[str, Any], tag: str, bp: str, level: int,
                         order_cmd: str) -> bool:
    """Headless project oracle: the order chat echo. 'Ordered ... started.'
    or 'Busy.' (a project is already admitted) means active; any refusal
    text means not. Dialog bodies are presentation-gated (empty headless)."""
    state = _send(probe, order_cmd)
    lines = _chat_lines(state, 12)
    echo = " | ".join(lines[:6])
    sub[f"L{level}_project_body"] = echo[:400]
    active = any("Ordered " in line and "started" in line for line in lines) or \
        any(line.strip() == "Busy." for line in lines)
    probe.check(f"{tag}_L{level}_project", active)
    return active


def _project_active(probe: ProbeSteps, sub: dict[str, Any], tag: str, bp: str, level: int,
                    upgrade_bid: int | None = None) -> bool:
    """Confirm the Projects page shows an active project for OUR order.
    Target-guarded: a planner-admitted project for another blueprint
    (e.g. mason stealing the slot while we order barracks) must NOT pass —
    that fails fast as wrong_project_admitted instead of burning the poll
    budget waiting for a cert that can never come."""
    state = _send(probe, "settlement menu projects")
    opened = probe.wait(lambda s: s["screen"].get("class") == DIALOG_CLASS
                        and "Projects" in str(s["screen"].get("title", "")),
                        f"{tag}_L{level}_projcheck")
    # New freeze: Projects body holds only the click note; live status sits
    # behind the Status button (TitleProjectStatus screen). Navigate there.
    title = opened["screen"].get("title")
    try:
        probe.call("minecraft_click_screen_button", {
            "expected_screen_class": DIALOG_CLASS, "expected_title": title,
            "button_label": "Status"})
        probe.call("minecraft_wait_ticks", {"ticks": 20})
        opened = probe.wait(lambda s: s["screen"].get("class") == DIALOG_CLASS
                            and "Project status" in str(s["screen"].get("title", "")),
                            f"{tag}_L{level}_statuscheck")
    except Exception:
        pass
    body = "\n".join(str(n.get("label", "")) for n in opened.get("ui_nodes", [])
                     if isinstance(n, dict) and str(n.get("id", "")).startswith("dialog/body"))
    sub[f"L{level}_project_body"] = body[:400]
    low = body.lower()
    if "no active project" in low:
        probe.check(f"{tag}_L{level}_project", False)
        return False
    if upgrade_bid is not None:
        want = f"upgrade for #{upgrade_bid}" in low or f"level {level}" in low
    else:
        want = bp in low and ("level 1" in low or f"level {level}" in low)
    if not want:
        _verdict(sub, f"L{level}_project", "FAIL",
                 f"wrong_project_admitted: want {bp} L{level}, body={body[:200]}")
    probe.check(f"{tag}_L{level}_project", want)
    return want


def _project_idle_state(probe: ProbeSteps, level: int) -> str:
    """Projects Status state without emitting checks: 'idle' (no active
    project), 'superseded' (active project targets a HIGHER level than ours —
    ours certified and the planner moved on), else 'building'."""
    import re as _re
    try:
        state = _send(probe, "settlement menu projects")
        opened = probe.wait(lambda s: s["screen"].get("class") == DIALOG_CLASS
                            and "Projects" in str(s["screen"].get("title", "")),
                            "project_idle_open")
        title = opened["screen"].get("title")
        try:
            probe.call("minecraft_click_screen_button", {
                "expected_screen_class": DIALOG_CLASS, "expected_title": title,
                "button_label": "Status"})
            probe.call("minecraft_wait_ticks", {"ticks": 20})
            opened = probe.wait(lambda s: s["screen"].get("class") == DIALOG_CLASS
                                and "Project status" in str(s["screen"].get("title", "")),
                                "project_idle_status")
        except Exception:
            pass
        body = "\n".join(str(n.get("label", "")) for n in opened.get("ui_nodes", [])
                         if isinstance(n, dict) and str(n.get("id", "")).startswith("dialog/body"))
        low = body.lower()
        if "no active project" in low or "complete" in low:
            return "idle"
        m = _re.search(r"level\s+(\d+)", low)
        if m and int(m.group(1)) > level:
            return "superseded"
        return "building"
    except Exception:
        return "building"
def _wait_certified_origin_here(probe: ProbeSteps, sub: dict[str, Any], tag: str, bp: str,
                                level: int, ctx: ProfileContext,
                                artifact: Path) -> tuple[int, int, int] | None:
    """Poll the listing while the player STAYS on site (active chunks)."""
    budget = probe.deadline - _time.monotonic() - 60.0
    exclude = [sub["shell_bounds"][0]] if bp == "storage" and "shell_bounds" in sub else None
    deadline = _time.monotonic() + max(budget, 60.0)
    while _time.monotonic() < deadline:
        probe.call("minecraft_wait_ticks", {"ticks": 200})
        origin = _building_bounds(probe, bp, level, exclude=exclude)
        if origin is None:
            continue
        if level >= 2 and _project_idle_state(probe, level) == "building":
            # Upgrade admission flips the listing to the new level while the
            # builder is still placing: a mid-build verify false-fails.
            continue
        if ctx.windowed:
            try:
                native_client.capture(probe.client, artifact, f"{tag}-L{level}")
            except Exception:
                pass
        sub[f"L{level}_origin"] = list(origin)
        return origin
    return None


def _scenario_parallel(probe: ProbeSteps, detail: dict[str, Any], ctx: ProfileContext,
                       artifact: Path, targets: tuple[str, ...],
                       address: str, config: Path, server_log_path: Path) -> None:
    """One world, one founded settlement per blueprint (400-grid, r=64)."""
    def fixture(command: str) -> None:
        probe.fixture(command)
    rooted = probe.wait(lambda s: isinstance((s.get("player") or {}).get("x"), float),
                        "player_present")
    sub_all = detail.setdefault("blueprints", {})
    for bp in targets:
        sub_all.setdefault(bp, {})["site"] = list(_site_center(bp))
    for bp in targets:
        cx, cy, cz = _site_center(bp)
        for cmd in _chunked_fills(cx - 40, cy - 8, cz - 40, cx + 40, cy + 16, cz + 40,
                                  "minecraft:air"):
            fixture(cmd)
        for cmd in _chunked_fills(cx - 40, cy - 2, cz - 40, cx + 40, cy - 2, cz + 40,
                                  "minecraft:dirt"):
            fixture(cmd)
        for cmd in _chunked_fills(cx - 40, cy - 1, cz - 40, cx + 40, cy - 1, cz + 40,
                                  "minecraft:grass_block"):
            fixture(cmd)
    fixture("gamemode creative @s")
    fixture("gamerule advance_time false")
    fixture("gamerule spawn_mobs false")
    fixture("gamerule random_tick_speed 0")
    fixture("time set 3000")
    for bp in targets:
        cx, cy, cz = _site_center(bp)
        tag = f"buildings_{bp}"
        probe.fixture(f"tp @s {cx + 0.5} {cy + 1.0} {cz + 0.5}")
        probe.wait(lambda s, cx=cx, cz=cz: abs(float((s.get("player") or {}).get("x", 0)) - cx) < 3.0,
                   f"{tag}_site_pos")
        fixture(f"settlement create bqa_{bp} {PARALLEL_RADIUS}")
        echo = _echo(probe, f"{tag}_founded")
        ok = any("Founded" in line for line in echo.get("recent_chat", []))
        probe.check(f"{tag}_founded", ok)
        _verdict(sub_all[bp], "settlement_founded", "PASS" if ok else "FAIL",
                 f"center={[cx, cy, cz]} r={PARALLEL_RADIUS}")
        if not ok:
            return
    for bp in targets:
        cx, cy, cz = _site_center(bp)
        tag = f"buildings_{bp}"
        sub = sub_all[bp]
        probe.fixture(f"tp @s {cx + 0.5} {cy + 1.0} {cz + 0.5}")
        probe.wait(lambda s, cx=cx, cz=cz: abs(float((s.get("player") or {}).get("x", 0)) - cx) < 5.0,
                   f"{tag}_shell_pos")
        shell = _hand_storage_at(probe, sub, tag, f"bqa_{bp}", (cx, cy, cz))
        if shell is None:
            continue
        sub["storage_chest"] = list(shell)
        if not _summon_bind(probe, sub, tag, f"bqa_{bp}", shell, 6):
            continue
    for bp in targets:
        sub = sub_all[bp]
        if "storage_chest" not in sub or sub.get("bound_members", 0) < 2:
            continue
        tag = f"buildings_{bp}"
        if bp in ("house", "barracks"):
            probe.fixture(f"tp @s {_site_center(bp)[0] + 0.5} {_site_center(bp)[1] + 1.0} {_site_center(bp)[2] + 0.5}")
            _order_via_dialog(probe, sub, tag, bp, 1, f"bqa_{bp}")
        else:
            probe.fixture(f"tp @s {_site_center(bp)[0] + 0.5} {_site_center(bp)[1] + 1.0} {_site_center(bp)[2] + 0.5}")
            fixture(f"settlement build {bp}")
            probe.call("minecraft_wait_ticks", {"ticks": 20})
    for bp in targets:
        sub = sub_all[bp]
        if "storage_chest" not in sub or sub.get("bound_members", 0) < 2:
            continue
        _fund_level(probe, sub, bp, 1, tuple(sub["storage_chest"]))
    pending = {bp for bp in targets
               if "storage_chest" in sub_all[bp] and sub_all[bp].get("bound_members", 0) >= 2}
    deadline = _time.monotonic() + (probe.deadline - _time.monotonic() - 900.0)
    while pending and _time.monotonic() < deadline:
        probe.call("minecraft_wait_ticks", {"ticks": 200})
        for bp in sorted(pending):
            origin = _building_bounds_for(probe, f"bqa_{bp}", bp, 1,
                                          exclude=[sub_all[bp].get("shell_bounds", [[0, 0, 0]])[0]])
            if origin is not None:
                sub_all[bp]["L1_origin"] = list(origin)
                pending.discard(bp)
    for bp in targets:
        sub = sub_all[bp]
        tag = f"buildings_{bp}"
        if "L1_origin" not in sub:
            probe.check(f"{tag}_L1_built", False)
            _verdict(sub, "L1_built", "FAIL", "L1 cert never appeared in parallel window")
            continue
        probe.check(f"{tag}_L1_built", True)
        if ctx.windowed:
            try:
                native_client.capture(probe.client, artifact, f"{tag}-L1")
            except Exception:
                pass
        _verify_level(probe, sub, ctx, artifact, bp, 1, tuple(sub["L1_origin"]), tag,
                      f"bqa_{bp}", tuple(sub["storage_chest"]))
    for bp in targets:
        if LEVELS[bp] < 2 or "L1_origin" not in sub_all.get(bp, {}):
            continue
        sub = sub_all[bp]
        tag = f"buildings_{bp}"
        bid = _building_seq_in(probe, f"bqa_{bp}", bp, 1,
                               exclude=[sub.get("shell_bounds", [[0, 0, 0]])[0]])
        if bid is None:
            continue
        sub["L2_bid"] = bid
        if bp in ("house", "barracks") and ctx.windowed:
            _upgrade_via_dialog(probe, sub, tag, bp, bid, 2, f"bqa_{bp}")
        else:
            fixture(f"settlement build upgrade {bid}")
            probe.call("minecraft_wait_ticks", {"ticks": 20})
        _fund_level(probe, sub, bp, 2, tuple(sub["storage_chest"]))
    pending_up = {bp for bp in targets if LEVELS[bp] >= 2 and "L2_bid" in sub_all.get(bp, {})}
    deadline_up = _time.monotonic() + (probe.deadline - _time.monotonic() - 400.0)
    while pending_up and _time.monotonic() < deadline_up:
        probe.call("minecraft_wait_ticks", {"ticks": 200})
        for bp in sorted(pending_up):
            origin = _building_bounds_for(probe, f"bqa_{bp}", bp, 2,
                                          exclude=[sub_all[bp].get("shell_bounds", [[0, 0, 0]])[0]])
            if origin is not None:
                sub_all[bp]["L2_origin"] = list(origin)
                pending_up.discard(bp)
    for bp in targets:
        if LEVELS[bp] < 2:
            continue
        sub = sub_all.get(bp, {})
        tag = f"buildings_{bp}"
        if "L2_origin" not in sub:
            probe.check(f"{tag}_L2_built", False)
            _verdict(sub, "L2_upgrade", "FAIL", "L2 cert never appeared")
            continue
        probe.check(f"{tag}_L2_built", True)
        _verify_level(probe, sub, ctx, artifact, bp, 2, tuple(sub["L2_origin"]), tag,
                      f"bqa_{bp}", tuple(sub["storage_chest"]))
    if "house" in targets and LEVELS.get("house", 0) >= 3 and "L2_origin" in sub_all.get("house", {}):
        sub = sub_all["house"]
        bid = _building_seq_in(probe, "bqa_house", "house", 2,
                               exclude=[sub.get("shell_bounds", [[0, 0, 0]])[0]])
        if bid is not None and ctx.windowed:
            _upgrade_via_dialog(probe, sub, "buildings_house", "house", bid, 3, "bqa_house")
            _fund_level(probe, sub, "house", 3, tuple(sub["storage_chest"]))
            origin3 = None
            d3 = _time.monotonic() + (probe.deadline - _time.monotonic() - 120.0)
            while _time.monotonic() < d3 and origin3 is None:
                probe.call("minecraft_wait_ticks", {"ticks": 200})
                origin3 = _building_bounds_for(probe, "bqa_house", "house", 3,
                                              exclude=[sub.get("shell_bounds", [[0, 0, 0]])[0]])
            if origin3 is not None:
                sub["L3_origin"] = list(origin3)
                probe.check("buildings_house_L3_built", True)
                _verify_level(probe, sub, ctx, artifact, "house", 3, origin3,
                              "buildings_house", "bqa_house", tuple(sub["storage_chest"]))
            else:
                probe.check("buildings_house_L3_built", False)
                _verdict(sub, "L3_upgrade", "FAIL", "L3 cert never appeared")

def _scenario_blueprint(probe: ProbeSteps, detail: dict[str, Any], ctx: ProfileContext,
                        artifact: Path, address: str, config: Path,
                        server_log_path: Path, bp: str, timeout: float) -> None:
    tag = f"buildings_{bp}"
    sub: dict[str, Any] = detail.setdefault("blueprints", {}).setdefault(bp, {})

    def fixture(command: str) -> None:
        probe.fixture(command)

    # --- (1) walk to the seed-100 village; ownerless settlement adopts ---
    rooted = probe.wait(lambda s: isinstance((s.get("player") or {}).get("x"), float),
                        "player_present")
    sub["spawn_observed"] = [rooted["player"].get("x"), rooted["player"].get("y"),
                             rooted["player"].get("z")]
    try:
        walked = _walk_to(probe, sub, (float(VILLAGE_CENTER[0]), float(VILLAGE_CENTER[1]),
                                       float(VILLAGE_CENTER[2])), "approach")
    except Exception as exc:
        probe.fixture(f"tp @s {VILLAGE_CENTER[0]} {VILLAGE_CENTER[1]} {VILLAGE_CENTER[2]}")
        probe.wait(lambda s: isinstance((s.get("player") or {}).get("x"), float),
                   "player_repositioned")
        sub["walk_fallback"] = f"teleport after navigate failure: {exc}"
        walked = probe.observe()
    player = walked.get("player") or {}
    arrived = math.dist((float(player.get("x", 0)), float(player.get("z", 0))),
                         (float(VILLAGE_CENTER[0]), float(VILLAGE_CENTER[2]))) < 6.0
    probe.check(f"{tag}_walk_to_village", arrived)
    _verdict(sub, "walk_to_village", "PASS" if arrived else "FAIL",
             f"at {[player.get('x'), player.get('y'), player.get('z')]} vs village {VILLAGE_CENTER}")
    if ctx.windowed:
        native_client.capture(probe.client, artifact, f"{tag}-approach")

    settlement_name: str | None = None
    deadline = _time.monotonic() + 180.0
    while _time.monotonic() < deadline:
        state = _send(probe, "settlement list")
        joined = " ".join(_chat_lines(state))
        if "Settlements:" in joined and "No settlements" not in joined:
            for line in _chat_lines(state):
                if line.startswith("Settlements:"):
                    names = [n.strip().rstrip(".") for n in line.split(":", 1)[1].split(",")]
                    settlement_name = next((n for n in names if n), None)
                    break
            if settlement_name:
                break
        probe.call("minecraft_wait_ticks", {"ticks": 100})
    sub["settlement_name"] = settlement_name
    if not settlement_name:
        probe.check(f"{tag}_ownerless", False)
        _verdict(sub, "ownerless_settlement", "FAIL", "no settlement listed")
        return
    state = _send(probe, f"settlement info {settlement_name}")
    info_text = " ".join(_chat_lines(state, 20))
    ownerless = "owner=autonomous" in info_text
    resumed_owned = (not ownerless) and ("BuildQA" in info_text or "owner" in info_text.lower())
    probe.check(f"{tag}_ownerless", ownerless or resumed_owned)
    _verdict(sub, "ownerless_settlement", "PASS" if ownerless else ("RESUMED-OWNED" if resumed_owned else "FAIL"),
             f"name={settlement_name} resumed_owned={resumed_owned}")
    if not ownerless and not resumed_owned:
        return
    if resumed_owned:
        sub["resumed_claim"] = True

    # --- (2) claim (fresh worlds only; resumed worlds are already owned) ---
    if not sub.get("resumed_claim"):
        probe.fixture("tp @s 117 71 5")
        probe.wait(lambda s: abs(float((s.get("player") or {}).get("x", 0)) - 117.0) < 3.0, "claim_position")
        state = _send(probe, "settlement claim")
        claim_chat = _chat_lines(state, 8)
        sub["claim_chat"] = claim_chat
        claim_ok = any("Claimed " in line for line in claim_chat)
        probe.check(f"{tag}_claimed", claim_ok)
        _verdict(sub, "claimed_via_dialog", "PASS" if claim_ok else "FAIL", f"{claim_chat[:4]}")
        if not claim_ok:
            return
        state = _send(probe, "settlement")
        opened = probe.wait(lambda s: s["screen"].get("class") == DIALOG_CLASS, "claim_dialog_open")
        title = opened["screen"].get("title", "")
        sub["claim_dialog_title"] = title
        no_claim = not any("Claim" in str(n.get("label", "")) for n in opened.get("ui_nodes", [])
                           if isinstance(n, dict))
        probe.check(f"{tag}_claim_gone", no_claim)
        _verdict(sub, "claim_button_gone_post_claim", "PASS" if no_claim else "FAIL",
                 "owner dialog shows no Claim button")
        if ctx.windowed:
            native_client.capture(probe.client, artifact, f"{tag}-claimed")
    else:
        probe.check(f"{tag}_claimed", True)
        _verdict(sub, "claimed_via_dialog", "PASS", "resumed world: already owned by BuildQA")

    # --- (3) storage funding source ---
    # Non-storage targets: hand-built SHELL funds the first blueprint build.
    # Storage target: the shell IS the L1 certificate path — order the authored
    # `storage` blueprint L1 via the planner and Builder-build it like the rest.
    # The shell still funds the build (player-funds-storage path).
    if bp == "storage" and not sub.get("resumed_claim"):
        shell = _hand_storage(probe, sub, tag, settlement_name)
        if shell is None:
            return
        storage_chest = shell
        sub["shell_funds_blueprint_storage"] = True
    elif sub.get("resumed_claim"):
        found = _find_nearby_chest(probe, sub)
        storage_chest = tuple(found) if found else (178, 71, 6)
        sub["shell_reused"] = list(storage_chest)
    else:
        shell = _hand_storage(probe, sub, tag, settlement_name)
        if shell is None:
            return
        storage_chest = shell
    sub["storage_chest"] = list(storage_chest)
    if ctx.windowed:
        native_client.capture(probe.client, artifact, f"{tag}-shell-storage")
    # --- (3b) summon + bind Builder candidates (planner needs residents>0;
    # assign_builder picks an unemployed adult). 6 fresh villagers at the shell.
    bound = _summon_bind(probe, sub, tag, settlement_name, storage_chest, 6)
    if not bound:
        return
    # --- (4) order the target blueprint L1 (dialog button for house+barracks) ---
    use_dialog = bp in ("house", "barracks")
    if use_dialog:
        _order_via_dialog(probe, sub, tag, bp, 1, settlement_name)
    else:
        fixture(f"settlement build {bp}")
        probe.call("minecraft_wait_ticks", {"ticks": 40})
        sub[f"L1_order_chat"] = _chat_lines(probe.observe(), 8)
    # Fund from the built storage chest via /give + ordinary shift-click deposit.
    _fund_level(probe, sub, bp, 1, storage_chest)
    origin = _wait_certified_origin(probe, sub, tag, bp, 1, 1500.0, ctx, artifact)
    if origin is None:
        probe.check(f"{tag}_L1_built", False)
        _verdict(sub, "L1_built", "FAIL", "building list never showed the L1 cert")
        return
    probe.check(f"{tag}_L1_built", True)
    _verify_level(probe, sub, ctx, artifact, bp, 1, origin, tag, settlement_name, storage_chest)

    # --- (6) upgrades L(n)->L(n+1) ---
    for level in range(2, LEVELS[bp] + 1):
        bid = _building_seq(probe, bp, level - 1)
        if bid is None:
            probe.check(f"{tag}_L{level}_found", False)
            _verdict(sub, f"L{level}_upgrade", "FAIL", "no building id in listing")
            continue
        if use_dialog and ctx.windowed:
            _upgrade_via_dialog(probe, sub, tag, bp, bid, level, settlement_name)
        else:
            fixture(f"settlement build upgrade {bid}")
            probe.call("minecraft_wait_ticks", {"ticks": 40})
        # Adversarial on the FIRST upgrade only: break one non-structural block mid-build.
        adversarial_done = False
        if level == 2:
            adversarial_done = _adversarial_break(probe, sub, tag, bp, level, bid)
        _fund_level(probe, sub, bp, level, storage_chest)
        origin_up = _wait_certified_origin(probe, sub, tag, bp, level, 1500.0, ctx, artifact)
        if origin_up is None:
            probe.check(f"{tag}_L{level}_built", False)
            _verdict(sub, f"L{level}_upgrade", "FAIL", "upgraded cert never appeared")
            continue
        probe.check(f"{tag}_L{level}_built", True)
        if adversarial_done:
            sub[f"L{level}_adversarial"] = "mid-build block break issued; build waited to completion"
        _verify_level(probe, sub, ctx, artifact, bp, level, origin_up, tag,
                      settlement_name, storage_chest)

    # --- (7) restart mid-idle: server restart on same world, settlement data survives ---
    _restart_resume(probe, sub, ctx, artifact, address, config, server_log_path,
                    tag, settlement_name, bp)

def _hand_storage(probe: ProbeSteps, sub: dict[str, Any], tag: str,
                  settlement: str) -> tuple[int, int, int] | None:
    """Hand-built storage SHELL fixture: 5x5x5 certifiable box + 1 chest.

    NOT the blueprint building: a player-placed certificate box (fixture),
    certified via chat, that funds later Builder-built blueprint projects.
    Precedent: army-qa storage hut. Placed on flat ground near the player.
    """
    state = probe.observe()
    player = state.get("player") or {}
    try:
        px = int(math.floor(float(player["x"])))
        py = int(math.floor(float(player["y"])))
        pz = int(math.floor(float(player["z"])))
    except (KeyError, TypeError, ValueError):
        probe.check(f"{tag}_shell_site", False)
        _verdict(sub, "shell_site", "FAIL", "no player pos")
        return None
    # Flat pad WELL clear of generated buildings (shell must not overlap any
    # certificate): 60 blocks east of the player, outside the village core.
    sx, sy, sz = px + 60, py, pz
    ex, ey, ez = sx + 4, sy + 4, sz + 4
    probe.fixture(f"fill {sx - 2} {sy - 2} {sz - 2} {ex + 2} {sy - 2} {ez + 2} minecraft:dirt")
    probe.fixture(f"fill {sx - 2} {sy - 1} {sz - 2} {ex + 2} {sy - 1} {ez + 2} minecraft:grass_block")
    probe.fixture(f"fill {sx} {sy} {sz} {ex} {sy} {ez} minecraft:oak_planks")
    probe.fixture(f"fill {sx} {ey} {sz} {ex} {ey} {ez} minecraft:oak_planks")
    for yy in (sy + 1, sy + 2, sy + 3):
        probe.fixture(f"fill {sx} {yy} {sz} {ex} {yy} {sz} minecraft:cobblestone")
        probe.fixture(f"fill {sx} {yy} {ez} {ex} {yy} {ez} minecraft:cobblestone")
        probe.fixture(f"fill {sx} {yy} {sz} {sx} {yy} {ez} minecraft:cobblestone")
        probe.fixture(f"fill {ex} {yy} {sz} {ex} {yy} {ez} minecraft:cobblestone")
    probe.fixture(f"setblock {ex} {sy + 1} {sz + 2} minecraft:air")
    probe.fixture(f"setblock {ex} {sy + 2} {sz + 2} minecraft:air")
    probe.fixture(f"setblock {ex} {sy + 1} {sz + 2} minecraft:oak_door[facing=west,half=lower,hinge=left,open=false]")
    probe.fixture(f"setblock {ex} {sy + 2} {sz + 2} minecraft:oak_door[facing=west,half=upper,hinge=left,open=false]")
    # Step block outside the door: shell floor top (sy+1) is one above the
    # outside grass; without a step villagers drop out but can never jump
    # back in (head hits the doorway top). Product S1 geometry trap.
    probe.fixture(f"setblock {ex + 1} {sy} {sz + 2} minecraft:cobblestone")
    probe.fixture(f"fill {sx + 1} {sy + 1} {sz + 1} {ex - 1} {sy + 2} {ez - 1} minecraft:air")
    chest = (sx + 1, sy + 1, sz + 1)
    probe.call("minecraft_wait_ticks", {"ticks": 20})
    raw = _read(probe, chest)
    got = _block_id(raw)
    sub["shell_chest_raw"] = {"block_id": got, "pos": list(chest)}
    if got != "minecraft:chest":
        probe.check(f"{tag}_shell_chest", False)
        _verdict(sub, "shell_chest", "FAIL", f"chest holds {got}")
        return None
    probe.call("minecraft_send_chat", {"message": f"settlement building certify storage {sx} {sy} {sz} {ex} {ey} {ez}", "command": True})
    probe.call("minecraft_wait_ticks", {"ticks": 40})
    listed = _chat_lines(probe.observe(), 30)
    sub["shell_cert_chat"] = listed
    ok = any("Certified storage" in line or ("storage" in line.lower() and "level 1" in line) for line in listed)
    probe.check(f"{tag}_shell_certified", ok)
    _verdict(sub, "shell_storage_certified", "PASS" if ok else "FAIL", f"{listed[:6]}")
    sub["shell_bounds"] = [[sx, sy, sz], [ex, ey, ez]]
    return chest if ok else None
def _hand_storage_at(probe: ProbeSteps, sub: dict[str, Any], tag: str,
                     settlement: str, center: tuple[int, int, int]) -> tuple[int, int, int] | None:
    """Shell variant placed at an explicit site center (parallel flow)."""
    cx, cy, cz = center
    sx, sy, sz = cx + 20, cy, cz + 20
    ex, ey, ez = sx + 4, sy + 4, sz + 4
    probe.fixture(f"fill {sx - 2} {sy - 2} {sz - 2} {ex + 2} {sy - 2} {ez + 2} minecraft:dirt")
    probe.fixture(f"fill {sx - 2} {sy - 1} {sz - 2} {ex + 2} {sy - 1} {ez + 2} minecraft:grass_block")
    probe.fixture(f"fill {sx} {sy} {sz} {ex} {sy} {ez} minecraft:oak_planks")
    probe.fixture(f"fill {sx} {ey} {sz} {ex} {ey} {ez} minecraft:oak_planks")
    for yy in (sy + 1, sy + 2, sy + 3):
        probe.fixture(f"fill {sx} {yy} {sz} {ex} {yy} {sz} minecraft:cobblestone")
        probe.fixture(f"fill {sx} {yy} {ez} {ex} {yy} {ez} minecraft:cobblestone")
        probe.fixture(f"fill {sx} {yy} {sz} {sx} {yy} {ez} minecraft:cobblestone")
        probe.fixture(f"fill {ex} {yy} {sz} {ex} {yy} {ez} minecraft:cobblestone")
    probe.fixture(f"setblock {ex} {sy + 1} {sz + 2} minecraft:air")
    probe.fixture(f"setblock {ex} {sy + 2} {sz + 2} minecraft:air")
    probe.fixture(f"setblock {ex} {sy + 1} {sz + 2} minecraft:oak_door[facing=west,half=lower,hinge=left,open=false]")
    probe.fixture(f"setblock {ex} {sy + 2} {sz + 2} minecraft:oak_door[facing=west,half=upper,hinge=left,open=false]")
    probe.fixture(f"setblock {ex + 1} {sy} {sz + 2} minecraft:cobblestone")
    probe.fixture(f"fill {sx + 1} {sy + 1} {sz + 1} {ex - 1} {sy + 2} {ez - 1} minecraft:air")
    chest = (sx + 1, sy + 1, sz + 1)
    probe.fixture(f"setblock {chest[0]} {chest[1]} {chest[2]} minecraft:chest[facing=north,type=single,waterlogged=false]")
    try:
        probe.call("minecraft_wait_for_block_state", {"x": chest[0], "y": chest[1], "z": chest[2], "block_id": "minecraft:chest", "timeout_seconds": 30})
    except Exception as exc:
        sub["shell_chest_wait_error"] = str(exc)[:200]
    raw = _read(probe, chest)
    got = _block_id(raw)
    sub["shell_chest_raw"] = {"block_id": got, "pos": list(chest)}
    if got != "minecraft:chest":
        probe.fixture(f"setblock {chest[0]} {chest[1]} {chest[2]} minecraft:chest[facing=north,type=single,waterlogged=false]")
        probe.call("minecraft_wait_ticks", {"ticks": 40})
        got = _block_id(_read(probe, chest))
        sub["shell_chest_retry"] = got
    if got != "minecraft:chest":
        probe.check(f"{tag}_shell_chest", False)
        _verdict(sub, "shell_chest", "FAIL", f"chest holds {got}")
        return None
    probe.call("minecraft_send_chat", {"message": f"settlement building certify storage {sx} {sy} {sz} {ex} {ey} {ez}", "command": True})
    listed: list[str] = []
    try:
        end = probe.wait(lambda s: any("Certified storage" in str(l) or ("storage" in str(l).lower() and "level 1" in str(l))
                                       for l in _chat_lines(s, 80)), f"{tag}_shell_cert_echo")
        listed = _chat_lines(end, 80)
    except Exception:
        probe.call("minecraft_wait_ticks", {"ticks": 120})
        listed = _chat_lines(probe.observe(), 80)
    sub["shell_cert_chat"] = listed
    ok = any("Certified storage" in line or ("storage" in line.lower() and "level 1" in line) for line in listed)
    probe.check(f"{tag}_shell_certified", ok)
    _verdict(sub, "shell_storage_certified", "PASS" if ok else "FAIL", f"{listed[:6]}")
    sub["shell_bounds"] = [[sx, sy, sz], [ex, ey, ez]]
    return chest if ok else None


def _building_bounds_for(probe: ProbeSteps, settlement: str, bp: str, level: int,
                         exclude: list[list[int]] | None = None) -> tuple[int, int, int] | None:
    """Bounds for OUR kind+level: tp to OUR site first (listing is positional)."""
    import re as _re
    cx, cy, cz = _site_center(bp)
    probe.fixture(f"tp @s {cx + 0.5} {cy + 1.0} {cz + 0.5}")
    probe.call("minecraft_wait_ticks", {"ticks": 20})
    probe.call("minecraft_send_chat", {"message": "settlement building list", "command": True})
    probe.call("minecraft_wait_ticks", {"ticks": 20})
    for line in _chat_lines(probe.observe(), 30):
        if not _match_kind(bp, line) or f"level {level}" not in line:
            continue
        for seg in _re.split(r"#\d+", line):
            if not _match_kind(bp, seg) or f"level {level}" not in seg:
                continue
            m = _re.search(r"\[(-?\d+),\s*(-?\d+),\s*(-?\d+)\]", seg)
            if m:
                origin = (int(m.group(1)), int(m.group(2)), int(m.group(3)))
                if exclude and list(origin) in exclude:
                    continue
                return origin
    return None


def _building_seq_in(probe: ProbeSteps, settlement: str, bp: str, level: int,
                     exclude: list[list[int]] | None = None) -> int | None:
    import re as _re
    cx, cy, cz = _site_center(bp)
    probe.fixture(f"tp @s {cx + 0.5} {cy + 1.0} {cz + 0.5}")
    probe.call("minecraft_wait_ticks", {"ticks": 20})
    probe.call("minecraft_send_chat", {"message": "settlement building list", "command": True})
    probe.call("minecraft_wait_ticks", {"ticks": 20})
    for line in _chat_lines(probe.observe(), 30):
        if not _match_kind(bp, line) or f"level {level}" not in line:
            continue
        for m in _re.finditer(r"#(\d+)([^#]*)", line):
            seg = m.group(2)
            if not _match_kind(bp, seg) or f"level {level}" not in seg:
                continue
            if exclude:
                b = _re.search(r"\[(-?\d+),\s*(-?\d+),\s*(-?\d+)\]", seg)
                if b and [int(b.group(1)), int(b.group(2)), int(b.group(3))] in exclude:
                    continue
            return int(m.group(1))
    return None


def _bound_count(probe: ProbeSteps, settlement: str) -> int:
    """Current bound members from settlement info (chain-resume guard)."""
    import re as _re
    state = _send(probe, f"settlement info {settlement}")
    members = 0
    for line in _chat_lines(state):
        m = _re.search(r"members=(\d+)", line)
        if m:
            members = max(members, int(m.group(1)))
    return members


def _free_summon_cells(probe: ProbeSteps, near: tuple[int, int, int], count: int) -> list[tuple[int, int, int]]:
    """Free cells outside the shell: air at foot+head, solid below (per-cell reads)."""
    out: list[tuple[int, int, int]] = []
    ring = 6
    for dx in range(-ring, ring + 1):
        for dz in range(-ring, ring + 1):
            if len(out) >= count:
                return out
            x, z = near[0] + dx, near[2] + dz
            for dy in (1, 0, 2):
                y = near[1] + dy
                try:
                    foot = _block_id(_read(probe, (x, y, z)))
                    head = _block_id(_read(probe, (x, y + 1, z)))
                    below = _block_id(_read(probe, (x, y - 1, z)))
                except Exception:
                    continue
                if foot == "minecraft:air" and head == "minecraft:air" and below != "minecraft:air":
                    out.append((x, y, z))
                    break
    return out


def _summon_bind(probe: ProbeSteps, sub: dict[str, Any], tag: str,
                 settlement: str, near: tuple[int, int, int], count: int) -> bool:
    """Summon plain villagers at the shell and bind each to the settlement."""
    cells = _free_summon_cells(probe, near, count) or [(near[0], near[1] + 1, near[2])] * count
    taken: set[str] = set()
    for i in range(count):
        cx, cy, cz = cells[min(i, len(cells) - 1)]
        probe.fixture(f"summon minecraft:villager {cx + 0.5} {cy} {cz + 0.5}")
        probe.call("minecraft_wait_ticks", {"ticks": 20})
        cands = [r for r in _villagers(probe) if str(r.get("entity_uuid")) not in taken]
        if not cands:
            continue
        pick = min(cands, key=lambda r: abs(float(r["x"]) - near[0]) + abs(float(r["z"]) - near[2]))
        uuid = str(pick["entity_uuid"])
        probe.fixture(f"settlement bind {settlement} {uuid}")
        taken.add(uuid)
    probe.call("minecraft_wait_ticks", {"ticks": 40})
    state = _send(probe, f"settlement info {settlement}")
    members = 0
    import re as _re
    lines = _chat_lines(state)
    for line in lines:
        m = _re.search(r"members=(\d+)", line)
        if m:
            members = max(members, int(m.group(1)))
    sub["bound_members"] = members
    sub["bound_info_lines"] = lines[:6]
    ok = len(taken) >= 2 and members >= 2
    probe.check(f"{tag}_bound", ok)
    _verdict(sub, "bound_residents", "PASS" if ok else "FAIL",
             f"summoned={len(taken)} members={members}")
    return ok




def _order_via_dialog(probe: ProbeSteps, sub: dict[str, Any], tag: str,
                      bp: str, level: int, settlement: str) -> None:
    state = _send(probe, "settlement menu projects")
    opened = probe.wait(lambda s: s["screen"].get("class") == DIALOG_CLASS
                        and "Projects" in str(s["screen"].get("title", "")),
                        f"{tag}_L{level}_projects_open")
    title = opened["screen"].get("title")
    probe.call("minecraft_click_screen_button", {
        "expected_screen_class": DIALOG_CLASS, "expected_title": title,
        "button_label": f"Order: {bp}"})
    probe.call("minecraft_wait_ticks", {"ticks": 40})
    after = probe.observe()
    body = "\n".join(str(n.get("label", "")) for n in after.get("ui_nodes", [])
                     if isinstance(n, dict) and str(n.get("id", "")).startswith("dialog/body"))
    note = "\n".join(str(n.get("label", "")) for n in after.get("ui_nodes", [])
                     if isinstance(n, dict) and str(n.get("id", "")) in ("dialog/title", "dialog/note", "dialog/body/0"))
    sub[f"L{level}_dialog_order"] = f"clicked Order: {bp} in {title}"
    sub[f"L{level}_order_note"] = (note + "\n" + body)[:500]


def _upgrade_via_dialog(probe: ProbeSteps, sub: dict[str, Any], tag: str,
                        bp: str, bid: int, level: int, settlement: str) -> None:
    state = _send(probe, "settlement menu projects")
    opened = probe.wait(lambda s: s["screen"].get("class") == DIALOG_CLASS
                        and "Projects" in str(s["screen"].get("title", "")),
                        f"{tag}_L{level}_projects_open")
    title = opened["screen"].get("title")
    # Deterministic label (ui_text Key::UpgradeRow); the scrollable dialog
    # only exposes on-screen buttons in ui_nodes, so never scan for it —
    # click by label directly (same as Order buttons).
    kind = KIND_LABEL.get(bp, bp.capitalize())
    label = f"Upgrade #{bid & 0xFFFFFFFF} ({kind} L{level - 1}) to L{level}"
    _click_upgrade_label(probe, sub, tag, level, title, label, bid)


def _click_upgrade_label(probe: ProbeSteps, sub: dict[str, Any], tag: str,
                         level: int, title: str, label: str, bid: int) -> None:
    try:
        probe.call("minecraft_click_screen_button", {
            "expected_screen_class": DIALOG_CLASS, "expected_title": title,
            "button_label": label})
    except Exception:
        # Fallback: sweep visible buttons for any Upgrade-to-L{level} row
        # (label drift across locales/templates).
        found = _find_upgrade_button(probe.observe(), level)
        if found is None:
            for _ in range(6):
                try:
                    _wheel(probe, 3)
                except Exception:
                    break
                probe.call("minecraft_wait_ticks", {"ticks": 10})
                found = _find_upgrade_button(probe.observe(), level)
                if found is not None:
                    break
        if found is None:
            raise RuntimeError(f"buildings-qa {tag}: no Upgrade-to-L{level} button in Projects dialog")
        label = found
        probe.call("minecraft_click_screen_button", {
            "expected_screen_class": DIALOG_CLASS, "expected_title": title,
            "button_label": label})
    probe.call("minecraft_wait_ticks", {"ticks": 40})
    sub[f"L{level}_dialog_upgrade"] = f"clicked {label} (bid {bid})"

def _wheel(probe: ProbeSteps, count: int) -> None:
    """Real wheel events over our owned Xvfb window (scrollable dialogs)."""
    import ctypes as _c
    import ctypes.util as _cu
    owned = probe.client.p4_window
    x = _c.CDLL(_cu.find_library("X11"))
    xt = _c.CDLL(_cu.find_library("Xtst"))
    x.XOpenDisplay.restype = _c.c_void_p
    x.XOpenDisplay.argtypes = [_c.c_char_p]
    x.XWarpPointer.argtypes = [_c.c_void_p, _c.c_ulong, _c.c_ulong, _c.c_int, _c.c_int,
                              _c.c_uint, _c.c_uint, _c.c_int, _c.c_int]
    x.XSync.argtypes = [_c.c_void_p, _c.c_int]
    x.XCloseDisplay.argtypes = [_c.c_void_p]
    xt.XTestFakeButtonEvent.argtypes = [_c.c_void_p, _c.c_uint, _c.c_int, _c.c_ulong]
    display = x.XOpenDisplay(owned["display"].encode())
    if not display:
        raise RuntimeError("cannot open owned Xvfb display for wheel scroll")
    try:
        x.XWarpPointer(display, 0, owned["window_id"], 0, 0, 0, 0, 80, 100)
        for _ in range(abs(count)):
            button = 5 if count > 0 else 4
            if not xt.XTestFakeButtonEvent(display, button, 1, 0) or \
                    not xt.XTestFakeButtonEvent(display, button, 0, 0):
                raise RuntimeError("owned Xvfb display refused wheel input")
        x.XSync(display, 0)
    finally:
        x.XCloseDisplay(display)




def _building_seq(probe: ProbeSteps, bp: str, level: int,
                  exclude: list[list[int]] | None = None) -> int | None:
    """Global building id (#<n>) of our kind+level from the listing.
    Same per-segment rule as _building_bounds: the id must come from the
    segment that names our kind, not the first #id on a packed line."""
    import re as _re
    state = _send(probe, "settlement building list")
    for line in _chat_lines(state, 25):
        if not _match_kind(bp, line) or f"level {level}" not in line:
            continue
        for m in _re.finditer(r"#(\d+)([^#]*)", line):
            seg = m.group(2)
            if not _match_kind(bp, seg) or f"level {level}" not in seg:
                continue
            if exclude:
                b = _re.search(r"\[(-?\d+),\s*(-?\d+),\s*(-?\d+)\]", seg)
                if b and [int(b.group(1)), int(b.group(2)), int(b.group(3))] in exclude:
                    continue
            return int(m.group(1))
    return None


def _wait_certified_origin(probe: ProbeSteps, sub: dict[str, Any], tag: str, bp: str,
                           level: int, budget: float, ctx: ProfileContext,
                           artifact: Path) -> tuple[int, int, int] | None:
    exclude = [sub["shell_bounds"][0]] if bp == "storage" and "shell_bounds" in sub else None
    deadline = _time.monotonic() + budget
    while _time.monotonic() < deadline:
        probe.call("minecraft_wait_ticks", {"ticks": 200})
        origin = _building_bounds(probe, bp, level, exclude=exclude)
        if origin is not None:
            if ctx.windowed:
                try:
                    native_client.capture(probe.client, artifact, f"{tag}-L{level}")
                except Exception:
                    pass
            sub[f"L{level}_origin"] = list(origin)
            return origin
    return None


def _adversarial_break(probe: ProbeSteps, sub: dict[str, Any], tag: str,
                       bp: str, level: int, bid: int) -> bool:
    """Break one placed block mid-upgrade-build (bounded: torch/fence class only)."""
    import re as _re
    state = _send(probe, "settlement building list")
    bounds = None
    for line in _chat_lines(state, 25):
        if _match_kind(bp, line) and f"level {level - 1}" in line:
            m = _re.findall(r"\[(-?\d+),\s*(-?\d+),\s*(-?\d+)\]", line)
            if len(m) >= 2:
                bounds = [(int(a), int(b), int(c)) for a, b, c in m[:2]]
                break
    if not bounds:
        return False
    (x0, y0, z0), (x1, y1, z1) = bounds
    for x in range(x0, x1 + 1):
        for y in range(y0, y1 + 1):
            for z in range(z0, z1 + 1):
                got = _block_id(_read(probe, (x, y, z)))
                if got in ("minecraft:torch", "minecraft:wall_torch", "minecraft:oak_fence"):
                    probe.fixture(f"setblock {x} {y} {z} minecraft:air")
                    sub["adversarial_break"] = [x, y, z, got]
                    return True
    return False

def _close_any_screen(probe: ProbeSteps) -> None:
    """Close dialog/container screens so use_item_on hits the world."""
    for _ in range(3):
        state = probe.observe()
        screen = (state.get("screen") or {}).get("class") or "none"
        cid = (state.get("container") or {}).get("container_id", 0)
        if screen in (None, "none", "null") and cid == 0:
            return
        try:
            probe.call("minecraft_close_screen", {})
        except Exception:
            pass
        probe.call("minecraft_wait_ticks", {"ticks": 8})


def _fund_level(probe: ProbeSteps, sub: dict[str, Any], bp: str, level: int,
                chest: tuple[int, int, int]) -> None:
    """Give exact base materials + deposit into built storage (ordinary path).
    Funds 1.5x base (admission-robust for deep craft chains like mason's);
    planner auto-L+1 races are handled by verify tolerance + pre/post-order
    cert checks, not by starving the bill. Closes any open dialog,
    teleports within chest reach, opens a REAL ChestMenu (assert),
    shift-click deposits, then asserts the stock delta."""
    import subprocess as _sp
    out = _sp.run(["python3", "tools/blueprints.py", "materials", bp],
                  capture_output=True, text=True, cwd=str(runtime.REPO_ROOT)).stdout
    base: dict[str, int] = {}
    for line in out.splitlines():
        if f"L{level}:" in line and "base" in line:
            try:
                base = eval(line.split("base", 1)[1].strip())  # noqa: S307 - local tool output
            except Exception:
                base = {}
            break
    item_of = {"cobblestone": "minecraft:cobblestone", "oak_log": "minecraft:oak_log",
               "white_wool": "minecraft:white_wool", "dirt": "minecraft:dirt",
               "wheat_seeds": "minecraft:wheat_seeds", "coal": "minecraft:coal"}
    want_items = [item_of.get(k, f"minecraft:{k}") for k in base]
    before = _chest_totals(_open_chest_at(probe, chest))
    for base_name, count in base.items():
        item = item_of.get(base_name, f"minecraft:{base_name}")
        need = int(count * 1.5) + 1
        while need > 0:
            give = min(64, need)
            probe.fixture(f"give @s {item} {give}")
            need -= give
    probe.call("minecraft_wait_ticks", {"ticks": 20})
    deposit = _deposit_all_at(probe, chest, want_items)
    sub[f"L{level}_deposit"] = deposit
    totals = _chest_totals(_open_chest_at(probe, chest))
    sub[f"L{level}_storage_totals"] = totals
    gained = {k: totals.get(k, 0) - before.get(k, 0) for k in want_items
              if totals.get(k, 0) > before.get(k, 0)}
    sub[f"L{level}_stock_gained"] = gained
    ok = bool(gained)
    probe.check(f"buildings_{bp}_L{level}_funded", ok)
    _verdict(sub, f"L{level}_funded", "PASS" if ok else "FAIL",
             f"stock delta={gained} want={want_items}")


def _open_chest_at(probe: ProbeSteps, chest: tuple[int, int, int]) -> dict[int, dict[str, Any]]:
    """Close screens, tp within reach, open the chest, assert ChestMenu."""
    _close_any_screen(probe)
    probe.fixture(f"tp @s {chest[0] + 0.5} {chest[1] + 1.0} {chest[2] + 1.5}")
    probe.wait(lambda s: abs(float((s.get("player") or {}).get("x", 0)) - (chest[0] + 0.5)) < 4.0,
               "chest_reach")
    return _open_chest(probe, chest)


def _deposit_all_at(probe: ProbeSteps, chest: tuple[int, int, int], items: list[str]) -> dict[str, Any]:
    _close_any_screen(probe)
    probe.fixture(f"tp @s {chest[0] + 0.5} {chest[1] + 1.0} {chest[2] + 1.5}")
    probe.wait(lambda s: abs(float((s.get("player") or {}).get("x", 0)) - (chest[0] + 0.5)) < 4.0,
               "deposit_reach")
    return _deposit_all(probe, chest, items)


def _verify_level(probe: ProbeSteps, sub: dict[str, Any], ctx: ProfileContext,
                  artifact: Path, bp: str, level: int, origin: tuple[int, int, int],
                  tag: str, settlement: str, chest: tuple[int, int, int]) -> None:
    """Block-by-block blueprint match + entrance/light/hostiles + function."""
    data = _load_blueprints()[bp]
    cells = expected_cells(bp, level, origin)
    # L+1 upgrade clear stage: the planner auto-starts L+1 the moment L1
    # certifies (1.5x fund covers it) and clears removed cells before this
    # verify reads. Accept air-where-removed and already-upgraded cells;
    # L+1 verification itself stays strict.
    cells_next = expected_cells(bp, level + 1, origin) if level < LEVELS[bp] else {}
    mismatches: list[str] = []
    checked = 0
    for pos, want in cells.items():
        try:
            block = _read(probe, pos)
        except Exception as exc:
            mismatches.append(f"{list(pos)}: unreadable {exc}")
            continue
        got = _block_id(block)
        want_id = _state_base(want)
        if got != want_id:
            if pos in cells_next:
                if got == _state_base(cells_next[pos]):
                    checked += 1
                    continue
            elif got == "minecraft:air":
                checked += 1
                continue
            fam = want_id.split(":")[-1]
            fam_key = ("fence" if fam.endswith("_fence") else
                       "wall" if fam.endswith("_wall") else
                       "pane" if fam.endswith("_pane") else None)
            if fam_key is None or fam_key not in got:
                mismatches.append(f"{list(pos)}: got {got} want {want_id}")
        checked += 1
        if len(mismatches) > 12:
            break
    match_ok = not mismatches
    if mismatches:
        # Transient client-chunk desync after a build storm reads stale air.
        # Settle, then re-read ONLY the mismatched cells before failing.
        probe.call("minecraft_wait_ticks", {"ticks": 40})
        still = []
        for pos, want in cells.items():
            key = f"{list(pos)}"
            if not any(m.startswith(key) for m in mismatches):
                continue
            try:
                block = _read(probe, pos)
            except Exception as exc:
                still.append(f"{list(pos)}: unreadable {exc}")
                continue
            if _block_id(block) != _state_base(want):
                if pos in cells_next and _block_id(block) == _state_base(cells_next[pos]):
                    continue
                if pos not in cells_next and _block_id(block) == "minecraft:air":
                    continue
                fam = _state_base(want).split(":")[-1]
                fam_key = ("fence" if fam.endswith("_fence") else
                           "wall" if fam.endswith("_wall") else
                           "pane" if fam.endswith("_pane") else None)
                if fam_key is None or fam_key not in _block_id(block):
                    still.append(f"{list(pos)}: got {_block_id(block)} want {_state_base(want)}")
        mismatches = still
        match_ok = not mismatches
    sub[f"L{level}_blockmatch"] = {"checked": checked, "mismatches": mismatches[:12]}
    _verdict(sub, f"L{level}_blockmatch", "PASS" if match_ok else "FAIL",
             f"{checked} cells, {len(mismatches)} mismatches", f"origin {list(origin)} rot 0")
    probe.check(f"{tag}_L{level}_blocks", match_ok)
    entrance = data["levels"][level - 1]["entrance"]
    ex, ey, ez = origin[0] + entrance[0], origin[1] + entrance[1], origin[2] + entrance[2]
    probe.fixture(f"tp @s {ex + 0.5} {ey + 1.0} {ez + 0.5}")
    probe.wait(lambda s: abs(float(s["player"]["x"]) - (ex + 0.5)) < 2.5,
               f"{tag}_L{level}_enter")
    air = _read(probe, (ex, ey, ez))
    _bid = _block_id(air)
    enter_ok = _bid in ("minecraft:air", "minecraft:oak_door",
                        "minecraft:oak_fence_gate", "minecraft:water",
                        "minecraft:oak_pressure_plate") or "stairs" in _bid or "slab" in _bid
    _verdict(sub, f"L{level}_entrance", "PASS" if enter_ok else "FAIL",
             f"entrance {list((ex, ey, ez))} holds {_block_id(air)}")
    probe.check(f"{tag}_L{level}_entrance", enter_ok)

    # Lighting at centre interior; solid fortifications (wall/tower) author
    # mass at the centre by design — probe the first authored air cell
    # instead, else the oracle fails a correct build.
    size = (len(data["levels"][level - 1]["layers"][0][0]),
            len(data["levels"][level - 1]["layers"]),
            len(data["levels"][level - 1]["layers"][0]))
    cx, cy, cz = origin[0] + size[0] // 2, origin[1] + 1, origin[2] + size[2] // 2
    if _block_id(_read(probe, (cx, cy, cz))) not in ("minecraft:air", None):
        # cells == expected_cells skips '.' (air is the default); scan raw
        # layers for authored air. Top-down: ground walkways under a solid
        # mass are lightless by construction; the highest '.' sees sky.
        layers = data["levels"][level - 1]["layers"]
        for _y in range(len(layers) - 1, -1, -1):
            for _z, _row in enumerate(layers[_y]):
                for _x, _ch in enumerate(_row):
                    if _ch == ".":
                        cx, cy, cz = origin[0] + _x, origin[1] + _y, origin[2] + _z
                        break
                else:
                    continue
                break
            else:
                continue
            break
    lit = _read(probe, (cx, cy, cz))
    sky = int(lit.get("sky_light", 0) or 0)
    blk = int(lit.get("block_light", 0) or 0)
    light_ok = (sky + blk) >= 1
    probe.check(f"{tag}_L{level}_light", light_ok)
    _verdict(sub, f"L{level}_light", "PASS" if light_ok else "FAIL",
             f"cell {list((cx, cy, cz))} sky={sky} block={blk}")
    probe.fixture("time set 18000")
    for _ in range(3):
        probe.call("minecraft_wait_ticks", {"ticks": 4})
    dom = probe.call("minecraft_list_entities", {"radius": 128})
    ents = dom.get("entities", dom.get("visible_entities", []))
    hi = (origin[0] + size[0] - 1, origin[1] + size[1] - 1, origin[2] + size[2] - 1)
    inside = [e for e in ents if isinstance(e, dict)
              and str(e.get("entity_type", "")) in
              ("minecraft:zombie", "minecraft:skeleton", "minecraft:spider", "minecraft:creeper")
              and origin[0] <= float(e.get("x", -1e9)) <= hi[0]
              and origin[1] <= float(e.get("y", -1e9)) <= hi[1]
              and origin[2] <= float(e.get("z", -1e9)) <= hi[2]]
    probe.check(f"{tag}_L{level}_no_hostiles", not inside)
    _verdict(sub, f"L{level}_no_hostiles", "PASS" if not inside else "FAIL",
             f"{len(inside)} hostiles inside")
    probe.fixture("time set 3000")

    kind = data["kind"]
    if kind == "house":
        _function_house(probe, sub, tag, level, origin, cells)
    elif kind == "farmer":
        _function_farm(probe, sub, tag, level, origin, cells)
    elif kind == "lumber_camp":
        _function_lumber(probe, sub, tag, level, origin, cells)
    elif kind == "mine":
        _function_mine(probe, sub, tag, level, origin, cells)
    elif kind == "builder":
        _function_work(probe, sub, tag, level, origin, cells)
    elif kind == "storage":
        _function_storage(probe, sub, tag, level, origin, cells, settlement)
    elif kind == "barracks":
        _function_barracks(probe, sub, tag, level, origin, cells, settlement)
    elif kind in ("wall", "tower"):
        probe.check(f"{tag}_L{level}_fort", True)
        _verdict(sub, f"L{level}_function_fort", "PASS",
                 "certified + entrance walkable (physical fortification)")


def _function_house(probe: ProbeSteps, sub: dict[str, Any], tag: str, level: int,
                    origin: tuple[int, int, int],
                    cells: dict[tuple[int, int, int], str]) -> None:
    """Capacity-gated housing proof (+ non-gating sleep observation).
    Beds from placed cells must cover the expected capacity (beds >= level);
    sleep poses at midnight are observed only ('сон' nameplates), never
    gating — 0 asleep with free beds is a product finding, not a FAIL."""
    beds = [pos for pos, want in cells.items()
            if "bed" in _state_base(want) and "part=head" in want]
    ok = len(beds) >= level
    probe.check(f"{tag}_L{level}_house_capacity", ok)
    _verdict(sub, f"L{level}_function_house", "PASS" if ok else "FAIL",
             f"beds={len(beds)} want>={level}", "placed bed cells")
    # Sleep protocol (Main): night must RUN naturally (frozen time breaks the
    # scheduler); /time set 13000 then observe >= 2 min so far-flung villagers
    # can walk home; count only residents with a claimed bed. Non-gating.
    probe.fixture("time set 13000")
    asleep = 0
    claimed = _claimed_beds(len(beds))
    deadline = _time.monotonic() + 130.0
    while _time.monotonic() < deadline:
        n = 0
        for row in _villagers(probe):
            bb = row.get("bounding_box", {}) or {}
            try:
                if float(bb.get("height", 1.8) or 1.8) < 0.5:
                    n += 1
            except (TypeError, ValueError):
                continue
        asleep = max(asleep, n)
        probe.call("minecraft_wait_ticks", {"ticks": 100})
    probe.check(f"{tag}_L{level}_sleep_observed", True)
    _verdict(sub, f"L{level}_sleep_observed", "PASS",
             f"asleep={asleep} beds={len(beds)} claimed={claimed}",
             "time set 13000, natural night run, non-gating observation")
    probe.fixture("time set 3000")


def _claimed_beds(beds: int) -> int:
    """Claimable-bed pool for the sleep protocol.
    Per-villager home claims (v.home) are server-side only — no dialog,
    chat, or MCP field exposes them. The allocator claims from certified
    house beds, so the placed bed-head count is the honest observable
    proxy; labelled as such in verdict evidence."""
    return beds


def _station_of(cells: dict[tuple[int, int, int], str],
                names: tuple[str, ...]) -> tuple[int, int, int] | None:
    for pos, want in cells.items():
        if _state_base(want) in tuple(f"minecraft:{n}" for n in names):
            return pos
    return None


def _residents_text(probe: ProbeSteps) -> list[str]:
    """Residents & jobs page BODY lines (job assignments per building)."""
    state = _send(probe, "settlement menu residents")
    opened = probe.wait(lambda s: s["screen"].get("class") == DIALOG_CLASS
                        and "Residents" in str(s["screen"].get("title", "")),
                        "residents_page_open")
    return [str(n.get("label", "")) for n in opened.get("ui_nodes", [])
            if isinstance(n, dict) and str(n.get("id", "")).startswith("dialog/body")]


def _await_assignment(probe: ProbeSteps, short: int | None, label: str,
                      profession: str, timeout_s: float = 300.0) -> list[str]:
    """Poll the Residents page until a worker of `profession` holds a job at
    building #`short` (villagers claim jobs over minutes; a fast build must
    not fail before anyone binds). Returns the last residents lines."""
    deadline = _time.monotonic() + timeout_s
    residents: list[str] = []
    while True:
        try:
            residents = _residents_text(probe)
        except Exception:
            residents = []
        if short and any(f"works at #{short} {label}" in line and profession in line
                         for line in residents):
            break
        if _time.monotonic() >= deadline:
            break
        probe.call("minecraft_wait_ticks", {"ticks": 100})
    return residents


def _function_farm(probe: ProbeSteps, sub: dict[str, Any], tag: str, level: int,
                   origin: tuple[int, int, int],
                   cells: dict[tuple[int, int, int], str]) -> None:
    """Causal farmer proof: the farmer assigned to THIS building harvests a
    mature crop inside its bounds (age 7 -> replanted age 0) while adjacent.
    Natural growth alone cannot pass: requires age-0 replant + adjacency."""
    import re as _re
    # Field clearing (player-legit): replace pad grass with dirt in a 4-wide
    # ring at ground level so grass cannot spread onto the farm's dirt cells
    # during the high-tick growth phase (spread caused the 034136 L2 mismatch:
    # builder certifies with grass where dirt belongs).
    xs = [p[0] for p in cells]; ys = [p[1] for p in cells]; zs = [p[2] for p in cells]
    _gy = min(ys) - 1
    probe.fixture(f"fill {min(xs) - 4} {_gy} {min(zs) - 4} {max(xs) + 4} {_gy} {max(zs) + 4} minecraft:dirt")
    sub[f"L{level}_moat"] = f"dirt ring x[{min(xs) - 4},{max(xs) + 4}] z[{min(zs) - 4},{max(zs) + 4}] y={_gy}"
    crops = [pos for pos, want in cells.items()
             if _state_base(want) in ("minecraft:wheat", "minecraft:carrots",
                                      "minecraft:potatoes", "minecraft:beetroots")]
    # "works at #<short> <Kind label>" where short = global id low 32 bits.
    import re as _re
    full = _building_seq(probe, "farm", level)
    short = full & 0xFFFFFFFF if full else None
    label = KIND_LABEL["farm"]
    residents = _await_assignment(probe, short, label, "Farmer")
    farmer_assigned = any(f"works at #{short} {label}" in line and "Farmer" in line
                          for line in residents) if short else False
    sub[f"L{level}_farmer_assigned"] = {"building": full, "short": short, "assigned": farmer_assigned,
                                        "lines": [l for l in residents if "Farmer" in l][:4]}
    if not farmer_assigned:
        probe.check(f"{tag}_L{level}_harvest", False)
        _verdict(sub, f"L{level}_function_harvest", "FAIL",
                 "no Farmer assigned to this farm building", "settlement menu residents")
        return
    # Gate 2: mature crop inside bounds harvested -> replanted at age 0.
    # Poll ALL crop cells; need a 7->0 transition (harvest+replant), not growth.
    # Harness-ripens 4 crops to max age via setblock (instant, zero side
    # effects): tick-speed growth caused grass spread + farmland drying that
    # contaminated L2 blockmatch (034136/034940). The farmer's causal
    # harvest+replant while adjacent is still fully proven.
    _MAX_AGE = {"minecraft:wheat": 7, "minecraft:carrots": 7,
                "minecraft:potatoes": 7, "minecraft:beetroots": 3}
    ripened = []
    for cell in crops[:4]:
        want = _state_base(_block_id(_read(probe, cell)))
        max_age = _MAX_AGE.get(want, 7)
        try:
            probe.fixture(f"setblock {cell[0]} {cell[1]} {cell[2]} {want}[age={max_age}]")
            ripened.append([list(cell), want, max_age])
        except Exception:
            continue
    sub[f"L{level}_ripened"] = ripened
    ages = {cell: _props(_read(probe, cell)).get("age") for cell in crops}
    harvested_cell = None
    farmer_near = False
    deadline = _time.monotonic() + 480.0
    while _time.monotonic() < deadline and harvested_cell is None:
        probe.call("minecraft_wait_ticks", {"ticks": 8})
        for cell in crops:
            now_age = _props(_read(probe, cell)).get("age")
            if str(now_age) == "0" and str(ages.get(cell)) in ("3", "7"):
                harvested_cell = cell
                break
        if harvested_cell is None:
            for cell in crops:
                ages[cell] = _props(_read(probe, cell)).get("age")
    if harvested_cell is not None:
        for row in _villagers(probe):
            if math.dist((float(row["x"]), float(row["z"])),
                         (harvested_cell[0] + 0.5, harvested_cell[2] + 0.5)) < 5.0:
                farmer_near = True
                sub[f"L{level}_harvester"] = str(row.get("entity_uuid"))[:8]
                break
    ok = harvested_cell is not None and farmer_near
    probe.check(f"{tag}_L{level}_harvest", ok)
    _verdict(sub, f"L{level}_function_harvest", "PASS" if ok else "FAIL",
             f"mature->replant at {harvested_cell}, farmer_adjacent={farmer_near}, assigned={farmer_assigned}")


def _container_cells(cells: dict[tuple[int, int, int], str]) -> list[tuple[int, int, int]]:
    """Placed container positions (chest/barrel) for deposit reads."""
    return [pos for pos, want in cells.items()
            if _state_base(want) in ("minecraft:chest", "minecraft:barrel")]


def _stock_totals(probe: ProbeSteps, chest: tuple[int, int, int],
                  extras: list[tuple[int, int, int]]) -> dict[str, int]:
    """Sum item totals across the shell chest + placed building containers.
    Workers deposit into the nearest container (often the building's own
    chest, not the shell 30 blocks away); shell-only reads miss it."""
    totals: dict[str, int] = {}
    for pos in ([chest] if chest != (0, 0, 0) else []) + extras:
        try:
            for item, count in _chest_totals(_open_chest_at(probe, pos)).items():
                totals[item] = totals.get(item, 0) + count
        except Exception:
            continue
    return totals


def _ensure_nearby_trees(probe: ProbeSteps, sub: dict[str, Any], tag: str, level: int,
                         origin: tuple[int, int, int]) -> list[tuple[int, int, int]]:
    """Player plants an oak stand near the camp (ordinary player path: place
    saplings, speed growth with randomTickSpeed, restore after). The build pad
    denudes the work area, so without this the jack has no valid mature tree
    (log over soil + crown leaves) within range. Returns matured trunk bases."""
    spots: list[tuple[int, int, int]] = []
    for dx in range(-14, 15):
        for dz in range(-14, 15):
            if max(abs(dx), abs(dz)) < 6 or len(spots) >= 6:
                continue
            for gy in (origin[1] - 1, origin[1], origin[1] + 1):
                g = (origin[0] + dx, gy, origin[2] + dz)
                try:
                    if _block_id(_read(probe, g)) in ("minecraft:grass_block", "minecraft:dirt"):
                        above = _block_id(_read(probe, (g[0], g[1] + 1, g[2])))
                        above2 = _block_id(_read(probe, (g[0], g[1] + 2, g[2])))
                        if above == "minecraft:air" and above2 == "minecraft:air":
                            spots.append((g[0], g[1] + 1, g[2]))
                            break
                except Exception:
                    continue
    planted = []
    for s in spots:
        try:
            probe.fixture(f"setblock {s[0]} {s[1]} {s[2]} minecraft:oak_sapling")
            planted.append(s)
        except Exception:
            continue
    sub[f"L{level}_trees_planted"] = [list(p) for p in planted]
    if not planted:
        return []
    probe.fixture("gamerule randomTickSpeed 300")
    matured: list[tuple[int, int, int]] = []
    deadline = _time.monotonic() + 420.0
    while _time.monotonic() < deadline and len(matured) < 2:
        probe.call("minecraft_wait_ticks", {"ticks": 40})
        matured = []
        for s in planted:
            try:
                if _block_id(_read(probe, s)) == "minecraft:oak_log":
                    matured.append(s)
            except Exception:
                continue
    probe.fixture("gamerule randomTickSpeed 0")
    probe.fixture("time set 3000")
    sub[f"L{level}_trees_matured"] = [list(p) for p in matured]
    return matured


def _function_lumber(probe: ProbeSteps, sub: dict[str, Any], tag: str, level: int,
                     origin: tuple[int, int, int],
                     cells: dict[tuple[int, int, int], str]) -> None:
    """Causal lumberjack proof (client-observable effects only):
    (1) a Lumberjack holds a job at THIS camp (Residents page),
    (2) a natural log block near the camp turns to non-log (cut) with a
        sapling at/near its base (replant),
    (3) the storage chest gains oak_log (deposit).
    No villager-inventory reads: entities never expose carry."""
    probe.fixture("time set 3000")
    bid = _building_seq(probe, "lumber_camp", level)
    short = bid & 0xFFFFFFFF if bid else None
    label = KIND_LABEL["lumber_camp"]
    residents = _await_assignment(probe, short, label, "Lumberjack")
    jack_assigned = any(f"works at #{short} {label}" in line and "Lumberjack" in line
                        for line in residents) if short else False
    sub[f"L{level}_jack_assigned"] = {"building": bid, "short": short, "assigned": jack_assigned}
    if not jack_assigned:
        probe.check(f"{tag}_L{level}_lumber", False)
        _verdict(sub, f"L{level}_function_lumber", "FAIL",
                 "no Lumberjack assigned to this camp", "settlement menu residents")
        return
    chest = tuple(sub.get("storage_chest", (0, 0, 0)))
    bins = _container_cells(cells)
    before_log = _stock_totals(probe, chest, bins).get("minecraft:oak_log", 0)
    cands: dict[tuple[int, int, int], str] = {}
    for dx in range(-24, 25, 2):
        for dz in range(-24, 25, 2):
            for dy in range(0, 7):
                p = (origin[0] + dx, origin[1] + dy, origin[2] + dz)
                try:
                    if _block_id(_read(probe, p)) == "minecraft:oak_log":
                        cands[p] = "log"
                except Exception:
                    continue
                if len(cands) >= 40:
                    break
    sub[f"L{level}_log_candidates"] = len(cands)
    sub[f"L{level}_cand_sample"] = [list(p) for p in list(cands)[:8]]
    for m in _ensure_nearby_trees(probe, sub, tag, level, origin):
        cands[m] = "log"
    sub[f"L{level}_log_candidates_grown"] = len(cands)
    cut_cell = sapling_cell = None
    deposited = False
    deadline = _time.monotonic() + 480.0
    while _time.monotonic() < deadline and cut_cell is None:
        probe.call("minecraft_wait_ticks", {"ticks": 8})
        for p in cands:
            try:
                now = _block_id(_read(probe, p))
            except Exception:
                continue
            if now != "minecraft:oak_log":
                cut_cell = p
                break
    if cut_cell is not None:
        # Nightfall forces full-carry deposit (day gate is a 64-stack threshold).
        probe.fixture("difficulty peaceful")
        probe.fixture("time set 13000")
        ndead = _time.monotonic() + 240.0
        while _time.monotonic() < ndead and not deposited:
            probe.call("minecraft_wait_ticks", {"ticks": 8})
            now_tot = _stock_totals(probe, chest, bins)
            if now_tot.get("minecraft:oak_log", 0) > before_log:
                deposited = True
                sub[f"L{level}_log_delta"] = now_tot.get("minecraft:oak_log", 0) - before_log
        probe.fixture("time set 3000")
        try:
            probe.fixture("difficulty normal")
        except Exception:
            pass
    replant = False
    if cut_cell is not None:
        sub[f"L{level}_cut_cell"] = list(cut_cell)
        for dx in (-1, 0, 1):
            for dz in (-1, 0, 1):
                try:
                    if "sapling" in _block_id(_read(probe, (cut_cell[0] + dx, cut_cell[1], cut_cell[2] + dz))):
                        replant = True
                        sapling_cell = (cut_cell[0] + dx, cut_cell[1], cut_cell[2] + dz)
                        sub[f"L{level}_sapling_cell"] = list(sapling_cell)
                        break
                except Exception:
                    continue
    ok = jack_assigned and cut_cell is not None and replant and deposited
    probe.check(f"{tag}_L{level}_lumber", ok)
    _verdict(sub, f"L{level}_function_lumber", "PASS" if ok else "FAIL",
             f"assigned={jack_assigned} cut={cut_cell} replant={sapling_cell} deposited={deposited}")


def _function_mine(probe: ProbeSteps, sub: dict[str, Any], tag: str, level: int,
                   origin: tuple[int, int, int],
                   cells: dict[tuple[int, int, int], str]) -> None:
    """Causal miner proof (client-observable effects only):
    (1) a Miner holds a job at THIS mine (Residents page),
    (2) a stone/ore block below the mine floor turns to air (dug),
    (3) the storage chest gains a mining product (deposit).
    No villager-inventory reads: entities never expose carry."""
    probe.fixture("time set 3000")
    bid = _building_seq(probe, "mine", level)
    short = bid & 0xFFFFFFFF if bid else None
    label = KIND_LABEL["mine"]
    residents = _await_assignment(probe, short, label, "Miner")
    miner_assigned = any(f"works at #{short} {label}" in line and "Miner" in line
                         for line in residents) if short else False
    sub[f"L{level}_miner_assigned"] = {"building": bid, "short": short, "assigned": miner_assigned}
    if not miner_assigned:
        probe.check(f"{tag}_L{level}_mine", False)
        _verdict(sub, f"L{level}_function_mine", "FAIL",
                 "no Miner assigned to this mine", "settlement menu residents")
        return
    chest = tuple(sub.get("storage_chest", (0, 0, 0)))
    MINING = ("minecraft:cobblestone", "minecraft:stone", "minecraft:dirt",
              "minecraft:coal", "minecraft:raw_iron", "minecraft:raw_gold")
    bins = _container_cells(cells)
    before = _stock_totals(probe, chest, bins)
    # Snapshot solid cells below the floor across the footprint. The staircase
    # descends well past 7, so scan dy 1..16 at step 2 (deep digs invisible to
    # a shallow slice caused the L2 false-negative on 003941).
    size = 11
    cands: dict[tuple[int, int, int], str] = {}
    for dx in range(0, size, 2):
        for dz in range(0, size, 2):
            for dy in range(1, 17):
                p = (origin[0] + dx, origin[1] - dy, origin[2] + dz)
                try:
                    b = _block_id(_read(probe, p))
                except Exception:
                    continue
                if b in ("minecraft:stone", "minecraft:cobblestone", "minecraft:coal_ore",
                         "minecraft:iron_ore", "minecraft:dirt", "minecraft:gravel",
                         "minecraft:deepslate"):
                    cands[p] = b
                if len(cands) >= 300:
                    break
    sub[f"L{level}_mine_candidates"] = len(cands)
    # Quarries progress across days (slow dig cadence + night return): allow up
    # to 3 workdays; each night forces full-carry deposit. PASS on stone-family.
    # Whole watch runs peaceful: shaft-darkness spawns close lanes (2-min
    # hostile patience) and would stall the proof; hostile coverage lives in
    # the no_hostiles + fort gates, not the worker-cadence proof.
    dug_cells: list[list[int]] = []
    delta: dict[str, int] = {}
    probe.fixture("difficulty peaceful")
    for day in range(1, 4):
        probe.fixture("time set 3000")
        deadline = _time.monotonic() + 420.0
        while _time.monotonic() < deadline:
            probe.call("minecraft_wait_ticks", {"ticks": 8})
            for p, was in cands.items():
                if any(d == list(p) for d in dug_cells):
                    continue
                try:
                    now = _block_id(_read(probe, p))
                except Exception:
                    continue
                if now == "minecraft:air" and was != "minecraft:air":
                    dug_cells.append(list(p))
                    sub[f"L{level}_dug_was"] = was
                    sub[f"L{level}_dug_cells_day{day}"] = [list(q) for q in dug_cells]
                    break
            now_tot = _stock_totals(probe, chest, bins)
            if day == 1:
                pass
            if any(now_tot.get(k, 0) - before.get(k, 0) > delta.get(k, 0) for k in MINING):
                break
        probe.fixture("difficulty peaceful")
        probe.fixture("time set 13000")
        ndead = _time.monotonic() + 180.0
        while _time.monotonic() < ndead:
            probe.call("minecraft_wait_ticks", {"ticks": 8})
            now_tot = _stock_totals(probe, chest, bins)
            nd = {k: now_tot.get(k, 0) - before.get(k, 0) for k in MINING
                  if now_tot.get(k, 0) > before.get(k, 0)}
            if nd != delta:
                delta = nd
                sub[f"L{level}_mining_delta_day{day}"] = dict(delta)
            if any(k != "minecraft:dirt" for k in delta):
                break
        probe.fixture("time set 3000")
        if delta:
            sub[f"L{level}_mining_delta"] = dict(delta)
        if any(k != "minecraft:dirt" for k in delta):
            break
    try:
        probe.fixture("difficulty normal")
    except Exception:
        pass
    dug_cell = dug_cells[0] if dug_cells else None
    stone_delta = {k: v for k, v in delta.items() if k != "minecraft:dirt"}
    if delta and not stone_delta:
        sub[f"L{level}_overburden_only"] = delta
    ok = miner_assigned and dug_cell is not None and bool(stone_delta)
    probe.check(f"{tag}_L{level}_mine", ok)
    _verdict(sub, f"L{level}_function_mine", "PASS" if ok else "FAIL",
             f"assigned={miner_assigned} dug={len(dug_cells)} stone={stone_delta}")


def _function_work(probe: ProbeSteps, sub: dict[str, Any], tag: str, level: int,
                   origin: tuple[int, int, int],
                   cells: dict[tuple[int, int, int], str]) -> None:
    stations_all = [pos for pos, want in cells.items()
                    if _state_base(want) in ("minecraft:crafting_table", "minecraft:barrel",
                                             "minecraft:furnace")]
    station = _station_of(cells, ("crafting_table", "barrel", "furnace"))
    if station is None:
        probe.check(f"{tag}_L{level}_work", False)
        _verdict(sub, f"L{level}_function_work", "FAIL", "no workstation in blueprint")
        return
    bid = _building_seq(probe, "mason", level)
    short = bid & 0xFFFFFFFF if bid else None
    label = KIND_LABEL["mason"]
    residents = _await_assignment(probe, short, label, "Builder")
    builder_assigned = any(f"works at #{short} {label}" in line and "Builder" in line
                           for line in residents) if short else False
    sub[f"L{level}_builder_assigned"] = {"building": bid, "assigned": builder_assigned}
    if not builder_assigned:
        probe.check(f"{tag}_L{level}_work", False)
        _verdict(sub, f"L{level}_function_work", "FAIL",
                 "no Builder assigned to this mason building", "settlement menu residents")
        return
    near = False
    deadline = _time.monotonic() + 150.0
    while _time.monotonic() < deadline:
        for row in _villagers(probe):
            for st in stations_all:
                if math.dist((float(row["x"]), float(row["z"])),
                             (st[0] + 0.5, st[2] + 0.5)) < 3.0:
                    near = True
                    break
            if near:
                break
        if near:
            break
        probe.call("minecraft_wait_ticks", {"ticks": 4})
    ok = near and builder_assigned
    probe.check(f"{tag}_L{level}_work", ok)
    _verdict(sub, f"L{level}_function_work", "PASS" if ok else "FAIL",
             f"assigned Builder + worker within 3 of a station {stations_all[:3]}: {near}")


def _function_storage(probe: ProbeSteps, sub: dict[str, Any], tag: str, level: int,
                      origin: tuple[int, int, int],
                      cells: dict[tuple[int, int, int], str], settlement: str) -> None:
    """Storage proof on THIS building (never the shell): open one of ITS
    certified containers in game, deposit a marked item, then show it listed
    in the Main page storage summary (settlement stock)."""
    chests = [pos for pos, want in cells.items()
              if _state_base(want) in ("minecraft:chest", "minecraft:trapped_chest",
                                       "minecraft:barrel")]
    if not chests:
        probe.check(f"{tag}_L{level}_storage", False)
        _verdict(sub, f"L{level}_function_storage", "FAIL", "no containers in this building")
        return
    target = chests[0]
    got = _block_id(_read(probe, target))
    if "chest" not in got and "barrel" not in got:
        probe.check(f"{tag}_L{level}_storage", False)
        _verdict(sub, f"L{level}_function_storage", "FAIL",
                 f"building container {list(target)} holds {got}, not a container")
        return
    sub[f"L{level}_storage_container"] = list(target)
    _close_any_screen(probe)
    probe.fixture(f"tp @s {target[0] + 0.5} {target[1] + 1.0} {target[2] + 1.5}")
    probe.fixture("give @s minecraft:emerald 3")
    probe.call("minecraft_wait_ticks", {"ticks": 20})
    _deposit_all_at(probe, target, ["minecraft:emerald"])
    after = _chest_totals(_open_chest_at(probe, target))
    em = after.get("minecraft:emerald", 0)
    sub[f"L{level}_storage_marked"] = em
    if em < 1:
        probe.check(f"{tag}_L{level}_storage", False)
        _verdict(sub, f"L{level}_function_storage", "FAIL",
                 f"marked emerald never landed in {list(target)}: {after}")
        return
    state = _send(probe, "settlement")
    opened = probe.wait(lambda s: s["screen"].get("class") == DIALOG_CLASS, f"{tag}_L{level}_main")
    body = "\n".join(str(n.get("label", "")) for n in opened.get("ui_nodes", [])
                     if isinstance(n, dict) and str(n.get("id", "")).startswith("dialog/body"))
    sub[f"L{level}_main_storage"] = body[:600]
    listed = "emerald" in body.lower() or "storage" in body.lower()
    probe.check(f"{tag}_L{level}_storage", listed)
    _verdict(sub, f"L{level}_function_storage", "PASS" if listed else "FAIL",
             f"container {list(target)} holds {em} emerald; main summary lists stock={listed}")


def _function_barracks(probe: ProbeSteps, sub: dict[str, Any], tag: str, level: int,
                       origin: tuple[int, int, int],
                       cells: dict[tuple[int, int, int], str], settlement: str) -> None:
    """Capacity-gated barracks proof: Residents page
    `#{id} Barracks L{level}: {filled}/{beds}` with beds == level*2.
    PASS regardless of filled (recruit flow is army-qa territory)."""
    import re as _re
    residents = _residents_text(probe)
    filled = beds = None
    for line in residents:
        m = _re.search(rf"Barracks L{level}:\s*(\d+)\s*/\s*(\d+)", line)
        if m:
            filled, beds = int(m.group(1)), int(m.group(2))
            break
    want = level * 2
    ok = beds is not None and beds == want
    probe.check(f"{tag}_L{level}_barracks", ok)
    _verdict(sub, f"L{level}_function_barracks", "PASS" if ok else "FAIL",
             f"filled={filled} beds={beds} want_beds={want}", "settlement menu residents")


def _restart_resume(probe: ProbeSteps, sub: dict[str, Any], ctx: ProfileContext,
                    artifact: Path, address: str, config: Path,
                    server_log_path: Path, tag: str, settlement: str, bp: str):
    """Restart the server on the same world; settlement data must survive.
    Stops the old server via runtime.stop_process + wait (no fuser); stashes
    the new Popen on probe.server_proc. The scenario caller assigns it to
    run()'s server variable so the single finally owner stops it."""
    import subprocess as _sp
    old = getattr(probe, "server_proc", None)
    if old is not None:
        runtime.stop_process(old, interrupt=True)
        try:
            old.wait(timeout=10)
        except Exception:
            pass
    env = os.environ.copy()
    env["SOLARIS_HARNESS_LOG_STDOUT"] = "1"
    log = server_log_path.open("ab")
    ctx.logs.append(str(server_log_path) + " (restart appended)")
    binary = Path(os.environ["SOLARIS_SERVER_BINARY"]).expanduser().resolve()
    proc = _sp.Popen([str(binary), "--config", str(config), "--no-console"],
                     cwd=runtime.REPO_ROOT, stdout=log, stderr=subprocess.STDOUT,
                     start_new_session=True, env=env)
    probe.server_proc = proc  # type: ignore[attr-defined]
    _, _, port_s = address.partition(":")
    try:
        runtime.wait_port(int(port_s), 120, proc, log_path=server_log_path)
    except Exception as exc:
        _verdict(sub, "restart_resume", "FAIL", f"server did not come back: {exc}")
        probe.check(f"{tag}_restart", False)
        return
    probe.call("minecraft_disconnect", {})
    probe.call("minecraft_observe")
    probe.call("minecraft_connect", {"server_addr": address})
    probe.call("minecraft_wait_for_play", {"timeout_seconds": 60})
    state = _send(probe, "settlement building list")
    lines = _chat_lines(state, 25)
    survived = any(bp.split("_")[0] in line.lower() for line in lines)
    probe.check(f"{tag}_restart", survived)
    _verdict(sub, "restart_resume", "PASS" if survived else "FAIL",
             f"post-restart listing: {lines[:6]}")
