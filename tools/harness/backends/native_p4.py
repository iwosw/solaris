"""P4 received-state probes; captures require independent visual review."""
from __future__ import annotations

from typing import Any

from .native_p1 import _aim, _capture, _inventory, _rows
from .native_v2 import _block, _click, _echo, _open, _point, _set, _use
from .probe_steps import ProbeSteps, container_slot, item_count

RESTART_SCENARIOS = frozenset({"native-v2-p4-nameplates", "native-v2-p4-advancements",
                               "native-v2-p4-statistics"})


def _restart(steps: ProbeSteps) -> None:
    identity = steps.observe()["player"]["entity_uuid"]
    steps.fixture("save-all")
    _echo(steps, "save_command_ordered_receipt")
    steps.call("minecraft_disconnect")
    steps.check("restart_client_left_play", steps.call("minecraft_observe")["in_play"] is False)
    address = steps.client.p4_restart_server(steps.remaining())
    steps.report.setdefault("server_restarts", []).append(address)
    steps.call("minecraft_connect", {"server_addr": address})
    play = steps.call("minecraft_wait_for_play", {"timeout_seconds": steps.remaining()})
    steps.check("restart_client_entered_play", play["in_play"] is True)
    state = steps.wait(lambda state: state["player"]["on_ground"], "restart_player_grounded")
    steps.check("restart_same_player_uuid", state["player"]["entity_uuid"] == identity)
    _echo(steps, "restart_ordered_server_receipt")


def _entity_at(steps: ProbeSteps, kind: str, x: float, z: float) -> dict[str, Any]:
    found: dict[str, Any] = {}

    def received(_: dict[str, Any]) -> bool:
        row = next((row for row in _rows(steps, 16) if row["entity_type"] == kind
                    and abs(row["x"] - x) < .6 and abs(row["z"] - z) < .6), None)
        if row is None:
            return False
        found.update(row)
        return True

    steps.wait(received, f"received_{kind}_{x}_{z}")
    return found


def _target_capture(steps: ProbeSteps, row: dict[str, Any], name: str) -> None:
    _aim(steps, row)
    state = steps.wait(lambda state: state.get("target", {}).get("entity_uuid") == row["entity_uuid"],
                       name + "_actual_crosshair_target")
    steps.report.setdefault("targets", {})[name] = state["target"]
    _capture(steps, name)


