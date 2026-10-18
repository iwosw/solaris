"""Public-wire fixtures and fail-closed work oracles for the twelve controls.

Recipes are pure declarations, not offline world writers.  The preparation
regions require a received, complete view-distance-eight chunk window before
commands execute.  Inventory stock is inserted by real menu quick-moves; the
finite preparation clock is separate from the fixed measurement windows.
Natural simulation is never paused, accelerated, refilled, or extended until a
work floor passes.  A short window with no observed work fails that floor.
"""
from __future__ import annotations

import json
import hashlib
import math
from typing import Any
import uuid

CONTROL_IDS = (
    "dense-hostile", "spread-passive", "explore", "built-base", "fluids",
    "village", "combat", "projectiles", "farms", "plugin-heavy",
    "checkpoint-restart", "long-session",
)
RECIPE_IDS = CONTROL_IDS
_NAMESPACE = uuid.UUID("43a20e67-dceb-534d-9e84-b559c8ac013a")
_PASSIVE = ("cow", "sheep", "pig", "chicken")
_CROPS = ("wheat", "carrots", "potatoes", "beetroots")
_VILLAGER_CUSTOM_NAME = '{"text":"solaris-catalog-villager"}'


def _spec() -> dict[str, Any]:
    return {"schema_version": 1, "entities": [], "blocks": [], "containers": [],
            "actions": [], "action_hz": 4, "burst_action_hz": 8}


def _command(text: str, bot: int = 0, **timing: Any) -> dict[str, Any]:
    return {"kind": "command", "bot": bot, "command": text, **timing}


