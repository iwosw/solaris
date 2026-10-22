"""P5 vanilla graphical QA; screenshots are evidence for independent visual review."""
from __future__ import annotations

import ctypes as c
import ctypes.util
from pathlib import Path
from typing import Any

from . import native_focus, native_p4
from .driver_client import wait_for_valid_png
from .native_p1 import _aim, _inventory, _rows
from .native_v2 import _block, _click, _echo, _open, _point, _prepare, _set, _use
from .probe_steps import ProbeSteps, container_slot, item_count

SCENARIO_IDS = frozenset({"java-v2-p5-parity", "java-v2-p5-towns", "java-v2-p5-cursor"})
REQUIRED_TOOLS = frozenset({
    "minecraft_observe", "minecraft_wait_for_state_change", "minecraft_send_chat",
    "minecraft_read_block", "minecraft_wait_for_block_state", "minecraft_use_item_on",
    "minecraft_set_hotbar_slot", "minecraft_select_hotbar_item", "minecraft_wait_for_inventory",
    "minecraft_wait_ticks", "minecraft_look", "minecraft_press_inputs", "minecraft_close_screen",
    "minecraft_click_container_slot", "minecraft_quick_move_container_slot",
    "minecraft_click_screen_button", "minecraft_read_recipe_book", "minecraft_screenshot",
    "minecraft_list_entities", "minecraft_break_block", "minecraft_wait_for_health_below",
    "minecraft_drop_selected_item", "minecraft_attack_entity_once",
})


def acquire_window(display: str, timeout: float) -> dict[str, Any]:
    """Find this Java client's window on its private, harness-owned Xvfb."""
    x = c.CDLL(ctypes.util.find_library("X11"))
    pointer, window = c.c_void_p, c.c_ulong
    signatures = {
        "XOpenDisplay": (pointer, [c.c_char_p]),
        "XDefaultRootWindow": (window, [pointer]),
        "XInternAtom": (window, [pointer, c.c_char_p, c.c_int]),
        "XQueryTree": (c.c_int, [pointer, window, c.POINTER(window), c.POINTER(window), c.POINTER(c.POINTER(window)), c.POINTER(c.c_uint)]),
        "XGetWindowProperty": (c.c_int, [pointer, window, window, c.c_long, c.c_long, c.c_int, window, c.POINTER(window), c.POINTER(c.c_int), c.POINTER(window), c.POINTER(window), c.POINTER(pointer)]),
        "XFree": (c.c_int, [pointer]),
        "XCloseDisplay": (c.c_int, [pointer]),
    }
    for name, (restype, argtypes) in signatures.items():
        getattr(x, name).restype, getattr(x, name).argtypes = restype, argtypes
    connection = x.XOpenDisplay(display.encode())
    if not connection:
        raise RuntimeError("cannot open this Java runner's Xvfb")
    try:
        atom = x.XInternAtom(connection, b"_NET_WM_PID", 0)

        def find(parent: int) -> int | None:
            root, owner, count = window(), window(), c.c_uint()
            children = c.POINTER(window)()
            if not x.XQueryTree(connection, parent, c.byref(root), c.byref(owner), c.byref(children), c.byref(count)):
                return None
            try:
                for index in range(count.value):
                    child = children[index]
                    actual, format_, items, remaining, data = window(), c.c_int(), window(), window(), pointer()
                    x.XGetWindowProperty(connection, child, atom, 0, 1, 0, 6, c.byref(actual), c.byref(format_), c.byref(items), c.byref(remaining), c.byref(data))
                    if data:
                        try:
                            if format_.value == 32 and items.value == 1:
                                pid = c.cast(data, c.POINTER(window))[0]
                                try:
                                    environment = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
                                    if f"DISPLAY={display}".encode() in environment:
                                        return pid
                                except OSError:
                                    pass
                        finally:
                            x.XFree(data)
                    nested = find(child)
                    if nested is not None:
                        return nested
                return None
            finally:
                if children:
                    x.XFree(children)

        pid = find(x.XDefaultRootWindow(connection))
    finally:
        x.XCloseDisplay(connection)
    if pid is None:
        raise RuntimeError("connected Java client has no window on its private Xvfb")
    return native_focus.acquire(display, pid, timeout)


