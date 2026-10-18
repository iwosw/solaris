"""One graphical observer plus one headless actor for worn/dyed armor QA."""
from __future__ import annotations

from .native_p1 import _aim, _capture, _identity, _interact, _inventory, _rows
from .native_p4 import _entity_at, _place_ingredients
from .native_v2 import _click, _echo, _give, _open, _point, _set, _use
from .probe_steps import ProbeSteps, container_slot


def _clear(steps: ProbeSteps) -> None:
    steps.fixture("clear @s")
    steps.wait(lambda state: all(row.get("count") == 0 for row in state["inventory"]
               if 0 <= row["slot"] <= 40), "armor_fixture_inventory_cleared")


def _select(steps: ProbeSteps, item: str) -> None:
    steps.call("minecraft_select_hotbar_item", {"item_id": item, "count": 1,
               "timeout_seconds": steps.remaining()})


def _component_receipt(steps: ProbeSteps, label: str, expected: int) -> None:
    receipt = steps.call("minecraft_read_item_components", {"source": "selected"})
    steps.report.setdefault("dyed_components", {})[label] = receipt
    steps.check(label + "_received_color", receipt["components"].get("minecraft:dyed_color") == expected)


def _craft_dyed_chestplate(steps: ProbeSteps, origin: tuple[int, int, int], *, graphical: bool) -> None:
    """Use ordinary table clicks, not a pre-dyed give, for the second color."""
    table = _point(origin, dx=2, dz=-2)
    _set(steps, table, "minecraft:crafting_table")
    for item in ("minecraft:leather_chestplate", "minecraft:cyan_dye"):
        steps.fixture(f"give @s {item} 1")
        _inventory(steps, item, 1)
    _open(steps, table, "CraftingMenu", 46)
    _place_ingredients(steps, "minecraft:leather_chestplate", (1,))
    _place_ingredients(steps, "minecraft:cyan_dye", (2,))
    result = steps.wait(lambda state: container_slot(state, 0)["item_id"] == "minecraft:leather_chestplate"
                        and container_slot(state, 0)["count"] == 1, "crafted_cyan_leather_result")
    steps.report["dye_crafting_result"] = container_slot(result, 0)
    if graphical:
        _capture(steps, "p6-cyan-leather-crafting-result")
    _click(steps, 0, quick=True)
    steps.call("minecraft_close_screen")
    _inventory(steps, "minecraft:leather_chestplate", 1)
    _inventory(steps, "minecraft:cyan_dye", 0)
    _select(steps, "minecraft:leather_chestplate")
    _component_receipt(steps, "crafted_cyan_chestplate", 1_481_884)

def _actor_capture(steps: ProbeSteps, identity: str, name: str) -> None:
    found = {}

    def received(_: dict) -> bool:
        row = next((row for row in _rows(steps, 16) if row["entity_uuid"] == identity), None)
        if row is None:
            return False
        found.update(row)
        return True

    steps.wait(received, name + "_same_actor_visible")
    _aim(steps, found)
    steps.call("minecraft_wait_ticks", {"ticks": 4})
    _capture(steps, name)