def _arena(anchor: list[int]) -> list[str]:
    # Exactly the legacy 15x15-chunk envelope; no raised fill-limit gamerule.
    x0 = (anchor[0] // 16 - 7) * 16
    z0 = (anchor[1] // 16 - 7) * 16
    commands = []
    for z in range(z0, z0 + 240, 16):
        for x in range(x0, x0 + 240, 16):
            commands.extend((
                f"fill {x} 58 {z} {x + 15} 58 {z + 15} minecraft:bedrock",
                f"fill {x} 59 {z} {x + 15} 62 {z + 15} minecraft:dirt",
                f"fill {x} 63 {z} {x + 15} 63 {z + 15} minecraft:grass_block",
                f"fill {x} 64 {z} {x + 15} 191 {z + 15} minecraft:air",
                f"fill {x} 192 {z} {x + 15} 319 {z + 15} minecraft:air",
            ))
    return commands


def _block(result: dict[str, Any], name: str, position: list[int],
           kind: str, properties: str = "", *, stable: bool = False) -> None:
    result["commands"].append(
        f"setblock {' '.join(map(str, position))} minecraft:{kind}{properties}")
    result["mechanic_spec"]["blocks"].append({"name": name, "position": position})
    if stable:
        result["expected"]["stable_blocks"][name] = "minecraft:" + kind
        if properties:
            result["expected"].setdefault("stable_properties", {})[name] = dict(
                pair.split("=", 1) for pair in properties[1:-1].split(","))


def _mob(result: dict[str, Any], kind: str, position: list[float],
         *, phase: str | None = None, offset: int = 0, motion: bool = False) -> None:
    expected = result["expected"]["entities"].setdefault("minecraft:" + kind, [])
    identity = str(uuid.uuid5(_NAMESPACE, f"{result['id']}:{kind}:{len(expected)}"))
    expected.append(identity)
    tag = '{UUID:"' + identity + '"'
    if kind == "villager":
        tag += ",CustomName:" + json.dumps(_VILLAGER_CUSTOM_NAME)
    tag += (",Motion:[0.0d,0.0d,-1.5d]" if motion else "") + "}"
    command = f"summon minecraft:{kind} {' '.join(map(str, position))} {tag}"
    if phase is None:
        result["commands"].append(command)
    else:
        result["mechanic_spec"]["actions"].append(
            _command(command, window=phase, at_ms=offset))


def _herd(result: dict[str, Any], anchor: list[int], count: int, hostile: bool) -> None:
    width, spacing = (32, 1.25) if hostile else (16, 3)
    for n in range(count):
        kind = ("zombie" if n % 2 == 0 else "husk") if hostile else _PASSIVE[n % 4]
        _mob(result, kind, [anchor[0] - 23.5 + (n % width) * spacing,
                            64.0, anchor[1] + 16.5 + (n // width) * spacing])


def _base(result: dict[str, Any], clients: int) -> None:
    result["expected"]["families"].append("base")
    result["expected"]["stocks"] = {"furnaces": 64, "raw_iron": 4096,
        "coal": 4096, "hoppers": 128, "hopper_dirt": 4096,
        "loose_dirt_stacks": 64, "loose_dirt_items": 4096, "buttons": 512}
    prep = result["preparation_spec"]
    result["mechanic_spec"].update(action_hz=8, burst_action_hz=16)
    clock = 0
    cursors: dict[tuple[int, str], int] = {}
    for n in range(64):
        x, z = -64 + (n % 8) * 8, -64 + (n // 8) * 8
        owner = n % clients
        furnace = f"furnace_{n}"
        containers = [
            {"name": furnace, "position": [x, 64, z], "bot": owner, "slots": 3},
            {"name": f"hopper_{n}_a", "position": [x + 2, 64, z], "bot": owner, "slots": 5},
            {"name": f"hopper_{n}_b", "position": [x + 3, 64, z], "bot": owner, "slots": 5},
        ]
        result["mechanic_spec"]["containers"].extend(containers)
        prep["containers"].extend({**container, "bot": 0} for container in containers)
        _block(result, furnace, [x, 64, z], "furnace")
        _block(result, f"hopper_{n}_a", [x + 2, 64, z], "hopper", "[facing=east,enabled=true]")
        _block(result, f"hopper_{n}_b", [x + 3, 64, z], "hopper", "[facing=west,enabled=true]")
        # No block-entity NBT or summoned Item SNBT.  Clear only the setup actor;
        # give places these two stacks in hotbar 0/1, menu slots 30/31.
        steps = [
            (0, _command("clear @s")),
            (250, _command(f"tp @s {x + 1.5} 64 {z + 0.5}")),
            (750, _command("give @s minecraft:raw_iron 64")),
            (1000, _command("give @s minecraft:coal 64")),
            (1500, {"kind": "open_container", "position": [x, 64, z]}),
            (2250, {"kind": "click_slot", "slot": 30, "button": 0, "mode": 1}),
            (3000, {"kind": "click_slot", "slot": 31, "button": 0, "mode": 1}),
            (3750, {"kind": "close_container"}),
            (4000, _command("give @s minecraft:dirt 64")),
            (4500, {"kind": "open_container", "position": [x + 2, 64, z]}),
            (5250, {"kind": "click_slot", "slot": 32, "button": 0, "mode": 1}),
            (6000, {"kind": "close_container"}),
            (6250, _command(f"tp @s {x + 2.5} 66 {z + 0.5}")),
            (6500, _command("give @s minecraft:dirt 64")),
            (6750, {"kind": "select_slot", "slot": 0}),
            (7000, {"kind": "set_rotation", "yaw": 0, "pitch": 90}),
            (7500, {"kind": "drop_stack"}),
            (8250, _command(f"tp @s {x + 1.5} 64 {z + 0.5}")),
            (8500, {"kind": "open_container", "position": [x + 2, 64, z]}),
            (8700, {"kind": "close_container"}),
            (8950, {"kind": "open_container", "position": [x + 3, 64, z]}),
            (9150, {"kind": "close_container"}),
            (9400, {"kind": "open_container", "position": [x, 64, z]}),
            (9700, {"kind": "close_container"}),
        ]
        prep["actions"].extend({"bot": 0, **action, "window": "steady", "at_ms": clock + at}
                               for at, action in steps)
        clock += 10000
        # A received menu, not a remote synthetic inventory query.  Probe all
        # modules, distributed over the actual requested clients.
        for phase, interval in (("steady", 125), ("burst", 63)):
            cursor = cursors.get((owner, phase), 0)
            # Transport is fixed wall time, not a wait for a teleport response.
            # Give the authoritative pose time to arrive before the first use;
            # one central pose reaches the entire four-button group.
            result["mechanic_spec"]["actions"].append({
                **_command(f"tp @s {x + 1.5} 64 {z + 0.5}", owner),
                "window": phase, "at_ms": cursor})
            cursor += 250
            # Opens keep their fixed wall slots. A close is offered 200ms later,
            # but its single packet follows matching received menu content if
            # that content is late; neither later offers nor windows are shifted.
            for container in (containers[0], containers[2], containers[1]):
                result["mechanic_spec"]["actions"].append({
                    "bot": owner, "kind": "open_container", "position": container["position"],
                    "window": phase, "at_ms": cursor})
                cursor += 200
                result["mechanic_spec"]["actions"].append({
                    "bot": owner, "kind": "close_container",
                    "window": phase, "at_ms": cursor})
                cursor += 63
            button_start = 0 if phase == "steady" else 4
            result["mechanic_spec"]["actions"].append({
                **_command(f"tp @s {x + 4.5} 64 {z + button_start + 2.0}", owner),
                "window": phase, "at_ms": cursor})
            cursor += 250
            for button in range(button_start, button_start + 4):
                result["mechanic_spec"]["actions"].append({
                    "bot": owner, "kind": "use_block", "position": [x + 5, 64, z + button],
                    "window": phase, "at_ms": cursor})
                cursor += interval
            cursors[owner, phase] = cursor
        for button in range(8):
            bp = [x + 5, 64, z + button]
            _block(result, f"button_{n}_{button}", bp, "stone_button", "[face=floor,powered=false]")
            _block(result, f"wire_{n}_{button}", [x + 6, 64, z + button], "redstone_wire", "[power=0]")
            _block(result, f"lamp_{n}_{button}", [x + 7, 64, z + button], "redstone_lamp", "[lit=false]")
    result["preparation_duration_ms"] = clock + 2000
    result["constraints"]["minimum_window_ms"] = {
        phase: max((value for (owner, window), value in cursors.items() if window == phase), default=0)
        for phase in ("steady", "burst")}
    result["constraints"]["finite_stock"] = (
        "64 inputs/fuels per furnace; 64 seeded dirt plus one 64-item public drop per hopper pair. "
        "Preparation consumes fuel/cooks/picks up naturally; input exhaustion is not repaired. "
        "512 buttons are pressed once, half per active window; ordinary 20-tick release.")


def _farms(result: dict[str, Any]) -> None:
    result["expected"]["families"].append("farm")
    crops = result["expected"]["crops"] = {}
    for x in range(-80, 80):
        if x % 9 == 0:
            result["commands"].append(f"fill {x} 63 16 {x} 63 95 minecraft:water[level=0]")
        else:
            result["commands"].append(f"fill {x} 63 16 {x} 63 95 minecraft:farmland[moisture=7]")
            for z in range(16, 96):
                kind = _CROPS[(x + z) % 4]
                name = f"crop_{x}_{z}"
                _block(result, name, [x, 64, z], kind, "[age=0]")
                crops[name] = "minecraft:" + kind
                result["mechanic_spec"]["blocks"].append(
                    {"name": f"soil_{x}_{z}", "position": [x, 63, z]})
    result["constraints"]["natural_growth"] = (
        "11440 age-zero crops over 143 irrigated columns, natural random_tick_speed=3. "
        "Preparation/readiness may already grow crops. No fixed RNG growth count or "
        "maximum preparation age is promised; measured age increases must be received.")


def _village(result: dict[str, Any], origin: list[int], spacing: int) -> None:
    result["expected"]["families"].append("village")
    poi_positions = result["expected"].setdefault("poi_positions", {"home": [], "job_site": []})
    for n in range(64):
        x, z = origin[0] + (n % 8) * spacing, origin[1] + (n // 8) * spacing
        _block(result, f"job_{n}", [x + 4, 64, z + 4], "composter", stable=True)
        poi_positions["job_site"].append([x + 4, 64, z + 4])
        _block(result, f"bell_{n}", [x + 8, 64, z + 8], "bell", stable=True)
        for dx, part in ((0, "foot"), (1, "head")):
            _block(result, f"bed_{n}_{part}", [x + dx, 64, z], "red_bed",
                   f"[facing=east,part={part}]", stable=True)
            poi_positions["home"].append([x + dx, 64, z])
        _mob(result, "villager", [x + 2.5, 64.0, z + 2.5])
    result["constraints"]["village_acquisition"] = (
        "Ordinary adult villagers naturally acquire the 64 composters/beds/bells; "
        "no unavailable VillagerData or Brain summon NBT. Farmer metadata must be "
        "received. A 50-second control is not a full day/work/rest/sleep oracle.")


def recipe(scenario_id: str, clients: int, smoke: bool) -> dict[str, Any] | None:
    """Return a fresh, JSON-serializable recipe; unknown IDs are not owned here."""
    if scenario_id not in CONTROL_IDS:
        return None
    if isinstance(clients, bool) or not isinstance(clients, int) or clients < 1:
        raise ValueError("clients must be a positive integer")
    if not isinstance(smoke, bool):
        raise ValueError("smoke must be boolean")
    anchors = [[0, 0], [256, 0], [0, 256], [256, 256]] if scenario_id == "spread-passive" else [[0, 0]]
    result: dict[str, Any] = {
        "id": scenario_id, "clients": clients, "smoke": smoke,
        "view_distance": 6,
        "commands": ["gamerule doMobSpawning false", "gamerule doDaylightCycle false",
                     "gamerule doWeatherCycle false", "gamerule randomTickSpeed 3",
                     "weather clear", "time set 6000", "difficulty easy", "setworldspawn 0 64 0"],
        "anchors": anchors, "group_size": max(1, clients // 4) if len(anchors) == 4 else clients,
        "client_args": ["--no-edits"], "client_roles": [{"role": "resident", "bots": list(range(clients))}],
        "mechanic_spec": _spec(), "preparation_spec": _spec(), "preparation_duration_ms": 0,
        "schedule_overrides": {"steady": {"movement_hz": 20, "edit_hz": 0, "chat_hz": 0, "command_hz": 0},
                               "burst": {"movement_hz": 20, "edit_hz": 0, "chat_hz": 0, "command_hz": 0},
                               "recovery": {"movement_hz": 0, "edit_hz": 0, "chat_hz": 0, "command_hz": 0}},
        "restart_check": scenario_id == "checkpoint-restart", "constraints": {"minimum_clients": len(anchors)},
        "expected": {"families": [], "entities": {}, "stable_blocks": {}, "clients": clients},
        "source_evidence": ["crates/mc-test-harness/src/bin/load_matrix_fixtures.rs:148-499",
                            "crates/mc-core/src/command_world.rs:60-63",
                            "crates/mc-core/src/command_summon.rs:69-107"],
    }
    for anchor in anchors:
        result["commands"].extend(_arena(anchor))
    for n, anchor in enumerate(anchors):
        _block(result, f"marker_{n}", [anchor[0], 58, anchor[1]], "bedrock", stable=True)
    if scenario_id == "dense-hostile":
        _herd(result, [0, 0], 1024, True)
        result["expected"]["families"] += ["entities", "hostile"]
    elif scenario_id == "spread-passive":
        for anchor in anchors:
            _herd(result, anchor, 256, False)
        result["expected"]["families"].append("entities")
    elif scenario_id == "explore":
        result["client_args"].append("--explore")
        result["expected"]["families"].append("explore")
    elif scenario_id in ("built-base", "checkpoint-restart", "long-session"):
        _base(result, clients)
        if scenario_id == "built-base":
            result["client_args"] = []
            for phase in ("steady", "burst"):
                result["schedule_overrides"][phase]["edit_hz"] = 1
            result["expected"]["families"].append("edits")
        else:
            _herd(result, [0, 0], 128 if scenario_id == "checkpoint-restart" else 256, False)
            result["expected"]["families"].append("entities")
            if scenario_id == "checkpoint-restart":
                planting_start = len(result["commands"])
                _farms(result)
                planting_commands = result["commands"][planting_start:]
                del result["commands"][planting_start:]
                result["post_preparation_regions"] = [{
                    "name": "fresh_checkpoint_farms", "anchor": [0, 0],
                    "view_distance": 8, "commands": planting_commands,
                }]
                result["constraints"]["planting_phase"] = (
                    "Plant the full11440 age-zero field once AFTER finite menu stocking; "
                    "otherwise640-second preparation would spend natural crop growth before measurement.")
                _village(result, [8, -96], 12)
                result["expected"]["families"].append("restart")
                for phase in ("steady", "burst"):
                    result["schedule_overrides"][phase].update(command_hz=0, command_interval_ms=10000, commands=["save-all flush"])
            else:
                result["expected"]["families"].append("long")
                result["constraints"]["minimum_measurement_ms"] = 0 if smoke else 600_000
    elif scenario_id == "fluids":
        result["expected"]["families"].append("fluid")
        sources = result["expected"]["fluid_sources"] = {}
        for n in range(128):
            x, z = -80 + (n % 16) * 10, 16 + (n // 16) * 10
            kind = "water" if n % 2 == 0 else "lava"
            result["commands"].append(f"setblock {x} 69 {z} minecraft:glass")
            # Cap sides too: no fluid work consumed during setup.  Publicly remove
            # finite plugs in the measurement clock; no private scheduled ticks.
            for dx, dz in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                result["commands"].append(f"setblock {x + dx} 70 {z + dz} minecraft:glass")
            _block(result, f"source_{n}", [x, 70, z], kind, "[level=0]")
            for y in range(64, 70):
                result["mechanic_spec"]["blocks"].append({"name": f"flow_{n}_{y}", "position": [x, y, z]})
            phase = "steady" if n < 64 else "burst"
            result["mechanic_spec"]["actions"].append(_command(
                f"setblock {x} 69 {z} minecraft:air", window=phase, at_ms=(n % 64) * 125))
            sources[f"source_{n}"] = "minecraft:" + kind
        result["constraints"]["finite_fluid_releases"] = "128 capped elevated sources; 64 one-shot bottom-plug releases per active wall window. Natural fluid cadence/topology; no injection of legacy tick deadlines."
        result["constraints"]["minimum_window_ms"] = {"steady": 12_000, "burst": 10_000}
    elif scenario_id in ("village", "plugin-heavy"):
        _village(result, [-64, -64], 16)
        result["expected"]["families"].append("entities")
        result["commands"] += ["time set 2000", "gamerule doDaylightCycle true"]
        if scenario_id == "plugin-heavy":
            result["required_plugins"] = ["solaris-towns", "solaris-economy"]
            result["constraints"].update(minimum_clients=2,
                minimum_window_ms={"steady": 12_000, "burst": 8_000})
            result["scope_notes"] = [
                "Two public-command actors share the ordinary gameplay clock; remaining bots walk.",
                "Town/claim and exact balance readbacks are state queries, not command echoes.",
                "Operator teleports, item grants and initial balances are declared fixture assistance.",
            ]
            # The setup identity differs from both measured actors. Reopening
            # this world must recover the named town and both seeded balances.
            result["preparation_spec"]["actions"] += [
                _command("town create load_matrix", window="steady", at_ms=0,
                         expected_feedback="Town load_matrix created."),
                _command("town claim", window="steady", at_ms=1000,
                         expected_feedback="Chunk claimed for load_matrix."),
            ]
            for bot in range(2):
                identity = str(uuid.UUID(bytes=hashlib.md5(
                    f"OfflinePlayer:scale_{bot:04}".encode()).digest(), version=3))
                result["preparation_spec"]["actions"].append(_command(
                    f"econadmin set {identity} 100", window="steady", at_ms=2000 + bot * 1000,
                    expected_feedback="Balance set to 100."))
            result["preparation_duration_ms"] = 5000
            result["expected"]["families"] += ["plugin", "plugin_inventory"]
            result["expected"]["town_name"] = "load_matrix"
            result["mechanic_spec"].update(action_hz=8, burst_action_hz=8)
            actions = result["mechanic_spec"]["actions"]
            for bot in range(2):
                town, x = f"load_actor{bot}", 32 + bot * 32
                _block(result, f"plugin_chest_{bot}", [x, 64, -96], "chest")
                result["mechanic_spec"]["containers"].append({
                    "name": f"plugin_chest_{bot}", "position": [x, 64, -96], "bot": bot, "slots": 27})
                actions += [
                    _command(f"town create {town}", bot, window="steady", at_ms=bot * 1000,
                             expected_feedback=f"Town {town} created."),
                    _command(f"tp @s {x + 0.5} 64 -94.5", bot, window="steady", at_ms=2000),
                    _command("town claim", bot, window="steady", at_ms=3000 + bot * 1000,
                             expected_feedback=f"Chunk claimed for {town}."),
                    # Assisted scale login grants its default dirt stack.
                    # Declare and remove that stock before the one-item ledger.
                    _command("clear @s", bot, window="steady", at_ms=4250),
                    _command("give @s minecraft:diamond 1", bot, window="steady", at_ms=4500),
                ]
                for phase in ("steady", "burst"):
                    start = 6000 if phase == "steady" else 0
                    actions.append(_command(f"tp @s {x + 0.5} 64 -94.5", bot,
                                            window=phase, at_ms=start))
                    for offset, action in [
                        (500, {"kind": "open_container", "position": [x, 64, -96]}),
                        # Chest -> player quick-move fills the reverse inventory
                        # range, so the returned diamond occupies hotbar eight.
                        (1000, {"kind": "click_slot", "slot": 54 if phase == "steady" else 62,
                                "button": 0, "mode": 1}),
                        (1500, {"kind": "click_slot", "slot": 0, "button": 0, "mode": 1}),
                        (2000, {"kind": "close_container"}),
                    ]:
                        actions.append({"bot": bot, **action, "window": phase, "at_ms": start + offset})
                    for repetition in range(4):
                        for offset, command, feedback in [
                            (0, f"town info {town}", f"{town}: 1 members, 1 claims."),
                            (250, "money", f"Balance: {93 if bot == 0 else 107} coins."),
                        ]:
                            actions.append(_command(command, bot, window=phase,
                                at_ms=start + 2500 + repetition * 500 + offset,
                                expected_feedback=feedback))
            recipient = str(uuid.UUID(bytes=hashlib.md5(
                b"OfflinePlayer:scale_0001").digest(), version=3))
            actions += [
                _command(f"pay {recipient} 7 load_payment", window="steady", at_ms=5000,
                         expected_feedback="Paid 7 coins."),
                _command(f"pay {recipient} 7 load_payment", window="burst", at_ms=2100,
                         expected_feedback="That transfer token was already committed."),
                _command("town info load_matrix", window="steady", at_ms=5500,
                         expected_feedback="load_matrix: 1 members, 1 claims."),
                _command("town info load_matrix", window="burst", at_ms=2250,
                         expected_feedback="load_matrix: 1 members, 1 claims."),
            ]
            result["source_evidence"] += [
                "../solaris-default-plugins/sources/solaris-towns/src/lib.rs",
                "../solaris-default-plugins/sources/solaris-economy/src/lib.rs"]
    elif scenario_id == "combat":
        _herd(result, [0, 0], 128, True)
        result["expected"]["families"] += ["entities", "hostile", "combat"]
        for n in range(96):
            x, z = -80 + (n % 12) * 13, 24 + (n // 12) * 10
            _block(result, f"chain_tnt_{n}", [x + 2, 64, z], "tnt")
            _mob(result, "tnt", [x + 0.5, 64.0, z + 0.5],
                 phase="steady" if n < 48 else "burst", offset=(n % 48) * 125)
        result["constraints"]["finite_tnt"] = "96 ordinary default-fuse primed TNT commands, 48 per active window; 96 finite chain targets. Fuse SNBT unsupported; no legacy deadline equivalence."
        for phase in ("steady", "burst"):
            result["schedule_overrides"][phase].update(command_hz=0, command_interval_ms=6000, commands=["summon minecraft:creeper 8 64 8"])
    elif scenario_id == "projectiles":
        result["expected"]["families"] += ["entities", "hostile", "projectile"]
        for n in range(128):
            _mob(result, "skeleton", [-48.0 + (n % 16) * 6, 64.0, 16.0 + (n // 16) * 4])
            _mob(result, "arrow", [-48.0 + (n % 16) * 6, 74.0 + n // 16, 64.0],
                 phase="steady", offset=n * 25, motion=True)
        result["client_args"].append("--anchor-chunk-walk")
        result["constraints"]["initial_projectiles"] = "128 one-shot moving arrows after readiness; 128 ordinary skeletons continue natural launches. Startup arrows never prove continued skeleton work."
    elif scenario_id == "farms":
        _farms(result)
    if scenario_id in ("dense-hostile", "combat", "projectiles"):
        result["commands"] += ["time set 18000", "gamerule doDaylightCycle false"]
        result["client_args"] += ["--survival-on-run", "--respawn"]
    result["mechanic_spec"]["entities"] = [
        {"kind": kind, "count": len(identities)} for kind, identities in result["expected"]["entities"].items()]
    result["preparation_spec"]["entities"] = result["mechanic_spec"]["entities"]
    # Regions own chunk residency, not world files.  Spread has the same rules,
    # but only the local arena/entities are submitted in each held window.
    if len(anchors) == 1:
        result["preparation_regions"] = [{"name": scenario_id, "anchor": anchors[0],
                                           "view_distance": 8, "commands": result["commands"]}]
    else:
        common = result["commands"][:8]
        result["preparation_regions"] = []
        for index, anchor in enumerate(anchors):
            commands = [*common, *_arena(anchor)]
            marker = f"setblock {anchor[0]} 58 {anchor[1]} minecraft:bedrock"
            commands.append(marker)
            for command in result["commands"]:
                if not command.startswith("summon "):
                    continue
                words = command.split()
                x, z = float(words[2]), float(words[4])
                if anchor[0] - 112 <= x <= anchor[0] + 127 and anchor[1] - 112 <= z <= anchor[1] + 127:
                    commands.append(command)
            result["preparation_regions"].append({"name": f"arena_{index}", "anchor": anchor,
                                                   "view_distance": 8, "commands": commands})
    result["constraints"]["minimum_preparation_ms"] = result["preparation_duration_ms"]
    result["constraints"]["smoke_preserves_density"] = True
    return result




class _Oracle:
    def __init__(self) -> None:
        self.failures: list[str] = []
        self.infra_errors: list[str] = []
        self.completed: dict[str, Any] = {}

    def fail(self, message: str) -> None:
        if message not in self.failures:
            self.failures.append(message)

    def missing(self, message: str) -> None:
        if message not in self.infra_errors:
            self.infra_errors.append(message)

    def number(self, obj: Any, key: str, context: str) -> int | float | None:
        value = obj.get(key) if isinstance(obj, dict) else None
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
            self.missing(f"{context}: missing/invalid {key}")
            return None
        return value

    def result(self) -> dict[str, Any]:
        return {"passed": not self.failures and not self.infra_errors,
                "failures": self.failures, "infra_errors": self.infra_errors,
                "completed": self.completed,
                "oracle": "fresh received identity/physical cell/menu/durable checkpoint state, never packet writes"}


def _named(value: Any) -> dict[str, dict[str, Any]]:
    if isinstance(value, dict):
        return {key: row for key, row in value.items() if isinstance(row, dict)}
    if isinstance(value, list):
        return {row["name"]: row for row in value if isinstance(row, dict) and isinstance(row.get("name"), str)}
    return {}


def _state(oracle: _Oracle, row: dict[str, Any], stage: str, name: str) -> dict[str, Any] | None:
    state = row.get(stage)
    if not isinstance(state, dict) or not isinstance(state.get("name"), str) or not isinstance(state.get("properties"), dict):
        oracle.missing(f"{name}: no fresh decoded {stage} block state")
        return None
    if not isinstance(state.get("state"), int) or oracle.number(state, "state", name) is None:
        oracle.missing(f"{name}: undecoded numeric block identity")
        return None
    if any(not isinstance(key, str) or not isinstance(value, str)
           for key, value in state["properties"].items()):
        oracle.missing(f"{name}: malformed decoded block properties")
        return None
    return state


def _slots(oracle: _Oracle, row: dict[str, Any], stage: str, name: str, limit: int,
           *, slot_offset: int = 0) -> dict[int, tuple[str, int]] | None:
    value = row.get(stage)
    if isinstance(value, dict):
        value = value.get("slots")
    if not isinstance(value, list):
        oracle.missing(f"{name}: missing {stage} slot ledger")
        return None
    slots: dict[int, tuple[str, int]] = {}
    for item in value:
        if not isinstance(item, dict):
            oracle.missing(f"{name}: malformed slot ledger")
            return None
        slot, count = item.get("slot"), item.get("count")
        if isinstance(slot, bool) or not isinstance(slot, int) or not slot_offset <= slot < slot_offset + limit:
            # Menus may additionally report player slots; they are not container stock.
            if isinstance(slot, int) and not isinstance(slot, bool) and slot >= slot_offset + limit:
                continue
            oracle.fail(f"{name}: illegal slot index")
            return None
        if slot in slots:
            oracle.fail(f"{name}: duplicate slot identity")
            return None
        if isinstance(count, bool) or not isinstance(count, int) or not 0 <= count <= 64:
            oracle.fail(f"{name}: illegal slot count")
            return None
        identity = item.get("name")
        if count and (not isinstance(identity, str) or not identity.startswith("minecraft:")):
            oracle.missing(f"{name}: item identity was not decoded")
            return None
        slots[slot] = (identity or "minecraft:air", count)
    if len(slots) != limit:
        oracle.missing(f"{name}: incomplete {stage} slot ledger")
        return None
    return slots


def _entity_samples(oracle: _Oracle, row: dict[str, Any], kind: str) -> dict[str, dict[str, Any]]:
    samples = row.get("samples")
    if not isinstance(samples, list):
        oracle.missing(f"{kind}: UUID lifecycle samples missing")
        return {}
    result = {}
    for sample in samples:
        if not isinstance(sample, dict) or not isinstance(sample.get("uuid"), str):
            oracle.missing(f"{kind}: undecoded entity UUID")
            continue
        identity = sample["uuid"]
        try:
            uuid.UUID(identity)
        except ValueError:
            oracle.fail(f"{kind}: illegal UUID")
            continue
        if identity in result:
            oracle.fail(f"{kind}: duplicate entity identity")
        result[identity] = sample
        for stage in ("initial_position", "final_position"):
            position = sample.get(stage)
            if not isinstance(position, list) or len(position) != 3 or any(
                    isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v) for v in position):
                oracle.missing(f"{kind}: missing received UUID position trace")
            elif abs(position[0]) >= 30_000_000 or abs(position[2]) >= 30_000_000:
                oracle.fail(f"{kind}: position outside world bounds")
    return result


def _base_oracle(oracle: _Oracle, mechanics: dict[str, Any], *, preparation: bool = False) -> None:
    containers = _named(mechanics.get("containers"))
    cooked = 0
    changed_pairs = 0
    for n in range(64):
        name = f"furnace_{n}"
        row = containers.get(name)
        if row is None:
            oracle.missing("base: all 64 received furnace menus required")
            continue
        final = _slots(oracle, row, "final", name, 3)
        initial = None if preparation else _slots(oracle, row, "initial", name, 3)
        if final is not None:
            raw = final[0][1] if final[0][0] == "minecraft:raw_iron" else 0
            output = final[2][1] if final[2][0] == "minecraft:iron_ingot" else 0
            if raw + output != 64:
                oracle.fail("base: finite raw-iron/ingot conservation failed")
            if final[0][1] and final[0][0] != "minecraft:raw_iron" or final[2][1] and final[2][0] != "minecraft:iron_ingot":
                oracle.fail("base: unexpected smelting transformation")
            if final[1][1] and final[1][0] != "minecraft:coal":
                oracle.fail("base: unexpected furnace fuel")
            if initial is not None and not preparation:
                initial_output = initial[2][1] if initial[2][0] == "minecraft:iron_ingot" else 0
                initial_raw = initial[0][1] if initial[0][0] == "minecraft:raw_iron" else 0
                if initial_raw + initial_output != 64:
                    oracle.fail("base: initial received finite smelting stock mismatch")
                if output < initial_output or final[1][1] > initial[1][1]:
                    oracle.fail("base: unexplained smelting reversal/fuel refill")
                cooked += output - initial_output
        pair = []
        for side in ("a", "b"):
            hp = f"hopper_{n}_{side}"
            record = containers.get(hp)
            if record is None:
                oracle.missing("base: all 128 received hopper menus required")
                continue
            after = _slots(oracle, record, "final", hp, 5)
            before = None if preparation else _slots(oracle, record, "initial", hp, 5)
            if after is not None:
                if any(count and identity != "minecraft:dirt" for identity, count in after.values()):
                    oracle.fail("base: circulation changed item identity")
                pair.extend(after.values())
                slot_updates = (oracle.number(record, "container_slot_updates", hp)
                                if "container_slot_updates" in record else 0)
                if before is not None and (before != after or
                        slot_updates is not None and slot_updates > 0):
                    changed_pairs += 1
        # Sequential wire menus are not an atomic transfer snapshot. The
        # observed pair may straddle one transfer; durable validation below
        # requires exact128, so this skew cannot hide lost/duplicated stock.
        if len(pair) == 10:
            dirt = sum(count for identity, count in pair if identity == "minecraft:dirt")
            if not 127 <= dirt <= 129:
                oracle.fail("base: seeded plus dropped dirt debit/credit conservation failed")
    if not preparation:
        oracle.completed.update(cooked_output_delta=cooked, changed_hopper_menus=changed_pairs)
        if cooked <= 0:
            oracle.fail("base: no received new cooked output")
        if changed_pairs <= 0:
            oracle.fail("base: no received circulating hopper transfer")


def _action_oracle(oracle: _Oracle, mechanics: dict[str, Any], spec: dict[str, Any],
                   *, preparation: bool = False) -> None:
    receipts = mechanics.get("actions")
    if not isinstance(receipts, list):
        oracle.missing("finite public action receipts missing")
        return
    by_index = {}
    for row in receipts:
        if not isinstance(row, dict) or isinstance(row.get("action"), bool) or not isinstance(row.get("action"), int):
            oracle.missing("finite action receipt identity malformed")
            continue
        if row["action"] in by_index:
            oracle.fail("duplicate finite action identity")
        by_index[row["action"]] = row
    for index, action in enumerate(spec.get("actions", [])):
        row = by_index.get(index)
        if row is None:
            oracle.missing("finite action list is incompletely accounted")
            continue
        if row.get("kind") != action["kind"] or row.get("bot") != action["bot"]:
            oracle.fail("finite action receipt differs from declared actor/action")
        offered = oracle.number(row, "offered", "finite action")
        admitted = oracle.number(row, "admitted", "finite action")
        written = oracle.number(row, "writes_completed", "finite action")
        if offered != 1 or admitted != 1 or written != 1:
            oracle.fail("finite declared action was not offered/admitted/written exactly once")
        if row.get("errors") or any(row.get(key, 0) for key in
                ("dropped_late", "dropped_unavailable", "dropped_capacity")):
            oracle.fail("finite public action errored or dropped; no repair/refill allowed")
        if row.get("window") == "recovery" and (offered or admitted):
            oracle.fail("recovery offered/admitted new gameplay stimulus")
        # Commands/selection/close have no universal public success packet.
        # Their effects are checked by physical cells, identities and stock,
        # never by substituting writes_completed for a work completion.
        if action["kind"] in ("open_container", "click_slot", "drop_stack", "use_block"):
            completed = oracle.number(row, "completed", "finite action")
            if completed != 1:
                oracle.fail("finite menu/item/block action lacks received completion")
    if preparation:
        containers = _named(mechanics.get("containers"))
        for n in range(64):
            row = containers.get(f"furnace_{n}", {})
            inventory = row.get("initial_player_inventory")
            if not isinstance(inventory, list):
                oracle.missing("preparation: actual granted raw/fuel inventory ledger missing")
                continue
            totals: dict[str, int] = {}
            for slot in inventory:
                if isinstance(slot, dict) and isinstance(slot.get("name"), str):
                    count = oracle.number(slot, "count", "preparation player inventory")
                    if isinstance(count, int):
                        totals[slot["name"]] = totals.get(slot["name"], 0) + count
            if totals.get("minecraft:raw_iron") != 64 or totals.get("minecraft:coal") != 64:
                oracle.fail("preparation: each furnace must receive exactly64 raw iron and64 coal")


def validate_preparation(report: dict[str, Any], recipe: dict[str, Any]) -> dict[str, Any]:
    """Validate real preparation menus, allowing natural cooking/fuel consumption."""
    prepared = recipe
    oracle = _Oracle()
    bots = report.get("bots", report)
    mechanics = bots.get("mechanics") if isinstance(bots, dict) else None
    if not isinstance(mechanics, dict) or mechanics.get("schema_version") != 1:
        oracle.missing("preparation: received mechanic schema missing")
        return oracle.result()
    if "base" in prepared.get("expected", {}).get("families", []):
        _base_oracle(oracle, mechanics, preparation=True)
    _action_oracle(oracle, mechanics, prepared["preparation_spec"],
                   preparation="base" in prepared.get("expected", {}).get("families", []))
    for error in mechanics.get("probe_errors", []):
        oracle.missing(f"preparation probe: {error}")
    return oracle.result()


def _fresh_probe_oracle(oracle: _Oracle, mechanics: dict[str, Any], spec: dict[str, Any]) -> None:
    blocks = _named(mechanics.get("blocks"))
    missing = 0
    for probe in spec["blocks"]:
        row = blocks.get(probe["name"])
        if not isinstance(row, dict) or isinstance(row.get("fresh_reads"), bool) or not isinstance(row.get("fresh_reads"), int) or row["fresh_reads"] <= 0:
            missing += 1
            continue
        if not isinstance(row.get("initial"), dict) or not isinstance(row.get("final"), dict):
            missing += 1
    if missing:
        oracle.missing(f"physical cells: {missing} required probes lack fresh section reads")
    oracle.completed["fresh_physical_cells"] = len(spec["blocks"]) - missing


def _explore_oracle(oracle: _Oracle, mechanics: dict[str, Any], clients: int) -> None:
    """Use snapshot-derived server cache centers, never predicted movement writes."""
    players = mechanics.get("players")
    if not isinstance(players, list) or len(players) != clients:
        oracle.missing("explore: per-client received chunk centers missing")
        return
    transitions = 0
    displacement = 0
    def center(value: Any) -> bool:
        return isinstance(value, list) and len(value) == 2 and all(type(n) is int for n in value)
    for index, player in enumerate(players):
        if not isinstance(player, dict):
            oracle.missing(f"explore client {index}: center receipt missing")
            continue
        initial, final = player.get("initial_chunk_center"), player.get("final_chunk_center")
        trace = player.get("chunk_center_trace")
        if not center(initial) or not center(final) or not isinstance(trace, list):
            oracle.missing(f"explore client {index}: received center baseline/trace missing")
            continue
        count = oracle.number(player, "chunk_center_transitions", f"explore client {index}")
        packets = oracle.number(player, "chunk_center_packets", f"explore client {index}")
        truncated = oracle.number(player, "chunk_center_traces_truncated", f"explore client {index}")
        if truncated:
            oracle.missing(f"explore client {index}: received center trace truncated")
        previous = initial
        for step in trace:
            if (not isinstance(step, dict) or step.get("window") not in ("steady", "burst", "recovery")
                    or not center(step.get("center")) or not center(step.get("previous")) or step["previous"] != previous
                    or step["center"] == previous):
                oracle.fail(f"explore client {index}: invalid received center transition chain")
                break
            previous = step["center"]
        if previous != final or count != len(trace) or (packets is not None and count is not None and packets < count):
            oracle.fail(f"explore client {index}: inconsistent received center accounting")
        if count is not None:
            transitions += count
        # Endpoint cells prove only this conservative displacement, not a full
        # trajectory or exact per-movement distance within either endpoint chunk.
        displacement += max(0, max(abs(final[n] - initial[n]) for n in range(2)) - 1) * 16
    if transitions < 16:
        oracle.fail("explore: no actual work floor for received_chunk_center_transitions")
    if displacement < 1:
        oracle.fail("explore: no actual work floor for received_displacement_lower_bound")
    oracle.completed["received_chunk_center_transitions"] = transitions
    oracle.completed["received_displacement_lower_bound_blocks"] = displacement
    oracle.completed["movement_scope"] = "server snapshot-derived cache centers; conservative endpoint lower bound, not predicted walking or per-move acknowledgement"


def validate(scenario_id: str, report: dict[str, Any], recipe: dict[str, Any]) -> dict[str, Any]:
    """Classify observed work without conflating command offers with completions.

    Accept the backend result (``bots`` plus ``restart``) or a plain bot report.
    Missing probes are infrastructure errors; decoded mismatches are correctness
    failures.  Validators never extend a window or infer unavailable work.
    """
    prepared = recipe
    oracle = _Oracle()
    if scenario_id not in RECIPE_IDS or prepared.get("id") != scenario_id:
        oracle.fail("control recipe identity mismatch")
        return oracle.result()
    expected = prepared["expected"]
    families = expected["families"]
    bots = report.get("bots", report)
    if not isinstance(bots, dict):
        oracle.missing("bot report missing")
        return oracle.result()
    ready = oracle.number(bots, "ready_clients", "client readiness")
    if ready is not None and ready != expected["clients"]:
        oracle.fail("not every requested client was ready")
    if expected["clients"] < prepared["constraints"].get("minimum_clients", 1):
        oracle.missing("requested clients cannot keep every declared arena resident")
    disconnects = oracle.number(bots, "disconnects", "client liveness")
    if disconnects:
        oracle.fail("client disconnected")
    schedule = bots.get("open_loop")
    if not isinstance(schedule, dict):
        oracle.missing("fixed-window bot accounting missing")
        return oracle.result()
    if schedule.get("schedule_complete") is not True:
        oracle.fail("fixed wall schedule did not complete")
    windows = {row.get("name"): row for row in schedule.get("windows", []) if isinstance(row, dict)}
    for phase in ("steady", "burst", "recovery"):
        row = windows.get(phase)
        if row is None:
            oracle.missing(f"{phase}: independent receive/accounting window missing")
            continue
        if phase == "recovery":
            for kind in ("movement", "edit", "command", "chat", "action", "mechanic"):
                for key in ("offered", "admitted"):
                    if row.get(kind, {}).get(key, 0):
                        oracle.fail("recovery offered/admitted new gameplay stimulus")
        minimum = prepared["constraints"].get("minimum_window_ms", {}).get(phase, 0)
        if minimum:
            duration = oracle.number(row, "duration_ms", phase)
            if duration is not None and duration < minimum:
                oracle.missing(f"{phase}: fixed window too short for full finite control stimulus")
    mechanics = bots.get("mechanics")
    if not isinstance(mechanics, dict) or mechanics.get("schema_version") != 1:
        oracle.missing("received mechanic schema missing")
        return oracle.result()
    if mechanics.get("baseline_captured") is not True:
        oracle.missing("received mechanic Run baseline missing")
    received_windows = mechanics.get("windows")
    if not isinstance(received_windows, dict):
        oracle.missing("measured mechanic receive windows missing")
        received_windows = {}
    measured = {}
    for metric, family in (("health_damage", "hostile"), ("explosions", "combat"),
                           ("projectile_spawns", "projectile")):
        if family in families:
            values = [oracle.number(received_windows.get(phase), metric,
                                    f"{phase} measured {family}")
                      for phase in ("steady", "burst", "recovery")]
            measured[metric] = sum(values) if all(value is not None for value in values) else None
    for error in mechanics.get("probe_errors", []):
        oracle.missing(f"mechanic probe: {error}")
    _action_oracle(oracle, mechanics, prepared["mechanic_spec"])
    if mechanics.get("capacity_errors"):
        oracle.missing("bounded mechanic probe capacity was exceeded")
    for key in ("identity_errors", "position_errors", "duplicate_spawns", "ghost_updates"):
        value = oracle.number(mechanics, key, "entity lifecycle")
        if value:
            oracle.fail(f"entity lifecycle: {key}")
    entities = _named(mechanics.get("entities"))
    # Entities are keyed by kind, rather than block/container probe name.
    if isinstance(mechanics.get("entities"), list):
        entities = {r["kind"]: r for r in mechanics["entities"] if isinstance(r, dict) and "kind" in r}
    moved = 0
    for kind, identities in expected["entities"].items():
        row = entities.get(kind)
        if row is None:
            oracle.missing(f"{kind}: received population missing")
            continue
        for key in ("observed", "active", "moved", "removed"):
            oracle.number(row, key, kind)
        samples = _entity_samples(oracle, row, kind)
        absent = set(identities) - samples.keys()
        if absent:
            oracle.missing(f"{kind}: {len(absent)} authored UUID lifecycle receipts missing")
        value = row.get("moved")
        if isinstance(value, int) and not isinstance(value, bool):
            moved += value
        # Wire active/removal means visibility, not authoritative death. Native
        # final UUID/kind population is the owner-loss oracle, never this count.
        if kind in ("minecraft:cow", "minecraft:sheep", "minecraft:pig", "minecraft:chicken", "minecraft:villager"):
            native = report.get("native_snapshot")
            if not isinstance(native, dict):
                oracle.missing("passive population: authoritative native snapshot missing")
            else:
                population = _named(native.get("entities")).get(kind)
                if not isinstance(population, dict):
                    oracle.missing("passive population: native UUID/kind records missing")
                else:
                    native_samples = _entity_samples(oracle, population, kind)
                    durable_ids = set(native_samples)
                    if kind == "minecraft:villager":
                        # Native villages remain enabled; census only the
                        # persisted fixture cohort, not every loaded villager.
                        durable_ids = {identity for identity, sample in native_samples.items()
                            if isinstance(sample.get("payload"), dict)
                            and sample["payload"].get("CustomName") == _VILLAGER_CUSTOM_NAME}
                    expected_ids = set(identities)
                    if durable_ids != expected_ids:
                        oracle.fail(f"{kind}: authoritative population lost or duplicated")
    oracle.completed["moving_unique_entities"] = moved
    native = report.get("native_snapshot")
    if "base" in families:
        if not isinstance(native, dict):
            oracle.missing("base: simultaneous durable stock snapshot missing")
        else:
            _durable_stock_oracle(oracle, native)
    if "entities" in families and moved <= 0:
        oracle.fail("no received authored entity position change")
    blocks = _named(mechanics.get("blocks"))
    _fresh_probe_oracle(oracle, mechanics, prepared["mechanic_spec"])
    for name, kind in expected["stable_blocks"].items():
        row = blocks.get(name)
        if row is None:
            oracle.missing("stable marker/POI block receipts missing")
            continue
        final = _state(oracle, row, "final", name)
        if final is not None and final["name"] != kind:
            oracle.fail("stable marker/POI physical block mismatch")
        if final is not None and any(final["properties"].get(key) != value for key, value in
                expected.get("stable_properties", {}).get(name, {}).items()):
            oracle.fail("stable marker/POI physical properties mismatch")
    if "base" in families:
        _base_oracle(oracle, mechanics)
        releases = 0
        for n in range(64):
            for probe, kind, facing in (
                    (f"furnace_{n}", "minecraft:furnace", None),
                    (f"hopper_{n}_a", "minecraft:hopper", "east"),
                    (f"hopper_{n}_b", "minecraft:hopper", "west")):
                row = blocks.get(probe)
                if row is None:
                    oracle.missing("base: furnace/hopper physical owner probes missing")
                    continue
                final = _state(oracle, row, "final", probe)
                if final is not None and (final["name"] != kind or facing is not None and
                        (final["properties"].get("facing") != facing or final["properties"].get("enabled") != "true")):
                    oracle.fail("base: furnace/hopper physical topology mismatch")
            for button in range(8):
                name = f"button_{n}_{button}"
                row = blocks.get(name)
                if row is None:
                    oracle.missing("base: all 512 physical button receipts required")
                    continue
                final = _state(oracle, row, "final", name)
                if final is not None and (final["name"] != "minecraft:stone_button" or final["properties"].get("powered") != "false"):
                    oracle.fail("base: scheduled button failed matching-block release")
                if isinstance(row.get("update_count"), int) and row["update_count"] >= 2:
                    releases += 1
                for probe, kind, property_name, value in (
                        (f"wire_{n}_{button}", "minecraft:redstone_wire", "power", "0"),
                        (f"lamp_{n}_{button}", "minecraft:redstone_lamp", "lit", "false")):
                    record = blocks.get(probe)
                    if record is None:
                        oracle.missing("base: all512 matching wire/lamp physical probes required")
                        continue
                    final = _state(oracle, record, "final", probe)
                    if final is not None and (final["name"] != kind or final["properties"].get(property_name) != value):
                        oracle.fail("base: matching wire/lamp did not converge after release")
        if releases != 512:
            oracle.fail("base: all512 finite button press/release transitions must be received")
        oracle.completed["released_buttons"] = releases
    if "farm" in families:
        growth = 0
        for name, kind in expected["crops"].items():
            row = blocks.get(name)
            soil = blocks.get(name.replace("crop_", "soil_", 1))
            if row is None or soil is None:
                oracle.missing("farm: complete crop/substrate physical probes required")
                continue
            states = [_state(oracle, row, stage, name) for stage in ("initial", "final")]
            ages = []
            for state in states:
                if state is None:
                    continue
                try:
                    age = int(state["properties"]["age"])
                except (KeyError, TypeError, ValueError):
                    oracle.fail("farm: invalid received crop age")
                    continue
                if state["name"] != kind or not 0 <= age <= (3 if kind == "minecraft:beetroots" else 7):
                    oracle.fail("farm: illegal crop type/age")
                ages.append(age)
            substrate = _state(oracle, soil, "final", name)
            if substrate is not None and substrate["name"] != "minecraft:farmland":
                oracle.fail("farm: crop substrate lost")
            if len(ages) == 2:
                if ages[1] < ages[0]:
                    oracle.fail("farm: unexplained harvest/age regression")
                growth += ages[1] - ages[0]
        if growth <= 0:
            oracle.fail("farm: no received natural crop-age growth")
        oracle.completed["crop_age_delta"] = growth
    if "fluid" in families:
        flows = 0
        for name, kind in expected["fluid_sources"].items():
            row = blocks.get(name)
            if row is None:
                oracle.missing("fluids: all 128 source probes required")
                continue
            state = _state(oracle, row, "final", name)
            if state is not None and (state["name"] != kind or state["properties"].get("level") != "0"):
                oracle.fail("fluids: source identity/topology mismatch")
            n = name.split("_")[1]
            row = blocks.get(f"flow_{n}_69")
            if row is None:
                oracle.missing("fluids: released non-source column probe missing")
                continue
            state = _state(oracle, row, "final", name)
            if state is not None:
                if state["name"] != kind or state["properties"].get("level") != "8":
                    oracle.fail("fluids: natural falling-column topology mismatch")
                elif row.get("update_count", 0) > 0:
                    flows += 1
        if flows == 0:
            oracle.fail("fluids: no received non-source fluid propagation")
        oracle.completed["flowing_columns"] = flows
    if "hostile" in families:
        value = measured["health_damage"]
        if value is not None and value <= 0:
            oracle.fail("survival combat: no received health damage")
        deaths = oracle.number(mechanics, "deaths", "survival lifecycle")
        respawns = oracle.number(mechanics, "respawns", "survival lifecycle")
        if deaths is not None and respawns is not None and respawns > deaths:
            oracle.fail("respawn without received death causality")
    if "combat" in families:
        value = measured["explosions"]
        if value is not None and value <= 0:
            oracle.fail("combat: no received explosion")
        tnt = entities.get("minecraft:tnt", {})
        if tnt.get("observed", 0) < 96 or tnt.get("removed", 0) < 96 or tnt.get("active") != 0:
            oracle.fail("combat: finite 96-TNT lifecycle did not drain")
        destroyed = 0
        for n in range(96):
            row = blocks.get(f"chain_tnt_{n}")
            if row is None:
                oracle.missing("combat: chain-TNT physical receipts missing")
                continue
            final = _state(oracle, row, "final", "chain TNT")
            if final is not None and final["name"] == "minecraft:air" and row.get("update_count", 0):
                destroyed += 1
        if destroyed <= 0:
            oracle.fail("combat: explosion packet without received block effect")
        oracle.completed["chain_targets_destroyed"] = destroyed
    if "projectile" in families:
        received = measured["projectile_spawns"]
        if received is not None and received <= 128:
            oracle.fail("projectiles: startup 128 arrows do not prove continued skeleton launches")
        arrows = entities.get("minecraft:arrow", {})
        if arrows.get("moved", 0) <= 0 or arrows.get("removed", 0) <= 0:
            oracle.fail("projectiles: no received launch/move/removal lifecycle")
        if arrows.get("observed", 0) <= 128:
            oracle.fail("projectiles: unique natural launches beyond finite128-arrow stock missing")
        _projectile_trace_oracle(oracle, mechanics, arrows, expected)
    if "village" in families:
        native = report.get("native_snapshot", {})
        metadata = native.get("villagers") if isinstance(native, dict) else None
        if not isinstance(metadata, list) or len(metadata) != 64:
            oracle.missing("village: 64 real native profession/Brain/POI identity probes required")
        else:
            claims: set[tuple[Any, ...]] = set()
            legal_claims = {field: {tuple(position) for position in positions}
                            for field, positions in expected["poi_positions"].items()}
            identities = set()
            for villager in metadata:
                if not isinstance(villager, dict):
                    oracle.missing("village: native villager record malformed")
                    continue
                identity = villager.get("uuid")
                if not isinstance(identity, str) or identity in identities:
                    oracle.fail("village: missing/duplicate native UUID identity")
                    continue
                identities.add(identity)
                if villager.get("profession") != "minecraft:farmer":
                    oracle.fail("village: actual farmer profession missing")
                if villager.get("brain_valid") is not True:
                    oracle.fail("village: illegal retained native Brain")
                for field in ("home", "job_site"):
                    position = villager.get(field)
                    if not isinstance(position, list) or len(position) != 3 or any(
                            isinstance(v, bool) or not isinstance(v, int) for v in position):
                        oracle.missing("village: actual retained native POI claim missing")
                        continue
                    if tuple(position) not in legal_claims[field]:
                        oracle.fail("village: native claim does not match authored physical POI")
                    claim = (field, *position)
                    if claim in claims:
                        oracle.fail("village: duplicate POI claim")
                    claims.add(claim)
            if identities != set(expected["entities"]["minecraft:villager"]):
                oracle.fail("village: native farmer UUID set differs from authored population")
    if "edits" in families:
        for key in ("break_updates_observed", "place_updates_observed"):
            if not oracle.number(bots, key, "base edits"):
                oracle.fail(f"base edits: missing {key}")
    if "player_gameplay" in families:
        players = mechanics.get("players")
        if not isinstance(players, list):
            oracle.missing("player gameplay: final received inventories missing")
        else:
            planks = sum(stack.get("count", 0) for player in players if isinstance(player, dict)
                         for stack in player.get("final_inventory", []) if isinstance(stack, dict)
                         and stack.get("name") == "minecraft:oak_planks")
            if planks != 4:
                oracle.fail("player gameplay: one log did not produce exactly four received planks")
            oracle.completed["crafted_planks_received"] = planks
        hits = [row for row in mechanics.get("actions", [])
                if isinstance(row, dict) and row.get("kind") == "attack_entity"]
        if len(hits) != 1 or hits[0].get("completed") != 1:
            oracle.fail("player gameplay: finite nonlethal hit lacks received completion")
        else:
            oracle.completed["nonlethal_hits_received"] = 1
    if "explore" in families:
        chunks = [oracle.number(windows.get(phase, {}).get("received"), "chunks_received",
                                f"{phase} measured explore") for phase in ("steady", "burst", "recovery")]
        if all(value is not None for value in chunks) and sum(chunks) < 16:
            oracle.fail("explore: no actual work floor for measured_chunks_received")
        _explore_oracle(oracle, mechanics, expected["clients"])
    if "plugin" in families:
        plugin = report.get("plugin", mechanics.get("plugin"))
        if not isinstance(plugin, dict):
            oracle.missing("plugin: deployed WASM/host and durable town/claim probes missing")
        else:
            for error in plugin.get("infra_errors", []):
                oracle.missing(error)
            if plugin.get("runtime_ready") is not True:
                oracle.missing("plugin: real runtime_ready evidence missing")
            if plugin.get("town_name") != expected["town_name"] or plugin.get("durable") is not True:
                oracle.fail("plugin: actual durable named town/claim missing")
            readbacks = plugin.get("state_readbacks")
            if not isinstance(readbacks, list) or not readbacks:
                oracle.missing("plugin: per-actor state-query evidence missing")
            else:
                for action in prepared["mechanic_spec"]["actions"]:
                    if not action.get("expected_feedback"):
                        continue
                    matches = [row for row in readbacks if isinstance(row, dict)
                        and row.get("bot") == action["bot"] and row.get("window") == action["window"]
                        and row.get("text") == action["expected_feedback"]]
                    count = sum(a.get("expected_feedback") == action["expected_feedback"]
                        and a["bot"] == action["bot"] and a["window"] == action["window"]
                        for a in prepared["mechanic_spec"]["actions"])
                    if len(matches) != 1 or matches[0].get("received") != count:
                        oracle.fail("plugin: exact per-actor/window state-query or mutation reply missing")
            for owner in prepared["required_plugins"]:
                journal = plugin.get("journal", {}).get(owner)
                if not isinstance(journal, dict):
                    oracle.missing(f"plugin: journal-ACKed result evidence missing for {owner}")
                    continue
                delivered = oracle.number(journal, "journaled_messages", f"plugin {owner}")
                if delivered is not None and delivered < 1:
                    oracle.fail(f"plugin: no actual journal-ACKed replies for {owner}")
                committed = oracle.number(journal, "committed", f"plugin {owner}")
                if committed is not None and committed < 1:
                    oracle.fail("plugin: no actual committed native/storage operation")
            for key in ("successful_replies", "host_operations", "wasm_invocations"):
                value = oracle.number(plugin, key, "plugin work")
                if value is not None and value < (20 if key == "successful_replies" else 1):
                    oracle.fail(f"plugin: no actual {key}")
            if plugin.get("errors"):
                oracle.fail("plugin: public command returned errors")
    if "plugin_inventory" in families:
        containers = _named(mechanics.get("containers"))
        for bot in range(2):
            name = f"plugin_chest_{bot}"
            row = containers.get(name, {})
            for stage in ("initial", "final"):
                chest = _slots(oracle, row, stage, name, 27)
                inventory = _slots(oracle, {"slots": row.get(stage + "_player_inventory")},
                                   "slots", name + " player", 36, slot_offset=27)
                if chest is not None and any(count for _, count in chest.values()):
                    oracle.fail("plugin gameplay: chest did not return to empty")
                if inventory is not None and (
                        sum(count for item, count in inventory.values() if item == "minecraft:diamond") != 1
                        or any(count for item, count in inventory.values()
                               if item not in ("minecraft:diamond", "minecraft:air"))):
                    oracle.fail("plugin gameplay: one granted diamond was not conserved")
            updates = oracle.number(row, "container_slot_updates", name)
            if updates is not None and updates < 4:
                oracle.fail("plugin gameplay: two chest custody round trips were not received")
            carried = row.get("carried")
            if not isinstance(carried, dict) or carried.get("count") != 0:
                oracle.fail("plugin gameplay: cursor custody was not empty")
    if "restart" in families:
        _restart_oracle(oracle, report.get("restart"), expected)
    if "long" in families:
        elapsed = oracle.number(schedule, "elapsed_ms", "long-session")
        if elapsed is not None and elapsed < prepared["constraints"].get("minimum_measurement_ms", 0):
            oracle.fail("long-session: explicit 600-second non-smoke control not completed")
        windows_metrics = report.get("windows")
        raw_samples = report.get("raw_samples")
        if not isinstance(raw_samples, str) or not raw_samples:
            oracle.missing("long-session: retained raw RSS/backlog time-series provenance missing")
        if not isinstance(windows_metrics, dict):
            oracle.missing("long-session: normalized independent resource windows missing")
        else:
            for phase in ("steady", "burst", "recovery"):
                row = windows_metrics.get(phase, {})
                queues = row.get("edge_send_queue", {})
                samples = oracle.number(queues, "samples", f"long-session {phase} queues")
                snapshots = queues.get("snapshots")
                if not isinstance(snapshots, list) or samples != len(snapshots) or not snapshots:
                    oracle.missing(f"long-session: {phase} actual queue time series missing")
                memory = row.get("memory")
                if not isinstance(memory, dict):
                    oracle.missing(f"long-session: {phase} normalized process RSS samples missing")
                    continue
                for process in ("server", "bots"):
                    rss = memory.get(process, {})
                    maximum = oracle.number(rss, "rss_max_bytes", f"{phase} {process} RSS")
                    final = oracle.number(rss, "rss_final_bytes", f"{phase} {process} RSS")
                    if maximum is not None and final is not None and (final <= 0 or maximum < final):
                        oracle.fail("long-session: inconsistent measured RSS endpoints")
            oracle.completed["resource_scope"] = (
                "Independent normalized RSS endpoints and actual queue snapshots; "
                "raw RSS time series retained by backend. Endpoint slope is not leak proof.")
    # Require work receipts from both active windows, not a startup aggregate.
    per_window = mechanics.get("windows")
    if not isinstance(per_window, dict):
        oracle.missing("mechanics: independent measured-window work evidence missing")
    else:
        for phase in ("steady", "burst"):
            row = per_window.get(phase)
            if not isinstance(row, dict):
                oracle.missing(f"{phase}: mechanic receive evidence missing")
                continue
            keys = []
            if "entities" in families:
                keys.append("entity_moves")
            if "base" in families:
                keys.extend(("block_updates", "container_updates"))
            if "farm" in families or "fluid" in families:
                keys.append("block_updates")
            for key in set(keys):
                value = oracle.number(row, key, phase)
                if value is not None and value <= 0:
                    oracle.fail(f"{phase}: no actual received {key}")
    return oracle.result()


def _restart_oracle(oracle: _Oracle, restart: Any, expected: dict[str, Any]) -> None:
    if not isinstance(restart, dict):
        oracle.missing("restart: public durable before/after snapshots missing")
        return
    checkpoint = restart.get("checkpoint")
    receipts = checkpoint if isinstance(checkpoint, list) else [checkpoint]
    saved = False
    for receipt in receipts:
        if isinstance(receipt, dict):
            event = receipt.get("checkpoint_event", receipt)
            saved |= isinstance(event, dict) and event.get("event") == "fixture_checkpoint_saved"
    if not saved:
        oracle.missing("restart: actual Saved-game completion receipt missing")
    if restart.get("same_world") is not True:
        oracle.missing("restart: exact source native world identity was not established")
    if restart.get("fixture_regenerated") is not False:
        oracle.fail("restart: fixture regeneration was not explicitly excluded")
    for field in ("world_before", "world_after"):
        snapshot = restart.get(field)
        if not isinstance(snapshot, dict) or not snapshot.get("sha256"):
            oracle.missing("restart: exact source world fingerprint missing")
    before, after = restart.get("before"), restart.get("after")
    if not isinstance(before, dict) or not isinstance(after, dict):
        oracle.missing("restart: received snapshots incomplete")
        return
    for stage in (before, after):
        blocks = _named(stage.get("blocks"))
        for name, kind in expected["stable_blocks"].items():
            row = blocks.get(name)
            if row is None:
                oracle.missing("restart: stable markers/POIs not freshly decoded")
                continue
            state = _state(oracle, row, "final", name)
            if state is not None and state["name"] != kind:
                oracle.fail("restart: stable physical marker/POI lost")
    for kind, authored in expected["entities"].items():
        populations = []
        for stage in (before, after):
            entities = _named(stage.get("entities"))
            row = entities.get(kind)
            if row is None:
                oracle.missing("restart: entity UUID/kind snapshot missing")
                populations.append(set())
            else:
                populations.append(set(_entity_samples(oracle, row, kind)))
        if populations[0] != populations[1] or set(authored) != populations[1]:
            oracle.fail("restart: authored entity owner identity lost/duplicated")
    if "base" in expected["families"]:
        # Cooking may progress after restart. Native owners must still conserve
        # the same finite raw+output and circulating stock, not equal timestamps.
        _base_oracle(oracle, after, preparation=True)
        _durable_stock_oracle(oracle, after)


def _durable_stock_oracle(oracle: _Oracle, native: dict[str, Any]) -> None:
    """An atomic clean-stop image closes the sequential-menu transfer skew."""
    containers = _named(native.get("containers"))
    for n in range(64):
        furnace = containers.get(f"furnace_{n}")
        if furnace is None:
            oracle.missing("durable base: all64 furnace owner records required")
            continue
        slots = _slots(oracle, furnace, "final", f"furnace_{n}", 3)
        if slots is not None:
            raw = slots[0][1] if slots[0][0] == "minecraft:raw_iron" else 0
            ingots = slots[2][1] if slots[2][0] == "minecraft:iron_ingot" else 0
            if raw + ingots != 64:
                oracle.fail("durable base: smelting owner lost/duplicated finite input/output")
        pair = []
        for side in ("a", "b"):
            name = f"hopper_{n}_{side}"
            row = containers.get(name)
            if row is None:
                oracle.missing("durable base: all128 hopper owner records required")
                continue
            slots = _slots(oracle, row, "final", name, 5)
            if slots is not None:
                pair.extend(slots.values())
        if len(pair) == 10 and sum(count for name, count in pair if name == "minecraft:dirt") != 128:
            oracle.fail("durable base: exact seeded+dropped dirt conservation failed")


def _projectile_trace_oracle(oracle: _Oracle, mechanics: dict[str, Any],
                             arrows: dict[str, Any], expected: dict[str, Any]) -> None:
    """Join received DamageEvent identities to the same arrow's removal trace."""
    hits = mechanics.get("projectile_hits")
    if not isinstance(hits, list):
        oracle.missing("projectiles: received causal projectile damage trace unavailable")
        return
    for key in ("damage_events_unresolved", "damage_events_truncated"):
        value = oracle.number(mechanics, key, "projectile causal trace")
        if value:
            oracle.missing(f"projectiles: incomplete causal trace ({key})")
    duplicates = oracle.number(mechanics, "projectile_duplicate_hits", "projectile causal trace")
    if duplicates:
        oracle.fail("projectiles: duplicate causal hit/damage receipt")
    samples = _entity_samples(oracle, arrows, "minecraft:arrow")
    startup = set(expected["entities"]["minecraft:arrow"])
    shooters = set(expected["entities"]["minecraft:skeleton"])
    seen = set()
    completed = 0
    for hit in hits:
        if not isinstance(hit, dict) or not isinstance(hit.get("projectile_uuid"), str):
            oracle.missing("projectiles: undecoded hit identity")
            continue
        identity = hit["projectile_uuid"]
        if identity in seen:
            oracle.fail("projectiles: duplicate hit for non-piercing arrow")
        seen.add(identity)
        if identity in startup:
            continue
        if hit.get("owner_uuid") not in shooters:
            oracle.fail("projectiles: natural shot owner is not an authored skeleton")
            continue
        target = hit.get("target_bot")
        if isinstance(target, bool) or not isinstance(target, int) or not 0 <= target < expected["clients"]:
            oracle.fail("projectiles: causal hit victim is not a requested survival client")
            continue
        sample = samples.get(identity)
        if sample is None or not isinstance(sample.get("remove_count"), int) or sample["remove_count"] <= 0:
            oracle.fail("projectiles: same hit arrow lacks a received final removal")
            continue
        if hit.get("window") not in ("steady", "burst", "recovery"):
            oracle.missing("projectiles: hit lacks independent measured-window attribution")
            continue
        completed += 1
    if completed <= 0:
        oracle.fail("projectiles: no received natural skeleton launch/hit/removal chain")
    oracle.completed["natural_projectile_hit_removal_chains"] = completed