def _nameplates(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    x, y, z = origin
    fixtures = (("armor_stand", x + 2.5, z + 2.5, '{CustomName:"QA Stand",CustomNameVisible:1b}'),
                ("armor_stand", x - 1.5, z + 2.5, '{CustomName:"Hidden Stand"}'),
                ("pig", x + .5, z + 2.5, '{CustomName:"QA Pig",NoAI:1b}'))
    entities = []
    for kind, px, pz, tags in fixtures:
        steps.fixture(f"summon {kind} {px} {y} {pz} {tags}")
        _echo(steps, f"{kind}_{px}_summon_ordered_receipt")
        entities.append(_entity_at(steps, "minecraft:" + kind, px, pz))
    steps.report["fixtures"] = entities
    for row, name in zip(entities, ("p4-stand-visible", "p4-stand-hidden", "p4-pig-targeted")):
        _target_capture(steps, row, name)
    steps.call("minecraft_look", {"yaw_deg": 18, "pitch_deg": 0})
    state = steps.observe()
    steps.check("pig_untargeted_crosshair", state.get("target", {}).get("entity_uuid") != entities[2]["entity_uuid"])
    _capture(steps, "p4-pig-not-targeted")
    _restart(steps)
    restored = _entity_at(steps, "minecraft:armor_stand", entities[0]["x"], entities[0]["z"])
    steps.check("visible_stand_uuid_survives_server_restart", restored["entity_uuid"] == entities[0]["entity_uuid"])
    _target_capture(steps, restored, "p4-stand-visible-after-restart")
    hidden = _entity_at(steps, "minecraft:armor_stand", entities[1]["x"], entities[1]["z"])
    steps.check("hidden_stand_uuid_survives_server_restart", hidden["entity_uuid"] == entities[1]["entity_uuid"])
    _target_capture(steps, hidden, "p4-stand-hidden-after-restart")
    steps.report["visual_review_required"] = ["QA Stand visible before/after restart", "Hidden Stand absent even when targeted", "QA Pig visible only when targeted"]


def _button(steps: ProbeSteps, label: str) -> None:
    screen = steps.observe()["screen"]
    steps.call("minecraft_click_screen_button", {
        "expected_screen_class": screen["class"], "expected_title": screen["title"], "button_label": label,
    })


def _advancement_labels(state: dict[str, Any]) -> dict[str, str]:
    return {node["id"]: node["label"] for node in state.get("ui_nodes", [])
            if node.get("id", "").startswith("advancement/")}


def _scroll(steps: ProbeSteps, count: int) -> None:
    """Real wheel events on this runner's focused, owned Xvfb window."""
    import ctypes as c
    import ctypes.util

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
        raise RuntimeError("cannot open this P4 runner's Xvfb display for ordinary scrolling")
    try:
        x.XWarpPointer(display, 0, owned["window_id"], 0, 0, 0, 0, 80, 100)
        for _ in range(abs(count)):
            button = 5 if count > 0 else 4
            if not xt.XTestFakeButtonEvent(display, button, 1, 0) or not xt.XTestFakeButtonEvent(display, button, 0, 0):
                raise RuntimeError("owned Xvfb display refused ordinary wheel input")
        x.XSync(display, 0)
    finally:
        x.XCloseDisplay(display)


def _advancement_screen(steps: ProbeSteps, name: str) -> dict[str, Any]:
    steps.call("minecraft_open_pause_menu")
    _button(steps, "Advancements")
    state = steps.wait(lambda state: state["screen"]["class"].endswith("AdvancementsScreen"), name + "_screen")
    _button(steps, "Minecraft")
    state = steps.observe()
    labels = _advancement_labels(state)
    required = {"advancement/minecraft:story/root", "advancement/minecraft:story/mine_stone",
                "advancement/minecraft:story/upgrade_tools"}
    _capture(steps, name)
    for page in range(24):
        if required <= labels.keys():
            break
        previous = _advancement_labels(state)
        _scroll(steps, 12)
        state = steps.wait(lambda current: _advancement_labels(current) != previous, name + f"_scroll_{page}")
        current = _advancement_labels(state)
        labels.update(current)
        if required.intersection(current):
            _capture(steps, name + f"-story-{page}")
    steps.check(name + "_story_nodes_accessible", required <= labels.keys())
    steps.report.setdefault("advancement_screens", {})[name] = labels
    _button(steps, "Done")
    steps.call("minecraft_close_screen")
    return state


def _place_ingredients(steps: ProbeSteps, item: str, targets: tuple[int, ...]) -> None:
    state = steps.observe()
    sources = [row for row in state["container"]["slots"]
               if row["slot"] >= 10 and row["item_id"] == item and row["count"] == len(targets)]
    steps.check(item + "_unique_crafting_source", len(sources) == 1)
    _click(steps, sources[0]["slot"])
    for target in targets:
        steps.call("minecraft_click_container_slot", {
            "slot": target, "button": "secondary", "timeout_seconds": steps.remaining(),
        })
    steps.check(item + "_crafting_cursor_empty", steps.observe()["container"]["carried"]["count"] == 0)


def _advancements(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    steps.report["recipes_before"] = steps.call("minecraft_read_recipe_book", {"limit": 8192})
    _advancement_screen(steps, "p4-advancements-fresh")
    steps.fixture("give @s minecraft:oak_log 1")
    _inventory(steps, "minecraft:oak_log", 1)
    steps.fixture("give @s minecraft:crafting_table 1")
    _inventory(steps, "minecraft:crafting_table", 1)
    _echo(steps, "wood_and_table_received")
    _advancement_screen(steps, "p4-advancements-root")
    steps.fixture("give @s minecraft:cobblestone 3")
    _inventory(steps, "minecraft:cobblestone", 3)
    steps.fixture("give @s minecraft:stick 2")
    _inventory(steps, "minecraft:stick", 2)
    table = _point(origin)
    steps.call("minecraft_select_hotbar_item", {"item_id": "minecraft:crafting_table", "count": 1,
                                               "timeout_seconds": steps.remaining()})
    _use(steps, _point(origin, dy=-1))
    _block(steps, table, "minecraft:crafting_table")
    _open(steps, table, "CraftingMenu", 46)
    _place_ingredients(steps, "minecraft:cobblestone", (1, 2, 3))
    _place_ingredients(steps, "minecraft:stick", (5, 8))
    result = steps.wait(lambda state: container_slot(state, 0)["item_id"] == "minecraft:stone_pickaxe",
                        "earned_stone_pickaxe_result")
    steps.report["crafting_result"] = container_slot(result, 0)
    _capture(steps, "p4-stone-pickaxe-crafting-result")
    _click(steps, 0, quick=True)
    steps.call("minecraft_close_screen")
    _inventory(steps, "minecraft:stone_pickaxe", 1)
    earned = _echo(steps, "stone_pickaxe_crafted_ordered_receipt")
    steps.report["completion_chat"] = earned["recent_chat"]
    steps.report["recipes_after"] = steps.call("minecraft_read_recipe_book", {"limit": 8192})
    _advancement_screen(steps, "p4-advancements-earned")
    _restart(steps)
    _inventory(steps, "minecraft:stone_pickaxe", 1)
    _advancement_screen(steps, "p4-advancements-after-restart")
    steps.report["recipes_after_restart"] = steps.call("minecraft_read_recipe_book", {"limit": 8192})
    # Reopen without awarding anything: progress/UI must not clear or announce again.
    _advancement_screen(steps, "p4-advancements-reopened")
    steps.report["visual_review_required"] = ["Minecraft story root", "Stone Age and Getting an Upgrade completion", "same completions after server restart", "completion chat; no join toast flood"]


def _damage(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    x, y, z = origin
    steps.fixture("time set 18000")
    _echo(steps, "damage_night_fixture_ordered_receipt")
    steps.call("minecraft_wait_ticks", {"ticks": 20})
    steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 0})
    baseline = steps.observe()["player"]["health"]
    steps.report["baseline_health"] = baseline
    _capture(steps, "p4-damage-before")
    steps.fixture(f"summon zombie {x + .5} {y} {z + 2.5}")
    hit = steps.call("minecraft_wait_for_health_below", {"health": baseline, "timeout_seconds": steps.remaining()})
    steps.report["hit"] = hit
    hurt = steps.wait(lambda current: current["player"].get("hurt_tick") is not None
                      and abs(current["player"].get("hurt_tilt_deg", 0)) > .05,
                      "received_damage_event_nonzero_camera_tilt")
    steps.report["hurt_observation"] = {key: hurt["player"].get(key) for key in
                                       ("hurt_tick", "hurt_tilt_deg", "hurt_direction", "health")}
    steps.report["hurt_observation"]["client_tick"] = hurt["client_tick"]
    _capture(steps, "p4-damage-hit")
    # One bounded adjacent check: live owner still accepts input and receives state.
    steps.call("minecraft_press_inputs", {"keys": ["back"], "ticks": 4})
    state = steps.observe()
    steps.check("post_hit_still_in_play", state["in_play"] is True and state["player"]["health"] > 0)
    _capture(steps, "p4-damage-after")
    steps.report["visual_review_required"] = ["hit frame hurt direction/tilt versus before", "no client errors or runtime disconnect"]


def _hopper(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    hopper = _point(origin, dz=1)
    _set(steps, hopper, "minecraft:hopper[facing=down]")
    steps.fixture("give @s minecraft:diamond 1")
    _inventory(steps, "minecraft:diamond", 1)
    steps.call("minecraft_select_hotbar_item", {"item_id": "minecraft:diamond", "count": 1,
                                               "timeout_seconds": steps.remaining()})
    steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 0})
    steps.call("minecraft_wait_ticks", {"ticks": 4})
    _echo(steps, "hopper_throw_rotation_ordered_receipt")
    _capture(steps, "p4-hopper-before-q")
    before_q = steps.observe()
    steps.report["throw_tick_before"] = {"client_tick": before_q["client_tick"], "time": before_q["time"]}
    steps.call("minecraft_press_inputs", {"keys": ["key.keyboard.q"], "ticks": 1})
    _inventory(steps, "minecraft:diamond", 0)
    after_q = steps.observe()
    steps.report["throw_tick_after"] = {"client_tick": after_q["client_tick"], "time": after_q["time"]}
    steps.report["drop_trace"] = []
    elapsed = 0
    for ticks in (1, 2, 4, 8, 16, 32):
        steps.call("minecraft_wait_ticks", {"ticks": ticks})
        elapsed += ticks
        state = steps.observe()
        steps.report["drop_trace"].append({
            "wait_ticks_sum": elapsed,
            "client_tick": state["client_tick"],
            "time": state["time"],
            "diamond_inventory": item_count(state, "minecraft:diamond"),
            "items": [row for row in _rows(steps, 8) if row["entity_type"] == "minecraft:item"],
        })
        if elapsed in (3, 15):
            _capture(steps, f"p4-hopper-dropped-item-{elapsed}")
    opened = _open(steps, hopper, "HopperMenu", 41)
    received = steps.wait(lambda state: sum(row["count"] for row in state["container"]["slots"]
                                           if row["slot"] < 5 and row["item_id"] == "minecraft:diamond") == 1,
                          "hopper_received_exact_dropped_diamond")
    steps.check("hopper_no_player_repickup", item_count(received, "minecraft:diamond") == 0)
    steps.report["hopper_slots"] = [row for row in received["container"]["slots"] if row["slot"] < 5]
    _capture(steps, "p4-hopper-item-in-menu")
    slot = next(row["slot"] for row in received["container"]["slots"] if row["slot"] < 5 and row["item_id"] == "minecraft:diamond")
    _click(steps, slot, quick=True)
    steps.call("minecraft_close_screen")
    _inventory(steps, "minecraft:diamond", 1)
    steps.call("minecraft_wait_ticks", {"ticks": 20})
    reopened = _open(steps, hopper, "HopperMenu", 41)
    steps.check("adversarial_hopper_withdraw_no_duplicate", all(row["count"] == 0 for row in reopened["container"]["slots"] if row["slot"] < 5))
    _capture(steps, "p4-hopper-empty-after-withdraw")
    steps.call("minecraft_close_screen")
    steps.check("adversarial_exact_diamond_custody", item_count(steps.observe(), "minecraft:diamond") == 1)


