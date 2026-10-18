#!/usr/bin/env python3
"""Shared real-client inventory probe."""

from __future__ import annotations

from pathlib import Path
from typing import Any


INVENTORY_SCENARIO_ID = "m94-03a-inventory-oak-log-to-planks"
INVENTORY_TOOLS = frozenset({
    "minecraft_observe", "minecraft_wait_for_state_change", "minecraft_send_chat",
    "minecraft_close_screen", "minecraft_open_inventory", "minecraft_set_hotbar_slot",
    "minecraft_wait_for_inventory", "minecraft_wait_for_container_slot",
    "minecraft_click_container_slot", "minecraft_quick_move_container_slot",
    "minecraft_select_hotbar_item",
})

def run_inventory_probe(mcp: Any, run_dir: Path, timeout_seconds: float = 120.0) -> dict[str, Any]:
    """One Python oak-log craft, cursor round-trip and selection for both clients."""
    from .backends.gameplay import require_tools
    from .backends.probe_steps import ProbeSteps, container_slot, item_count

    require_tools(mcp.list_tools(), INVENTORY_TOOLS, INVENTORY_SCENARIO_ID)
    mcp.request_timeout_seconds = min(timeout_seconds, 120.0) + 10.0
    probe_dir = run_dir / "inventory-probe"
    with ProbeSteps(mcp, probe_dir, timeout_seconds, INVENTORY_SCENARIO_ID) as steps:
        steps.call("minecraft_close_screen")
        initial = steps.observe()
        steps.check("fixture_apples_present", item_count(initial, "minecraft:apple") == 2)
        steps.check("fresh_crafting_inventory",
                    item_count(initial, "minecraft:oak_log") == 0
                    and item_count(initial, "minecraft:oak_planks") == 0)
        # Same declared operator setup as the old in-client M94 focused phase.
        steps.fixture("debug give minecraft:oak_log 1 0")
        steps.call("minecraft_set_hotbar_slot", {"slot": 0})
        steps.call("minecraft_wait_for_inventory", {
            "item_id": "minecraft:oak_log", "count": 1, "timeout_seconds": steps.remaining(),
        })
        steps.call("minecraft_open_inventory")
        opened = steps.wait(lambda state: state["screen"]["open"]
                            and state["container"]["slot_count"] == 46, "inventory_screen_open")
        steps.check("empty_cursor_before_craft", opened["container"]["carried"]["count"] == 0)
        # Normalize the two-apples fixture through ordinary clicks if it was
        # delivered to a hotbar slot rather than the legacy economy's slot 9.
        apples = [stack for stack in opened["inventory"] if stack["item_id"] == "minecraft:apple"]
        steps.check("one_apples_stack", len(apples) == 1 and apples[0]["count"] == 2)
        apple_slot = apples[0]["slot"]
        if apple_slot != 9:
            steps.check("empty_apples_destination", container_slot(opened, 9)["count"] == 0)
            source_slot = 36 + apple_slot if apple_slot < 9 else apple_slot
            for slot in (source_slot, 9):
                steps.call("minecraft_click_container_slot", {
                    "slot": slot, "button": "primary", "timeout_seconds": steps.remaining(),
                })
            steps.call("minecraft_wait_for_container_slot", {
                "slot": 9, "item_id": "minecraft:apple", "count": 2,
                "timeout_seconds": steps.remaining(),
            })
        baseline = steps.observe()
        steps.report["inventory_before"] = baseline["inventory"]
        # Pick up the real log, place it in the 2x2 grid, then take its output.
        for slot in (36, 1):
            steps.call("minecraft_click_container_slot", {
                "slot": slot, "button": "primary", "timeout_seconds": steps.remaining(),
            })
        steps.call("minecraft_wait_for_container_slot", {
            "slot": 0, "item_id": "minecraft:oak_planks", "count": 4,
            "timeout_seconds": steps.remaining(),
        })
        steps.call("minecraft_quick_move_container_slot", {
            "slot": 0, "timeout_seconds": steps.remaining(),
        })
        steps.call("minecraft_wait_for_inventory", {
            "item_id": "minecraft:oak_planks", "count": 4, "timeout_seconds": steps.remaining(),
        })
        crafted = steps.observe()
        steps.check("one_log_consumed", item_count(crafted, "minecraft:oak_log") == 0)
        steps.check("four_planks_created", item_count(crafted, "minecraft:oak_planks") == 4)
        steps.check("crafting_grid_empty", all(container_slot(crafted, slot)["count"] == 0
                                              for slot in range(1, 5)))
        steps.check("crafting_cursor_empty", crafted["container"]["carried"]["count"] == 0)
        steps.check("crafting_preserved_apples", item_count(crafted, "minecraft:apple") == 2)
        steps.report["crafting"] = {"id": INVENTORY_SCENARIO_ID, "result": "passed",
                                   "oak_log_consumed": 1, "oak_planks_created": 4}
        # Cursor evidence is independent of the slot-click acknowledgement.
        steps.call("minecraft_click_container_slot", {
            "slot": 9, "button": "primary", "timeout_seconds": steps.remaining(),
        })
        carried = steps.wait(lambda state: state["container"]["carried"]["item_id"] == "minecraft:apple"
                             and state["container"]["carried"]["count"] == 2,
                             "apples_on_cursor")
        steps.check("apples_not_duplicated_on_pickup", item_count(carried, "minecraft:apple") == 0
                    and container_slot(carried, 9)["count"] == 0)
        steps.call("minecraft_click_container_slot", {
            "slot": 9, "button": "primary", "timeout_seconds": steps.remaining(),
        })
        steps.call("minecraft_wait_for_container_slot", {
            "slot": 9, "item_id": "minecraft:apple", "count": 2,
            "timeout_seconds": steps.remaining(),
        })
        returned = steps.observe()
        steps.check("cursor_roundtrip_conserved_apples", item_count(returned, "minecraft:apple") == 2
                    and returned["container"]["carried"]["count"] == 0)
        steps.call("minecraft_close_screen")
        steps.call("minecraft_select_hotbar_item", {
            "item_id": "minecraft:oak_planks", "count": 4, "timeout_seconds": steps.remaining(),
        })
        final = steps.observe()
        steps.check("crafted_stack_selected", final["selected_item"]["item_id"] == "minecraft:oak_planks"
                    and final["selected_item"]["count"] == 4)
        steps.check("selection_conserved_inventory", item_count(final, "minecraft:oak_planks") == 4
                    and item_count(final, "minecraft:apple") == 2)
        steps.report.update(crafting_scenario=INVENTORY_SCENARIO_ID, cursor_roundtrip_apples=2,
                            selected_crafted_planks=4, inventory_after=final["inventory"])
    return steps.report








