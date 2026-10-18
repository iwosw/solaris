"""P1/P2 graphical parity probes; fixtures and ordinary actions stay separate.

Captures supplement received-state predicates. They require human visual review;
this module does not equate an item debit with rendered armor/text/admiration.
"""
from __future__ import annotations

import math
import json
import select
import socket
import threading
import uuid
import zlib
from pathlib import Path
from typing import Any

from . import native_client
from .native_v2 import _block, _echo, _give, _point, _set, _use
from .probe_steps import ProbeSteps, item_count


def _varint(data: bytes | bytearray, offset: int = 0) -> tuple[int, int] | None:
    value = 0
    for index in range(5):
        if offset + index >= len(data):
            return None
        byte = data[offset + index]
        value |= (byte & 127) << (7 * index)
        if byte < 128:
            return value, offset + index + 1
    raise ValueError("oversized wire VarInt")


def _hook_biting(payload: bytes) -> tuple[int, bool] | None:
    """Pinned hook metadata tail: accessor 9, BOOLEAN serializer 8, terminator."""
    identity = _varint(payload)
    if identity is None or payload[-4:] not in (b"\x09\x08\x00\xff", b"\x09\x08\x01\xff"):
        return None
    return identity[0], payload[-2] == 1


class SignWireTrace:
    """Unmodified forwarding with sign, hook metadata and equipment receipts."""

    def __init__(self, server_port: int, artifact: Path, *, fishing: bool = False) -> None:
        self.server_port = server_port
        self.path = artifact / ("fishing-wire-trace.json" if fishing else "sign-wire-trace.json")
        self.fishing = fishing
        self.biting: dict[int, dict[str, Any]] = {}
        self.equipment: list[dict[str, Any]] = []
        self.players: dict[int, dict[str, str]] = {}
        self.rows: list[dict[str, Any]] = []
        self.errors: list[str] = []
        self.connections: list[threading.Thread] = []
        self.wake, self.stop = socket.socketpair()
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen()
        self.address = f"127.0.0.1:{self.listener.getsockname()[1]}"
        self.worker = threading.Thread(target=self._accept, daemon=True)
        self.worker.start()

    def _accept(self) -> None:
        while self.listener in select.select([self.listener, self.wake], [], [])[0]:
            client, _ = self.listener.accept()
            worker = threading.Thread(target=self._forward, args=(client, len(self.connections)), daemon=True)
            self.connections.append(worker)
            worker.start()

    def _forward(self, client: socket.socket, connection: int) -> None:
        try:
            with client, socket.create_connection(("127.0.0.1", self.server_port), timeout=10) as server:
                buffer = bytearray()
                phase, compressed, sequence = "login", False, 0
                while True:
                    ready = select.select([client, server, self.wake], [], [])[0]
                    if self.wake in ready:
                        return
                    for source, target in ((client, server), (server, client)):
                        if source not in ready:
                            continue
                        data = source.recv(65536)
                        if not data:
                            return
                        if source is client:
                            target.sendall(data)
                            continue
                        buffer.extend(data)
                        while (header := _varint(buffer)) is not None:
                            length, start = header
                            if length > 16 * 1024 * 1024:
                                raise ValueError("oversized local server frame")
                            if len(buffer) < start + length:
                                break
                            body = bytes(buffer[start:start + length])
                            del buffer[:start + length]
                            if compressed:
                                raw_length, payload_start = _varint(body)
                                body = zlib.decompress(body[payload_start:]) if raw_length else body[payload_start:]
                            packet_id, payload_start = _varint(body)
                            sequence += 1
                            if phase == "play" and packet_id in (0x08, 0x06, 0x3C):
                                packed = int.from_bytes(body[payload_start:payload_start + 8], "big")
                                signed = lambda value, bits: value - (1 << bits) if value & (1 << (bits - 1)) else value
                                row = {"connection": connection, "sequence": sequence, "packet_id": packet_id,
                                       "packet": {0x08: "BlockUpdate", 0x06: "BlockEntityData", 0x3C: "OpenSignEditor"}[packet_id],
                                       "position": [signed(packed >> 38, 26), signed(packed & 4095, 12),
                                                    signed((packed >> 12) & 0x3FFFFFF, 26)],
                                       "payload_hex": body[payload_start:].hex()}
                                self.rows.append(row)
                            if phase == "play" and self.fishing and packet_id == 0x63:
                                payload = body[payload_start:]
                                entity_id, _ = _varint(payload)
                                row = {"connection": connection, "sequence": sequence, "packet_id": packet_id,
                                       "packet": "SetEntityData", "entity_id": entity_id, "payload_hex": payload.hex()}
                                bite = _hook_biting(payload)
                                if bite is not None:
                                    row["hook_biting"] = bite[1]
                                    self.biting[bite[0]] = row
                                self.rows.append(row)
                            if phase == "play" and packet_id == 0x66:
                                payload = body[payload_start:]
                                entity_id, slot_start = _varint(payload)
                                stack_count, _ = _varint(payload, slot_start + 1)
                                row = {"connection": connection, "sequence": sequence, "packet_id": packet_id,
                                       "packet": "SetEntityEquipment", "entity_id": entity_id,
                                       "first_slot": payload[slot_start] & 127, "first_stack_count": stack_count,
                                       "payload_hex": payload.hex()}
                                self.equipment.append(row)
                                self.rows.append(row)
                            if phase == "login" and packet_id == 3:
                                compressed = True
                            elif phase == "login" and packet_id == 2:
                                payload = body[payload_start:]
                                name_length, name_start = _varint(payload, 16)
                                self.players[connection] = {
                                    "entity_uuid": str(uuid.UUID(bytes=payload[:16])),
                                    "username": payload[name_start:name_start + name_length].decode("utf-8"),
                                }
                                phase = "configuration"
                            elif phase == "configuration" and packet_id == 3:
                                phase = "play"
                        target.sendall(data)
        except Exception as error:
            self.errors.append(f"{type(error).__name__}: {error}")

    def close(self) -> None:
        self.stop.send(b"x")
        self.worker.join(timeout=10)
        for worker in self.connections:
            worker.join(timeout=10)
        self.listener.close()
        self.wake.close()
        self.stop.close()
        self.path.write_text(json.dumps({"origin": "unmodified_loopback_server_stream",
                                        "errors": self.errors, "players": self.players,
                                        "packets": self.rows}, indent=2) + "\n")