def run(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    peer = steps.client.p5_armor_actor
    with ProbeSteps(peer, steps.artifact_dir / "headless-actor", steps.remaining(), "p5-headless-armor-actor") as actor:
        try:
            x, y, z = origin
            actor.fixture(f"tp @s {x + .5} {y} {z + 3.5}")
            actor.fixture("gamemode survival @s")
            actor.call("minecraft_look", {"yaw_deg": 180, "pitch_deg": 0})
            identity = actor.observe()["player"]["entity_uuid"]
            steps.report.update(actor_uuid=identity, actor_graphical=False,
                                observer_graphical=True, graphical_clients=1)
            for material in ("iron", "diamond", "leather"):
                _clear(actor)
                for piece, equipment_slot in (("boots", 36), ("leggings", 37), ("chestplate", 38), ("helmet", 39)):
                    item = f"minecraft:{material}_{piece}"
                    components = "[minecraft:dyed_color=16711935]" if material == "leather" else ""
                    actor.fixture(f"give @s {item}{components} 1")
                    _inventory(actor, item, 1)
                    _select(actor, item)
                    if material == "leather":
                        _component_receipt(actor, "given_magenta_" + piece, 16_711_935)
                    actor.call("minecraft_wait_ticks", {"ticks": 4})
                    actor.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 1})
                    actor.wait(lambda state: any(row.get("slot") == equipment_slot and row.get("item_id") == item
                               and row.get("count") == 1 for row in state["inventory"]), material + "_" + piece + "_equipped")
                _actor_capture(steps, identity, "p5-player-armor-" + ("leather-dyed" if material == "leather" else material))
                steps.report.setdefault("actor_inventory", {})[material] = actor.observe()["inventory"]
            _clear(actor)
            _craft_dyed_chestplate(actor, (x, y, z + 3), graphical=False)
            actor.call("minecraft_look", {"yaw_deg": 180, "pitch_deg": -35})
            actor.call("minecraft_wait_ticks", {"ticks": 4})
            actor.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 1})
            actor.wait(lambda state: any(row.get("slot") == 38 and row.get("item_id") == "minecraft:leather_chestplate"
                       and row.get("count") == 1 for row in state["inventory"]), "crafted_cyan_chestplate_equipped")
            _actor_capture(steps, identity, "p6-player-armor-crafted-cyan")
            steps.report["crafted_actor_inventory"] = actor.observe()["inventory"]
            actor.fixture(f"tp @s {x + .5} {y} {z - 3.5}")
            _echo(actor, "actor_moved_behind_observer")

            _give(steps, "minecraft:armor_stand", 0)
            steps.call("minecraft_set_hotbar_slot", {"slot": 0})
            _use(steps, _point(origin, dy=-1))
            _inventory(steps, "minecraft:armor_stand", 0)
            stand = _identity(steps, "minecraft:armor_stand")
            _give(steps, "minecraft:iron_chestplate", 1)
            steps.call("minecraft_set_hotbar_slot", {"slot": 1})
            _interact(steps, stand)
            _inventory(steps, "minecraft:iron_chestplate", 0)
            _capture(steps, "p5-armor-stand-still-equipped")
            steps.call("minecraft_set_hotbar_slot", {"slot": 8})
            _interact(steps, stand)
            _inventory(steps, "minecraft:iron_chestplate", 1)
            _capture(steps, "p5-armor-stand-withdrawn")
            _interact(steps, stand)
            _inventory(steps, "minecraft:iron_chestplate", 0)
            _capture(steps, "p5-armor-stand-reequipped")
            _interact(steps, stand)
            _inventory(steps, "minecraft:iron_chestplate", 1)
            _clear(steps)
            steps.fixture("give @s minecraft:leather_chestplate[minecraft:dyed_color=16711935] 1")
            _inventory(steps, "minecraft:leather_chestplate", 1)
            _select(steps, "minecraft:leather_chestplate")
            _component_receipt(steps, "given_magenta_stand_chestplate", 16_711_935)
            _interact(steps, stand)
            _inventory(steps, "minecraft:leather_chestplate", 0)
            _capture(steps, "p6-stand-armor-given-magenta")
            _interact(steps, stand)
            _inventory(steps, "minecraft:leather_chestplate", 1)
            _clear(steps)
            _craft_dyed_chestplate(steps, origin, graphical=True)
            _interact(steps, stand)
            _inventory(steps, "minecraft:leather_chestplate", 0)
            _capture(steps, "p6-stand-armor-crafted-cyan")
            _interact(steps, stand)
            _inventory(steps, "minecraft:leather_chestplate", 1)
            _component_receipt(steps, "withdrawn_crafted_cyan_chestplate", 1_481_884)
            _interact(steps, stand)
            _inventory(steps, "minecraft:leather_chestplate", 0)
            _capture(steps, "p6-stand-armor-crafted-cyan-reequipped")

            for kind, material, dx in (("zombie", "iron", -2), ("skeleton", "diamond", 2)):
                equipment = ",".join(f"{slot}:{{id:'minecraft:{material}_{piece}'}}"
                                     for slot, piece in (("feet", "boots"), ("legs", "leggings"),
                                                         ("chest", "chestplate"), ("head", "helmet")))
                command = f"summon {kind} {x + dx + .5} {y} {z + 4.5} {{NoAI:1b,PersistenceRequired:1b,Invulnerable:1b,equipment:{{{equipment}}}}}"
                steps.fixture(command)
                _echo(steps, kind + "_equipped_summon_ordered_receipt")
                row = _entity_at(steps, "minecraft:" + kind, x + dx + .5, z + 4.5)
                steps.report.setdefault("mob_equipment_fixtures", []).append({"command": command, "entity": row})
                _aim(steps, row)
                steps.call("minecraft_wait_ticks", {"ticks": 4})
                _capture(steps, "p6-" + kind + "-armor-" + material)
            steps.report["visual_review_required"] = ["same player iron/diamond and magenta-dyed all four pieces",
                "crafted cyan chestplate on player and stand", "given magenta stand chestplate",
                "stand custody and unchanged iron rendering", "summoned iron zombie and diamond skeleton"]
        except Exception:
            try:
                _capture(steps, "p5-native-armor-failure")
            except Exception as error:
                steps.report["failure_capture_error"] = str(error)
            raise
    steps.report["actor"] = actor.report
