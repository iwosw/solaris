"""One ordinary, observed mine/pickup/place scenario for either Minecraft client."""
from __future__ import annotations

import json
import math
import time
from pathlib import Path
from typing import Any

from ..mcp import McpClient

REQUIRED_TOOLS = frozenset({
    "minecraft_observe",
    "minecraft_wait_for_state_change",
    "minecraft_wait_for_loaded_block",
    "minecraft_scan_blocks",
    "minecraft_list_entities",
    "minecraft_break_block",
    "minecraft_wait_for_block_state",
    "minecraft_wait_for_inventory",
    "minecraft_select_hotbar_item",
    "minecraft_look",
    "minecraft_press_inputs",
    "minecraft_wait_ticks",
    "minecraft_look_at_block",
    "minecraft_use_item_on",
})

DIRT = "minecraft:dirt"
GROUND = frozenset({DIRT, "minecraft:grass_block"})
AIR = frozenset({"minecraft:air", "minecraft:cave_air", "minecraft:void_air"})


def _count(observation: dict[str, Any], item_id: str) -> int:
    return sum(int(stack["count"]) for stack in observation["inventory"]
               if stack["item_id"] == item_id)


def _position(block: dict[str, Any]) -> tuple[int, int, int]:
    return int(block["x"]), int(block["y"]), int(block["z"])


def _candidates(
    blocks: list[dict[str, Any]], player: dict[str, Any],
    block_types: frozenset[str] = GROUND,
) -> list[dict[str, Any]]:
    """Reachable exposed ground, excluding the player's current standing column."""
    indexed = {_position(block): block for block in blocks}
    px, py, pz = (float(player[axis]) for axis in ("x", "y", "z"))
    result = []
    for (x, y, z), block in indexed.items():
        if block["block_id"] not in block_types or y + 1 > py + 1.1:
            continue
        # A full block placed above this support must not intersect the player.
        if x < px + 0.31 and x + 1 > px - 0.31 and z < pz + 0.31 and z + 1 > pz - 0.31:
            continue
        if not all(indexed.get((x, y + offset, z), {}).get("block_id") in AIR
                   for offset in (1, 2)):
            continue
        # Stay comfortably inside ordinary survival reach and pickup distance.
        if (x + 0.5 - px) ** 2 + (z + 0.5 - pz) ** 2 > 2.5 ** 2:
            continue
        if (x + 0.5 - px) ** 2 + (y + 1 - (py + 1.62)) ** 2 + (z + 0.5 - pz) ** 2 > 3.5 ** 2:
            continue
        result.append(block)
    return sorted(result, key=lambda block: (
        (block["x"] + 0.5 - px) ** 2 + (block["z"] + 0.5 - pz) ** 2,
        _position(block),
    ))