def _capture(steps: ProbeSteps, name: str) -> None:
    metadata = native_client.capture(steps.client, steps.artifact_dir.parent, name)
    steps.report.setdefault("screenshots", []).append(metadata)


def _inventory(steps: ProbeSteps, item: str, count: int) -> dict[str, Any]:
    steps.call("minecraft_wait_for_inventory", {
        "item_id": item, "count": count, "timeout_seconds": steps.remaining(),
    })
    return steps.observe()


def _identity(steps: ProbeSteps, kind: str) -> dict[str, Any]:
    target = steps.call("minecraft_wait_for_visible_entity", {
        "entity_type": kind, "radius": 12, "timeout_seconds": steps.remaining(),
    })
    identity = {key: target[key] for key in ("entity_id", "entity_uuid", "entity_type")}
    steps.report["entity"] = identity
    steps.call("minecraft_approach_entity", {
        "entity_id": identity["entity_id"], "timeout_seconds": steps.remaining(),
    })
    return identity


def _interact(steps: ProbeSteps, identity: dict[str, Any]) -> None:
    steps.call("minecraft_interact_entity", {**identity, "hand": "main_hand"})


def _armor_stand(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    _give(steps, "minecraft:armor_stand")
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    _capture(steps, "p1-armor-stand-before-placement")
    _use(steps, _point(origin, dy=-1))
    _inventory(steps, "minecraft:armor_stand", 0)
    identity = _identity(steps, "minecraft:armor_stand")
    _give(steps, "minecraft:iron_chestplate", 1)
    steps.call("minecraft_set_hotbar_slot", {"slot": 1})
    _interact(steps, identity)
    _inventory(steps, "minecraft:iron_chestplate", 0)
    try:
        steps.report["native_raw_observation"] = steps.call("client_observe")
    except RuntimeError as error:
        steps.report["native_raw_observation_unavailable"] = str(error)
    _capture(steps, "p1-armor-stand-equipped")
    steps.call("minecraft_set_hotbar_slot", {"slot": 8})
    _interact(steps, identity)
    _inventory(steps, "minecraft:iron_chestplate", 1)
    _capture(steps, "p1-armor-stand-taken-back")
    # Adjacent regression: repeated empty use cannot duplicate withdrawn armor.
    steps.call("minecraft_set_hotbar_slot", {"slot": 7})
    _interact(steps, identity)
    _echo(steps, "armor_empty_second_use_receipt")
    steps.check("adversarial_empty_use_no_duplicate", item_count(steps.observe(), "minecraft:iron_chestplate") == 1)
    steps.call("minecraft_set_hotbar_slot", {"slot": 8})
    _interact(steps, identity)
    _inventory(steps, "minecraft:iron_chestplate", 0)
    steps.fixture('give @s minecraft:name_tag[custom_name={text:"P1 Stand"}] 1')
    _inventory(steps, "minecraft:name_tag", 1)
    steps.call("minecraft_select_hotbar_item", {"item_id": "minecraft:name_tag", "count": 1, "timeout_seconds": steps.remaining()})
    _interact(steps, identity)
    _inventory(steps, "minecraft:name_tag", 0)
    _capture(steps, "p1-armor-stand-named")
    steps.call("minecraft_set_hotbar_slot", {"slot": 7})
    # Two ordinary punches, not the health-based attack helper: a stand's first
    # punch is a wobble rather than living-entity damage.
    steps.call("minecraft_press_inputs", {"keys": ["attack"], "ticks": 1})
    steps.call("minecraft_wait_ticks", {"ticks": 1})
    steps.call("minecraft_press_inputs", {"keys": ["attack"], "ticks": 1})
    steps.call("minecraft_wait_for_entity_removed", {**identity, "timeout_seconds": steps.remaining()})
    drops = steps.call("minecraft_list_entities", {"radius": 12, "limit": 128})
    steps.report["break_drops"] = drops
    for item in ("minecraft:armor_stand", "minecraft:iron_chestplate"):
        steps.check(f"break_{item}_visible", any(row.get("item", {}).get("item_id") == item for row in drops["entities"]))
    steps.call("minecraft_press_inputs", {"keys": ["forward"], "ticks": 8})
    _inventory(steps, "minecraft:armor_stand", 1)
    _inventory(steps, "minecraft:iron_chestplate", 1)
    _capture(steps, "p1-armor-stand-break-pickup")
    steps.report["visual_review_required"] = ["chestplate rendered on torso", "P1 Stand name visible"]


def _signs(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    support = _point(origin, dy=-1)
    sign = _point(origin)
    _give(steps, "minecraft:oak_sign")
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    _capture(steps, "p1-sign-before-placement")
    _use(steps, support)
    _block(steps, sign, "minecraft:oak_sign")
    state = steps.wait(lambda state: state["screen"]["class"].endswith("SignEditScreen"), "sign_editor_received")
    _capture(steps, "p1-sign-editor")
    steps.call("minecraft_set_screen_input", {
        "expected_screen_class": state["screen"]["class"], "expected_title": state["screen"]["title"],
        "input_label": "Sign line 1", "value": "P1 PARITY",
    })
    steps.call("minecraft_click_screen_button", {
        "expected_screen_class": state["screen"]["class"], "expected_title": state["screen"]["title"],
        "button_label": "Done",
    })
    steps.wait(lambda observed: not observed["screen"]["open"], "real_sign_editor_submitted_and_closed")
    _echo(steps, "sign_submit_ordered_server_receipt")
    steps.call("minecraft_look_at_block", {**sign, "face": "north"})
    _capture(steps, "p1-sign-black-text")
    for slot, item, label in ((1, "minecraft:red_dye", "dyed"), (2, "minecraft:glow_ink_sac", "glowing"),
                              (3, "minecraft:honeycomb", "waxed")):
        _give(steps, item, slot)
        steps.call("minecraft_set_hotbar_slot", {"slot": slot})
        if label == "glowing":
            steps.fixture("time set 18000")
            steps.wait(lambda observed: 18000 <= observed["time"]["clock_time"] % 24000 < 19000,
                       "night_clock_received_for_glow_visual_check")
            _capture(steps, "p1-sign-before-glow-dark")
        _use(steps, sign, "north")
        _inventory(steps, item, 0)
        _capture(steps, f"p1-sign-{label}")
    steps.call("minecraft_set_hotbar_slot", {"slot": 8})
    _use(steps, sign, "north")
    _echo(steps, "waxed_edit_attempt_server_receipt")
    state = steps.observe()
    steps.check("waxed_edit_refused", not state["screen"]["class"].endswith("SignEditScreen"))
    # Repeat refusal after a slot change; no stale editor may be resurrected.
    steps.call("minecraft_set_hotbar_slot", {"slot": 7})
    _use(steps, sign, "north")
    _echo(steps, "waxed_second_edit_attempt_receipt")
    steps.check("adversarial_waxed_second_edit_refused", not steps.observe()["screen"]["class"].endswith("SignEditScreen"))
    _capture(steps, "p1-sign-waxed-refused")
    broken = steps.call("minecraft_break_block", {
        **sign, "face": "north", "expected_drop_item_id": "minecraft:oak_sign",
        "expected_drop_count": 1, "timeout_seconds": steps.remaining(),
    })
    steps.check("adversarial_waxed_sign_player_broken_and_picked_up",
                broken.get("became_air") is True and broken.get("pickup_confirmed") is True)
    steps.report["replacement_sign_break"] = broken
    _block(steps, sign, "minecraft:air")
    _inventory(steps, "minecraft:oak_sign", 1)
    steps.call("minecraft_select_hotbar_item", {
        "item_id": "minecraft:oak_sign", "count": 1, "timeout_seconds": steps.remaining(),
    })
    _use(steps, support)
    _block(steps, sign, "minecraft:oak_sign")
    replacement = steps.wait(lambda observed: observed["screen"]["class"].endswith("SignEditScreen"),
                             "adversarial_replacement_sign_editor_received")
    steps.report["replacement_editor"] = replacement["screen"]
    _capture(steps, "p1-sign-replacement-empty-editor")
    steps.call("minecraft_click_screen_button", {
        "expected_screen_class": replacement["screen"]["class"],
        "expected_title": replacement["screen"]["title"], "button_label": "Done",
    })
    steps.wait(lambda observed: not observed["screen"]["open"], "replacement_empty_editor_submitted_and_closed")
    _echo(steps, "replacement_empty_sign_ordered_server_receipt")
    # Capture the named stand; a visible label also needs CustomNameVisible support.
    steps.fixture("time set 6000")
    steps.wait(lambda observed: 6000 <= observed["time"]["clock_time"] % 24000 < 7000,
               "day_clock_received_for_named_stand_capture")
    _give(steps, "minecraft:armor_stand", 0)
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    _use(steps, _point(origin, dy=-1, dz=4))
    _inventory(steps, "minecraft:armor_stand", 0)
    stand = _identity(steps, "minecraft:armor_stand")
    steps.report["shared_glyph_named_stand"] = stand
    steps.fixture('give @s minecraft:name_tag[custom_name={text:"P1 Stand"}] 1')
    _inventory(steps, "minecraft:name_tag", 1)
    steps.call("minecraft_select_hotbar_item", {
        "item_id": "minecraft:name_tag", "count": 1, "timeout_seconds": steps.remaining(),
    })
    _interact(steps, stand)
    _inventory(steps, "minecraft:name_tag", 0)
    _echo(steps, "named_stand_tag_server_receipt")
    _capture(steps, "p1-signfix-named-armor-stand")
    steps.report["visual_review_required"] = ["P1 PARITY text", "black to red text", "glow presentation",
                                            "replacement editor empty", "P1 Stand name above armor stand"]


def _piglin(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    x, y, z = origin
    _give(steps, "minecraft:golden_helmet", 1)
    steps.call("minecraft_open_inventory")
    steps.call("minecraft_quick_move_container_slot", {"slot": 37, "timeout_seconds": steps.remaining()})
    steps.wait(lambda state: any(stack.get("slot") == 39 and stack.get("item_id") == "minecraft:golden_helmet"
                                and stack.get("count") == 1 for stack in state["inventory"]),
               "gold_helmet_equipped_for_fixture_safety")
    steps.call("minecraft_close_screen")
    # Ordinary overworld summon; do not use NoAI, which disables real barter.
    steps.fixture(f"summon minecraft:piglin {x + .5} {y} {z + 2.5}")
    identity = _identity(steps, "minecraft:piglin")
    _give(steps, "minecraft:gold_ingot")
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    _capture(steps, "p1-piglin-before-gold")
    before_ids = {row["entity_uuid"] for row in steps.call("minecraft_list_entities", {"radius": 12, "limit": 128})["entities"]}
    _interact(steps, identity)
    _inventory(steps, "minecraft:gold_ingot", 0)
    _capture(steps, "p1-piglin-admiring")
    payout: dict[str, Any] = {}
    def new_loot(_: dict[str, Any]) -> bool:
        for row in steps.call("minecraft_list_entities", {"radius": 12, "limit": 128})["entities"]:
            if row["entity_type"] == "minecraft:item" and row["entity_uuid"] not in before_ids and row.get("item", {}).get("item_id") != "minecraft:gold_ingot":
                payout.update(row)
                return True
        return False
    steps.wait(new_loot, "barter_new_uuid_loot_thrown")
    steps.report["payout"] = payout
    _capture(steps, "p1-piglin-loot-thrown")
    # Bounded adjacent no-input check: a completed barter must not pay again.
    steps.call("minecraft_wait_ticks", {"ticks": 160})
    rows = steps.call("minecraft_list_entities", {"radius": 12, "limit": 128})["entities"]
    steps.report["adversarial_entities_after_wait"] = rows
    steps.check("adversarial_no_second_payout_without_gold", not any(row["entity_type"] == "minecraft:item" and row["entity_uuid"] not in before_ids | {payout["entity_uuid"]} for row in rows))
    steps.report["visual_review_required"] = ["piglin holds/admires gold before payout"]


def _trader(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    x, y, z = origin
    steps.fixture(f"summon minecraft:wandering_trader {x + .5} {y} {z + 2.5} {{NoAI:1b}}")
    identity = _identity(steps, "minecraft:wandering_trader")
    _capture(steps, "p1-trader-before-use")
    _interact(steps, identity)
    state = steps.wait(lambda state: state["container"]["container_id"] != 0
                       and state["container"]["menu_class"].endswith("MerchantMenu"), "trader_merchant_received")
    steps.report["merchant"] = state
    _capture(steps, "p1-trader-offers")
    steps.call("minecraft_close_screen")
    _interact(steps, identity)
    reopened = steps.wait(lambda state: state["container"]["container_id"] != 0
                          and state["container"]["menu_class"].endswith("MerchantMenu"), "adversarial_trader_reopen_merchant")
    steps.check("adversarial_trader_no_inventory_credit", all(stack.get("count", 0) == 0 for stack in reopened["inventory"] if stack.get("slot") not in (41, 42)))
    _capture(steps, "p1-trader-reopened-offers")
    steps.call("minecraft_close_screen")
    steps.report["visual_review_required"] = ["visible nonempty merchant offer list"]


def _leaves(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    x, y, z = origin
    steps.fixture("gamerule randomTickSpeed 0")
    _echo(steps, "leaf_random_ticks_disabled_fixture_receipt")
    log = _point(origin, dz=2)
    upper_log = _point(origin, dy=1, dz=2)
    # Keep canopy above player head height. Each ordinary mining receipt must
    # also observe its log pickup before random ticks are enabled.
    steps.fixture(f"fill {x - 2} {y + 2} {z} {x + 2} {y + 3} {z + 4} minecraft:oak_leaves[persistent=false,distance=1]")
    _set(steps, log, "minecraft:oak_log")
    _set(steps, upper_log, "minecraft:oak_log")
    leaf = _point(origin, dy=2)
    _block(steps, leaf, "minecraft:oak_leaves", {"distance": "1", "persistent": "false"})
    persistent = _point(origin, dx=3, dy=2)
    _set(steps, persistent, "minecraft:oak_leaves[persistent=true,distance=7]")
    _give(steps, "minecraft:diamond_axe")
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    steps.call("minecraft_look_at_block", {**log, "face": "north"})
    _capture(steps, "p1-leaves-supported-canopy")
    steps.report["log_breaks"] = []
    for point in (log, upper_log):
        result = steps.call("minecraft_break_block", {**point, "face": "north", "expected_drop_item_id": "minecraft:oak_log",
                                                     "expected_drop_count": 1, "timeout_seconds": steps.remaining()})
        steps.report["log_breaks"].append(result)
        steps.check(f"log_{point['y']}_player_mined", result.get("became_air") is True)
        _block(steps, point, "minecraft:air")
    _inventory(steps, "minecraft:oak_log", 2)
    steps.report["log_pickup_inventory"] = steps.observe()["inventory"]
    _block(steps, leaf, "minecraft:oak_leaves", {"distance": "7", "persistent": "false"})
    _capture(steps, "p1-leaves-unsupported-canopy")
    steps.fixture("gamerule randomTickSpeed 4096")
    _block(steps, leaf, "minecraft:air")
    drops: list[dict[str, Any]] = []
    def leaf_drops(_: dict[str, Any]) -> bool:
        drops[:] = [row for row in steps.call("minecraft_list_entities", {"radius": 16, "limit": 128})["entities"]
                    if row.get("item", {}).get("item_id") in ("minecraft:oak_sapling", "minecraft:stick", "minecraft:apple")]
        return bool(drops)
    steps.wait(leaf_drops, "leaf_decay_visible_drops")
    steps.report["decay_drops"] = drops
    def canopy_gone(_: dict[str, Any]) -> bool:
        scan = steps.call("minecraft_scan_blocks", {
            "min_x": x - 2, "min_y": y + 2, "min_z": z,
            "max_x": x + 2, "max_y": y + 3, "max_z": z + 4,
        })
        steps.report["final_canopy_scan"] = scan
        return all(block["block_id"] == "minecraft:air" for block in scan["blocks"])
    steps.wait(canopy_gone, "whole_natural_canopy_decayed")
    _block(steps, persistent, "minecraft:oak_leaves", {"persistent": "true"})
    steps.check("adversarial_persistent_leaf_survived_natural_decay", True)
    steps.call("minecraft_look_at_block", {**persistent, "face": "north"})
    _capture(steps, "p1-leaves-decayed-with-persistent-control")
    steps.fixture("gamerule randomTickSpeed 0")


def _rows(steps: ProbeSteps, radius: int = 32) -> list[dict[str, Any]]:
    return steps.call("minecraft_list_entities", {"radius": radius, "limit": 128})["entities"]


def _aim(steps: ProbeSteps, row: dict[str, Any]) -> tuple[int, int]:
    player = steps.observe()["player"]
    dx, dz = row["x"] - player["x"], row["z"] - player["z"]
    box = row["bounding_box"]
    dy = (box["min_y"] + box["max_y"]) / 2 - (player["y"] + 1.62)
    yaw = round(math.degrees(math.atan2(-dx, dz)))
    pitch = round(-math.degrees(math.atan2(dy, math.hypot(dx, dz))))
    steps.call("minecraft_look", {"yaw_deg": yaw, "pitch_deg": pitch})
    return yaw, pitch


def _slime(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    x, y, z = origin
    steps.fixture(f"summon minecraft:slime {x + .5} {y} {z + 2.5} {{Size:1,NoAI:1b}}")
    receipt = _echo(steps, "slime_summon_ordered_receipt")
    steps.report["summon_chat"] = receipt["recent_chat"]
    _capture(steps, "p2-slime-summon-receipt")
    steps.check("medium_slime_summon_fixture_accepted", any("Summoned new Slime" in line for line in receipt["recent_chat"]))
    identity = _identity(steps, "minecraft:slime")
    parent = next(row for row in _rows(steps) if row["entity_uuid"] == identity["entity_uuid"])
    parent_height = parent["bounding_box"]["max_y"] - parent["bounding_box"]["min_y"]
    steps.check("medium_slime_received", parent.get("health") == 4 and parent_height > 1)
    _capture(steps, "p2-slime-medium")
    _give(steps, "minecraft:diamond_sword")
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    steps.report["parent_attack"] = steps.call("minecraft_attack_entity_once", {**identity, "timeout_seconds": steps.remaining()})
    steps.call("minecraft_wait_for_entity_removed", {**identity, "timeout_seconds": steps.remaining()})
    children: list[dict[str, Any]] = []
    def split(_: dict[str, Any]) -> bool:
        children[:] = [row for row in _rows(steps) if row["entity_type"] == "minecraft:slime"
                      and row["entity_uuid"] != identity["entity_uuid"]]
        return 2 <= len(children) <= 4
    steps.wait(split, "medium_slime_split_into_two_to_four")
    steps.report["children"] = children.copy()
    steps.check("children_are_small_distinct_live_slimes",
                len({row["entity_uuid"] for row in children}) == len(children)
                and all(row.get("health") == 1 and row["bounding_box"]["max_y"] - row["bounding_box"]["min_y"] < parent_height * .6 for row in children))
    _aim(steps, children[0])
    _capture(steps, "p2-slime-small-children")
    # Adjacent death rule: a size-one child must not recursively split again.
    child = {key: children[0][key] for key in ("entity_id", "entity_uuid", "entity_type")}
    steps.call("minecraft_approach_entity", {"entity_id": child["entity_id"], "timeout_seconds": steps.remaining()})
    steps.call("minecraft_attack_entity_once", {**child, "timeout_seconds": steps.remaining()})
    steps.call("minecraft_wait_for_entity_removed", {**child, "timeout_seconds": steps.remaining()})
    steps.call("minecraft_wait_ticks", {"ticks": 40})
    after = [row for row in _rows(steps) if row["entity_type"] == "minecraft:slime"]
    steps.report["after_small_child_death"] = after
    steps.check("adversarial_small_child_does_not_split",
                {row["entity_uuid"] for row in after} == {row["entity_uuid"] for row in children} - {child["entity_uuid"]})
    _capture(steps, "p2-slime-small-child-no-split")


def _enderman(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    x, y, z = origin
    steps.fixture(f"fill {x - 24} {y - 1} {z - 24} {x + 24} {y - 1} {z + 24} minecraft:obsidian")
    steps.fixture(f"fill {x - 24} {y} {z - 24} {x + 24} {y + 4} {z + 24} minecraft:air")
    steps.fixture(f"summon minecraft:enderman {x + .5} {y} {z + 8.5} {{NoAI:1b}}")
    target = steps.call("minecraft_wait_for_visible_entity", {"entity_type": "minecraft:enderman", "radius": 64, "timeout_seconds": steps.remaining()})
    identity = {key: target[key] for key in ("entity_id", "entity_uuid", "entity_type")}
    steps.report["entity"] = identity
    before = next(row for row in _rows(steps, 64) if row["entity_uuid"] == identity["entity_uuid"])
    steps.check("enderman_health_received", before.get("health") == 40)
    _aim(steps, before)
    _capture(steps, "p2-enderman-before-arrow")
    _give(steps, "minecraft:bow")
    _give(steps, "minecraft:arrow", 1)
    before_ids = {row["entity_uuid"] for row in _rows(steps, 64)}
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    steps.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 30})
    _inventory(steps, "minecraft:arrow", 0)
    response: dict[str, Any] = {}
    arrow_peak: dict[str, float] = {}
    def deflected(_: dict[str, Any]) -> bool:
        rows = _rows(steps, 64)
        arrows = []
        for row in rows:
            if row["entity_type"] != "minecraft:arrow" or row["entity_uuid"] in before_ids:
                continue
            peak = arrow_peak.get(row["entity_uuid"], row["z"])
            if row["z"] < peak - .1:
                arrows.append(row)
            arrow_peak[row["entity_uuid"]] = max(peak, row["z"])
        mob = next((row for row in rows if row["entity_uuid"] == identity["entity_uuid"]), None)
        if not arrows or mob is None:
            return False
        distance = math.sqrt(sum((mob[key] - before[key]) ** 2 for key in ("x", "y", "z")))
        if distance <= 2:
            return False
        response.update(arrow=arrows[0], arrow_peak_z=arrow_peak[arrows[0]["entity_uuid"]],
                        enderman=mob, teleport_distance=distance)
        return True
    steps.wait(deflected, "arrow_reversed_and_same_enderman_teleported")
    steps.check("enderman_arrow_did_not_damage", response["enderman"]["health"] == before["health"])
    steps.report["response"] = response
    steps.report["observability_gap"] = "Canonical projectile velocity remains the received launch value; deflection uses same-arrow received position reversal instead."
    _aim(steps, response["enderman"])
    _capture(steps, "p2-enderman-after-teleport")
    steps.call("minecraft_wait_ticks", {"ticks": 40})
    after = next(row for row in _rows(steps, 64) if row["entity_uuid"] == identity["entity_uuid"])
    steps.check("adversarial_no_delayed_arrow_damage", after["health"] == before["health"])


def _fishing(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    trace = getattr(steps.client, "p2_wire_trace", None)
    steps.check("fishing_real_hook_metadata_trace_available", trace is not None)
    x, y, z = origin
    steps.fixture(f"fill {x - 8} {y - 4} {z + 1} {x + 8} {y - 4} {z + 28} minecraft:obsidian")
    steps.fixture(f"fill {x - 8} {y - 3} {z + 1} {x + 8} {y - 1} {z + 28} minecraft:water[level=0]")
    steps.fixture(f"fill {x - 8} {y} {z + 1} {x + 8} {y + 8} {z + 28} minecraft:air")
    _block(steps, _point(origin, dy=-1, dz=10), "minecraft:water", {"level": "0"})
    _give(steps, "minecraft:fishing_rod")
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 20})
    _capture(steps, "p2-fishing-open-water")
    before = steps.observe()
    steps.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 1})
    hook = steps.call("minecraft_wait_for_visible_entity", {"entity_type": "minecraft:fishing_bobber", "radius": 32, "timeout_seconds": steps.remaining()})
    identity = {key: hook[key] for key in ("entity_id", "entity_uuid", "entity_type")}
    steps.report["hook"] = identity
    _capture(steps, "p2-fishing-cast")
    floating: dict[str, Any] = {}
    def in_source_water(_: dict[str, Any]) -> bool:
        row = next(row for row in _rows(steps) if row["entity_uuid"] == identity["entity_uuid"])
        point = {key: math.floor(row[key]) for key in ("x", "y", "z")}
        block = steps.call("minecraft_read_block", point)
        if block["block_id"] != "minecraft:water" or block["properties"].get("level") != "0":
            return False
        floating.update(row)
        return True
    steps.wait(in_source_water, "hook_entered_received_source_water")
    row = floating
    bx, by, bz = (math.floor(row[key]) for key in ("x", "y", "z"))
    scan = steps.call("minecraft_scan_blocks", {"min_x": bx - 2, "min_y": by - 1, "min_z": bz - 2,
                                                "max_x": bx + 2, "max_y": by + 2, "max_z": bz + 2})
    steps.report["open_water_scan"] = scan
    steps.check("hook_has_open_source_water_and_air_layers",
                len(scan["blocks"]) == 100 and all(block["block_id"] in ("minecraft:water", "minecraft:air")
                and (block["block_id"] != "minecraft:water" or block["properties"].get("level") == "0") for block in scan["blocks"]))
    _capture(steps, "p2-fishing-floating-before-bite-wait")
    def biting(_: dict[str, Any]) -> bool:
        row = trace.biting.get(identity["entity_id"])
        if row is not None and row.get("hook_biting") is True:
            steps.report["bite_wire_receipt"] = row
            return True
        return False
    steps.wait(biting, "same_real_hook_received_biting_metadata")
    steps.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 1})
    steps.call("minecraft_wait_for_entity_removed", {**identity, "timeout_seconds": steps.remaining()})
    final = steps.wait(lambda state: sum(stack.get("count") or 0 for stack in state["inventory"]) >
                       sum(stack.get("count") or 0 for stack in before["inventory"]), "caught_item_landed_in_player_inventory")
    steps.report["catch_inventory"] = final["inventory"]
    steps.check("real_rod_retained", item_count(final, "minecraft:fishing_rod") >= 1)
    steps.call("minecraft_open_inventory")
    _capture(steps, "p2-fishing-catch-inventory")
    steps.call("minecraft_close_screen")
    steps.wait(lambda state: not state["screen"]["open"], "fishing_inventory_closed_before_recast")
    _echo(steps, "recast_ready_ordered_server_receipt")
    # An immediate second cast/reel cannot award a catch before a real bite.
    steps.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 1})
    early = steps.call("minecraft_wait_for_visible_entity", {"entity_type": "minecraft:fishing_bobber", "radius": 32, "timeout_seconds": steps.remaining()})
    early_identity = {key: early[key] for key in ("entity_id", "entity_uuid", "entity_type")}
    steps.check("adversarial_new_hook_uuid", early_identity["entity_uuid"] != identity["entity_uuid"])
    steps.check("adversarial_early_hook_not_biting", not trace.biting.get(early_identity["entity_id"], {}).get("hook_biting", False))
    steps.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 1})
    steps.call("minecraft_wait_for_entity_removed", {**early_identity, "timeout_seconds": steps.remaining()})
    _echo(steps, "early_reel_server_receipt")
    steps.check("adversarial_early_reel_no_free_catch",
                steps.observe()["inventory"] == final["inventory"])
    steps.report["observability_gap"] = "Canonical native hook projection omits biting; unmodified forwarded metadata supplies only the bite wait, never cast/reel/reward actions."


