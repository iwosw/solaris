"""Bounded native v2 gameplay probes through the ordinary MCP action owner.

Operator setup is explicit; predicates use received blocks, menus and inventory,
never a local dispatch acknowledgement. Windowed runs retain the shared capture.
"""
from __future__ import annotations

import math
import uuid
from pathlib import Path
from typing import Any

from .. import scenarios as scenario_catalog
from .gameplay import require_tools
from .probe_steps import ProbeSteps, container_slot, item_count

NATIVE_SCENARIO_IDS = frozenset(
    entry.scenario_id for entry in scenario_catalog.SCENARIOS.values()
    if "tools/harness/backends/native_v2.py" in entry.sources and entry.scenario_id is not None
)
REQUIRED_TOOLS = frozenset({
    "minecraft_observe", "minecraft_wait_for_state_change", "minecraft_send_chat", "minecraft_wait_for_play",
    "minecraft_read_block", "minecraft_wait_for_block_state", "minecraft_use_item_on",
    "minecraft_set_hotbar_slot", "minecraft_wait_for_inventory", "minecraft_wait_ticks",
    "minecraft_look", "minecraft_press_inputs", "minecraft_close_screen",
    "minecraft_click_container_slot", "minecraft_quick_move_container_slot",
    "minecraft_read_item_components", "minecraft_wait_for_visible_entity",
    "minecraft_wait_for_entity_removed", "minecraft_respawn", "minecraft_list_entities",
})


def _point(origin: tuple[int, int, int], dx: int = 0, dy: int = 0, dz: int = 2) -> dict[str, int]:
    return dict(zip(("x", "y", "z"), (origin[0] + dx, origin[1] + dy, origin[2] + dz)))


def _block(steps: ProbeSteps, point: dict[str, int], block: str,
           properties: dict[str, str] | None = None) -> dict[str, Any]:
    return steps.call("minecraft_wait_for_block_state", {
        **point, "block_id": block, "properties": properties or {}, "timeout_seconds": steps.remaining(),
    })


def _set(steps: ProbeSteps, point: dict[str, int], state: str) -> None:
    steps.fixture(f"setblock {point['x']} {point['y']} {point['z']} {state}")
    _block(steps, point, state.split("[", 1)[0])


def _echo(steps: ProbeSteps, label: str) -> dict[str, Any]:
    marker = f"native v2 {uuid.uuid4().hex}"
    steps.call("minecraft_send_chat", {"message": marker, "command": False})
    return steps.wait(lambda state: marker in "\n".join(state["recent_chat"]), label)


def _standard_inventory_empty(state: dict[str, Any]) -> bool:
    """Known empty slots are omitted; unknown ordinary slots must still fail."""
    # Native slots 41/42 are auxiliary body/saddle, outside standard 0..40.
    return all(stack.get("count") == 0 for stack in state["inventory"]
               if stack.get("slot") not in (41, 42))


def _prepare(steps: ProbeSteps) -> tuple[int, int, int]:
    before = steps.wait(lambda state: state["player"]["on_ground"], "initial_grounded")
    origin = tuple(math.floor(before["player"][key]) for key in ("x", "y", "z"))
    x, y, z = origin
    steps.fixture("clear @s")
    cleared = steps.wait(_standard_inventory_empty, "fixture_standard_inventory_empty")
    steps.report["fixture_inventory_scope"] = "hotbar/main/armor/offhand (native slots 0..40)"
    steps.report["fixture_auxiliary_equipment"] = [
        stack for stack in cleared["inventory"] if stack.get("slot") in (41, 42)
    ]
    steps.fixture(f"fill {x - 5} {y - 1} {z - 12} {x + 5} {y - 1} {z + 6} minecraft:obsidian")
    steps.fixture(f"fill {x - 5} {y} {z - 12} {x + 5} {y + 5} {z + 6} minecraft:air")
    _block(steps, _point(origin, dy=-1), "minecraft:obsidian")
    _block(steps, _point(origin), "minecraft:air")
    steps.call("minecraft_set_hotbar_slot", {"slot": 8})
    steps.report.update(origin=origin, player_uuid=before["player"]["entity_uuid"],
                        dimension=before["dimension"],
                        setup="declared clear/fill/setblock/give fixtures, not earned survival")
    return origin