def run(client: McpClient, artifact_dir: Path, timeout: float) -> dict[str, Any]:
    """Run after the parent connects to a fresh survival world.

    Action acknowledgements never constitute success: authoritative block state
    and independently observed inventory deltas fence both gameplay transitions.
    Tool waits are client-state-event driven; there are no wall-clock sleep loops.
    """
    artifact_dir = Path(artifact_dir)
    artifact_dir.mkdir(parents=True, exist_ok=True)
    transcript: list[dict[str, Any]] = []
    report: dict[str, Any] = {"id": "survival", "passed": False, "assertions": {}}
    deadline = time.monotonic() + timeout

    def remaining() -> float:
        seconds = deadline - time.monotonic()
        if not math.isfinite(seconds) or seconds < 0.1:
            raise TimeoutError("survival scenario deadline expired")
        return min(seconds, 120.0)

    def call(name: str, arguments: dict[str, Any] | None = None) -> dict[str, Any]:
        remaining()
        entry: dict[str, Any] = {"tool": name, "arguments": arguments or {}}
        transcript.append(entry)
        try:
            result = client.call_tool(name, arguments or {})
            entry["result"] = result
            return result
        except Exception as error:
            entry["error"] = str(error)
            raise

    def check(name: str, condition: bool) -> None:
        report["assertions"][name] = condition
        if not condition:
            raise RuntimeError(f"survival assertion failed: {name}")

    def observe() -> dict[str, Any]:
        state = call("minecraft_observe")
        check("in_play", state.get("in_play") is True)
        return state

    def scan(state: dict[str, Any]) -> list[dict[str, Any]]:
        x, y, z = (math.floor(state["player"][axis]) for axis in ("x", "y", "z"))
        # Fence loaded terrain before asking for the inclusive local box.
        for bx, bz in ((x - 3, z - 3), (x + 3, z + 3)):
            call("minecraft_wait_for_loaded_block", {
                "x": bx, "y": y - 2, "z": bz, "timeout_seconds": remaining(),
            })
        return call("minecraft_scan_blocks", {
            "min_x": x - 3, "max_x": x + 3,
            "min_y": y - 2, "max_y": y + 2,
            "min_z": z - 3, "max_z": z + 3, "max_blocks": 245,
        })["blocks"]

    def wait_block(position: tuple[int, int, int], block_id: str) -> dict[str, Any]:
        x, y, z = position
        matched = call("minecraft_wait_for_block_state", {
            "x": x, "y": y, "z": z, "block_id": block_id,
            "timeout_seconds": remaining(),
        })
        check("block_state_wait_matched", matched.get("status") == "matched")
        block = matched["block"]
        check(f"observed_{block_id}", block.get("block_id") == block_id and _position(block) == position)
        return block

    def safe(value: Any) -> Any:
        # Never persist credentials, even if a client includes them in an error.
        if isinstance(value, dict):
            return {key: safe(item) for key, item in value.items()
                    if "token" not in key.lower() and "authorization" not in key.lower()}
        if isinstance(value, list):
            return [safe(item) for item in value]
        if isinstance(value, str):
            token = getattr(client, "token", "")
            return value.replace(token, "[REDACTED]") if token else value
        return value

    try:
        remaining()
        before = observe()
        while not before["player"]["on_ground"]:
            call("minecraft_wait_for_state_change", {
                "observed_version": before["state_version"], "timeout_seconds": remaining(),
            })
            before = observe()
        initial_count = _count(before, DIRT)
        check("fresh_inventory_has_no_dirt", initial_count == 0)
        blocks = scan(before)
        candidates = _candidates(blocks, before["player"])
        if not candidates:
            # Seed 81 has one naturally generated snow-block layer over dirt.
            # Clear it through the same ordinary mining path, without claiming
            # a snow drop or injecting a tool. Never excavate unknown support.
            indexed = {_position(block): block for block in blocks}
            px, py, pz = (math.floor(before["player"][axis]) for axis in ("x", "y", "z"))
            standing = (px, py - 1, pz)
            check("known_snow_over_solid_ground",
                  indexed.get(standing, {}).get("block_id") == "minecraft:snow_block"
                  and indexed.get((px, py - 2, pz), {}).get("block_id") in GROUND)
            clear_positions = [standing]
            snow = [
                block for block in _candidates(blocks, before["player"], frozenset({"minecraft:snow_block"}))
                if indexed.get((block["x"], block["y"] - 1, block["z"]), {}).get("block_id") in GROUND
            ]
            check("known_adjacent_snow_over_ground", len(snow) >= 2)
            clear_positions.extend(_position(block) for block in snow[:2])
            report["ordinary_snow_clearance"] = []
            for position in clear_positions:
                x, y, z = position
                clearance = call("minecraft_break_block", {
                    "x": x, "y": y, "z": z, "face": "up",
                    "expected_drop_item_id": DIRT, "expected_drop_count": 0,
                    "timeout_seconds": remaining(),
                })
                wait_block(position, "minecraft:air")
                report["ordinary_snow_clearance"].append({"position": list(position), "result": clearance})
            before = observe()
            while not before["player"]["on_ground"] or before["player"]["y"] >= py - 0.5:
                call("minecraft_wait_for_state_change", {
                    "observed_version": before["state_version"], "timeout_seconds": remaining(),
                })
                before = observe()
            check("snow_clearance_did_not_create_dirt", _count(before, DIRT) == initial_count)
            candidates = _candidates(scan(before), before["player"])
        check("safe_loaded_mining_target", bool(candidates))
        target = candidates[0]
        target_position = _position(target)
        report["mining_target"] = target
        x, y, z = target_position
        broken = call("minecraft_break_block", {
            "x": x, "y": y, "z": z, "face": "up",
            "expected_drop_item_id": DIRT, "expected_drop_count": 0,
            "timeout_seconds": remaining(),
        })
        check("ordinary_mining_started", broken.get("started") is True)
        wait_block(target_position, "minecraft:air")
        check("block_removed", broken.get("became_air") is True)
        # A fixed walk can leave the player on the rim above the actual drop.
        # Follow received item positions through ordinary one-tick movement and
        # stop only on this player's independently observed inventory gain.
        collected = observe()
        pickup_uuid = None
        while _count(collected, DIRT) < initial_count + 1:
            visible = call("minecraft_list_entities", {"radius": 4.0, "limit": 128})["entities"]
            drops = [entity for entity in visible
                     if entity["entity_type"] == "minecraft:item"
                     and entity.get("item", {}).get("item_id") == DIRT
                     and (pickup_uuid is None or entity["entity_uuid"] == pickup_uuid)
                     and abs(entity["x"] - (x + 0.5)) <= 2
                     and abs(entity["y"] - y) <= 2
                     and abs(entity["z"] - (z + 0.5)) <= 2]
            if not drops:
                call("minecraft_wait_for_state_change", {
                    "observed_version": collected["state_version"], "timeout_seconds": remaining(),
                })
                collected = observe()
                continue
            drop = min(drops, key=lambda entity: (entity["x"] - (x + 0.5)) ** 2
                       + (entity["y"] - y) ** 2 + (entity["z"] - (z + 0.5)) ** 2)
            pickup_uuid = drop["entity_uuid"]
            report["pickup_entity"] = drop
            dx = drop["x"] - collected["player"]["x"]
            dz = drop["z"] - collected["player"]["z"]
            call("minecraft_look", {
                "yaw_deg": round(math.degrees(math.atan2(-dx, dz))), "pitch_deg": 0,
            })
            if dx * dx + dz * dz > 0.15 ** 2:
                call("minecraft_press_inputs", {"keys": ["forward"], "ticks": 1})
            else:
                call("minecraft_wait_ticks", {"ticks": 1})
            collected = observe()
        call("minecraft_wait_for_inventory", {
            "item_id": DIRT, "count": initial_count + 1, "timeout_seconds": remaining(),
        })
        collected_count = _count(collected, DIRT)
        check("drop_picked_up", collected_count == initial_count + 1)
        report["inventory_before"] = before["inventory"]
        report["inventory_after_pickup"] = collected["inventory"]

        call("minecraft_select_hotbar_item", {
            "item_id": DIRT, "count": 1, "timeout_seconds": remaining(),
        })
        selected = observe()
        slot = selected["player"]["selected_hotbar_slot"]
        check("hotbar_item_selected", 0 <= slot <= 8
              and selected["selected_item"].get("item_id") == DIRT
              and selected["selected_item"].get("count") == 1
              and any(stack.get("slot") == slot and stack.get("item_id") == DIRT
                      and stack.get("count") == 1 for stack in selected["inventory"]))
        check("selection_preserved_inventory", _count(selected, DIRT) == collected_count)
        supports = _candidates(scan(selected), selected["player"])
        check("safe_loaded_placement_face", bool(supports))
        support = supports[0]
        x, y, z = _position(support)
        destination = (x, y + 1, z)
        # Explicitly observe empty destination immediately before dispatch.
        wait_block(destination, "minecraft:air")
        report["placement_support"] = support
        report["placement_position"] = list(destination)
        face = {"x": x, "y": y, "z": z, "face": "up"}
        call("minecraft_look_at_block", face)
        call("minecraft_use_item_on", {**face, "hand": "main_hand"})
        report["placed_block"] = wait_block(destination, DIRT)
        call("minecraft_wait_for_inventory", {
            "item_id": DIRT, "count": collected_count - 1, "timeout_seconds": remaining(),
        })
        after = observe()
        check("placed_item_consumed", _count(after, DIRT) == collected_count - 1)
        report["inventory_after_placement"] = after["inventory"]
        report["passed"] = True
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        report = safe(report)
        (artifact_dir / "transcript.json").write_text(json.dumps(safe(transcript), indent=2) + "\n", encoding="utf-8")
        (artifact_dir / "scenario.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return report