def _creaking(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    x, y, z = origin
    steps.fixture(f"fill {x - 4} {y - 1} {z + 3} {x + 4} {y - 1} {z + 9} minecraft:obsidian")
    steps.fixture(f"fill {x - 4} {y} {z + 3} {x + 4} {y + 4} {z + 9} minecraft:air")
    steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 0})
    steps.fixture(f"summon minecraft:creaking {x + .5} {y} {z + 5.5}")
    target = steps.call("minecraft_wait_for_visible_entity", {"entity_type": "minecraft:creaking", "radius": 16, "timeout_seconds": steps.remaining()})
    identity = {key: target[key] for key in ("entity_id", "entity_uuid", "entity_type")}
    steps.report["entity"] = identity
    before = next(row for row in _rows(steps) if row["entity_uuid"] == identity["entity_uuid"])
    yaw, _ = _aim(steps, before)
    _capture(steps, "p2-creaking-looking-at")
    steps.call("minecraft_wait_ticks", {"ticks": 40})
    frozen = next(row for row in _rows(steps) if row["entity_uuid"] == identity["entity_uuid"])
    distance = lambda a, b: math.sqrt(sum((a[key] - b[key]) ** 2 for key in ("x", "y", "z")))
    steps.check("creaking_frozen_while_observed", distance(before, frozen) < .05)
    steps.call("minecraft_look", {"yaw_deg": (yaw + 180) % 360, "pitch_deg": 0})
    moved: dict[str, Any] = {}
    def walking(_: dict[str, Any]) -> bool:
        row = next(row for row in _rows(steps) if row["entity_uuid"] == identity["entity_uuid"])
        if distance(frozen, row) < .5:
            return False
        moved.update(row)
        return True
    steps.wait(walking, "creaking_moves_when_player_looks_away")
    _capture(steps, "p2-creaking-looking-away")
    _aim(steps, moved)
    _echo(steps, "creaking_second_gaze_receipt")
    again = next(row for row in _rows(steps) if row["entity_uuid"] == identity["entity_uuid"])
    steps.call("minecraft_wait_ticks", {"ticks": 40})
    refrozen = next(row for row in _rows(steps) if row["entity_uuid"] == identity["entity_uuid"])
    steps.check("adversarial_creaking_refreezes_on_second_gaze", distance(again, refrozen) < .05)
    steps.report["poses"] = {"before": before, "frozen": frozen, "moved": moved, "refrozen": refrozen}
    _capture(steps, "p2-creaking-refrozen-closer")