def _give(steps: ProbeSteps, item: str, slot: int = 0) -> None:
    steps.fixture(f"debug give {item} 1 {slot}")
    steps.call("minecraft_wait_for_inventory", {
        "item_id": item, "count": 1, "timeout_seconds": steps.remaining(),
    })


def _use(steps: ProbeSteps, point: dict[str, int], face: str = "up") -> None:
    steps.call("minecraft_use_item_on", {**point, "face": face, "hand": "main_hand"})


def _open(steps: ProbeSteps, point: dict[str, int], menu: str, slots: int) -> dict[str, Any]:
    steps.call("minecraft_set_hotbar_slot", {"slot": 8})
    _use(steps, point)
    state = steps.wait(lambda state: state["container"]["container_id"] != 0
                       and state["container"]["menu_class"] == f"net.minecraft.world.inventory.{menu}",
                       f"{menu}_received")
    steps.check(f"{menu}_slot_count", state["container"]["slot_count"] == slots)
    return state


def _click(steps: ProbeSteps, slot: int, quick: bool = False) -> dict[str, Any]:
    args: dict[str, Any] = {"slot": slot, "timeout_seconds": steps.remaining()}
    if not quick:
        args["button"] = "primary"
    return steps.call("minecraft_quick_move_container_slot" if quick else "minecraft_click_container_slot", args)


def _insert(steps: ProbeSteps, item: str, target: int) -> None:
    state = steps.observe()
    sources = [stack for stack in state["container"]["slots"]
               if stack["item_id"] == item and stack["count"] == 1 and stack["slot"] != target]
    steps.check(f"{item}_unique_source", len(sources) == 1)
    _click(steps, sources[0]["slot"])
    _click(steps, target)
    state = steps.observe()
    placed = container_slot(state, target)
    steps.check(f"{item}_inserted_{target}", placed["item_id"] == item and placed["count"] == 1
                and state["container"]["carried"]["count"] == 0 and item_count(state, item) == 0)


