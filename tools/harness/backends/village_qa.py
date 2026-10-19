"""Real-client acceptance for issue #1 settlement behaviour (village QA).

Canonical harness lifecycle sibling of the `dialogs` backend: fresh local
release server from this tree, fresh world, op player through the native
client (windowed on its own Xvfb, MCP). Scenario: build a small house with a
wooden door and a valid bed, a workshop (composter + mature wheat farmland
inside reach), a storage (chest), a top-of-zone mine (smithing table at grade)
and a lumber camp (fletching table) near mature persistent=false oaks;
`/settlement create` with a computed radius covering the fixtures, certify
buildings via chat commands, `/summon` plain villagers (no profession NBT:
the allocator assigns certified slots), sequential nearest-bind per role.
Frozen daylight (`gamerule advance_time false`, no natural clock advance) and frozen
random growth (`gamerule random_tick_speed 0`, mature saplings/leaves persist) + real
event ticks, single `time set` per phase (3000 work, 18000 night, 3000 morning, 9000):

1. a worker physically crosses a certified door (open seen + plane flip by a
   worker UUID, close awaited on a bounded exact predicate);
2. the same 4 entities persist through the night, one in sleep pose by the
   bed, and each worker returns within 6 blocks of its station at 3000;
3. farmer harvests mature wheat (age 7 -> replanted);
4. miner digs inside the zone and the chest holds mining products;
5. lumberjack cuts a mature oak AND replants AND the chest holds oak logs
   (deposit trips run at day-end 9000, polled with a bounded chest wait).

Every check is a strict PASS/FAIL verdict in
`probe.report["settlement_acceptance"]`; fixture chat commands are receipted
separately and never count as behaviour evidence. Screenshots + server log +
chest slot contents are retained. Product code is untouched.
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

USERNAME = "VillageQA"
SETTLEMENT = "qa"

# Mining products the miner can deposit (drop_name() mapping + natural stone).
MINING_ITEMS = frozenset({
    "minecraft:cobblestone", "minecraft:stone", "minecraft:cobbled_deepslate",
    "minecraft:deepslate", "minecraft:dirt", "minecraft:gravel",
    "minecraft:sand", "minecraft:granite", "minecraft:diorite",
    "minecraft:andesite", "minecraft:tuff", "minecraft:calcite",
    "minecraft:coal", "minecraft:raw_iron", "minecraft:raw_copper",
    "minecraft:raw_gold", "minecraft:redstone", "minecraft:lapis_lazuli",
    "minecraft:emerald", "minecraft:diamond",
})

# Phase plan: 18000 build-night (villagers staged in-house), 3000 work,
# 18000 night sleep, 3000 morning return, 9000 day-end deposits. One set each.
TIME_SET_PHASES = ("time set 18000", "time set 3000", "time set 18000", "time set 3000", "time set 9000")


def _abs(origin: tuple[int, int, int], dx: int, dy: int, dz: int) -> tuple[int, int, int]:
    return (origin[0] + dx, origin[1] + dy, origin[2] + dz)


def _rect(origin: tuple[int, int, int], lo: tuple[int, int, int],
          hi: tuple[int, int, int]) -> tuple[tuple[int, int, int], tuple[int, int, int]]:
    return (_abs(origin, *lo), _abs(origin, *hi))


def bed_cells(hx0: int, hy0: int, hz0: int) -> tuple[tuple[int, int, int], tuple[int, int, int]]:
    """Bed foot west of head (vanilla east-facing pair, cf. brain_tests)."""
    return ((hx0 + 1, hy0 + 1, hz0 + 1), (hx0 + 2, hy0 + 1, hz0 + 1))


def bed_facing_valid(foot: tuple[int, int, int], head: tuple[int, int, int], facing: str) -> bool:
    """East-facing head must sit one block east of its foot, same y/z."""
    if facing != "east":
        return False
    return head[0] == foot[0] + 1 and head[1] == foot[1] and head[2] == foot[2]


def house_headroom_ok(bed_y: int, roof_y: int) -> bool:
    """Two full air blocks above the bed: roof at least 3 above bed level."""
    return roof_y - bed_y >= 3


def door_crossed(before_x: float, after_x: float, door_x: int) -> bool:
    """True when a track flips sides of the door plane (block centre +/- 0.5)."""
    return (before_x - (door_x + 0.5)) * (after_x - (door_x + 0.5)) < 0


def chest_has(items: dict[str, int], wanted: frozenset[str] | set[str]) -> bool:
    return any(items.get(name, 0) > 0 for name in wanted)


def count_time_sets(commands: list[str]) -> int:
    return sum(1 for command in commands if command.startswith("time set "))


def all_behaviours_pass(behaviour: dict[str, str]) -> bool:
    required = ("door_walk", "sleep_return", "farmer_harvest", "miner_deposit", "lumberjack")
    return all(behaviour.get(name) == "PASS" for name in required)


def trees_separated(soils: list[tuple[int, int, int]], min_gap: int = 5) -> bool:
    """5x5 crowns (radius 2) need Chebyshev distance >= 5 or trunks get overwritten."""
    for i, (ax, _, az) in enumerate(soils):
        for bx, _, bz in soils[i + 1:]:
            if max(abs(ax - bx), abs(az - bz)) < min_gap:
                return False
    return True


def required_radius(anchor: tuple[int, int, int], points: list[tuple[int, int, int]]) -> int:
    """Smallest radius sphere around the anchor covering every fixture point."""
    far = max(math.dist(anchor, point) for point in points)
    return max(1, min(256, math.ceil(far) + 1))


def is_sleep_pose(y: float, bed_y: int, tol: float = 0.15) -> bool:
    """Sleeping entity rests at bed.y + 0.6875 (core sleeping pose)."""
    return abs(y - (bed_y + 0.6875)) <= tol


def sleeping_at_bed(row: dict[str, Any], bed: tuple[int, int, int]) -> bool:
    # The native client's received DATA_POSE projects SLEEPING_DIMENSIONS
    # (0.2 high); this proves pose, unlike mere proximity to a bed. Relative
    # movement quantization can drift the separately exposed entity position.
    bounds = row.get("bounding_box", {})
    height = float(bounds.get("max_y", 0)) - float(bounds.get("min_y", 0))
    return (abs(height - 0.2) < 0.02 and float(row.get("health", 0)) > 0
            and is_sleep_pose(float(row["y"]), bed[1], 0.5)
            and math.dist((float(row["x"]), float(row["z"])),
                          (bed[0] + 0.5, bed[2] + 0.5)) < 0.75)


def _block_id(block: dict[str, Any]) -> str:
    return str(block.get("block_id", ""))


def _props(block: dict[str, Any]) -> dict[str, Any]:
    props = block.get("properties") or {}
    return dict(props) if isinstance(props, dict) else {}


def _unwrap_block(result: dict[str, Any]) -> dict[str, Any]:
    """`minecraft_wait_for_block_state` returns a wait envelope around the block."""
    if isinstance(result.get("block"), dict):
        return result["block"]
    return result


def _read(steps: ProbeSteps, pos: tuple[int, int, int]) -> dict[str, Any]:
    x, y, z = pos
    return _unwrap_block(steps.call("minecraft_read_block", {"x": x, "y": y, "z": z}))


def _expect_block(steps: ProbeSteps, pos: tuple[int, int, int],
                  block_id: str, label: str) -> dict[str, Any]:
    x, y, z = pos
    block = _unwrap_block(steps.call(
        "minecraft_wait_for_block_state",
        {"x": x, "y": y, "z": z, "block_id": block_id,
         "properties": {}, "timeout_seconds": steps.remaining()}))
    if _block_id(block) != block_id:
        raise RuntimeError(f"village-qa fixture failed at {label}: "
                           f"expected {block_id} at {pos}, got {block}")
    return block


def _chat_echo(steps: ProbeSteps, label: str) -> dict[str, Any]:
    import uuid as _uuid
    marker = f"village-qa {label} {_uuid.uuid4().hex[:8]}"
    steps.call("minecraft_send_chat", {"message": marker, "command": False})
    return steps.wait(lambda s: any(marker in line for line in s.get("recent_chat", [])),
                      f"{label}_server_echo")


def _villagers(steps: ProbeSteps) -> list[dict[str, Any]]:
    """Client-visible villagers via the explicit list tool (observe has none)."""
    result = steps.call("minecraft_list_entities", {"radius": 64, "limit": 128})
    rows = result.get("entities", [])
    return [row for row in rows if isinstance(row, dict)
            and row.get("entity_type") == "minecraft:villager"]


def _tick(steps: ProbeSteps) -> None:
    """Wait for four native simulation ticks, never a wall-clock sleep."""
    steps.call("minecraft_wait_ticks", {"ticks": 4})


def _verdict(detail: dict[str, Any], name: str, verdict: str, evidence: str = "",
             repro: str = "") -> None:
    detail["behaviour"][name] = verdict
    if evidence:
        detail["evidence"][name] = evidence
    if repro:
        detail["repro"][name] = repro


def _open_chest(steps: ProbeSteps, chest: tuple[int, int, int]) -> dict[int, dict[str, Any]]:
    """Open a chest through the ordinary use path; return chest slots 0..26."""
    steps.call("minecraft_set_hotbar_slot", {"slot": 8})
    steps.call("minecraft_use_item_on",
               {"x": chest[0], "y": chest[1], "z": chest[2], "face": "up", "hand": "main_hand"})
    state = steps.wait(
        lambda s: s["container"]["container_id"] != 0
        and s["container"]["menu_class"] == "net.minecraft.world.inventory.ChestMenu",
        "chest_menu_received")
    if state["container"]["slot_count"] != 63:
        raise RuntimeError(f"village-qa chest slot_count is {state['container']['slot_count']}, want 63")
    slots: dict[int, dict[str, Any]] = {}
    for row in state["container"]["slots"]:
        if isinstance(row, dict) and 0 <= int(row.get("slot", -1)) < 27:
            slots[int(row["slot"])] = {"item_id": row.get("item_id"), "count": row.get("count", 0)}
    steps.call("minecraft_close_screen", {})
    steps.wait(lambda s: s["container"]["container_id"] == 0, "chest_menu_closed")
    return slots


def _chest_totals(slots: dict[int, dict[str, Any]]) -> dict[str, int]:
    totals: dict[str, int] = {}
    for entry in slots.values():
        name = str(entry.get("item_id", "minecraft:air"))
        if name != "minecraft:air" and int(entry.get("count", 0)) > 0:
            totals[name] = totals.get(name, 0) + int(entry.get("count", 0))
    return totals


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.client != "native":
        raise ValueError("village-qa requires --client native")
    if ctx.extra_args or ctx.platform is not None:
        raise ValueError("village-qa accepts only --client, --windowed and --timeout-seconds")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else 600.0
    if not 0 < timeout <= 600:
        raise ValueError("village-qa --timeout-seconds must be in (0, 600]")
    artifact = ctx.artifact_dir
    override = os.environ.get("SOLARIS_SERVER_BINARY")
    if not override:
        raise RuntimeError("village-qa requires SOLARIS_SERVER_BINARY (a solaris-v2 built from this tree)")
    binary = Path(override).expanduser().resolve()
    if not binary.is_file():
        raise RuntimeError(f"missing executable server binary: {binary}")
    server_port = runtime.reserve_port()
    address = f"127.0.0.1:{server_port}"
    config = artifact / "village-qa.toml"
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
            raise RuntimeError(f"expected one village-qa config token: {old_tok}")
        source = source.replace(old_tok, new_tok, 1)
    config.write_text(source)
    (artifact / "plugins").mkdir(parents=True, exist_ok=True)
    prepare_operator_fixture(ctx, [USERNAME])
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
        mcp.request_timeout_seconds = min(timeout + 10, 130.0)
        with ProbeSteps(mcp, artifact / "village-qa", timeout, "village-qa-settlements") as probe:
            probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
            if ctx.windowed:
                native_client.capture(mcp, artifact, "village-qa-spawn")
            detail: dict[str, Any] = {"behaviour": {}, "evidence": {}, "repro": {}}
            probe.report["settlement_acceptance"] = detail
            _scenario(probe, detail, ctx, artifact)
            if ctx.windowed:
                native_client.capture(mcp, artifact, "village-qa-final")
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


def _scenario(probe: ProbeSteps, detail: dict[str, Any], ctx: ProfileContext, artifact: Path) -> None:
    rooted = probe.wait(lambda s: isinstance((s.get("player") or {}).get("x"), float),
                        "player_present")
    px, py, pz = (math.floor(rooted["player"][axis]) for axis in ("x", "y", "z"))
    origin = (int(px), int(py), int(pz))
    detail["origin_block"] = list(origin)

    def fixture(command: str) -> None:
        probe.fixture(command)

    def settled(label: str) -> dict[str, Any]:
        return _chat_echo(probe, label)

    def teleport(x: float, y: float, z: float) -> None:
        fixture(f"tp @s {x:.1f} {y:.1f} {z:.1f}")
        probe.wait(lambda s: abs(float(s["player"]["x"]) - x) < 3.0
                   and abs(float(s["player"]["z"]) - z) < 3.0, "player_teleported")

    # --- level a pad through ordinary chat commands (one daylight freeze, no time spam) ---
    x0, y0, z0 = origin
    # 61*61*17 exceeds vanilla's 32768-command block limit; each slab fits.
    for low, high in ((-8, -1), (0, 7), (8, 8)):
        fixture(f"fill {x0 - 30} {y0 + low} {z0 - 30} {x0 + 30} {y0 + high} {z0 + 30} minecraft:air")
    fixture(f"fill {x0 - 30} {y0 - 2} {z0 - 30} {x0 + 30} {y0 - 2} {z0 + 30} minecraft:dirt")
    fixture(f"fill {x0 - 30} {y0 - 1} {z0 - 30} {x0 + 30} {y0 - 1} {z0 + 30} minecraft:grass_block")
    fixture(f"fill {x0 - 2} {y0 - 1} {z0 - 2} {x0 + 2} {y0 - 1} {z0 + 2} minecraft:stone")
    fixture("gamemode creative @s")
    fixture("effect give @s minecraft:regeneration infinite 255 true")
    fixture("gamerule advance_time false")
    fixture("gamerule spawn_mobs false")
    fixture("gamerule random_tick_speed 0")
    fixture("time set 18000")
    _expect_block(probe, (x0, y0 - 1, z0), "minecraft:stone", "spawn_pad_leveled")

    # --- house 5x5x5: floor y0, roof y0+4, walls y0+1..y0+3, east door, valid east bed ---
    house_lo, house_hi = _rect(origin, (0, 0, 0), (4, 4, 4))
    hx0, hy0, hz0 = house_lo
    hx1, hy1, hz1 = house_hi
    fixture(f"fill {hx0} {hy0} {hz0} {hx1} {hy0} {hz1} minecraft:oak_planks")
    fixture(f"fill {hx0} {hy1} {hz0} {hx1} {hy1} {hz1} minecraft:oak_planks")
    fixture(f"fill {hx0} {hy0 + 1} {hz0} {hx1} {hy0 + 3} {hz0} minecraft:cobblestone")
    fixture(f"fill {hx0} {hy0 + 1} {hz1} {hx1} {hy0 + 3} {hz1} minecraft:cobblestone")
    fixture(f"fill {hx0} {hy0 + 1} {hz0} {hx0} {hy0 + 3} {hz1} minecraft:cobblestone")
    fixture(f"fill {hx1} {hy0 + 1} {hz0} {hx1} {hy0 + 3} {hz1} minecraft:cobblestone")
    door = (hx1, hy0 + 1, hz0 + 2)
    door_top = (hx1, hy0 + 2, hz0 + 2)
    fixture(f"setblock {door[0]} {door[1]} {door[2]} minecraft:air")
    fixture(f"setblock {door_top[0]} {door_top[1]} {door_top[2]} minecraft:air")
    fixture(f"setblock {door[0]} {door[1]} {door[2]} minecraft:oak_door[facing=west,half=lower,hinge=left,open=false]")
    fixture(f"setblock {door_top[0]} {door_top[1]} {door_top[2]} minecraft:oak_door[facing=west,half=upper,hinge=left,open=false]")
    bed_foot, bed_head = bed_cells(hx0, hy0, hz0)
    assert bed_facing_valid(bed_foot, bed_head, "east"), "bed fixture must be east foot->head"
    assert house_headroom_ok(bed_head[1], hy1), "bed needs two air blocks under the roof"
    fixture(f"setblock {bed_foot[0]} {bed_foot[1]} {bed_foot[2]} minecraft:red_bed[facing=east,part=foot,occupied=false]")
    fixture(f"setblock {bed_head[0]} {bed_head[1]} {bed_head[2]} minecraft:red_bed[facing=east,part=head,occupied=false]")
    _expect_block(probe, door, "minecraft:oak_door", "house_door_placed")
    _expect_block(probe, bed_head, "minecraft:red_bed", "house_bed_placed")
    sleeper_stage = (hx0 + 3, hy0 + 1, hz0 + 3)
    fixture(f"setblock {sleeper_stage[0]} {sleeper_stage[1]} {sleeper_stage[2]} minecraft:air")
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "village-qa-house")

    # --- workshop 5x5x5 with west door and open interior; farm inside reach east ---
    work_lo, work_hi = _rect(origin, (8, 0, 0), (12, 4, 4))
    wx0, wy0, wz0 = work_lo
    wx1, wy1, wz1 = work_hi
    fixture(f"fill {wx0} {wy0} {wz0} {wx1} {wy0} {wz1} minecraft:oak_planks")
    fixture(f"fill {wx0} {wy1} {wz0} {wx1} {wy1} {wz1} minecraft:oak_planks")
    fixture(f"fill {wx0} {wy0 + 1} {wz0} {wx1} {wy0 + 3} {wz0} minecraft:cobblestone")
    fixture(f"fill {wx0} {wy0 + 1} {wz1} {wx1} {wy0 + 3} {wz1} minecraft:cobblestone")
    fixture(f"fill {wx0} {wy0 + 1} {wz0} {wx0} {wy0 + 3} {wz1} minecraft:cobblestone")
    fixture(f"fill {wx1} {wy0 + 1} {wz0} {wx1} {wy0 + 3} {wz1} minecraft:cobblestone")
    work_door = (wx0, wy0 + 1, wz0 + 2)
    work_door_top = (wx0, wy0 + 2, wz0 + 2)
    fixture(f"setblock {work_door[0]} {work_door[1]} {work_door[2]} minecraft:air")
    fixture(f"setblock {work_door_top[0]} {work_door_top[1]} {work_door_top[2]} minecraft:air")
    fixture(f"setblock {work_door[0]} {work_door[1]} {work_door[2]} minecraft:oak_door[facing=west,half=lower,hinge=left,open=false]")
    fixture(f"setblock {work_door_top[0]} {work_door_top[1]} {work_door_top[2]} minecraft:oak_door[facing=west,half=upper,hinge=left,open=false]")
    composter = (wx0 + 1, wy0 + 1, wz0 + 1)
    fixture(f"setblock {composter[0]} {composter[1]} {composter[2]} minecraft:composter")
    farmer_stage = (wx0 + 2, wy0 + 1, wz0 + 2)
    fixture(f"setblock {farmer_stage[0]} {farmer_stage[1]} {farmer_stage[2]} minecraft:air")
    fixture(f"setblock {farmer_stage[0]} {farmer_stage[1] + 1} {farmer_stage[2]} minecraft:air")
    # Farmland row inside the east wall line: within farmer REACH 4 of the station.
    farm = [(wx1, wy0, wz0 + 1 + i) for i in range(3)]
    for fx, fy, fz in farm:
        fixture(f"setblock {fx} {fy} {fz} minecraft:farmland[moisture=7]")
        fixture(f"setblock {fx} {fy + 1} {fz} minecraft:wheat[age=7]")
    fixture(f"setblock {wx1 + 1} {wy0} {wz0 + 2} minecraft:water")
    _expect_block(probe, composter, "minecraft:composter", "workshop_composter_placed")
    _expect_block(probe, (farm[0][0], farm[0][1] + 1, farm[0][2]), "minecraft:wheat", "wheat_mature_placed")
    wheat_props = _props(_read(probe, (farm[0][0], farm[0][1] + 1, farm[0][2])))
    if wheat_props.get("age") != "7":
        raise RuntimeError(f"village-qa fixture failed: wheat age is {wheat_props}, want age=7")
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "village-qa-workshop")

    # --- storage 5x5x5 with west door, open interior, one chest ---
    store_lo, store_hi = _rect(origin, (8, 0, 6), (12, 4, 10))
    sx0, sy0, sz0 = store_lo
    sx1, sy1, sz1 = store_hi
    fixture(f"fill {sx0} {sy0} {sz0} {sx1} {sy0} {sz1} minecraft:oak_planks")
    fixture(f"fill {sx0} {sy1} {sz0} {sx1} {sy1} {sz1} minecraft:oak_planks")
    fixture(f"fill {sx0} {sy0 + 1} {sz0} {sx1} {sy0 + 3} {sz0} minecraft:cobblestone")
    fixture(f"fill {sx0} {sy0 + 1} {sz1} {sx1} {sy0 + 3} {sz1} minecraft:cobblestone")
    fixture(f"fill {sx0} {sy0 + 1} {sz0} {sx0} {sy0 + 3} {sz1} minecraft:cobblestone")
    fixture(f"fill {sx1} {sy0 + 1} {sz0} {sx1} {sy0 + 3} {sz1} minecraft:cobblestone")
    store_door = (sx0, sy0 + 1, sz0 + 2)
    store_door_top = (sx0, sy0 + 2, sz0 + 2)
    fixture(f"setblock {store_door[0]} {store_door[1]} {store_door[2]} minecraft:air")
    fixture(f"setblock {store_door_top[0]} {store_door_top[1]} {store_door_top[2]} minecraft:air")
    fixture(f"setblock {store_door[0]} {store_door[1]} {store_door[2]} minecraft:oak_door[facing=west,half=lower,hinge=left,open=false]")
    fixture(f"setblock {store_door_top[0]} {store_door_top[1]} {store_door_top[2]} minecraft:oak_door[facing=west,half=upper,hinge=left,open=false]")
    chest = (sx0 + 1, sy0 + 1, sz0 + 1)
    fixture(f"setblock {chest[0]} {chest[1]} {chest[2]} minecraft:chest[facing=north,type=single,waterlogged=false]")
    store_stage = (sx0 + 2, sy0 + 1, sz0 + 2)
    fixture(f"setblock {store_stage[0]} {store_stage[1]} {store_stage[2]} minecraft:air")
    fixture(f"setblock {store_stage[0]} {store_stage[1] + 1} {store_stage[2]} minecraft:air")
    _expect_block(probe, chest, "minecraft:chest", "storage_chest_placed")
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "village-qa-storage")

    # --- mine zone at grade: stone to y0, air above, smithing table at top plane ---
    mine_lo, mine_hi = _rect(origin, (14, -6, -2), (24, 1, 8))
    mx1, my1, _mz1 = mine_hi
    fixture(f"fill {mine_lo[0]} {mine_lo[1]} {mine_lo[2]} {mx1} {my1 - 1} {mine_hi[2]} minecraft:stone")
    fixture(f"fill {mine_lo[0]} {my1} {mine_lo[2]} {mx1} {my1 + 3} {mine_hi[2]} minecraft:air")
    entrance = (mine_lo[0] + 1, my1, mine_lo[2] + 1)
    fixture(f"setblock {entrance[0]} {entrance[1]} {entrance[2]} minecraft:smithing_table")
    miner_stage = (entrance[0] + 1, my1, entrance[2])
    fixture(f"setblock {miner_stage[0]} {miner_stage[1]} {miner_stage[2]} minecraft:air")
    _expect_block(probe, entrance, "minecraft:smithing_table", "mine_station_placed")
    # Permanent hazards can select any surface lane. Observe the first two
    # descending floor cells of every lane, never just the original entrance.
    dig_cells = [(entrance[0] + dx, entrance[1] - dx, z)
                 for dx in (1, 2) for z in range(mine_lo[2] + 1, mine_hi[2])]
    dig_before = {cell: _block_id(_read(probe, cell)) for cell in dig_cells}
    detail["mine_probe_cells"] = len(dig_cells)

    # --- lumber camp 5x5x5 with west door; two separated mature persistent=false oaks ---
    camp_lo, camp_hi = _rect(origin, (14, 0, 12), (18, 4, 16))
    cx0, cy0, cz0 = camp_lo
    cx1, cy1, cz1 = camp_hi
    fixture(f"fill {cx0} {cy0} {cz0} {cx1} {cy0} {cz1} minecraft:oak_planks")
    fixture(f"fill {cx0} {cy1} {cz0} {cx1} {cy1} {cz1} minecraft:oak_planks")
    fixture(f"fill {cx0} {cy0 + 1} {cz0} {cx1} {cy0 + 3} {cz0} minecraft:cobblestone")
    fixture(f"fill {cx0} {cy0 + 1} {cz1} {cx1} {cy0 + 3} {cz1} minecraft:cobblestone")
    fixture(f"fill {cx0} {cy0 + 1} {cz0} {cx0} {cy0 + 3} {cz1} minecraft:cobblestone")
    fixture(f"fill {cx1} {cy0 + 1} {cz0} {cx1} {cy0 + 3} {cz1} minecraft:cobblestone")
    camp_door = (cx0, cy0 + 1, cz0 + 2)
    camp_door_top = (cx0, cy0 + 2, cz0 + 2)
    fixture(f"setblock {camp_door[0]} {camp_door[1]} {camp_door[2]} minecraft:air")
    fixture(f"setblock {camp_door_top[0]} {camp_door_top[1]} {camp_door_top[2]} minecraft:air")
    fixture(f"setblock {camp_door[0]} {camp_door[1]} {camp_door[2]} minecraft:oak_door[facing=west,half=lower,hinge=left,open=false]")
    fixture(f"setblock {camp_door_top[0]} {camp_door_top[1]} {camp_door_top[2]} minecraft:oak_door[facing=west,half=upper,hinge=left,open=false]")
    fletcher = (cx0 + 1, cy0 + 1, cz0 + 1)
    fixture(f"setblock {fletcher[0]} {fletcher[1]} {fletcher[2]} minecraft:fletching_table")
    lumber_stage = (cx0 + 2, cy0 + 1, cz0 + 2)
    fixture(f"setblock {lumber_stage[0]} {lumber_stage[1]} {lumber_stage[2]} minecraft:air")
    fixture(f"setblock {lumber_stage[0]} {lumber_stage[1] + 1} {lumber_stage[2]} minecraft:air")
    _expect_block(probe, fletcher, "minecraft:fletching_table", "camp_station_placed")
    # Chebyshev >= 5 apart so neither 5x5 crown overwrites the other's trunk.
    tree_soils = [(cx0 + 1, cy0, cz1 + 2), (cx0 + 4, cy0, cz1 + 7)]
    assert trees_separated(tree_soils), "tree crowns must not overlap trunks"
    for tx, ty, tz in tree_soils:
        fixture(f"setblock {tx} {ty} {tz} minecraft:dirt")
        fixture(f"fill {tx - 2} {ty + 3} {tz - 2} {tx + 2} {ty + 5} {tz + 2} minecraft:oak_leaves[persistent=false,waterlogged=false]")
        fixture(f"fill {tx} {ty + 1} {tz} {tx} {ty + 4} {tz} minecraft:oak_log")
    settled("trees_planted")
    tree_trunks = {soil: [(soil[0], soil[1] + h, soil[2]) for h in (1, 2, 3, 4)]
                   for soil in tree_soils}
    for soil, cells in tree_trunks.items():
        for cell in cells:
            _expect_block(probe, cell, "minecraft:oak_log", f"tree_trunk_{soil}")
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "village-qa-camp")

    # Raised floors have ordinary half-step approaches: a 1.95-high villager
    # cannot jump a full block through a two-block-high lintel.
    for cell, dx, facing in ((door, 1, "west"), (work_door, -1, "east"),
                             (store_door, -1, "east"), (camp_door, -1, "east")):
        fixture(f"setblock {cell[0] + dx} {cell[1] - 1} {cell[2]} "
                f"minecraft:oak_stairs[facing={facing},half=bottom,shape=straight]")

    # --- settlement radius covers every fixture, then certification ---
    cover = [house_lo, house_hi, work_lo, work_hi, store_lo, store_hi,
             mine_lo, mine_hi, camp_lo, camp_hi, *tree_soils,
             farmer_stage, miner_stage, lumber_stage, sleeper_stage]
    radius = required_radius(origin, cover)
    detail["settlement_radius"] = radius
    fixture(f"settlement create {SETTLEMENT} {radius}")
    settled("settlement_created")
    fixture(f"settlement building certify house {house_lo[0]} {house_lo[1]} {house_lo[2]} "
            f"{house_hi[0]} {house_hi[1]} {house_hi[2]}")
    house_echo = settled("house_certified")
    if not any("Certified house" in line for line in house_echo.get("recent_chat", [])):
        raise RuntimeError("village-qa fixture failed: house not certified: "
                           f"{house_echo.get('recent_chat', [])[-4:]}")
    fixture(f"settlement building certify farmer {work_lo[0]} {work_lo[1]} {work_lo[2]} "
            f"{work_hi[0]} {work_hi[1]} {work_hi[2]}")
    farmer_echo = settled("farmer_certified")
    probe.check("farmer_certificate_accepted",
                any("Certified farmer" in line for line in farmer_echo.get("recent_chat", [])))
    fixture(f"settlement building certify storage {store_lo[0]} {store_lo[1]} {store_lo[2]} "
            f"{store_hi[0]} {store_hi[1]} {store_hi[2]}")
    storage_echo = settled("storage_certified")
    probe.check("storage_certificate_accepted",
                any("Certified storage" in line for line in storage_echo.get("recent_chat", [])))
    fixture(f"settlement building certify mine {mine_lo[0]} {mine_lo[1]} {mine_lo[2]} "
            f"{mine_hi[0]} {mine_hi[1]} {mine_hi[2]}")
    mine_echo = settled("mine_certified")
    probe.check("mine_certificate_accepted",
                any("Certified mine" in line for line in mine_echo.get("recent_chat", [])))
    fixture(f"settlement building certify lumber_camp {camp_lo[0]} {camp_lo[1]} {camp_lo[2]} "
            f"{camp_hi[0]} {camp_hi[1]} {camp_hi[2]}")
    camp_echo = settled("camp_certified")
    probe.check("camp_certificate_accepted",
                any("Certified lumber_camp" in line for line in camp_echo.get("recent_chat", [])))

    # --- chest starts empty (MCP slot evidence baseline) ---
    teleport(float(store_stage[0]) + 0.5, float(store_stage[1]), float(store_stage[2]) + 0.5)
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "village-qa-chest-empty")
    empty_totals = _chest_totals(_open_chest(probe, chest))
    detail["chest_empty_baseline"] = empty_totals
    if empty_totals:
        raise RuntimeError(f"village-qa fixture failed: chest not empty: {empty_totals}")

    # --- summon + bind sequentially, nearest fresh villager per role (sleeper last) ---
    detail["villager_uuids"] = []
    role_plan = (("farmer", sleeper_stage), ("miner", miner_stage),
                 ("lumberjack", lumber_stage), ("sleeper", farmer_stage))
    role_uuid: dict[str, str] = {}
    for role, stage in role_plan:
        fixture(f"summon minecraft:villager {stage[0]} {stage[1]} {stage[2]}")
        settled(f"{role}_summoned")
        candidates = _villagers(probe)
        if not candidates:
            raise RuntimeError(f"village-qa fixture failed: no villagers listed after {role} summon")
        taken = set(role_uuid.values())
        fresh = [row for row in candidates if row.get("entity_uuid") not in taken]
        probe.check(f"{role}_has_fresh_entity", bool(fresh))
        pick = min(fresh,
                   key=lambda row: abs(float(row["x"]) - stage[0]) + abs(float(row["z"]) - stage[2]))
        uuid = str(pick["entity_uuid"])
        teleport(float(pick["x"]), float(pick["y"]) + 1.0, float(pick["z"]))
        fixture(f"settlement bind {SETTLEMENT} {uuid}")
        role_uuid[role] = uuid
        detail["villager_uuids"].append({"role": role, "uuid": uuid, "stage": list(stage)})
    bound_echo = settled("villagers_bound")
    detail["bind_chat"] = bound_echo.get("recent_chat", [])[:12]
    detail["bind_ok_count"] = sum(1 for line in probe.observe().get("recent_chat", [])
                                  if "Bound " in line and f" to {SETTLEMENT}." in line)
    probe.check("four_villagers_bound", len(set(role_uuid.values())) == 4 and detail["bind_ok_count"] >= 4)
    detail["villagers_visible"] = len(_villagers(probe))
    detail["villager_uuids_observed"] = list(role_uuid.values())
    if count_time_sets(probe.report["fixture_commands"]) != 1:
        raise RuntimeError("village-qa fixture failed: expected one initial night phase")
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "village-qa-bound")

    stations = {"farmer": composter, "miner": entrance, "lumberjack": fletcher}
    worker_uuids = {role_uuid[role] for role in stations}
    doors = [door, work_door, store_door, camp_door]
    teleport(x0 + 6.5, y0 + 1.0, z0 + 7.5)
    probe.call("minecraft_look", {"yaw_deg": 180, "pitch_deg": 0})

    # --- (1) door walk: frozen day 3000, worker crossing + open + awaited close ---
    fixture("time set 3000")
    settled("day_work_begins")
    for cell in doors:
        if "door" not in _block_id(_read(probe, cell)):
            _verdict(detail, "door_walk", "FAIL", f"door cell {list(cell)} holds no door",
                     f"minecraft_read_block {cell[0]} {cell[1]} {cell[2]}")
            break
    else:
        tracks: dict[str, list[tuple[float, float, float]]] = {}
        crossed: dict[str, tuple[int, int, int]] = {}
        opened_doors: set[tuple[int, int, int]] = set()
        flips: dict[str, int] = {}
        deadline = _time.monotonic() + 70.0
        while _time.monotonic() < deadline and not any(
                cell in opened_doors for cell in crossed.values()):
            for row in _villagers(probe):
                uuid = str(row.get("entity_uuid"))
                pos = (float(row["x"]), float(row["y"]), float(row["z"]))
                history = tracks.setdefault(uuid, [])
                history.append(pos)
                if len(history) >= 2 and uuid in worker_uuids:
                    for cell in doors:
                        if abs(pos[1] - cell[1]) < 3.5 \
                                and abs(pos[2] - (cell[2] + 0.5)) < 0.9 \
                                and door_crossed(history[-2][0], pos[0], cell[0]):
                            crossed[uuid] = cell
                            flips[uuid] = flips.get(uuid, 0) + 1
            for cell in doors:
                if _props(_read(probe, cell)).get("open") == "true":
                    opened_doors.add(cell)
            if not any(cell in opened_doors for cell in crossed.values()):
                _tick(probe)
        detail["door_flips"] = {uuid[:8]: count for uuid, count in flips.items()}
        crossings = [(uuid, cell) for uuid, cell in crossed.items() if cell in opened_doors]
        if crossings:
            # Bounded exact predicate: the crossed door must actually close again.
            crosser, cell = crossings[0]
            close_deadline = _time.monotonic() + 40.0
            closed = _props(_read(probe, cell)).get("open") == "false"
            while not closed and _time.monotonic() < close_deadline:
                _tick(probe)
                closed = _props(_read(probe, cell)).get("open") == "false"
            if closed:
                _verdict(detail, "door_walk", "PASS",
                         f"worker {crosser[:8]} crossed door {list(cell)} "
                         f"(open seen, flips={flips.get(crosser)}, ends open=false)")
            else:
                _verdict(detail, "door_walk", "FAIL",
                         f"worker {crosser[:8]} crossed {list(cell)} but it never closed",
                         f"time set 3000 (frozen); minecraft_read_block {cell[0]} {cell[1]} {cell[2]}; "
                         "server log: villager door open/close lines")
        else:
            _verdict(detail, "door_walk", "FAIL",
                     f"crossed={list(crossed)} opened={[list(c) for c in opened_doors]}",
                     "time set 3000 (frozen); minecraft_read_block at the four doors; "
                     "server log: villager door open/close lines")
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "village-qa-workday")

    # --- (2) night sleep: same entities persist, one in sleep pose by the bed ---
    evening = {str(row["entity_uuid"]): (float(row["x"]), float(row["y"]), float(row["z"]))
               for row in _villagers(probe)}
    fixture("time set 18000")
    settled("night_falls")
    night_tracks: dict[str, tuple[float, float, float]] = {}
    sleeper: str | None = None
    night_deadline = _time.monotonic() + 50.0
    while _time.monotonic() < night_deadline:
        for row in _villagers(probe):
            uuid = str(row.get("entity_uuid"))
            pos = (float(row["x"]), float(row["y"]), float(row["z"]))
            night_tracks[uuid] = pos
            if uuid in worker_uuids and sleeping_at_bed(row, bed_head):
                sleeper = uuid
        if sleeper is not None and set(evening) <= set(night_tracks):
            break
        _tick(probe)
    # --- morning return at work hours 3000 (not idle 23000), same UUIDs to same jobs ---
    fixture("time set 3000")
    settled("morning_returns")
    morning: dict[str, tuple[float, float, float]] = {}
    morning_deadline = _time.monotonic() + 45.0
    morning_near: dict[str, bool] = {}
    while _time.monotonic() < morning_deadline:
        for row in _villagers(probe):
            morning[str(row.get("entity_uuid"))] = (float(row["x"]), float(row["y"]), float(row["z"]))
        morning_near = {
            role: role_uuid[role] in morning and math.dist(
                (morning[role_uuid[role]][0], morning[role_uuid[role]][2]),
                (stations[role][0] + 0.5, stations[role][2] + 0.5)) < 2.5
            for role in stations
        }
        returned = any(role_uuid[role] == sleeper and near for role, near in morning_near.items())
        if set(evening) <= set(morning) and returned:
            break
        _tick(probe)
    same_entities = set(evening) != set() and set(evening) <= set(morning)
    returned = any(role_uuid[role] == sleeper and near for role, near in morning_near.items())
    if same_entities and sleeper is not None and returned:
        _verdict(detail, "sleep_return", "PASS",
                 f"same 4 entities persist; {sleeper[:8]} slept by bed {list(bed_head)}; "
                 f"same worker returned within 2.5 of its original certified station: {morning_near}")
    else:
        _verdict(detail, "sleep_return", "FAIL",
                 f"same_entities={same_entities} sleeper={sleeper} morning_near={morning_near}",
                 "time set 18000; minecraft_list_entities radius 64; time set 3000; "
                 "re-observe same entity_uuid near same job site")
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "village-qa-morning")

    # --- (3) farmer harvests mature wheat (frozen day, poll age, no time spam) ---
    wheat_cell = (farm[0][0], farm[0][1] + 1, farm[0][2])
    wheat_now = _read(probe, wheat_cell)
    harvest_deadline = _time.monotonic() + 70.0
    while _props(wheat_now).get("age") == "7" and _time.monotonic() < harvest_deadline:
        _tick(probe)
        wheat_now = _read(probe, wheat_cell)
    age_now = _props(wheat_now).get("age")
    if _block_id(wheat_now) == "minecraft:wheat" and age_now is not None and int(age_now) < 7:
        _verdict(detail, "farmer_harvest", "PASS",
                 f"wheat at {list(wheat_cell)} changed age 7 -> {age_now} (harvested and replanted)")
    else:
        _verdict(detail, "farmer_harvest", "FAIL",
                 f"wheat still age=7 at {list(wheat_cell)} after 70s frozen workday",
                 f"time set 3000 (frozen); minecraft_read_block {wheat_cell[0]} {wheat_cell[1]} {wheat_cell[2]}; "
                 "server log: harvest lines")
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "village-qa-farm")

    # --- (4)+(5) work window: miner digs, lumberjack cuts (frozen 3000) ---
    dug_seen: tuple[int, int, int] | None = None
    cut_seen: tuple[int, int, int] | None = None
    replant_seen = False
    work_deadline = probe.deadline - 190.0
    while (dug_seen is None or cut_seen is None or not replant_seen) and _time.monotonic() < work_deadline:
        _tick(probe)
        if dug_seen is None:
            for cell in dig_cells:
                if _block_id(_read(probe, cell)) != dig_before[cell]:
                    dug_seen = cell
                    break
        if cut_seen is None:
            for cells in tree_trunks.values():
                for cell in cells:
                    if _block_id(_read(probe, cell)) != "minecraft:oak_log":
                        cut_seen = cell
                        break
                if cut_seen is not None:
                    break
        for soil in tree_soils:
            if _block_id(_read(probe, (soil[0], soil[1] + 1, soil[2]))) == "minecraft:oak_sapling":
                replant_seen = True
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "village-qa-mine")

    # --- day-end 9000 triggers deposit trips; bounded chest wait for both products ---
    fixture("time set 9000")
    settled("day_end_deposits")
    totals: dict[str, int] = {}
    deposit_deadline = probe.deadline - 70.0
    last_chest_check = 0.0
    while _time.monotonic() < deposit_deadline:
        _tick(probe)
        for soil in tree_soils:
            base = (soil[0], soil[1] + 1, soil[2])
            if _block_id(_read(probe, base)) == "minecraft:oak_sapling":
                replant_seen = True
        if _time.monotonic() - last_chest_check >= 20.0:
            last_chest_check = _time.monotonic()
            teleport(float(store_stage[0]) + 0.5, float(store_stage[1]), float(store_stage[2]) + 0.5)
            totals = _chest_totals(_open_chest(probe, chest))
            if chest_has(totals, MINING_ITEMS) and totals.get("minecraft:oak_log", 0) > 0:
                break
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "village-qa-lumber")

    # --- final chest slot evidence (one open, MCP slots 0..26) ---
    teleport(float(store_stage[0]) + 0.5, float(store_stage[1]), float(store_stage[2]) + 0.5)
    totals = _chest_totals(_open_chest(probe, chest))
    detail["chest_final_totals"] = totals

    if dug_seen is not None and chest_has(totals, MINING_ITEMS):
        _verdict(detail, "miner_deposit", "PASS",
                 f"dug {list(dug_seen)} {dig_before[dug_seen]} -> "
                 f"{_block_id(_read(probe, dug_seen))}; chest holds mining {totals}")
    else:
        _verdict(detail, "miner_deposit", "FAIL",
                 f"dug={dug_seen} chest={totals}",
                 f"time set 9000 (frozen day-end); open chest at {list(chest)} "
                 "(minecraft_use_item_on face up -> ChestMenu slots 0..26); "
                 "server log: mining dig/deposit lines")
    if cut_seen is not None and replant_seen and totals.get("minecraft:oak_log", 0) > 0:
        _verdict(detail, "lumberjack", "PASS",
                 f"trunk {list(cut_seen)} cut, sapling replanted; "
                 f"chest oak_log={totals.get('minecraft:oak_log', 0)} totals={totals}")
    else:
        _verdict(detail, "lumberjack", "FAIL",
                 f"cut={cut_seen} replant={replant_seen} chest={totals}",
                 f"time set 9000 (frozen day-end); minecraft_read_block at {tree_soils[0]} trunk; "
                 "open chest (ChestMenu slots 0..26); server log: lumber cut/replant/deposit lines")

    # --- settlement evidence through ordinary menus/chat ---
    probe.call("minecraft_send_chat", {"message": f"settlement info {SETTLEMENT}", "command": True})
    info = _chat_echo(probe, "settlement_info")
    detail["settlement_info_chat"] = info.get("recent_chat", [])[:6]
    probe.call("minecraft_send_chat", {"message": "settlement building list", "command": True})
    listing = _chat_echo(probe, "building_list")
    detail["building_list_chat"] = listing.get("recent_chat", [])[:12]
    inv = probe.observe()
    detail["player_inventory"] = inv.get("inventory", [])
    detail["recent_chat_tail"] = inv.get("recent_chat", [])[-20:]
    detail["villager_positions"] = [
        {"uuid": v.get("entity_uuid"), "position": [v.get("x"), v.get("y"), v.get("z")]}
        for v in _villagers(probe)]
    detail["time_set_count"] = count_time_sets(probe.report["fixture_commands"])
    probe.check("all_behaviours_pass", all_behaviours_pass(detail["behaviour"]))