def _helmet(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    _give(steps, "minecraft:golden_helmet", 0)
    _give(steps, "minecraft:iron_helmet", 1)
    steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": -20})
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    steps.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 1})
    equipped = steps.wait(lambda state: any(stack.get("slot") == 39 and
                          stack.get("item_id") == "minecraft:golden_helmet" and stack.get("count") == 1
                          for stack in state["inventory"]), "air_use_gold_helmet_equipped")
    steps.check("air_use_helmet_debited_from_hand", not any(stack.get("slot") == 0 and stack.get("count", 0)
                                                        for stack in equipped["inventory"]))
    # Vanilla use_delay is four real client ticks (player/interaction.rs).
    steps.call("minecraft_wait_ticks", {"ticks": 4})
    steps.call("minecraft_set_hotbar_slot", {"slot": 1})
    steps.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 1})
    swapped = steps.wait(lambda state: any(stack.get("slot") == 39 and stack.get("item_id") == "minecraft:iron_helmet"
                         and stack.get("count") == 1 for stack in state["inventory"]), "occupied_head_slot_swapped_by_air_use")
    steps.check("old_helmet_returns_to_used_hand", any(stack.get("slot") == 1 and
                stack.get("item_id") == "minecraft:golden_helmet" and stack.get("count") == 1 for stack in swapped["inventory"]))
    steps.report["swapped_inventory"] = swapped["inventory"]
    steps.call("minecraft_open_inventory")
    _capture(steps, "p1-helmet-air-use-swapped")
    steps.call("minecraft_close_screen")
    steps.wait(lambda state: not state["screen"]["open"], "helmet_inventory_closed")
    steps.call("minecraft_set_hotbar_slot", {"slot": 8})
    steps.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 1})
    steps.call("minecraft_wait_ticks", {"ticks": 1})
    steps.call("minecraft_press_inputs", {"keys": ["use"], "ticks": 1})
    final = _echo(steps, "helmet_empty_repeat_server_receipt")
    steps.check("adversarial_helmet_exact_custody_after_empty_repeat",
                item_count(final, "minecraft:golden_helmet") == item_count(final, "minecraft:iron_helmet") == 1)


