"""A bounded, UUID-fenced pig feed/attack/drop probe for either client."""
from __future__ import annotations

from pathlib import Path
from typing import Any

from .probe_steps import ProbeSteps, item_count

PIG = "minecraft:pig"
CARROT = "minecraft:carrot"
PORK = "minecraft:porkchop"
SWORD = "minecraft:wooden_sword"
REQUIRED_TOOLS = frozenset({
    "minecraft_observe", "minecraft_wait_for_state_change", "minecraft_send_chat",
    "minecraft_list_entities", "minecraft_wait_for_visible_entity", "minecraft_approach_entity",
    "minecraft_interact_entity", "minecraft_attack_entity_once",
    "minecraft_attack_entity_until_drop_collected", "minecraft_wait_for_entity_removed",
    "minecraft_wait_for_inventory", "minecraft_set_hotbar_slot", "minecraft_select_hotbar_item",
})


def run(client: Any, artifact_dir: Path, timeout: float) -> dict[str, Any]:
    from .gameplay import require_tools

    require_tools(client.list_tools(), REQUIRED_TOOLS, "entities")
    with ProbeSteps(client, artifact_dir, timeout, "entities-pig-feed-attack-pickup") as steps:
        before = steps.wait(lambda state: state["player"]["on_ground"], "grounded_fixture_pose")
        baseline = steps.call("minecraft_list_entities", {"radius": 16.0, "limit": 128})["entities"]
        steps.check("no_preexisting_fixture_pig", not any(row["entity_type"] == PIG for row in baseline))
        steps.check("fresh_drop_inventory", item_count(before, PORK) == 0)
        player = before["player"]
        steps.fixture(f"summon {PIG} {player['x'] + 1.5:.3f} {player['y']:.3f} {player['z']:.3f}")
        target = steps.call("minecraft_wait_for_visible_entity", {
            "entity_type": PIG, "radius": 16.0, "timeout_seconds": steps.remaining(),
        })
        identity = {key: target[key] for key in ("entity_id", "entity_uuid", "entity_type")}
        steps.check("visible_fixture_type", identity["entity_type"] == PIG)
        visible = steps.call("minecraft_list_entities", {"radius": 16.0, "limit": 128})["entities"]
        steps.check("fixture_identity_listed", any(all(row[key] == value for key, value in identity.items())
                                                  for row in visible))
        steps.fixture("debug give minecraft:carrot 1 0")
        steps.call("minecraft_wait_for_inventory", {
            "item_id": CARROT, "count": 1, "timeout_seconds": steps.remaining(),
        })
        steps.call("minecraft_set_hotbar_slot", {"slot": 0})
        approach = steps.call("minecraft_approach_entity", {
            "entity_id": identity["entity_id"], "timeout_seconds": steps.remaining(),
        })
        steps.check("same_entity_in_reach", approach.get("in_reach") is True
                    and all(approach[key] == value for key, value in identity.items()))
        interaction = steps.call("minecraft_interact_entity", {**identity, "hand": "main_hand"})
        steps.call("minecraft_wait_for_inventory", {
            "item_id": CARROT, "count": 0, "timeout_seconds": steps.remaining(),
        })
        fed = steps.observe()
        steps.check("feed_consumed_exact_carrot", item_count(fed, CARROT) == 0)
        steps.report["interaction"] = interaction
        # A local interaction dispatch is not feeding success; the exact
        # authoritative inventory debit above is the consumer-visible evidence.
        # Use an ordinary survival weapon, not an unbounded fist chase across
        # generated terrain. Fixture supply is separate from damage/drop proof.
        steps.fixture(f"debug give {SWORD} 1 0")
        steps.call("minecraft_wait_for_inventory", {
            "item_id": SWORD, "count": 1, "timeout_seconds": steps.remaining(),
        })
        steps.call("minecraft_select_hotbar_item", {
            "item_id": SWORD, "count": 1, "timeout_seconds": steps.remaining(),
        })
        armed = steps.observe()
        steps.check("ordinary_combat_weapon_selected", armed["selected_item"]["item_id"] == SWORD
                    and armed["selected_item"]["count"] == 1 and item_count(armed, CARROT) == 0)
        damage = steps.call("minecraft_attack_entity_once", {**identity, "timeout_seconds": steps.remaining()})
        steps.check("one_attack_confirmed_damage", damage.get("confirmed") is True
                    and all(damage[key] == value for key, value in identity.items())
                    and damage.get("removed") is False
                    and damage["health_after"] < damage["health_before"])
        steps.report["first_attack"] = damage
        collected = steps.call("minecraft_attack_entity_until_drop_collected", {
            "entity_id": identity["entity_id"], "expected_drop_item_id": PORK,
            "expected_drop_count": 1, "timeout_seconds": steps.remaining(),
        })
        steps.check("same_entity_killed_and_pickup_confirmed", collected.get("entity_removed") is True
                    and collected.get("pickup_restored") is True
                    and all(collected[key] == value for key, value in identity.items()))
        removed = steps.call("minecraft_wait_for_entity_removed", {
            **identity, "timeout_seconds": steps.remaining(),
        })
        steps.check("removal_observed", removed.get("removed") is True)
        after = steps.observe()
        drop_count = item_count(after, PORK)
        steps.check("player_received_pig_drop", drop_count >= 1)
        final_entities = steps.call("minecraft_list_entities", {"radius": 16.0, "limit": 128})["entities"]
        steps.check("killed_uuid_absent", not any(row["entity_uuid"] == identity["entity_uuid"]
                                                  for row in final_entities))
        steps.call("minecraft_select_hotbar_item", {
            "item_id": PORK, "count": drop_count, "timeout_seconds": steps.remaining(),
        })
        selected = steps.observe()
        steps.check("collected_drop_selected", selected["selected_item"]["item_id"] == PORK
                    and selected["selected_item"]["count"] == drop_count
                    and item_count(selected, PORK) == drop_count)
        steps.report.update(entity=identity, drop_count=drop_count,
                            inventory_before=before["inventory"], inventory_after=selected["inventory"])
    return steps.report