def _redstone(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    fixtures = ((-2, "minecraft:lever[face=floor,facing=east]"),
                (-1, "minecraft:redstone_wire"), (0, "minecraft:repeater[facing=west,delay=1]"),
                (1, "minecraft:redstone_wire"), (2, "minecraft:redstone_lamp"))
    for dx, state in fixtures:
        _set(steps, _point(origin, dx=dx, dz=1), state)
    for powered in (False, True, False):
        if powered or steps.report["assertions"].get("circuit_off_received"):
            _use(steps, _point(origin, dx=-2, dz=1))
        value = "true" if powered else "false"
        for dx, block, props in ((-2, "lever", {"powered": value}),
                                 (-1, "redstone_wire", {"power": "15" if powered else "0"}),
                                 (0, "repeater", {"powered": value, "facing": "west"}),
                                 (1, "redstone_wire", {"power": "15" if powered else "0"}),
                                 (2, "redstone_lamp", {"lit": value})):
            _block(steps, _point(origin, dx=dx, dz=1), f"minecraft:{block}", props)
        steps.check(f"circuit_{'on' if powered else 'off'}_received", True)
    # Adversarial reversibility: another complete cycle cannot stick powered.
    _use(steps, _point(origin, dx=-2, dz=1))
    _block(steps, _point(origin, dx=2, dz=1), "minecraft:redstone_lamp", {"lit": "true"})
    _use(steps, _point(origin, dx=-2, dz=1))
    _block(steps, _point(origin, dx=2, dz=1), "minecraft:redstone_lamp", {"lit": "false"})
    steps.check("adversarial_second_cycle_reversible", True)


def _stations(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    point = _point(origin)
    # Menu admission is distinct from completing each station's operation.
    for block, menu, count in (("crafting_table", "CraftingMenu", 46),
                               ("anvil", "AnvilMenu", 39),
                               ("enchanting_table", "EnchantmentMenu", 38),
                               ("crafter", "CrafterMenu", 46)):
        _set(steps, point, f"minecraft:{block}")
        _open(steps, point, menu, count)
        steps.call("minecraft_close_screen")
    _set(steps, point, "minecraft:smithing_table")
    inputs = ("minecraft:netherite_upgrade_smithing_template", "minecraft:diamond_sword",
              "minecraft:netherite_ingot")
    for slot, item in enumerate(inputs):
        _give(steps, item, slot)
    _open(steps, point, "SmithingMenu", 40)
    for slot, item in enumerate(inputs):
        _insert(steps, item, slot)
    preview = steps.wait(lambda state: container_slot(state, 3)["item_id"] == "minecraft:netherite_sword"
                         and container_slot(state, 3)["count"] == 1, "smithing_exact_preview")
    steps.check("smithing_no_early_credit", item_count(preview, "minecraft:netherite_sword") == 0)
    _click(steps, 3, quick=True)
    state = steps.wait(lambda state: item_count(state, "minecraft:netherite_sword") == 1,
                       "smithing_result_received")
    steps.check("smithing_inputs_consumed_once", all(item_count(state, item) == 0 for item in inputs)
                and all(container_slot(state, slot)["count"] == 0 for slot in range(4))
                and state["container"]["carried"]["count"] == 0)
    steps.call("minecraft_close_screen")
    for block in ("chest", "copper_chest"):
        _set(steps, point, f"minecraft:{block}")
        _open(steps, point, "ChestMenu", 63)
        _insert(steps, "minecraft:netherite_sword", 0)
        steps.call("minecraft_close_screen")
        _open(steps, point, "ChestMenu", 63)
        state = steps.observe()
        steps.check(f"{block}_reopen_preserved_one", container_slot(state, 0)["count"] == 1
                    and container_slot(state, 0)["item_id"] == "minecraft:netherite_sword"
                    and item_count(state, "minecraft:netherite_sword") == 0)
        _click(steps, 0, quick=True)
        state = steps.wait(lambda state: item_count(state, "minecraft:netherite_sword") == 1,
                           f"{block}_returned_one")
        steps.check(f"{block}_no_duplicate_or_cursor", container_slot(state, 0)["count"] == 0
                    and state["container"]["carried"]["count"] == 0)
        steps.call("minecraft_close_screen")
    steps.report["limits"] = ["anvil/enchanting/crafter menu admission only; not their recipe/button behavior"]


def _brewing(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    point = _point(origin)
    _set(steps, point, "minecraft:brewing_stand")
    steps.fixture('give @s minecraft:potion[potion_contents={potion:"minecraft:water"}] 1')
    steps.call("minecraft_wait_for_inventory", {"item_id": "minecraft:potion", "count": 1,
                                                "timeout_seconds": steps.remaining()})
    _give(steps, "minecraft:nether_wart", 1)
    _give(steps, "minecraft:blaze_powder", 2)
    _open(steps, point, "BrewingStandMenu", 41)
    _insert(steps, "minecraft:potion", 0)
    before = steps.call("minecraft_read_item_components", {"source": "container", "slot": 0})
    steps.report["water_components"] = before
    steps.check("water_input_component", before["components"]["minecraft:potion_contents"]["potion"]
                == "minecraft:water")
    _insert(steps, "minecraft:nether_wart", 3)
    state = steps.observe()
    source = next(stack["slot"] for stack in state["container"]["slots"]
                  if stack["item_id"] == "minecraft:blaze_powder" and stack["count"] == 1)
    _click(steps, source, quick=True)
    steps.wait(lambda state: container_slot(state, 3)["count"] == 0
               and item_count(state, "minecraft:blaze_powder") == 0, "brewing_ingredient_consumed")
    after = steps.call("minecraft_read_item_components", {"source": "container", "slot": 0})
    steps.report["brewed_components"] = after
    steps.check("awkward_result_component", after["item_id"] == "minecraft:potion"
                and after["count"] == 1
                and after["components"]["minecraft:potion_contents"]["potion"] == "minecraft:awkward")
    _click(steps, 0, quick=True)
    final = steps.wait(lambda state: item_count(state, "minecraft:potion") == 1, "brewed_bottle_returned")
    steps.check("brew_conserved", all(container_slot(final, slot)["count"] == 0 for slot in range(5))
                and item_count(final, "minecraft:nether_wart") == 0
                and item_count(final, "minecraft:blaze_powder") == 0
                and final["container"]["carried"]["count"] == 0)
    steps.call("minecraft_close_screen")
    steps.report["limits"] = ["received potion components/conservation; no brewing progress/fuel HUD assertion"]


def _jukebox(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    point = _point(origin)
    baseline = steps.call("minecraft_list_entities", {"radius": 8, "limit": 128})
    steps.check("no_preexisting_ejected_item", not any(row["entity_type"] == "minecraft:item"
                                                     for row in baseline["entities"]))
    _set(steps, point, "minecraft:jukebox")
    _block(steps, point, "minecraft:jukebox", {"has_record": "false"})
    _give(steps, "minecraft:music_disc_13")
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    _use(steps, point)
    _block(steps, point, "minecraft:jukebox", {"has_record": "true"})
    inserted = steps.wait(lambda state: item_count(state, "minecraft:music_disc_13") == 0,
                          "record_debited_once")
    steps.call("minecraft_set_hotbar_slot", {"slot": 8})
    _use(steps, point)
    _block(steps, point, "minecraft:jukebox", {"has_record": "false"})
    # Ejection's actual item is server state, not inferred from a music event.
    item = steps.call("minecraft_wait_for_visible_entity", {
        "entity_type": "minecraft:item", "radius": 8, "timeout_seconds": steps.remaining(),
    })
    def exact_ejected(_: dict[str, Any]) -> bool:
        rows = steps.call("minecraft_list_entities", {"radius": 8, "limit": 128})["entities"]
        return any(row["entity_uuid"] == item["entity_uuid"]
                   and row.get("item") == {"item_id": "minecraft:music_disc_13", "count": 1}
                   for row in rows)
    steps.wait(exact_ejected, "uuid_fenced_exact_disc_ejected")
    steps.report.update(record_debit_inventory=inserted["inventory"], ejected_item=item,
                        limits=["record insertion/debit/ejection; no audible playback claim"])
    _echo(steps, "post_ejection_server_receipt")
    steps.check("record_not_duplicated_in_inventory", item_count(steps.observe(), "minecraft:music_disc_13") <= 1)


def _tnt_fire_light(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    point = _point(origin)
    sample = _point(origin, dx=1)
    steps.call("minecraft_wait_ticks", {"ticks": 10})
    before = steps.call("minecraft_read_block", sample)
    steps.report["light_before"] = before
    _give(steps, "minecraft:torch")
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    _use(steps, _point(origin, dy=-1))
    _block(steps, point, "minecraft:torch")
    # Light publications wake the same state-event producer as block updates.
    state = steps.observe()
    while True:
        after = steps.call("minecraft_read_block", sample)
        if after["block_light"] is not None and before["block_light"] is not None \
                and after["block_light"] > before["block_light"]:
            break
        steps.call("minecraft_wait_for_state_change", {"observed_version": state["state_version"],
                                                        "timeout_seconds": steps.remaining()})
        state = steps.observe()
    steps.report["light_after"] = after
    steps.check("received_block_light_increased", True)
    steps.wait(lambda state: item_count(state, "minecraft:torch") == 0, "torch_consumed_once")
    _set(steps, point, "minecraft:netherrack")
    _give(steps, "minecraft:flint_and_steel")
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    _use(steps, point)
    _block(steps, _point(origin, dy=1), "minecraft:fire")
    steps.check("ordinary_ignition_received", True)
    _set(steps, _point(origin, dy=1), "minecraft:air")
    _set(steps, point, "minecraft:tnt")
    victim = _point(origin, dx=1)
    _set(steps, victim, "minecraft:dirt")
    _use(steps, point, "north")
    target = steps.call("minecraft_wait_for_visible_entity", {
        "entity_type": "minecraft:tnt", "radius": 8, "timeout_seconds": steps.remaining(),
    })
    identity = {key: target[key] for key in ("entity_id", "entity_uuid", "entity_type")}
    steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 0})
    steps.call("minecraft_press_inputs", {"keys": ["back", "sprint"], "ticks": 40})
    _block(steps, point, "minecraft:air")
    steps.call("minecraft_wait_for_entity_removed", {**identity, "timeout_seconds": steps.remaining()})
    _block(steps, victim, "minecraft:air")
    final = steps.observe()
    steps.check("tnt_blast_live_player", final["player"]["health"] is not None and final["player"]["health"] > 0)
    steps.report.update(primed_tnt=identity,
                        limits=["torch block-light, fire ignition, TNT blast; no fire-spread/extinguishing claim"])


def _dimensions(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    identity = steps.observe()["player"]["entity_uuid"]
    steps.fixture("gamemode creative")
    steps.report["travels"] = []
    for dimension in ("the_nether", "the_end"):
        steps.fixture(f"execute in {dimension} run tp @s 8 150 8")
        arrived = steps.wait(lambda state: state["dimension"] == f"minecraft:{dimension}",
                             f"{dimension}_travel_received", wait_for_play=True)
        steps.check(f"{dimension}_identity_preserved", arrived["player"]["entity_uuid"] == identity)
        _echo(steps, f"{dimension}_own_chat")
        steps.fixture("kill")
        steps.wait(lambda state: state["player"]["health"] == 0 and state["screen"]["open"],
                   f"{dimension}_death_received")
        steps.call("minecraft_respawn", {"timeout_seconds": steps.remaining()})
        alive = steps.wait(lambda state: state["dimension"] == "minecraft:overworld"
                           and state["player"]["health"] is not None and state["player"]["health"] > 0
                           and not state["screen"]["open"], f"{dimension}_overworld_respawn",
                           wait_for_play=True)
        steps.check(f"{dimension}_respawn_uuid_preserved", alive["player"]["entity_uuid"] == identity)
        steps.report["travels"].append({"dimension": dimension, "arrived": arrived["player"],
                                          "respawned": alive["player"]})
    steps.report["limits"] = ["explicit dimension command travel and default multi-dimension death/respawn; not portal entry or dragon fight"]


def _ranged(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    x, y, z = origin
    steps.fixture(f"fill {x - 5} {y - 1} {z + 7} {x + 5} {y - 1} {z + 14} minecraft:obsidian")
    steps.fixture(f"fill {x - 5} {y} {z + 7} {x + 5} {y + 5} {z + 14} minecraft:air")
    # Iron golems do not naturally despawn; only stationary AI is needed here.
    steps.fixture(f'summon minecraft:iron_golem {x + 0.5} {y} {z + 8.5} {{NoAI:1b}}')
    receipt = _echo(steps, "ranged_target_fixture_ordered_receipt")
    steps.report["target_fixture_chat"] = receipt["recent_chat"]
    target = steps.call("minecraft_wait_for_visible_entity", {
        "entity_type": "minecraft:iron_golem", "radius": 16, "timeout_seconds": steps.remaining(),
    })
    identity = {key: target[key] for key in ("entity_id", "entity_uuid", "entity_type")}
    steps.report["target"] = identity
    steps.report["shots"] = []
    for weapon, projectile in (("bow", "arrow"), ("crossbow", "arrow"), ("trident", "trident")):
        rows = steps.call("minecraft_list_entities", {"radius": 16, "limit": 128})["entities"]
        mob = next(row for row in rows if row["entity_uuid"] == identity["entity_uuid"])
        health = mob.get("health")
        steps.check(f"{weapon}_target_health_received", health is not None and health > 0)
        before_ids = {row["entity_uuid"] for row in rows if row["entity_type"] == f"minecraft:{projectile}"}
        player = steps.observe()["player"]
        box = mob["bounding_box"]
        dx, dz = mob["x"] - player["x"], mob["z"] - player["z"]
        dy = (box["min_y"] + box["max_y"]) / 2 - (player["y"] + 1.62)
        steps.call("minecraft_look", {
            "yaw_deg": round(math.degrees(math.atan2(-dx, dz))),
            "pitch_deg": round(-math.degrees(math.atan2(dy, math.hypot(dx, dz)))),
        })
        _give(steps, f"minecraft:{weapon}")
        if weapon != "trident":
            _give(steps, "minecraft:arrow", 1)
        steps.call("minecraft_set_hotbar_slot", {"slot": 0})
        steps.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 30})
        if weapon == "crossbow":
            _echo(steps, "crossbow_load_server_receipt")
            components = steps.call("minecraft_read_item_components", {"source": "selected"})
            steps.check("crossbow_server_loaded_one",
                        len(components["components"].get("minecraft:charged_projectiles", [])) == 1)
            steps.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 1})
        shot: dict[str, Any] = {}
        def new_projectile(_: dict[str, Any]) -> bool:
            for row in steps.call("minecraft_list_entities", {"radius": 24, "limit": 128})["entities"]:
                if row["entity_type"] == f"minecraft:{projectile}" and row["entity_uuid"] not in before_ids:
                    shot.update(row)
                    return True
            return False
        steps.wait(new_projectile, f"{weapon}_new_projectile_uuid")
        steps.call("minecraft_wait_for_inventory", {
            "item_id": "minecraft:trident" if weapon == "trident" else "minecraft:arrow",
            "count": 0, "timeout_seconds": steps.remaining(),
        })
        received_damage: dict[str, Any] = {}
        def damaged(_: dict[str, Any]) -> bool:
            for row in steps.call("minecraft_list_entities", {"radius": 24, "limit": 128})["entities"]:
                if all(row[key] == value for key, value in identity.items()) and row.get("health") is not None:
                    if 0 < row["health"] < health:
                        received_damage.update(row)
                        return True
            return False
        steps.wait(damaged, f"{weapon}_uuid_fenced_received_damage")
        steps.report["shots"].append({"weapon": weapon, "projectile": shot,
                                      "health_before": health, "health_after": received_damage["health"]})
    steps.report["limits"] = ["bow/crossbow/trident launch, exact ammunition debit and received target damage; no pickup/Loyalty/enchantment parity"]


def _automation_storage(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    point = _point(origin)
    lever = _point(origin, dx=1)
    _set(steps, point, "minecraft:crafter[orientation=north_up]")
    _set(steps, lever, "minecraft:lever[face=floor,facing=north]")
    _give(steps, "minecraft:oak_log")
    _open(steps, point, "CrafterMenu", 46)
    _insert(steps, "minecraft:oak_log", 0)
    steps.call("minecraft_close_screen")
    _use(steps, lever)
    steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 0})
    steps.call("minecraft_press_inputs", {"keys": ["forward"], "ticks": 4})
    state = steps.wait(lambda state: item_count(state, "minecraft:oak_planks") == 4,
                       "crafter_one_pulse_four_planks_received")
    steps.check("crafter_input_debited", item_count(state, "minecraft:oak_log") == 0)
    steps.call("minecraft_wait_ticks", {"ticks": 20})
    _open(steps, point, "CrafterMenu", 46)
    held = steps.observe()
    steps.check("crafter_held_power_no_duplicate", container_slot(held, 0)["count"] == 0
                and item_count(held, "minecraft:oak_planks") == 4
                and held["container"]["carried"]["count"] == 0)
    steps.call("minecraft_close_screen")
    _set(steps, lever, "minecraft:air")
    _set(steps, point, "minecraft:oak_shelf[facing=north]")
    _give(steps, "minecraft:diamond", 0)
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    _use(steps, point, "north")
    steps.wait(lambda state: item_count(state, "minecraft:diamond") == 0,
               "shelf_deposited_one_diamond")
    _give(steps, "minecraft:emerald", 0)
    _use(steps, point, "north")
    swapped = steps.wait(lambda state: item_count(state, "minecraft:diamond") == 1
                         and item_count(state, "minecraft:emerald") == 0,
                         "shelf_swap_returns_diamond_debits_emerald")
    steps.check("shelf_selected_returned_diamond", swapped["selected_item"]["item_id"] == "minecraft:diamond"
                and swapped["selected_item"]["count"] == 1)
    steps.call("minecraft_set_hotbar_slot", {"slot": 8})
    _use(steps, point, "north")
    final = steps.wait(lambda state: item_count(state, "minecraft:diamond") == 1
                       and item_count(state, "minecraft:emerald") == 1, "shelf_empty_hand_withdraw_exact_one")
    steps.check("shelf_selected_returned_emerald", final["selected_item"]["item_id"] == "minecraft:emerald"
                and final["selected_item"]["count"] == 1)
    steps.call("minecraft_set_hotbar_slot", {"slot": 7})
    _use(steps, point, "north")
    _echo(steps, "shelf_empty_use_ordered_receipt")
    final = steps.observe()
    steps.check("shelf_empty_no_duplicate", item_count(final, "minecraft:diamond") == 1
                and item_count(final, "minecraft:emerald") == 1 and final["selected_item"]["count"] == 0)
    _set(steps, point, "minecraft:lever[face=floor,facing=north]")
    sensor = _point(origin, dx=2)
    _set(steps, sensor, "minecraft:sculk_sensor")
    _block(steps, sensor, "minecraft:sculk_sensor", {"sculk_sensor_phase": "inactive", "power": "0"})
    _use(steps, point)
    active = _block(steps, sensor, "minecraft:sculk_sensor", {"sculk_sensor_phase": "active"})
    steps.report["sensor_active_wait"] = active
    read = steps.call("minecraft_read_block", sensor)
    steps.check("sensor_ordinary_use_vibration_power", int(read["properties"]["power"]) > 0)
    _block(steps, sensor, "minecraft:sculk_sensor", {"sculk_sensor_phase": "cooldown", "power": "0"})
    _block(steps, sensor, "minecraft:sculk_sensor", {"sculk_sensor_phase": "inactive", "power": "0"})
    steps.check("sensor_active_cooldown_inactive_received", True)
    steps.report["limits"] = ["one crafter pulse/no held-power duplicate; one unpowered shelf slot swap; one sensor use-vibration cycle; no powered shelf chains, calibrated filtering, shrieker/warden or visual model parity"]