def _players(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    from . import native_focus

    peer, display, actor_pid, observer_pid = steps.client.p1_observer
    trace = steps.client.p1_wire_trace
    def focus(owner: ProbeSteps, pid: int, label: str) -> None:
        native_focus.acquire(display, pid, owner.remaining())
        tick = int(owner.observe()["client_tick"])
        owner.wait(lambda state: int(state["client_tick"]) > tick, f"{label}_native_event_tick")
        def client_focused(_: dict[str, Any]) -> bool:
            metadata = native_client.capture(owner.client, owner.artifact_dir.parent, label)
            owner.report.setdefault("screenshots", []).append(metadata)
            return metadata["window"]["focused"] is True
        owner.wait(client_focused, label)
    x, y, z = origin
    steps.fixture(f"tp HarnessObserver {x + .5} {y} {z + 4.5}")
    _give(steps, "minecraft:diamond_sword", 0)
    steps.call("minecraft_set_hotbar_slot", {"slot": 0})
    steps.artifact_dir.mkdir(parents=True, exist_ok=True)
    with ProbeSteps(peer, steps.artifact_dir / "observer", steps.remaining(), "native-v2-p1-player-observer-peer") as observer:
        observer.wait(lambda state: abs(state["player"]["x"] - x - .5) < .3 and
                      abs(state["player"]["z"] - z - 4.5) < .3, "observer_setup_teleport_received")
        observer_uuid = observer.observe()["player"]["entity_uuid"]
        actor: dict[str, Any] = {}
        def sees_actor(_: dict[str, Any]) -> bool:
            row = next((row for row in _rows(observer) if row["entity_uuid"] == steps.report["player_uuid"]), None)
            if row is None:
                return False
            actor.update(row)
            return True
        observer.wait(sees_actor, "observer_received_same_actor_uuid")
        observer.check("actor_is_received_minecraft_player", actor["entity_type"] == "minecraft:player")
        observer.report["actor"] = actor
        held: dict[str, Any] = {}
        def sword_packet(_: dict[str, Any]) -> bool:
            row = next((row for row in reversed(trace.equipment) if
                        trace.players.get(row["connection"], {}).get("entity_uuid") == observer_uuid and
                        row["entity_id"] == actor["entity_id"] and row["first_slot"] == 0 and row["first_stack_count"] == 1), None)
            if row is None:
                return False
            held.update(row)
            return True
        observer.wait(sword_packet, "observer_stream_received_held_item_equipment")
        observer.report["held_equipment_wire_receipt"] = held
        focus(observer, observer_pid, "p1-observer-focus-for-held-sword")
        _aim(observer, actor)
        _capture(observer, "p1-player-observer-held-sword")
        focus(steps, actor_pid, "p1-actor-refocus-before-slot-change")
        steps.call("minecraft_set_hotbar_slot", {"slot": 8})
        observer.wait(lambda _: any(row["connection"] == held["connection"] and row["entity_id"] == actor["entity_id"]
                      and row["sequence"] > held["sequence"] and row["first_slot"] == 0
                      and row["first_stack_count"] == 0 for row in trace.equipment), "adversarial_actor_slot_change_clears_observer_hand")
        focus(observer, observer_pid, "p1-observer-refocus-for-empty-hand")
        _capture(observer, "p1-player-observer-empty-hand")
    steps.report["observer"] = observer.report
    focus(steps, actor_pid, "p1-actor-final-focus")
    steps.report["observability_gap"] = "client_observe is not exposed by this native build; unmodified observer-stream equipment is supplemental to actual player/held-item screenshots."


def run(steps: ProbeSteps, origin: tuple[int, int, int], surface: str) -> None:
    steps.report["surface"] = surface
    steps.report["required_tools"] = ["client_capture", "minecraft_interact_entity", "minecraft_approach_entity", "minecraft_break_block", "minecraft_select_hotbar_item", "minecraft_look_at_block"]
    available = {tool["name"] for tool in steps.client.list_tools()}
    steps.check("p1_tools_available", set(steps.report["required_tools"]) <= available)
    steps.fixture("gamemode survival @s")
    _echo(steps, "survival_fixture_receipt")
    if surface in ("slime", "enderman", "fishing", "creaking"):
        _capture(steps, f"p2-{surface}-before-fixture")
    try:
        {"armor-stand": _armor_stand, "signs": _signs, "piglin": _piglin,
         "trader": _trader, "leaves": _leaves, "helmet-equip": _helmet, "player-observer": _players,
         "slime": _slime, "enderman": _enderman, "fishing": _fishing, "creaking": _creaking}[surface](steps, origin)
    except Exception:
        # Read-only observation preserves the actual terminal state; the prior
        # completed GPU captures are retained even if a failed owner revoked control.
        try:
            steps.report["failure_observation"] = steps.call("minecraft_observe")
        except Exception as error:
            steps.report["failure_observation_error"] = str(error)
        raise