def capture(steps: ProbeSteps, name: str) -> str:
    path = steps.artifact_dir.parent / "screenshots" / f"{name}.png"
    path.parent.mkdir(parents=True, exist_ok=True)
    steps.call("minecraft_screenshot", {"path": str(path)})
    error = wait_for_valid_png(path, min(steps.remaining(), 5))
    if error:
        raise RuntimeError(f"invalid Java graphical capture {path}: {error}")
    steps.report.setdefault("screenshots", []).append(str(path))
    return str(path)


def _pixel_click(steps: ProbeSteps, x_offset: int, y: int) -> None:
    """Click a screenshot-observed custom tab omitted by Java's Button API."""
    with Path(steps.report["screenshots"][-1]).open("rb") as image:
        width = int.from_bytes(image.read(24)[16:20], "big")
    owned = steps.client.p4_window
    x = c.CDLL(ctypes.util.find_library("X11"))
    xt = c.CDLL(ctypes.util.find_library("Xtst"))
    x.XOpenDisplay.restype = c.c_void_p
    x.XOpenDisplay.argtypes = [c.c_char_p]
    x.XWarpPointer.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_int, c.c_int,
                              c.c_uint, c.c_uint, c.c_int, c.c_int]
    x.XSync.argtypes = [c.c_void_p, c.c_int]
    x.XCloseDisplay.argtypes = [c.c_void_p]
    xt.XTestFakeButtonEvent.argtypes = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong]
    display = x.XOpenDisplay(owned["display"].encode())
    if not display:
        raise RuntimeError("cannot open the Java scenario's private Xvfb")
    try:
        x.XWarpPointer(display, 0, owned["window_id"], 0, 0, 0, 0, width // 2 + x_offset, y)
        if not xt.XTestFakeButtonEvent(display, 1, 1, 0) or not xt.XTestFakeButtonEvent(display, 1, 0, 0):
            raise RuntimeError("owned Xvfb refused ordinary left click")
        x.XSync(display, 0)
    finally:
        x.XCloseDisplay(display)
    steps.report.setdefault("ordinary_pixel_clicks", []).append(
        {"x": width // 2 + x_offset, "y": y, "gui_scale": 2})


def _pause(steps: ProbeSteps) -> None:
    steps.call("minecraft_press_inputs", {"keys": ["key.keyboard.escape"], "ticks": 1})
    steps.wait(lambda state: state["screen"]["class"].endswith("PauseScreen"), "vanilla_pause_screen")


def _stats_screen(steps: ProbeSteps, label: str) -> None:
    _pause(steps)
    native_p4._button(steps, "Statistics")
    steps.wait(lambda state: state["screen"]["class"].endswith("StatsScreen"), label + "_stats_screen")
    steps.call("minecraft_wait_ticks", {"ticks": 4})
    capture(steps, label + "-general")
    for page in range(16):
        native_p4._scroll(steps, 8)
        steps.call("minecraft_wait_ticks", {"ticks": 2})
        capture(steps, label + f"-general-{page + 1}")
    _pixel_click(steps, 0, 24)  # Items custom tab, observed at the window centre.
    steps.call("minecraft_wait_ticks", {"ticks": 2})
    capture(steps, label + "-items")
    native_p4._button(steps, "Done")
    steps.call("minecraft_close_screen")


def _statistics(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 0})
    for key in ("forward", "back"):
        steps.call("minecraft_press_inputs", {"keys": [key], "ticks": 8})
    steps.call("minecraft_press_inputs", {"keys": ["jump"], "ticks": 1})
    steps.wait(lambda state: state["player"]["on_ground"], "jump_landed")
    steps.fixture("give @s minecraft:iron_pickaxe 1")
    _inventory(steps, "minecraft:iron_pickaxe", 1)
    steps.call("minecraft_select_hotbar_item", {"item_id": "minecraft:iron_pickaxe", "count": 1, "timeout_seconds": steps.remaining()})
    for dx, block, drop in ((1, "stone", "cobblestone"), (-1, "dirt", "dirt")):
        point = _point(origin, dx=dx)
        _set(steps, point, "minecraft:" + block)
        result = steps.call("minecraft_break_block", {**point, "face": "up", "expected_drop_item_id": "minecraft:" + drop, "expected_drop_count": 1, "timeout_seconds": steps.remaining()})
        steps.check(block + "_ordinary_mine_and_pickup", result.get("became_air") is True and result.get("pickup_confirmed") is True)
        steps.report.setdefault("mining", {})[block] = result
    _stats_screen(steps, "p5-statistics-earned")
    native_p4._restart(steps)
    _stats_screen(steps, "p5-statistics-after-restart")
    _stats_screen(steps, "p5-statistics-repeated")
    steps.report["visual_review_required"] = ["nonzero walking/jump", "one stone and dirt mined and their pickups", "same counters after restart and repeated request"]


def _adv_screen(steps: ProbeSteps, label: str) -> None:
    _pause(steps)
    native_p4._button(steps, "Advancements")
    steps.wait(lambda state: state["screen"]["class"].endswith("AdvancementsScreen"), label + "_screen")
    steps.call("minecraft_wait_ticks", {"ticks": 2})
    capture(steps, label + "-default-tab")
    _pixel_click(steps, 30, 78)  # Grass-block Minecraft root in the observed 26.1.2 layout.
    steps.call("minecraft_wait_ticks", {"ticks": 2})
    capture(steps, label)
    steps.call("minecraft_close_screen")


def _advancements(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    steps.report["recipes_before"] = steps.call("minecraft_read_recipe_book", {"limit": 8192})
    _adv_screen(steps, "p5-advancements-fresh")
    for item, count in (("oak_log", 1), ("crafting_table", 1), ("cobblestone", 3), ("stick", 2)):
        steps.fixture(f"give @s minecraft:{item} {count}")
        _inventory(steps, "minecraft:" + item, count)
        if item == "crafting_table":
            steps.call("minecraft_wait_ticks", {"ticks": 10})
            capture(steps, "p5-advancement-root-toast")
    _adv_screen(steps, "p5-advancements-root")
    steps.call("minecraft_select_hotbar_item", {"item_id": "minecraft:crafting_table", "count": 1, "timeout_seconds": steps.remaining()})
    _use(steps, _point(origin, dy=-1))
    _block(steps, _point(origin), "minecraft:crafting_table")
    _open(steps, _point(origin), "CraftingMenu", 46)
    native_p4._place_ingredients(steps, "minecraft:cobblestone", (1, 2, 3))
    native_p4._place_ingredients(steps, "minecraft:stick", (5, 8))
    result = steps.wait(lambda state: container_slot(state, 0)["item_id"] == "minecraft:stone_pickaxe", "earned_pickaxe_result")
    steps.report["crafting_result"] = container_slot(result, 0)
    capture(steps, "p5-stone-pickaxe-crafting")
    _click(steps, 0, quick=True)
    steps.call("minecraft_close_screen")
    _inventory(steps, "minecraft:stone_pickaxe", 1)
    steps.call("minecraft_wait_ticks", {"ticks": 10})
    capture(steps, "p5-advancement-pickaxe-toast")
    steps.report["completion_chat"] = _echo(steps, "crafted_ordered_receipt")["recent_chat"]
    steps.report["recipes_after"] = steps.call("minecraft_read_recipe_book", {"limit": 8192})
    _adv_screen(steps, "p5-advancements-earned")
    native_p4._restart(steps)
    _inventory(steps, "minecraft:stone_pickaxe", 1)
    _adv_screen(steps, "p5-advancements-after-restart")
    steps.report["recipes_after_restart"] = steps.call("minecraft_read_recipe_book", {"limit": 8192})
    steps.report["visual_review_required"] = ["root and earned advancement toasts", "Minecraft story completion nodes", "recipe unlocks survive restart"]


def _nameplates(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    x, y, z = origin
    steps.fixture(f'summon armor_stand {x + .5} {y} {z + 2.5} {{CustomName:"QA Stand",CustomNameVisible:1b}}')
    _echo(steps, "stand_summon_ordered_receipt")
    row = native_p4._entity_at(steps, "minecraft:armor_stand", x + .5, z + 2.5)
    steps.call("minecraft_wait_ticks", {"ticks": 240})
    yaw, _ = _aim(steps, row)
    steps.call("minecraft_look", {"yaw_deg": yaw, "pitch_deg": 0})
    capture(steps, "p5-stand-visible")
    steps.call("minecraft_look", {"yaw_deg": 25, "pitch_deg": 0})
    capture(steps, "p5-stand-visible-untargeted")
    native_p4._restart(steps)
    restored = native_p4._entity_at(steps, "minecraft:armor_stand", x + .5, z + 2.5)
    steps.check("stand_uuid_persisted", restored["entity_uuid"] == row["entity_uuid"])
    steps.call("minecraft_wait_ticks", {"ticks": 240})
    yaw, _ = _aim(steps, restored)
    steps.call("minecraft_look", {"yaw_deg": yaw, "pitch_deg": 0})
    capture(steps, "p5-stand-visible-after-restart")
    steps.report["visual_review_required"] = ["QA Stand nameplate visible targeted, untargeted and after restart"]


def _damage(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    x, y, z = origin
    steps.fixture("time set 18000")
    _echo(steps, "night_fixture_receipt")
    steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 0})
    baseline = steps.observe()["player"]["health"]
    capture(steps, "p5-damage-before")
    steps.fixture(f"summon zombie {x + .5} {y} {z + 2.5}")
    try:
        steps.report["hit"] = steps.call("minecraft_wait_for_health_below", {"health": baseline, "timeout_seconds": steps.remaining()})
        capture(steps, "p5-damage-hit")
        steps.call("minecraft_press_inputs", {"keys": ["back"], "ticks": 4})
        state = steps.observe()
        steps.check("post_hit_playable", state["in_play"] and state["player"]["health"] > 0)
        capture(steps, "p5-damage-after")
    finally:
        steps.fixture("kill @e[type=minecraft:zombie]")
        steps.fixture("time set 1000")
    steps.report["visual_review_required"] = ["actual vanilla hurt camera tilt versus before"]


def _hopper(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    hopper = _point(origin, dz=1)
    _set(steps, hopper, "minecraft:hopper[facing=down]")
    steps.fixture("give @s minecraft:diamond 1")
    _inventory(steps, "minecraft:diamond", 1)
    steps.call("minecraft_select_hotbar_item", {"item_id": "minecraft:diamond", "count": 1, "timeout_seconds": steps.remaining()})
    steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 0})
    capture(steps, "p5-hopper-before-q")
    steps.call("minecraft_press_inputs", {"keys": ["key.keyboard.q"], "ticks": 1})
    _inventory(steps, "minecraft:diamond", 0)
    steps.call("minecraft_wait_ticks", {"ticks": 40})
    steps.report["nearby_items"] = [row for row in _rows(steps, 8) if row["entity_type"] == "minecraft:item"]
    _open(steps, hopper, "HopperMenu", 41)
    received = steps.wait(lambda state: sum(row["count"] for row in state["container"]["slots"] if row["slot"] < 5 and row["item_id"] == "minecraft:diamond") == 1, "hopper_exact_thrown_diamond")
    steps.check("not_repicked_by_player", item_count(received, "minecraft:diamond") == 0)
    capture(steps, "p5-hopper-diamond")
    slot = next(row["slot"] for row in received["container"]["slots"] if row["slot"] < 5 and row["item_id"] == "minecraft:diamond")
    _click(steps, slot, quick=True)
    steps.call("minecraft_close_screen")
    _inventory(steps, "minecraft:diamond", 1)
    _open(steps, hopper, "HopperMenu", 41)
    steps.check("withdraw_no_duplicate", all(row["count"] == 0 for row in steps.observe()["container"]["slots"] if row["slot"] < 5))
    capture(steps, "p5-hopper-withdrawn")
    steps.call("minecraft_close_screen")


def _teleport(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    x, y, z = origin
    steps.fixture(f"tp @s {x + .5} {y} {z - 6.5}")
    landed = steps.wait(lambda state: abs(state["player"]["z"] - (z - 6.5)) < .2 and state["player"]["on_ground"], "teleported_and_grounded")
    capture(steps, "p5-teleport-arrival")
    steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 0})
    steps.call("minecraft_press_inputs", {"keys": ["forward"], "ticks": 12})
    moved = steps.observe()
    steps.call("minecraft_wait_ticks", {"ticks": 40})
    stable = steps.observe()
    steps.report["positions"] = {name: state["player"] for name, state in (("landed", landed), ("moved", moved), ("stable", stable))}
    steps.check("walked_after_teleport", moved["player"]["z"] > landed["player"]["z"] + .5)
    steps.check("not_rubber_banded", stable["player"]["z"] > landed["player"]["z"] + .5 and abs(stable["player"]["z"] - moved["player"]["z"]) < 1)
    capture(steps, "p5-teleport-walk-stable")


def run_parity(client: Any, artifact: Path, timeout: float) -> dict[str, Any]:
    reports = {}
    for name, action in (("advancements", _advancements), ("statistics", _statistics), ("nameplates", _nameplates), ("damage", _damage), ("hopper", _hopper), ("teleport", _teleport)):
        steps = ProbeSteps(client, artifact / ("p5-" + name), timeout, "java-v2-p5-" + name)
        try:
            with steps:
                steps.call("minecraft_close_screen")
                origin = _prepare(steps)
                steps.fixture("gamemode survival @s")
                steps.fixture("gamerule doMobSpawning false")
                _echo(steps, "survival_fixture_receipt")
                try:
                    action(steps, origin)
                except Exception:
                    try:
                        capture(steps, "p5-" + name + "-failure")
                    except Exception as error:
                        steps.report["failure_capture_error"] = str(error)
                    raise
        except Exception:
            pass
        reports[name] = steps.report
    return {"passed": all(report["passed"] for report in reports.values()), "surfaces": reports}


def _command(steps: ProbeSteps, command: str, reply: str) -> None:
    marker = f"p5 command boundary {len(steps.transcript)}"
    steps.call("minecraft_send_chat", {"message": marker, "command": False})
    steps.wait(lambda state: any(marker in line for line in state["recent_chat"]), "command_boundary_received")
    steps.call("minecraft_send_chat", {"message": command, "command": True})

    def fresh_reply(state: dict[str, Any]) -> bool:
        lines = state["recent_chat"]
        boundary = next((index for index, line in enumerate(lines) if marker in line), None)
        return boundary is not None and any(reply in line for line in lines[:boundary])

    state = steps.wait(fresh_reply, command.replace(" ", "_") + "_reply")
    steps.report.setdefault("ordinary_commands", []).append({
        "command": command, "expected_reply": reply, "recent_chat": state["recent_chat"],
    })


def _deop(steps: ProbeSteps, username: str) -> None:
    steps.fixture("deop " + username)
    _echo(steps, username + "_deop_ordered_receipt")
    steps.call("minecraft_wait_ticks", {"ticks": 2})
    steps.call("minecraft_send_chat", {"message": "give @s minecraft:diamond 1", "command": True})
    state = _echo(steps, username + "_nonop_command_roundtrip")
    steps.check(username + "_nonop_cannot_give_diamond", item_count(state, "minecraft:diamond") == 0)
    steps.report.setdefault("nonop_evidence", {})[username] = state["recent_chat"]


def _town_build(steps: ProbeSteps, origin: tuple[int, int, int], allowed: bool, label: str) -> None:
    point = _point(origin, dx=1)
    before = item_count(steps.observe(), "minecraft:cobblestone")
    steps.call("minecraft_select_hotbar_item", {"item_id": "minecraft:cobblestone", "count": 1, "timeout_seconds": steps.remaining()})
    _use(steps, {**point, "y": point["y"] - 1})
    _echo(steps, label + "_placement_roundtrip")
    steps.call("minecraft_wait_ticks", {"ticks": 3})
    _block(steps, point, "minecraft:cobblestone" if allowed else "minecraft:air")
    after_place = item_count(steps.observe(), "minecraft:cobblestone")
    steps.report["assertions"][label + "_placement_inventory"] = after_place == before - int(allowed)
    steps.report.setdefault("inventory_custody", {})[label] = {
        "before": before, "after_place": after_place, "allowed": allowed,
    }
    capture(steps, label + "-place")
    if allowed:
        steps.call("minecraft_select_hotbar_item", {"item_id": "minecraft:iron_pickaxe", "count": 1, "timeout_seconds": steps.remaining()})
        result = steps.call("minecraft_break_block", {**point, "face": "up", "expected_drop_item_id": "minecraft:cobblestone", "expected_drop_count": 1, "timeout_seconds": steps.remaining()})
        steps.check(label + "_ordinary_break_pickup", result.get("became_air") is True and result.get("pickup_confirmed") is True)
        _inventory(steps, "minecraft:cobblestone", before)
    else:
        stone = _point(origin, dx=-1)
        steps.call("minecraft_select_hotbar_item", {"item_id": "minecraft:iron_pickaxe", "count": 1, "timeout_seconds": steps.remaining()})
        player = steps.observe()["player"]
        import math
        dx, dz = stone["x"] + .5 - player["x"], stone["z"] + .5 - player["z"]
        dy = stone["y"] + .5 - (player["y"] + 1.62)
        steps.call("minecraft_look", {"yaw_deg": round(math.degrees(math.atan2(-dx, dz))), "pitch_deg": round(-math.degrees(math.atan2(dy, math.hypot(dx, dz))))})
        steps.call("minecraft_press_inputs", {"keys": ["attack"], "ticks": 40})
        _echo(steps, label + "_mining_roundtrip")
        _block(steps, stone, "minecraft:stone")
        after_break = item_count(steps.observe(), "minecraft:cobblestone")
        steps.report["assertions"][label + "_denied_break_no_drop_or_item_loss"] = after_break == after_place
        steps.report["inventory_custody"][label]["after_denied_break"] = after_break
    capture(steps, label + "-break")


def _town_protection(steps: ProbeSteps, origin: tuple[int, int, int], allowed: bool, label: str, slot: int) -> None:
    """Chest, fire and animal damage inside the claim: only an allow-listed player may."""
    chest = _point(origin, dx=2)
    if allowed:
        _open(steps, chest, "ChestMenu", 63)
        steps.call("minecraft_close_screen")
    else:
        steps.call("minecraft_set_hotbar_slot", {"slot": 8})
        _use(steps, chest)
        _echo(steps, label + "_chest_roundtrip")
        steps.call("minecraft_wait_ticks", {"ticks": 10})
        steps.check(label + "_visitor_cannot_open_chest", steps.observe()["container"]["container_id"] == 0)
    capture(steps, label + "-chest")
    fire = _point(origin, dx=slot - 1, dz=-2)
    steps.call("minecraft_select_hotbar_item", {"item_id": "minecraft:flint_and_steel", "count": 1, "timeout_seconds": steps.remaining()})
    _use(steps, {**fire, "y": fire["y"] - 1})
    _echo(steps, label + "_fire_roundtrip")
    steps.call("minecraft_wait_ticks", {"ticks": 10})
    _block(steps, fire, "minecraft:fire" if allowed else "minecraft:air")
    capture(steps, label + "-fire")
    rows = steps.call("minecraft_list_entities", {"radius": 12, "limit": 128})["entities"]
    cows = [row for row in rows if row["entity_type"] == "minecraft:cow" and row.get("health") == 10]
    steps.check(label + "_untouched_cow_present", bool(cows))
    identity = {key: cows[0][key] for key in ("entity_id", "entity_uuid", "entity_type")}
    steps.call("minecraft_select_hotbar_item", {"item_id": "minecraft:iron_sword", "count": 1, "timeout_seconds": steps.remaining()})
    try:
        # The tool retries until damage lands: a refused attack is bounded to a short wait.
        steps.call("minecraft_attack_entity_once", {**identity, "timeout_seconds": min(steps.remaining(), 30 if allowed else 6)})
    except Exception as error:  # the cow's health below is the evidence
        steps.report.setdefault("attack_notes", {})[label] = str(error)
    _echo(steps, label + "_attack_roundtrip")
    steps.call("minecraft_wait_ticks", {"ticks": 10})
    rows = steps.call("minecraft_list_entities", {"radius": 12, "limit": 128})["entities"]
    after = next((row for row in rows if row["entity_uuid"] == identity["entity_uuid"]), None)
    hurt = after is None or after.get("health", 10) < 10
    steps.check(label + ("_member_hurts_cow" if allowed else "_visitor_cannot_hurt_cow"), hurt == allowed)
    capture(steps, label + "-cow")


def run_towns(client: Any, artifact: Path, timeout: float) -> dict[str, Any]:
    with ProbeSteps(client, artifact / "java-v2-p5-towns", timeout, "java-v2-p5-towns") as steps:
        try:
            initial = _prepare(steps)
            x, y, z = initial
            center = (x // 16 * 16 + 8, y, z // 16 * 16 + 8)
            steps.fixture(f"tp @s {center[0] + .5} {y} {center[2] + .5}")
            steps.wait(lambda state: abs(state["player"]["x"] - center[0] - .5) < .2 and abs(state["player"]["z"] - center[2] - .5) < .2, "setup_inside_claim_chunk")
            origin = _prepare(steps)
            _set(steps, _point(origin, dx=-1), "minecraft:stone")
            _set(steps, _point(origin, dx=2), "minecraft:chest")
            for dx, dz in ((-1.0, .5), (-1.0, -.5), (-1.0, 1.5)):
                steps.fixture(f"summon minecraft:cow {origin[0] + dx} {origin[1]} {origin[2] + dz} {{NoAI:1b}}")
            _echo(steps, "claim_fixtures_ordered_receipt")
            for name in ("P5Leader", "P5Guest"):
                steps.fixture("op " + name)
            _echo(steps, "temporary_fixture_operators_granted")
            steps.report["claim_chunk"] = [origin[0] // 16, origin[2] // 16]
            steps.report["configured_operator_only"] = "P5Setup"
            for name, allowed in (("P5Leader", True), ("P5Guest", False)):
                steps.client = steps.client.p5_switch_identity(name, steps.remaining())
                steps.fixture(f"tp @s {origin[0] + .5} {origin[1]} {origin[2] + .5}")
                steps.wait(lambda state: abs(state["player"]["x"] - origin[0] - .5) < .2 and abs(state["player"]["z"] - origin[2] - .5) < .2, name + "_fixture_position")
                steps.fixture("gamemode survival @s")
                steps.fixture("clear @s")
                steps.fixture("give @s minecraft:cobblestone 8")
                _inventory(steps, "minecraft:cobblestone", 8)
                steps.fixture("give @s minecraft:iron_pickaxe 1")
                _inventory(steps, "minecraft:iron_pickaxe", 1)
                steps.fixture("give @s minecraft:flint_and_steel 1")
                _inventory(steps, "minecraft:flint_and_steel", 1)
                steps.fixture("give @s minecraft:iron_sword 1")
                _inventory(steps, "minecraft:iron_sword", 1)
                _deop(steps, name)
                steps.report.setdefault("actor_uuids", {})[name] = steps.observe()["player"]["entity_uuid"]
                if allowed:
                    _command(steps, "town create p5qa", "Town p5qa created.")
                    _command(steps, "town claim", "Chunk claimed for p5qa.")
                _command(steps, "town info p5qa", "p5qa: 1 members, 1 claims.")
                _town_build(steps, origin, allowed, "p5-town-" + name.lower())
                _town_protection(steps, origin, allowed, "p5-protect-" + name.lower(), int(not allowed))
            identity = steps.observe()["player"]["entity_uuid"]
            steps.call("minecraft_disconnect")
            address = steps.client.p4_restart_server(steps.remaining())
            steps.call("minecraft_connect", {"server_addr": address})
            steps.call("minecraft_wait_for_play", {"timeout_seconds": steps.remaining()})
            state = steps.wait(lambda state: state["player"]["on_ground"], "guest_grounded_after_restart")
            steps.check("guest_uuid_persisted", state["player"]["entity_uuid"] == identity)
            _command(steps, "town info p5qa", "p5qa: 1 members, 1 claims.")
            _town_build(steps, origin, False, "p5-town-guest-after-restart")
            steps.client = steps.client.p5_switch_identity("P5Leader", steps.remaining())
            steps.check("leader_uuid_persisted", steps.observe()["player"]["entity_uuid"] == steps.report["actor_uuids"]["P5Leader"])
            _command(steps, "town info", "p5qa: 1 members, 1 claims.")
            _town_build(steps, origin, True, "p5-town-leader-after-restart")
            # The leader trusts the guest by UUID (the guest is offline); the same
            # guest may now build, open the chest, light fire and hurt the cow.
            _command(steps, "town trust " + steps.report["actor_uuids"]["P5Guest"], "Guest trusted in your claims.")
            steps.client = steps.client.p5_switch_identity("P5Guest", steps.remaining())
            _town_build(steps, origin, True, "p5-town-trusted-guest")
            _town_protection(steps, origin, True, "p5-protect-trusted-guest", 2)
            steps.check("all_permission_and_inventory_checks", all(steps.report["assertions"].values()))
        except Exception:
            try:
                capture(steps, "p5-towns-failure")
            except Exception as error:
                steps.report["failure_capture_error"] = str(error)
            raise
    return steps.report


def _menu_item_total(state: dict[str, Any], item: str) -> int:
    menu = state["container"]
    stacks = [row for row in menu["slots"] if row["slot"] != 0] + [menu["carried"]]
    return sum(row["count"] for row in stacks if row.get("item_id") == item)


def _cursor_receipt(steps: ProbeSteps, label: str, item: str, count: int) -> dict[str, Any]:
    _echo(steps, label + "_server_roundtrip")
    state = steps.observe()
    steps.report.setdefault("cursor_states", {})[label] = state["container"]
    carried = state["container"]["carried"]
    steps.check(label + "_cursor", carried["count"] == count
                and (count == 0 or carried["item_id"] == item))
    for expected_item, expected_count in (("minecraft:apple", 8), ("minecraft:diamond", 3)):
        steps.check(label + "_" + expected_item + "_conserved",
                    _menu_item_total(state, expected_item) == expected_count)
    return state


def run_cursor(client: Any, artifact: Path, timeout: float) -> dict[str, Any]:
    with ProbeSteps(client, artifact / "java-v2-p5-cursor", timeout, "java-v2-p5-cursor") as steps:
        try:
            origin = _prepare(steps)
            for item, count in (("minecraft:apple", 8), ("minecraft:diamond", 3)):
                steps.fixture(f"give @s {item} {count}")
                _inventory(steps, item, count)
            table = _point(origin)
            _set(steps, table, "minecraft:crafting_table")
            state = _open(steps, table, "CraftingMenu", 46)
            source = {}
            for item in ("minecraft:apple", "minecraft:diamond"):
                rows = [row for row in state["container"]["slots"]
                        if row["slot"] >= 10 and row["item_id"] == item]
                steps.check(item + "_unique_cursor_source", len(rows) == 1)
                source[item] = rows[0]["slot"]
            _click(steps, source["minecraft:apple"])
            _cursor_receipt(steps, "primary_pickup", "minecraft:apple", 8)
            capture(steps, "p6-java-cursor-primary-pickup")
            steps.call("minecraft_click_container_slot", {"slot": 1, "button": "secondary",
                       "timeout_seconds": steps.remaining()})
            _cursor_receipt(steps, "secondary_single_place", "minecraft:apple", 7)
            capture(steps, "p6-java-cursor-secondary-place")
            _click(steps, source["minecraft:diamond"])
            _cursor_receipt(steps, "different_item_swap", "minecraft:diamond", 3)
            capture(steps, "p6-java-cursor-different-item-swap")
            steps.call("minecraft_click_container_slot", {"slot": 2, "button": "secondary",
                       "timeout_seconds": steps.remaining()})
            _cursor_receipt(steps, "diamond_single_place", "minecraft:diamond", 2)
            _click(steps, source["minecraft:diamond"])
            _cursor_receipt(steps, "swap_back", "minecraft:apple", 7)
            steps.call("minecraft_click_container_slot", {"slot": 1, "button": "secondary",
                       "timeout_seconds": steps.remaining()})
            _cursor_receipt(steps, "same_item_merge", "minecraft:apple", 6)
            _click(steps, source["minecraft:apple"])
            _cursor_receipt(steps, "returned_remainder", "", 0)
            for slot in (1, 2):
                _click(steps, slot, quick=True)
            state = _cursor_receipt(steps, "quick_move_inputs", "", 0)
            steps.check("crafting_inputs_empty", all(container_slot(state, slot)["count"] == 0
                        for slot in range(1, 10)))
            apples = next(row["slot"] for row in state["container"]["slots"]
                          if row["slot"] >= 10 and row["item_id"] == "minecraft:apple")
            _click(steps, apples)
            _cursor_receipt(steps, "before_close_with_carried_stack", "minecraft:apple", 8)
            steps.call("minecraft_close_screen")
            _echo(steps, "closed_with_carried_stack_server_roundtrip")
            _inventory(steps, "minecraft:apple", 8)
            _inventory(steps, "minecraft:diamond", 3)
            capture(steps, "p6-java-cursor-closed-with-stack")
            _open(steps, table, "CraftingMenu", 46)
            _cursor_receipt(steps, "reopened_after_carried_close", "", 0)
            capture(steps, "p6-java-cursor-reopened-conserved")
            steps.call("minecraft_close_screen")
        except Exception:
            try:
                capture(steps, "p6-java-cursor-failure")
            except Exception as error:
                steps.report["failure_capture_error"] = str(error)
            raise
    return steps.report