def _storage_adversarial(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    """Close with a carried stack, then swap into an occupied chest slot."""
    point = _point(origin)
    _set(steps, point, "minecraft:chest")
    _give(steps, "minecraft:diamond", 0)
    _give(steps, "minecraft:emerald", 1)
    _open(steps, point, "ChestMenu", 63)
    _insert(steps, "minecraft:diamond", 0)
    _click(steps, 0)
    carried = steps.wait(lambda state: state["container"]["carried"]["item_id"] == "minecraft:diamond"
                         and state["container"]["carried"]["count"] == 1
                         and container_slot(state, 0)["count"] == 0,
                         "adversarial_carried_diamond_received")
    steps.check("adversarial_no_early_inventory_credit", item_count(carried, "minecraft:diamond") == 0)
    steps.call("minecraft_close_screen")
    steps.wait(lambda state: state["container"]["container_id"] == 0
               and item_count(state, "minecraft:diamond") == 1
               and item_count(state, "minecraft:emerald") == 1,
               "adversarial_close_returns_carried_exactly_once")
    reopened = _open(steps, point, "ChestMenu", 63)
    steps.check("adversarial_reopen_no_hidden_duplicate", container_slot(reopened, 0)["count"] == 0
                and reopened["container"]["carried"]["count"] == 0)
    _insert(steps, "minecraft:diamond", 0)
    sources = [stack for stack in steps.observe()["container"]["slots"]
               if stack["item_id"] == "minecraft:emerald" and stack["count"] == 1]
    steps.check("adversarial_unique_emerald_source", len(sources) == 1)
    _click(steps, sources[0]["slot"])
    _click(steps, 0)
    swapped = steps.wait(lambda state: container_slot(state, 0)["item_id"] == "minecraft:emerald"
                         and container_slot(state, 0)["count"] == 1
                         and state["container"]["carried"]["item_id"] == "minecraft:diamond"
                         and state["container"]["carried"]["count"] == 1,
                         "adversarial_occupied_slot_swap_received")
    steps.check("adversarial_swap_no_inventory_duplicate",
                item_count(swapped, "minecraft:diamond") == 0
                and item_count(swapped, "minecraft:emerald") == 0)
    steps.call("minecraft_close_screen")
    steps.wait(lambda state: item_count(state, "minecraft:diamond") == 1
               and item_count(state, "minecraft:emerald") == 0,
               "adversarial_swap_close_returns_only_carried")
    reopened = _open(steps, point, "ChestMenu", 63)
    steps.check("adversarial_swap_reopen_retains_exact_emerald",
                container_slot(reopened, 0)["item_id"] == "minecraft:emerald"
                and container_slot(reopened, 0)["count"] == 1
                and reopened["container"]["carried"]["count"] == 0)
    _click(steps, 0, quick=True)
    final = steps.wait(lambda state: item_count(state, "minecraft:diamond") == 1
                       and item_count(state, "minecraft:emerald") == 1
                       and container_slot(state, 0)["count"] == 0,
                       "adversarial_final_two_items_conserved")
    steps.check("adversarial_final_cursor_empty", final["container"]["carried"]["count"] == 0)
    steps.call("minecraft_close_screen")
    steps.report["limits"] = ["one occupied-slot swap and two close-with-cursor boundaries; no crash/reconnect or full-inventory overflow"]


def _p1(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    from . import native_p1

    native_p1.run(steps, origin, steps.report["id"].split("-", 3)[3])

def _p4(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    from . import native_p4

    native_p4.run(steps, origin, steps.report["id"].split("-", 3)[3])


def _p5(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    from . import native_p5

    native_p5.run(steps, origin)


def _light(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    from . import native_light

    native_light.run(steps, origin)


def _radius(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    from . import native_radius

    native_radius.run(steps, origin)

RUNNERS = {
    "native-v2-redstone": _redstone, "native-v2-stations": _stations,
    "native-v2-brewing": _brewing, "native-v2-jukebox": _jukebox,
    "native-v2-tnt-fire-light": _tnt_fire_light, "native-v2-dimensions": _dimensions,
    "native-v2-ranged": _ranged,
    "native-v2-automation-storage": _automation_storage,
    "native-v2-storage-adversarial": _storage_adversarial,
    "native-v2-p1-armor-stand": _p1,
    "native-v2-p1-signs": _p1,
    "native-v2-p1-piglin": _p1,
    "native-v2-p1-trader": _p1,
    "native-v2-p1-leaves": _p1,
    "native-v2-p1-helmet-equip": _p1,
    "native-v2-p1-player-observer": _p1,
    "native-v2-p2-slime": _p1,
    "native-v2-p2-enderman": _p1,
    "native-v2-p2-fishing": _p1,
    "native-v2-p2-creaking": _p1,
    "native-v2-p4-nameplates": _p4,
    "native-v2-p4-advancements": _p4,
    "native-v2-p4-damage": _p4,
    "native-v2-p4-hopper": _p4,
    "native-v2-p4-statistics": _p4,
    "native-v2-p5-humanoid-armor": _p5,
    "native-v2-light-seam": _light,
    "native-v2-radius-full-view": _radius,
}


def run(client: Any, artifact: Path, timeout: float, scenario: str) -> dict[str, Any]:
    if scenario not in RUNNERS:
        raise ValueError(f"unknown native v2 scenario: {scenario}")
    require_tools(client.list_tools(), REQUIRED_TOOLS, scenario)
    with ProbeSteps(client, artifact / scenario, timeout, scenario) as steps:
        origin = _prepare(steps)
        RUNNERS[scenario](steps, origin)
        final = _echo(steps, "final_ordered_server_chat_receipt")
        steps.check("player_identity_stable", final["player"]["entity_uuid"] == steps.report["player_uuid"])
        steps.report.update(evidence_domain="received_native_state; graphical only with --windowed capture",
                            final_inventory=final["inventory"], final_dimension=final["dimension"])
    return steps.report