def _statistics_request(steps: ProbeSteps, name: str) -> dict[tuple[str, str], int]:
    steps.call("minecraft_open_pause_menu")
    _button(steps, "Statistics")
    state = steps.wait(lambda current: current.get("statistics", {}).get("received") is True,
                       name + "_authoritative_stats_received")
    steps.check(name + "_still_play", state["in_play"] is True)
    steps.report.setdefault("statistics_receipts", {})[name] = state["statistics"]
    _capture(steps, name + "-general")
    for label in ("Items", "Mobs"):
        _button(steps, label)
        _capture(steps, name + "-" + label.lower())
    _button(steps, "Done")
    steps.call("minecraft_close_screen")
    return {(row["stat_type"], row["stat"]): row["value"] for row in state["statistics"]["counters"]}


def _stats(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 0})
    steps.call("minecraft_press_inputs", {"keys": ["forward"], "ticks": 8})
    steps.call("minecraft_press_inputs", {"keys": ["back"], "ticks": 8})
    steps.call("minecraft_press_inputs", {"keys": ["jump"], "ticks": 1})
    steps.wait(lambda state: state["player"]["on_ground"], "statistics_jump_landed")
    steps.fixture("give @s minecraft:iron_pickaxe 1")
    _inventory(steps, "minecraft:iron_pickaxe", 1)
    steps.call("minecraft_select_hotbar_item", {"item_id": "minecraft:iron_pickaxe", "count": 1,
                                               "timeout_seconds": steps.remaining()})
    for dx, block, drop in ((1, "stone", "cobblestone"), (-1, "dirt", "dirt")):
        point = _point(origin, dx=dx)
        _set(steps, point, "minecraft:" + block)
        result = steps.call("minecraft_break_block", {
            **point, "face": "up", "expected_drop_item_id": "minecraft:" + drop,
            "expected_drop_count": 1, "timeout_seconds": steps.remaining(),
        })
        steps.check("statistics_" + block + "_mined_and_picked_up",
                    result.get("became_air") is True and result.get("pickup_confirmed") is True)
        steps.report.setdefault("statistics_mining", {})[block] = result
    before = _statistics_request(steps, "p4-statistics-earned")
    walk = before.get(("minecraft:custom", "minecraft:walk_one_cm"), 0)
    jump = before.get(("minecraft:custom", "minecraft:jump"), 0)
    steps.check("statistics_walk_plausible_nonzero", 0 < walk < 2000)
    steps.check("statistics_jump_plausible_nonzero", 1 <= jump <= 3)
    exercised = (("minecraft:mined", "minecraft:stone"), ("minecraft:mined", "minecraft:dirt"),
                 ("minecraft:picked_up", "minecraft:cobblestone"), ("minecraft:picked_up", "minecraft:dirt"))
    for stat in exercised:
        steps.check("statistics_exact_" + "/".join(stat), before.get(stat) == 1)
    _restart(steps)
    after = _statistics_request(steps, "p4-statistics-after-restart")
    for stat in exercised + (("minecraft:custom", "minecraft:jump"),):
        steps.check("statistics_persisted_" + "/".join(stat), after.get(stat) == before[stat])
    steps.check("statistics_walk_persisted", after.get(("minecraft:custom", "minecraft:walk_one_cm"), 0) >= walk)
    # A second ordinary request must replace absolute counters, not accumulate them.
    repeated = _statistics_request(steps, "p4-statistics-repeated-request")
    steps.check("adversarial_statistics_repeated_request_no_double_count",
                all(repeated.get(stat) == after[stat] for stat in exercised))


def run(steps: ProbeSteps, origin: tuple[int, int, int], surface: str) -> None:
    steps.fixture("gamemode survival @s")
    _echo(steps, "p4_survival_fixture_receipt")
    steps.report["surface"] = surface
    try:
        {"nameplates": _nameplates, "advancements": _advancements, "damage": _damage,
         "hopper": _hopper, "statistics": _stats}[surface](steps, origin)
    except Exception:
        try:
            steps.report["failure_observation"] = steps.call("minecraft_observe")
            _capture(steps, "p4-" + surface + "-failure")
        except Exception as error:
            steps.report["failure_capture_error"] = str(error)
        raise
