"""Owner compound loads, checked by received and read-only native final images.

Final-state oracles deliberately do not claim rejected-attempt traces, queue-age
bounds or full vanilla parity. Windows and performance budgets remain the controls'.
Water drops are enclosed through their floor: in the compound scene the combat
roof otherwise catches upper spills and redirects them into pinned piston cells.
"""
from __future__ import annotations

from copy import deepcopy
from typing import Any
from collections import Counter

IDS = ("MIX-01", "MIX-02", "MIX-04", "MIX-08", "MIX-18", "MIX-21", "mixed-survival-long")
_PREREQUISITES = {
    "MIX-01": {"stimuli": ["dawn", "workstation_loss", "login_cohort"],
               "invariants": ["No destroyed workstation retains a job-site memory; at most one claimant per site.",
                              "At least one villager whose initial site was destroyed reassigns and performs actual work.",
                              "Every cohort member reaches ready and a received first chat echo."]},
    "MIX-02": {"stimuli": ["finite_stock", "full_branch", "shared_chest", "overflow"],
               "invariants": ["Finite item stock is conserved across containers, vehicles, ground and players.",
                              "The independent free branch receives items."]},
    "MIX-04": {"stimuli": ["adhesive_overlap", "gravity", "water"],
               "invariants": ["Native final blocks match the pinned moved-block image without duplication or loss."]},
    "MIX-08": {"stimuli": ["reader_limit", "death", "respawn", "reader_resume"],
               "invariants": ["Normal readers progress while one reader is limited.",
                              "The slow reader converges after respawn without stale entity identities."]},
    "MIX-18": {"stimuli": ["outbound", "return", "edit", "restart"],
               "invariants": ["The accepted edit survives native restart with the same world identity and seed."]},
    "MIX-21": {"stimuli": ["finite_blasts", "last_block", "last_move", "last_remove", "zero_input"],
               "invariants": ["Every declared resident reader's final block and entity images equal the native restart image without recovery inputs."]},
    "mixed-survival-long": {"stimuli": ["village", "adhesive_overlap", "gravity", "water", "final_mutations", "restart"],
                            "invariants": ["Reachable village, block and persistence mechanics retain their final invariants."]},
}


def _scene() -> list[str]:
    commands = ["gamerule doDaylightCycle false", "gamerule doMobSpawning false",
                "gamerule keepInventory true", "setworldspawn 0 64 0", "time set night"]
    for x in range(-96, 160, 32):
        commands.append(f"fill {x} 63 -32 {x + 31} 63 159 minecraft:stone")
        for y in range(64, 96, 4):
            commands.append(f"fill {x} {y} -32 {x + 31} {y + 3} 159 minecraft:air")
    return commands


# The compound long scene anchors residents at the village quarters, so the origin
# rig/blast arena is resident only after an actor moves there. Chunk loading is
# asynchronous: origin edits wait this long after that tp (plus the 100 ms of
# serialized slots that precede it) because /setblock refuses unloaded positions.
_RESIDENCY_SETTLE_MS = 2000
_ORIGIN_ACTION_PREFIXES = ("overlap_", "blast_", "last_", "neighbor_")


def _settle_origin_residency(result: dict[str, Any]) -> None:
    actions = result["mechanic_spec"]["actions"]
    for row in actions:
        if row["bot"] == 0 and row["window"] == "burst" and row["id"].startswith(_ORIGIN_ACTION_PREFIXES):
            row["at_ms"] += _RESIDENCY_SETTLE_MS + 100
    # Only the actor itself leaves its village slot: the other residents keep the
    # unshifted village fills at burst start resident until neighbor_unload.
    actions.append({"id": "origin_resident", "kind": "command", "bot": 0, "window": "burst",
                    "command": "tp @s 16.5 64 10.5", "at_ms": 0})


def _action(result: dict[str, Any], identifier: str, command: str, at: int = 0,
            phase: str = "burst", bot: int = 0) -> None:
    result["mechanic_spec"]["actions"].append({"id": identifier, "kind": "command",
        "bot": bot, "command": command, "window": phase, "at_ms": at})


def _probe(result: dict[str, Any], name: str, position: list[int], final: str | None = None) -> None:
    result["mechanic_spec"]["blocks"].append({"name": name, "position": position})
    if final is not None:
        result["expected"].setdefault("blocks", {})[name] = "minecraft:" + final


def _village(result: dict[str, Any]) -> None:
    result["expected"]["villagers"] = 16
    result["expected"]["villager_uuids"] = []
    result["expected"]["destroyed_workstations"] = []
    result["mechanic_spec"]["entities"].append({"kind": "minecraft:villager", "count": 16})
    result["commands"].append("time set 2000")
    for n in range(16):
        x, z = -24 + n % 4 * 12, 8 + n // 4 * 12
        identity = 1_000_000 + n
        result["expected"]["villager_uuids"].append(f"00000000-0000-0000-0000-{identity:012x}")
        result["commands"] += [f"setblock {x} 64 {z} minecraft:composter",
            f"summon minecraft:villager {x + 2.5} 64 {z + .5} {{UUID:[I;0,0,0,{identity}]}}"]
        _probe(result, f"job_{n}", [x, 64, z], "air" if n % 2 == 0 else "composter")
        if n % 2 == 0:
            result["expected"]["destroyed_workstations"].append([x, 64, z])
            _action(result, f"remove_job_{n}", f"setblock {x} 64 {z} minecraft:air")
            _action(result, f"replace_job_{n}", f"setblock {x + 3} 64 {z} minecraft:composter", 250)
            _probe(result, f"replacement_job_{n}", [x + 3, 64, z], "composter")
    _action(result, "dawn", "time set day")
    _action(result, "workday", "time set 2000", 500)
    # The measurement cohort is new to the persisted scene, not setup actors.
    result["schedule_overrides"]["steady"]["chat_hz"] = 1
    result["expected"]["cohort"] = True
    result["preparation_duration_ms"] = 3000
    result["preparation_spec"] = {"schema_version": 1, "entities": [], "blocks": [], "containers": [], "actions": []}


def _pistons(result: dict[str, Any]) -> None:
    # Both adhesive groups initially claim the central gold passenger. Ordered
    # activation moves it once; slime and honey themselves do not stick.
    result["commands"].append("setblock 15 64 15 minecraft:gold_block")
    _probe(result, "shared_passenger_origin", [15, 64, 15], "air")
    _probe(result, "shared_passenger_final", [16, 64, 15], "gold_block")
    for n, (z, adhesive) in enumerate(((14, "slime_block"), (16, "honey_block"))):
        result["commands"].append(f"setblock 15 63 {z} minecraft:air")
        result["commands"] += [f"setblock 14 64 {z} minecraft:piston[facing=east,extended=false]",
            f"setblock 15 64 {z} minecraft:{adhesive}", f"setblock 15 65 {z} minecraft:gold_block"]
        for x in (15, 16, 17):
            for y in (64, 65):
                final = adhesive if (x, y) == (16, 64) else "gold_block" if (x, y) == (16, 65) else "piston_head" if (x, y) == (15, 64) else "air"
                _probe(result, f"piston_{n}_{x}_{y}", [x, y, z], final)
        _action(result, f"piston_{n}", f"setblock 13 64 {z} minecraft:redstone_block")
    result["commands"] += ["setblock 18 66 4 minecraft:glass", "setblock 18 67 4 minecraft:sand",
        "setblock 20 67 4 minecraft:glass", "setblock 20 68 4 minecraft:water[level=0]"]
    # Contain the entire falling column, not only its source: composed rigs
    # must not spill over the combat roof into the strict piston air pins.
    for dx, dz in ((1, 0), (-1, 0), (0, 1), (0, -1)):
        result["commands"].append(f"setblock {20 + dx} 68 {4 + dz} minecraft:glass")
        result["commands"].append(f"fill {20 + dx} 64 {4 + dz} {20 + dx} 67 {4 + dz} minecraft:glass")
    for y in range(64, 68):
        _probe(result, f"sand_{y}", [18, y, 4], "sand" if y == 64 else "air")
    _probe(result, "water_source", [20, 68, 4], "water")
    _probe(result, "water_fall", [20, 64, 4], "water")
    _action(result, "gravity", "setblock 18 66 4 minecraft:air")
    _action(result, "water", "setblock 20 67 4 minecraft:air")
    _action(result, "neighbor_unload", "tp @a 512.5 100 512.5", 500)
    _action(result, "neighbor_reload", "tp @a .5 64 .5", 2500)


def _full_village(result: dict[str, Any]) -> None:
    expected = result["expected"]
    expected["villagers"] = 10_000
    expected["villager_uuids"] = []
    # Authored nearest-site association, not an injected Brain memory. Validation
    # must still use the actual captured initial native job_site for each UUID.
    expected["villager_initial_workstations"] = {}
    expected["destroyed_workstations"] = []
    expected["cohort"] = True
    result["mechanic_spec"]["entities"].append({
        "kind": "minecraft:villager", "count": 10_000})
    result["resident_clients"] = 25
    result["group_size"] = 1
    result["view_distance"] = 6
    result["anchors"] = []
    result["preparation_regions"] = []
    result["preparation_duration_ms"] = 3000
    result["preparation_spec"] = {
        "schema_version": 1, "entities": [], "blocks": [],
        "containers": [], "actions": []}
    result["schedule_overrides"]["steady"]["chat_hz"] = 1
    result["constraints"]["minimum_clients"] = 100
    result["scale"] = {**result.get("scale", {}),
        "requested_clients": 100,
        "resident_clients": 25,
        "cohort_clients": 75,
        "village_quarters": 25,
        "villagers_per_quarter": 400,
        "villagers": 10_000,
        "initial_workstations": 10_000,
        "destroyed_workstations": 5_000,
        "replacement_workstations": 5_000,
        "workstations": 15_000,
        "beds": 10_000,
        "bed_block_states": 20_000,
        "village_native_block_probes": 25_000,
        "village_preparation_commands": 45_450,
        "village_burst_commands": 27}
    result["scale_probe_prefixes"] = {
        **result.get("scale_probe_prefixes", {}),
        "beds": ["bed_"],
        "initial_workstations": ["job_"],
        "replacement_workstations": ["replacement_job_"],
        "workstations": ["job_", "replacement_job_"]}

    for quarter in range(25):
        ox = 384 + (quarter % 5) * 128
        oz = 384 + (quarter // 5) * 128
        anchor = [ox + 40, oz + 56]
        result["anchors"].append([ox + 88, oz + 56])
        commands = ["time set 2000",
            f"fill {ox - 16} 63 {oz} {ox + 111} 63 {oz + 127} minecraft:stone"]
        # One 16,384-cell floor and sixteen 32,768-cell clears. The complete
        # local 128x128 footprint fits the anchor's view-4 chunk square; no
        # command depends on chunks outside this quarter's preparation window.
        for y in range(64, 96, 2):
            commands.append(
                f"fill {ox - 16} {y} {oz} {ox + 111} {y + 1} {oz + 127} minecraft:air")

        for local in range(400):
            n = quarter * 400 + local
            x, z = ox + (local % 20) * 4, oz + (local // 20) * 4
            commands.append(f"setblock {x} 64 {z} minecraft:composter")
            # Complete east-facing beds occupy two otherwise unused cells of
            # the same four-by-four plot; only their feet need native probes.
            commands += [
                f"setblock {x + 1} 64 {z + 2} minecraft:red_bed[facing=east,part=foot,occupied=false]",
                f"setblock {x + 2} 64 {z + 2} minecraft:red_bed[facing=east,part=head,occupied=false]"]
            destroyed = local // 20 < 10
            _probe(result, f"job_{n}", [x, 64, z],
                   "air" if destroyed else "composter")
            _probe(result, f"bed_{n}", [x + 1, 64, z + 2], "red_bed")
            if destroyed:
                expected["destroyed_workstations"].append([x, 64, z])

        # All 200 replacements already exist before villagers are introduced.
        # z84..120 is beyond every villager's nearer original workstation. This
        # keeps the crisis to one filtered removal command per quarter rather
        # than 5,000 per-villager removals plus 5,000 late placement actions.
        for local in range(200):
            n = quarter * 200 + local
            x, z = ox + (local % 20) * 4, oz + 84 + (local // 20) * 4
            commands.append(f"setblock {x} 64 {z} minecraft:composter")
            _probe(result, f"replacement_job_{n}", [x, 64, z], "composter")

        for local in range(400):
            n = quarter * 400 + local
            identity_number = 1_000_000 + n
            identity = f"00000000-0000-0000-0000-{identity_number:012x}"
            x, z = ox + (local % 20) * 4, oz + (local // 20) * 4
            expected["villager_uuids"].append(identity)
            expected["villager_initial_workstations"][identity] = [x, 64, z]
            commands.append(
                f"summon minecraft:villager {x + 1.5} 64 {z + 1.5} "
                f"{{UUID:[I;0,0,0,{identity_number}]}}")

        result["preparation_regions"].append({
            "name": f"village_quarter_{quarter:02}", "anchor": anchor,
            "view_distance": 4, "commands": commands})
        # The north half is exactly rows z0,4,...,36: 200 initial composters.
        # Beds in that rectangle survive because replacement is block-filtered.
        # This 77x1x37 edit is 2,849 cells, below the 32,768 public fill cap.
        _action(result, f"remove_quarter_jobs_{quarter:02}",
                f"fill {ox} 64 {oz} {ox + 76} 64 {oz + 36} minecraft:air replace minecraft:composter",
                bot=quarter)

    _action(result, "dawn", "time set day", 100)
    _action(result, "workday", "time set 2000", 500)


def _stock(result: dict[str, Any], clients: int) -> None:
    # Finite gifts enter only once, through ordinary player/menu/drop actions.
    result["commands"] += ["setblock -8 64 0 minecraft:chest", "setblock -8 65 0 minecraft:hopper[facing=down,enabled=true]",
        "setblock 0 64 0 minecraft:chest", "setblock 0 65 0 minecraft:hopper[facing=down,enabled=true]",
        "setblock -7 64 0 minecraft:rail", "summon minecraft:hopper_minecart -6.5 64 .5"]
    result["commands"].append("clear @s")
    for name, position, slots in (("blocked_chest", [-8, 64, 0], 27), ("blocked_hopper", [-8, 65, 0], 5),
                                   ("free_chest", [0, 64, 0], 27), ("free_hopper", [0, 65, 0], 5)):
        result["mechanic_spec"]["containers"].append({"name": name, "position": position, "bot": 0, "slots": slots})
    result["mechanic_spec"]["entities"] += [{"kind": "minecraft:hopper_minecart", "count": 1}, {"kind": "minecraft:item", "count": 32}]
    actions = result["mechanic_spec"]["actions"]
    def step(kind: str, at: int, **kw: Any) -> None:
        actions.append({"kind": kind, "bot": 0, "window": "steady", "at_ms": at, **kw})
    # Saturate the chest once (27 stacks); hopper/cart contend over its overflow.
    step("command", 0, command="clear @a")
    step("command", 0, command="tp @s -7.5 64 2.5")
    step("command", 500, command="give @s minecraft:dirt 1728")
    step("open_container", 1000, position=[-8, 64, 0])
    for slot in range(27):
        step("click_slot", 1500 + slot * 125, slot=54 + slot if slot < 9 else 27 + slot - 9, button=0, mode=1)
    step("close_container", 5200)
    step("command", 5500, command="give @s minecraft:dirt 64")
    step("command", 6000, command="tp @s -7.1 67 .5")
    step("set_rotation", 6500, yaw=0, pitch=90)
    step("select_slot", 6750, slot=0)
    step("drop_stack", 7000)
    step("command", 7500, command="tp @s .5 66 2.5")
    step("command", 8000, command="give @s minecraft:dirt 64")
    step("open_container", 8500, position=[0, 65, 0])
    step("click_slot", 9250, slot=32, button=0, mode=1)
    step("close_container", 10000)
    # All clients compete over the same chest with legal quick-moves.
    for bot in range(min(clients, 8)):
        if bot:
            result["mechanic_spec"]["containers"].append({
                "name": f"shared_chest_{bot}", "position": [-8, 64, 0], "bot": bot, "slots": 27})
        for at, kind, kw in ((0, "command", {"command": "tp @s -7.5 64 2.5"}),
                             (750, "open_container", {"position": [-8, 64, 0]}),
                             (1500, "click_slot", {"slot": 0, "button": 0, "mode": 1, "allow_no_change": True}),
                             (2250, "close_container", {})):
            actions.append({"kind": kind, "bot": bot, "window": "burst", "at_ms": at, **kw})
    result["expected"]["stock"] = {"minecraft:dirt": 1856}
    result["constraints"]["minimum_window_ms"] = {"steady": 12000, "burst": 4000}


def _final_mutations(result: dict[str, Any]) -> None:
    result["view_distance"] = 8
    result["anchors"] = [[64, 0]]
    # Keep the finite combat stock away from the village/pinned final cells.
    # The barrier enclosure prevents long-run hostile paths into the village.
    result["commands"] += [
        "fill 24 64 -32 24 69 32 minecraft:barrier",
        "fill 112 64 -32 112 69 32 minecraft:barrier",
        "fill 24 64 -32 112 69 -32 minecraft:barrier",
        "fill 24 64 32 112 69 32 minecraft:barrier",
        "fill 24 69 -32 112 69 32 minecraft:barrier"]
    mob_count = 128 if result["expected"]["smoke"] else 1024
    for n in range(mob_count):
        x, z = ((40 + (n % 16) * 3, -12 + (n // 16) * 4)
                if result["expected"]["smoke"] else (38 + (n % 48) * 1.4, -27 + (n // 48) * 2.4))
        result["commands"].append(f"summon minecraft:husk {x + .5} 64 {z + .5} {{PersistenceRequired:1b}}")
    result["mechanic_spec"]["entities"].append({"kind": "minecraft:husk", "count": mob_count})
    for name, suffix, x in (("mix21_move", 2101, 16), ("mix21_remove", 2102, 18)):
        result["commands"].append(f"summon minecraft:armor_stand {x + .5} 64 -8.5 {{UUID:[I;0,0,0,{suffix}],CustomName:'{{\"text\":\"{name}\"}}'}}")
    result["mechanic_spec"]["entities"].append({"kind": "minecraft:armor_stand", "count": 2, "reader_images": True})
    _probe(result, "last_block", [2, 64, -8], "diamond_block")
    result["mechanic_spec"]["blocks"][-1]["reader_images"] = True
    _action(result, "last_block", "setblock 2 64 -8 minecraft:diamond_block", 8000)
    _action(result, "last_move", "tp 00000000-0000-0000-0000-000000000835 20.5 64 -8.5", 8000)
    _action(result, "last_remove", "kill 00000000-0000-0000-0000-000000000836", 8000)
    _action(result, "blast_viewer", "tp @a 64.5 64 .5", 4000 if not result["expected"]["smoke"] else 3000)
    for n in range(96):
        x, z = 32 + (n % 12) * 6, -21 + (n // 12) * 6
        _action(result, f"blast_{n}", f"summon minecraft:tnt {x + .5} 64 {z + .5}", n * 50)
    result["expected"]["entities"] = [
        {"uuid": "00000000-0000-0000-0000-000000000835", "active": True, "position": [20.5, 64.0, -8.5]},
        {"uuid": "00000000-0000-0000-0000-000000000836", "active": False}]
    result["expected"]["finite_blasts"] = 96
    result["restart_check"] = True
    result.setdefault("scale", {}).update(combat_mobs=mob_count, finite_tnt=96,
        final_block_mutations=1, final_entity_moves=1, final_entity_removals=1)


def _full_stock(result: dict[str, Any], clients: int) -> None:
    # Preserve the smoke core byte-for-byte: the 1,856 dirt stock, blocked/full
    # chest, cart, free branch and original first-eight contention actions.
    _stock(result, 25)
    spec = result["mechanic_spec"]
    actions = spec["actions"]
    for bot in range(8, 25):
        spec["containers"].append({"name": f"shared_chest_{bot}",
            "position": [-8, 64, 0], "bot": bot, "slots": 27})
        for at, kind, fields in (
            (0, "command", {"command": "tp @s -7.5 64 2.5"}),
            (750, "open_container", {"position": [-8, 64, 0]}),
            (1500, "click_slot", {"slot": 0, "button": 0, "mode": 1,
                                    "allow_no_change": True}),
            (2250, "close_container", {}),
        ):
            actions.append({"kind": kind, "bot": bot, "window": "burst",
                            "at_ms": at, **fields})

    region_anchors = [[192, 192], [320, 192], [192, 320], [320, 320]]
    result["anchors"] = [[0, 0], *region_anchors]
    result["group_size"] = 5
    result["view_distance"] = max(result.get("view_distance", 4), 4)
    result["constraints"]["minimum_clients"] = max(
        result["constraints"].get("minimum_clients", 1), 25)
    prep = result.setdefault("preparation_spec", {"schema_version": 1,
        "entities": [], "blocks": [], "containers": [], "actions": []})
    start = result.get("preparation_duration_ms", 0)
    prep_cursors = {bot: start + 250 for bot in range(25)}
    prep_owners = (list(range(0, 7)), list(range(7, 13)),
                   list(range(13, 19)), list(range(19, 25)))
    result["preparation_clients"] = 25
    result["preparation_group_size"] = 1
    result["preparation_anchors"] = [list(region_anchors[region])
        for region, owners in enumerate(prep_owners) for _ in owners]
    prep["actions"].extend({"id": f"industry_setup_clear_{bot}", "kind": "command",
        "command": "clear @s", "bot": bot, "window": "steady", "at_ms": start}
        for bot in range(25))
    cursors: dict[int, int] = {}
    observations: dict[int, list[dict[str, Any]]] = {bot: [] for bot in range(25)}
    regions = result.setdefault("preparation_regions", [])
    modules = []
    for region_index, (ax, az) in enumerate(region_anchors):
        # Every fill is bounded and within this region's public residency window.
        # These commands intentionally do not live in the origin commands list.
        commands = [f"fill {ax - 28} 63 {az - 28} {ax + 27} 63 {az + 27} minecraft:stone",
                    f"fill {ax - 28} 64 {az - 28} {ax + 27} 67 {az + 27} minecraft:air"]
        for local in range(64):
            number = region_index * 64 + local
            x, z = ax - 24 + (local % 8) * 6, az - 24 + (local // 8) * 6
            owner = 5 * (region_index + 1) + local % 5
            setup_owner = prep_owners[region_index][local % len(prep_owners[region_index])]
            hopper, chest = [x, 65, z], [x, 64, z]
            names = (f"sorter_{number}_hopper", f"sorter_{number}_chest")
            commands.extend((f"setblock {x} 64 {z} minecraft:chest",
                             f"setblock {x} 65 {z} minecraft:hopper[facing=down,enabled=true]"))
            for name, position, slots, block in (
                (names[0], hopper, 5, "hopper"),
                (names[1], chest, 27, "chest"),
            ):
                spec["containers"].append({"name": name, "position": position,
                                           "bot": owner, "slots": slots})
                _probe(result, name, position, block)
                prep["containers"].append({"name": f"setup_{name}", "position": position,
                                           "bot": setup_owner, "slots": slots})
            modules.append({"index": number, "hopper": hopper, "chest": chest,
                            "hopper_probe": names[0], "chest_probe": names[1],
                            "item": "minecraft:nether_wart", "input_count": 64,
                            "region": region_index, "bot": owner, "preparation_bot": setup_owner})
            # One finite grant per module through an ordinary player menu.
            # Twenty-five independent public actors deposit concurrently; no
            # clear/refill can destroy a failed prior deposit and hide a loss.
            clock = prep_cursors[setup_owner]
            steps = (
                (0, {"kind": "command", "command": f"tp @s {x + .5} 64 {z + 2.5}"}),
                (250, {"kind": "command", "command": "give @s minecraft:nether_wart 64"}),
                (500, {"kind": "open_container", "position": hopper}),
                (1000, {"kind": "click_slot", "slot": 32, "button": 0, "mode": 1}),
                (1250, {"kind": "close_container"}),
            )
            prep["actions"].extend({"bot": setup_owner, "window": "steady", "at_ms": clock + at,
                                    "id": f"stock_sorter_{number}_{offset}", **step}
                                   for offset, (at, step) in enumerate(steps))
            prep_cursors[setup_owner] = clock + 1500
            observations[setup_owner].append({"index": number, "chest": chest, "x": x, "z": z})
            # Real received inventory images, not fabricated remote wire reads.
            # Twenty regional actors inspect all 512 physical containers while
            # the five origin residents remain available to the dirt core.
            at = cursors.get(owner, 0)
            actions.extend((
                {"id": f"sorter_{number}_visit", "kind": "command", "bot": owner,
                 "window": "steady", "at_ms": at,
                 "command": f"tp @s {x + .5} 64 {z + 2.5}"},
                {"id": f"sorter_{number}_hopper_open", "kind": "open_container", "bot": owner,
                 "window": "steady", "at_ms": at + 250, "position": hopper},
                {"id": f"sorter_{number}_hopper_close", "kind": "close_container", "bot": owner,
                 "window": "steady", "at_ms": at + 500},
                {"id": f"sorter_{number}_chest_open", "kind": "open_container", "bot": owner,
                 "window": "steady", "at_ms": at + 650, "position": chest},
                {"id": f"sorter_{number}_chest_close", "kind": "close_container", "bot": owner,
                 "window": "steady", "at_ms": at + 900},
            ))
            cursors[owner] = at + 1250
        regions.append({"name": f"full_industry_{region_index}",
                        "anchor": [ax, az], "view_distance": 4, "commands": commands})

    # All 25 genuinely contend on the same chest. Restore the twenty regional
    # holders afterwards so the final world image is taken with every module live.
    for bot in range(5, 25):
        ax, az = region_anchors[(bot // 5) - 1]
        _action(result, f"industry_return_{bot}", f"tp @s {ax + .5} 64 {az + .5}",
                at=3000, phase="burst", bot=bot)
    # Allow the final finite64 input to drain at the ordinary hopper cadence,
    # then observe every destination with the same actor that stocked it.
    # The parent saves only after this final bounded observation window.
    observation_start = max(prep_cursors.values()) + 32000
    observation_ends = []
    for bot, destinations in observations.items():
        clock = observation_start
        for destination in destinations:
            number, x, z = destination["index"], destination["x"], destination["z"]
            prep["actions"].extend((
                {"id": f"stock_check_{number}_visit", "kind": "command", "bot": bot,
                 "window": "steady", "at_ms": clock,
                 "command": f"tp @s {x + .5} 64 {z + 2.5}"},
                {"id": f"stock_check_{number}_open", "kind": "open_container", "bot": bot,
                 "window": "steady", "at_ms": clock + 250, "position": destination["chest"]},
                {"id": f"stock_check_{number}_close", "kind": "close_container", "bot": bot,
                 "window": "steady", "at_ms": clock + 500},
            ))
            clock += 750
        observation_ends.append(clock)
    result["preparation_duration_ms"] = max(observation_ends) + 1000
    result["constraints"]["minimum_preparation_ms"] = result["preparation_duration_ms"]
    result["constraints"]["minimum_window_ms"] = {
        "steady": max(18000, max(cursors.values(), default=0) + 1000), "burst": 6000}
    result["constraints"]["finite_module_input"] = (
        "256 one-shot public give/menu grants of 64 nether wart before measurement; enabled "
        "downward hoppers feed one chest each. Preparation may consume finite transit; "
        "no steady refill or endless source. Public block-entity SNBT and /item replace "
        "are unsupported, so this is explicitly a public-menu substitute.")
    result["expected"]["stock"]["minecraft:nether_wart"] = 256 * 64
    result["expected"]["stock_modules"] = modules
    result.setdefault("scale", {}).update(players=clients, contention_players=25,
        sorter_modules=256, hoppers=258, storage_chests=258, module_hoppers=256,
        module_chests=256, container_probes=540, physical_container_positions=516,
        finite_module_nether_wart=16384, finite_core_dirt=1856, finite_item_stock=18240,
        industry_regions=4, preparation_clients=25, module_container_probes=512,
        shared_chest_wire_observers=24)


def _full_pistons(result: dict[str, Any]) -> None:
    # Direct coordinate generation: no textual translation of arbitrary commands.
    # All eight pistons straddle the identical x/z four-chunk corner (16, 16).
    # The upper source is y92; the public arena clears through y96 inclusive.
    result["commands"].append("fill 12 64 0 24 96 20 minecraft:air")
    for rig, y in enumerate((64, 72, 80, 88)):
        prefix = f"overlap_{rig}"
        result["commands"].append(f"fill 12 {y - 1} 0 24 {y - 1} 20 minecraft:stone")
        result["commands"].append(f"setblock 15 {y} 15 minecraft:gold_block")
        _probe(result, f"{prefix}_shared_passenger_origin", [15, y, 15], "air")
        _probe(result, f"{prefix}_shared_passenger_final", [16, y, 15], "gold_block")
        for machine, (z, adhesive) in enumerate(((14, "slime_block"), (16, "honey_block"))):
            result["commands"].extend((
                f"setblock 15 {y - 1} {z} minecraft:air",
                f"setblock 14 {y} {z} minecraft:piston[facing=east,extended=false]",
                f"setblock 15 {y} {z} minecraft:{adhesive}",
                f"setblock 15 {y + 1} {z} minecraft:gold_block",
            ))
            _probe(result, f"{prefix}_piston_{machine}_base", [14, y, z], "piston")
            _probe(result, f"{prefix}_piston_{machine}_power", [13, y, z], "redstone_block")
            for x in (15, 16, 17):
                for py in (y, y + 1):
                    final = (adhesive if (x, py) == (16, y) else
                             "gold_block" if (x, py) == (16, y + 1) else
                             "piston_head" if (x, py) == (15, y) else "air")
                    _probe(result, f"{prefix}_piston_{machine}_{x}_{py}", [x, py, z], final)
            _action(result, f"{prefix}_piston_{machine}",
                    f"setblock 13 {y} {z} minecraft:redstone_block", at=rig * 200 + machine * 50)
        result["commands"].extend((
            f"setblock 18 {y + 2} 4 minecraft:glass",
            f"setblock 18 {y + 3} 4 minecraft:sand",
            f"setblock 20 {y + 3} 4 minecraft:glass",
            f"setblock 20 {y + 4} 4 minecraft:water[level=0]",
        ))
        # Upper drops must reach their own floor without spilling onto lower
        # rigs or the neighboring combat enclosure.
        for dx, dz in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            result["commands"].append(f"setblock {20 + dx} {y + 4} {4 + dz} minecraft:glass")
            result["commands"].append(f"fill {20 + dx} {y} {4 + dz} {20 + dx} {y + 3} {4 + dz} minecraft:glass")
            _probe(result, f"{prefix}_water_wall_{dx}_{dz}", [20 + dx, y + 4, 4 + dz], "glass")
        for py in range(y, y + 4):
            _probe(result, f"{prefix}_sand_{py}", [18, py, 4], "sand" if py == y else "air")
        _probe(result, f"{prefix}_water_plug", [20, y + 3, 4], "water")
        _probe(result, f"{prefix}_water_source", [20, y + 4, 4], "water")
        _probe(result, f"{prefix}_water_fall", [20, y, 4], "water")
        _action(result, f"{prefix}_gravity", f"setblock 18 {y + 2} 4 minecraft:air", at=rig * 200 + 100)
        _action(result, f"{prefix}_water", f"setblock 20 {y + 3} 4 minecraft:air", at=rig * 200 + 150)
    _action(result, "neighbor_unload", "tp @a 512.5 100 512.5", at=1500)
    _action(result, "neighbor_reload", "tp @a .5 64 .5", at=3500)
    minimum = result["constraints"].setdefault("minimum_window_ms", {})
    minimum["burst"] = max(minimum.get("burst", 0), 10000)
    result.setdefault("scale", {}).update(piston_machines=8, overlap_rigs=4,
        shared_passengers=4, gravity_columns=4, finite_water_sources=4,
        piston_chunk_corner=[16, 16], piston_levels=[64, 72, 80, 88],
        piston_arena_max_y=96, neighbor_unload_reload_cycles=1)


def recipe(scenario_id: str, clients: int, smoke: bool) -> dict[str, Any] | None:
    if scenario_id not in IDS:
        return None
    if type(clients) is not int or clients <= 0:
        raise ValueError("clients must be a positive integer")
    result: dict[str, Any] = {"id": scenario_id, "commands": _scene(), "anchors": [[0, 0]],
        "group_size": clients, "view_distance": 4, "client_args": ["--no-edits"],
        "mechanic_spec": {"schema_version": 1, "entities": [], "blocks": [], "containers": [], "actions": []},
        "schedule_overrides": {phase: {"movement_hz": 0, "edit_hz": 0, "chat_hz": 0, "command_hz": 0, "commands": []}
                               for phase in ("steady", "burst", "recovery")},
        "constraints": {"minimum_clients": 2 if scenario_id == "MIX-08" else 1,
                        "minimum_window_ms": {"burst": 10000}},
        "restart_check": False, "capability": {"outcome": None},
        "expected": {"scenario_id": scenario_id, "clients": clients, "smoke": bool(smoke),
                     **deepcopy(_PREREQUISITES[scenario_id])}}
    if scenario_id in ("MIX-01", "mixed-survival-long"):
        if smoke:
            _village(result)
            result["constraints"]["minimum_clients"] = 2
        else:
            _full_village(result)
        result["expected"]["cohort_clients"] = max(0, clients - result.get("resident_clients", 1))
    if scenario_id == "MIX-02":
        (_stock if smoke else _full_stock)(result, clients)
    if scenario_id in ("MIX-04", "mixed-survival-long"):
        (_pistons if smoke else _full_pistons)(result)
    if scenario_id in ("MIX-21", "mixed-survival-long"):
        village_anchors = deepcopy(result["anchors"])
        _final_mutations(result)
        if not smoke and scenario_id == "mixed-survival-long":
            result["anchors"] = village_anchors
            result["view_distance"] = 6
            _settle_origin_residency(result)
    if scenario_id == "mixed-survival-long":
        result["constraints"]["minimum_measurement_ms"] = 0 if smoke else 600000
    if scenario_id == "MIX-08":
        result["client_args"].append("--respawn")
        mob_count = 2 if smoke else 128
        result["expected"]["known_mobs"] = mob_count
        result["expected"]["mob_uuids"] = []
        for n in range(mob_count):
            identity = 2_000_000 + n
            x, z = ((4.5 if n == 0 else -4.5, 4.5)
                    if smoke else (-24 + (n % 16) * 3 + .5, 8 + (n // 16) * 3 + .5))
            result["expected"]["mob_uuids"].append(f"00000000-0000-0000-0000-{identity:012x}")
            result["commands"].append(f"summon minecraft:cow {x} 64 {z} {{UUID:[I;0,0,0,{identity}]}}")
        result["mechanic_spec"]["entities"] = [{"kind": "minecraft:cow", "count": mob_count}]
        result.setdefault("scale", {}).update(known_mobs=mob_count, slow_readers=1)
        if not smoke:
            result["constraints"]["minimum_clients"] = 100
        result["mechanic_spec"]["actions"] += [
            {"id": "reader_limit", "kind": "reader_rate_limit", "bot": 1, "bytes_per_second": 1024, "window": "burst", "at_ms": 0},
            {"id": "reader_resume", "kind": "restore_reader", "bot": 1, "window": "recovery", "at_ms": 0}]
        _action(result, "death", "kill @s", 1000, bot=1)
        result["schedule_overrides"]["steady"]["chat_hz"] = 1
        result["schedule_overrides"]["burst"]["chat_hz"] = 1
    if scenario_id == "MIX-18":
        result["view_distance"] = 2
        result["commands"] = ["gamerule doMobSpawning false", "gamerule doDaylightCycle false",
            "fill -16 63 -16 16 63 16 minecraft:stone", "setworldspawn 0 64 0", "time set day"]
        result["initial_mechanic_spec"] = {"schema_version": 1, "entities": [], "blocks": [], "containers": [], "actions": []}
        # Fixed public waypoints exercise cold generation and an actual return,
        # independent of preflight safe-ground relocation and terrain auto-turns.
        # Move every reader beyond the target's view window before returning;
        # no runway setup may pre-generate the chunks being measured.
        result["constraints"]["minimum_window_ms"] = {"steady": 20000, "burst": 10000}
        route = [
            ("outbound", "steady", 0, 64),
            ("outbound_middle", "steady", 5000, 128),
            ("outbound_far", "steady", 10000, 192),
            ("return", "burst", 0, 128),
            ("return_target", "burst", 4000, 64),
        ]
        if not smoke:
            route = [(f"outbound_{n}", "steady", (n - 1) * 1000, n * 128) for n in range(1, 33)]
            route += [(f"return_{n}", "burst", (31 - n) * 1000, n * 128) for n in range(31, 0, -1)]
            result["constraints"]["minimum_window_ms"] = {"steady": 33000, "burst": 33000}
        target_x = 64 if smoke else 128
        result.setdefault("scale", {}).update(route_blocks=192 if smoke else 4096,
            route_waypoints=len(route), generated_save_buffer_capacity=1024,
            generated_save_batch_capacity=64, generated_save_inflight_jobs=1)
        result["expected"]["route"] = []
        for identifier, phase, at, x in route:
            _action(result, identifier, f"tp @a {x}.5 128 0.5", at, phase)
            result["expected"]["route"].append({"window": phase, "center": [x // 16, 0]})
        _probe(result, "returned_edit", [target_x, 64, 0], "diamond_block")
        _action(result, "edit", f"setblock {target_x} 64 0 minecraft:diamond_block", 8000 if smoke else 31000)
        result["restart_check"] = True
    else:
        result["preparation_regions"] = [{"name": scenario_id, "anchor": [32, 32],
            "view_distance": 8, "commands": result["commands"]}, *result.get("preparation_regions", [])]
    # The bot deliberately admits only the latest due action per poll. Declare
    # distinct fixed slots for each actor instead of demanding impossible
    # simultaneous writes or adding runtime catch-up/retry behaviour.
    previous: dict[tuple[int, str], int] = {}
    for action in sorted(result["mechanic_spec"]["actions"], key=lambda row: row["at_ms"]):
        key = (action["bot"], action["window"])
        action["at_ms"] = max(action["at_ms"], previous.get(key, -50) + 50)
        previous[key] = action["at_ms"]
    result.setdefault("scale", {}).update(requested_clients=clients, smoke=bool(smoke),
        entity_probes=sum(probe["count"] for probe in result["mechanic_spec"]["entities"]),
        block_probes=len(result["mechanic_spec"]["blocks"]),
        container_probes=len(result["mechanic_spec"]["containers"]),
        finite_actions=len(result["mechanic_spec"]["actions"]),
        preparation_regions=len(result.get("preparation_regions", [])),
        preparation_commands=sum(len(region["commands"]) for region in result.get("preparation_regions", [])))
    result["scale"]["view_distance"] = result["view_distance"]
    return result


def scale_receipt(fixture: dict[str, Any], native: dict[str, Any],
                  ready_clients: int | None = None) -> dict[str, Any]:
    """Keep authored demand separate from the actual persisted fixture census."""
    result = {"declared": deepcopy(fixture.get("scale", {})), "observed": None}
    if not native:
        return result
    entities = native.get("entities", {})
    counts = {kind: len(group.get("samples", [])) for kind, group in entities.items()}
    observed: dict[str, Any] = {"native_entities_by_kind": counts}
    persisted = native.get("persisted_chunks")
    if isinstance(persisted, list):
        observed["persisted_chunks"] = len(persisted)
        route_centers = {tuple(row["center"]) for row in fixture["expected"].get("route", [])}
        if route_centers:
            observed["persisted_route_centers"] = len(route_centers & {
                tuple(row["position"]) for row in persisted})
    if ready_clients is not None:
        observed["ready_clients"] = ready_clients
    owned = set(fixture["expected"].get("villager_uuids", []))
    present = {row["uuid"] for row in native.get("villagers", [])}
    if owned:
        observed["fixture_villagers"] = len(owned & present)
        observed["other_native_villagers"] = len(present - owned)
        identities = fixture["expected"]["villager_uuids"]
        size = fixture.get("scale", {}).get("villagers_per_quarter")
        if size:
            observed["complete_village_quarters"] = sum(
                set(identities[start:start + size]).issubset(present)
                for start in range(0, len(identities), size))
    known = set(fixture["expected"].get("mob_uuids", []))
    if known:
        observed["known_mobs"] = len(known & {
            row["uuid"] for group in entities.values() for row in group.get("samples", [])})
    blocks = {name: _block(native, name) for name in native.get("blocks", {})}
    observed["probed_blocks_by_kind"] = dict(Counter(
        row["name"] for row in blocks.values() if row.get("name") and row["name"] != "minecraft:air"))
    for category, prefixes in fixture.get("scale_probe_prefixes", {}).items():
        observed[category] = sum(name.startswith(tuple(prefixes)) and row.get("name") not in (None, "minecraft:air")
                                 for name, row in blocks.items())
    if fixture.get("scale", {}).get("piston_machines"):
        observed["piston_machines"] = sum(
            name.startswith("overlap_") and name.endswith("_base") and row.get("name") == "minecraft:piston"
            for name, row in blocks.items())
    modules = fixture["expected"].get("stock_modules", [])
    if modules:
        containers = native.get("containers", {})
        observed["sorter_modules"] = sum(
            blocks.get(row["hopper_probe"], {}).get("name") == "minecraft:hopper"
            and blocks.get(row["chest_probe"], {}).get("name") == "minecraft:chest" for row in modules)
        observed["sorter_modules_with_received_harvest"] = sum(
            _items(_object(containers.get(row["chest_probe"])).get("final")).get(row["item"], 0) > 0
            for row in modules)
        observed["module_harvest_items"] = sum(
            _items(_object(containers.get(row[key])).get("final")).get(row["item"], 0)
            for row in modules for key in ("hopper_probe", "chest_probe"))
        physical = {tuple(probe["position"]): _object(containers.get(probe["name"])).get("final")
                    for probe in fixture["mechanic_spec"]["containers"]}
        owners = list(physical.values()) + [row.get("payload", {}) for row in native.get("players", [])]
        owners += [row.get("payload", {}) for row in native.get("entity_records", []) if row.get("active")]
        totals = _items(owners)
        observed["finite_stock_by_item"] = {
            item: totals.get(item, 0) for item in fixture["expected"]["stock"]}
    result["observed"] = observed
    return result


def _object(value: Any) -> dict[str, Any]:
    return value if isinstance(value, dict) else {}


def _block(image: dict[str, Any], name: str) -> dict[str, Any]:
    return _object(_object(_object(image.get("blocks")).get(name)).get("final"))


def _samples(image: dict[str, Any], kind: str) -> list[dict[str, Any]]:
    return _object(_object(image.get("entities")).get(kind)).get("samples", [])


def _items(value: Any) -> dict[str, int]:
    """Sum serialized item stacks, not every incidental integer in NBT."""
    totals: dict[str, int] = {}
    if isinstance(value, dict):
        name, count = value.get("id", value.get("name")), value.get("count", value.get("Count"))
        if isinstance(name, str) and type(count) is int and count > 0:
            totals[name] = count
        else:
            for child in value.values():
                for item, amount in _items(child).items():
                    totals[item] = totals.get(item, 0) + amount
    elif isinstance(value, list):
        for child in value:
            for item, amount in _items(child).items():
                totals[item] = totals.get(item, 0) + amount
    return totals


def validate(scenario_id: str, report: dict[str, Any], fixture: dict[str, Any]) -> dict[str, Any]:
    failures: list[str] = []
    completed: dict[str, Any] = {}
    expected = fixture.get("expected", {})
    if scenario_id not in IDS or expected.get("scenario_id") != scenario_id:
        return {"passed": False, "failures": ["mixed recipe identity mismatch"], "infra_errors": [], "completed": {}}
    bots = _object(report.get("bots"))
    mechanics = _object(bots.get("mechanics"))
    native = _object(report.get("native_snapshot"))
    live = _object(_object(report.get("live_complete")).get("mechanics"))
    initial = _object(report.get("initial_native_snapshot"))
    cohort = _object(report.get("cohort"))
    cohort_bots = _object(cohort.get("bots"))
    cohort_clients = expected.get("cohort_clients", 0)
    if bots.get("ready_clients") != expected["clients"] - cohort_clients or (cohort_clients and cohort_bots.get("ready_clients") != cohort_clients):
        failures.append("not every requested resident/cohort client reached ready")
    if bots.get("disconnects", 0) or cohort_bots.get("disconnects", 0):
        failures.append("a cohort client disconnected")
    schedule = _object(bots.get("open_loop"))
    if schedule.get("schedule_complete") is not True:
        failures.append("finite wall schedule did not complete")
    for window in schedule.get("windows", []):
        if window.get("name") == "recovery":
            for kind in ("movement", "edit", "chat", "command"):
                if any(_object(window.get(kind)).get(key, 0) for key in ("offered", "admitted")):
                    failures.append(f"recovery offered new {kind} demand")
    receipts = {row.get("action"): row for row in mechanics.get("actions", [])}
    for index, action in enumerate(fixture["mechanic_spec"]["actions"]):
        row = receipts.get(index, {})
        if any(row.get(key) != 1 for key in ("offered", "admitted", "writes_completed")) or row.get("errors"):
            failures.append(f"finite stimulus {action.get('id', index)} was not admitted/written once")
    rejected = sum(1 for row in mechanics.get("command_replies", [])
                   if isinstance(row, dict) and row.get("text") == "Position is not loaded")
    if rejected:
        failures.append(f"origin command rejected: target chunk not resident ({rejected} {'reply' if rejected == 1 else 'replies'})")
    if not native.get("read_only") or not native.get("identity"):
        failures.append("read-only native final image/identity missing")
    for name, kind in expected.get("blocks", {}).items():
        if _block(native, name).get("name") != kind:
            failures.append(f"{name}: native final block differs from pinned {kind}")
    if expected.get("cohort"):
        if bots.get("chat_echo_clients") != expected["clients"] - cohort_clients or _object(cohort.get("complete")).get("chat_echo_clients") != cohort_clients:
            failures.append("not every cohort client has a received first-action chat echo")
        burst = _object(_object(report.get("windows")).get("burst"))
        start, end = burst.get("start_unix_ns"), burst.get("end_unix_ns")
        release = cohort.get("released_unix_ns")
        ready = _object(cohort.get("ready")).get("ready_unix_ns")
        completed_at = cohort.get("completed_unix_ns")
        complete = _object(_object(cohort.get("complete")).get("open_loop"))
        if (type(start) is not int or type(end) is not int or type(release) is not int
                or type(ready) is not int or type(completed_at) is not int
                or not start <= release <= ready <= completed_at < end
                or complete.get("schedule_complete") is not True):
            failures.append("fresh login cohort was not released and completed its first-action window inside burst")
        fixture_ids = set(expected.get("villager_uuids", []))
        villagers = [row for row in native.get("villagers", []) if not fixture_ids or row["uuid"] in fixture_ids]
        if len(villagers) != expected["villagers"]:
            failures.append("native villager population lost or duplicated")
        claims: set[tuple[int, ...]] = set()
        prior = {row["uuid"]: row for row in initial.get("villagers", [])}
        reassigned = worked = 0
        for villager in villagers:
            position = villager.get("job_site")
            if position is None:
                continue
            point = tuple(position)
            if position in expected["destroyed_workstations"] or point in claims:
                failures.append("destroyed or multiply claimed workstation retained a job-site memory")
            claims.add(point)
            before = prior.get(villager.get("uuid"), {})
            if before.get("job_site") in expected["destroyed_workstations"] and position != before.get("job_site"):
                reassigned += 1
            memory = _object(_object(villager.get("brain")).get("memories"))
            previous = _object(_object(before.get("brain")).get("memories"))
            now_work = _object(memory.get("minecraft:last_worked_at_poi")).get("value", 0)
            old_work = _object(previous.get("minecraft:last_worked_at_poi")).get("value", 0)
            if (type(now_work) is int and type(old_work) is int and now_work > old_work
                    and before.get("job_site") in expected["destroyed_workstations"] and position != before.get("job_site")):
                worked += 1
        if not claims:
            failures.append("no surviving workstation has an actual villager claimant")
        completed["valid_job_claims"] = len(claims)
        if not reassigned:
            failures.append("no villager with a destroyed initial job site re-acquired a surviving site")
        if not worked:
            failures.append("no positive native last_worked_at_poi change during the crisis")
        completed.update(reassigned_villagers=reassigned, working_villagers=worked)
    if scenario_id == "MIX-02":
        # Menu observers share a physical chest; custody is counted once by
        # position, not once per observer's separately attributed probe.
        containers = _object(native.get("containers"))
        physical = {tuple(probe["position"]): _object(containers.get(probe["name"])).get("final", {})
                    for probe in fixture["mechanic_spec"]["containers"]}
        owners = list(physical.values())
        owners += [row.get("payload", {}) for row in native.get("players", [])]
        owners += [row.get("payload", {}) for row in native.get("entity_records", []) if row.get("active")]
        totals = _items(owners)
        for item, amount in expected["stock"].items():
            if totals.get(item, 0) != amount:
                failures.append(f"finite stock {item}: expected {amount}, native owners contain {totals.get(item, 0)}")
        free = _items(_object(_object(native.get("containers")).get("free_chest")).get("final"))
        if free.get("minecraft:dirt", 0) <= 0:
            failures.append("independent free branch has no positive throughput")
        completed["native_stock"] = totals
    if fixture.get("restart_check"):
        restart = _object(report.get("restart"))
        after = _object(restart.get("after"))
        if restart.get("same_world") is not True or after.get("identity") != native.get("identity"):
            failures.append("native restart changed world identity/seed")
        if restart.get("fixture_regenerated") is not False or restart.get("repair_commands") != []:
            failures.append("native restart regenerated or repaired the source")
        for name, kind in expected.get("blocks", {}).items():
            if _block(after, name).get("name") != kind:
                failures.append(f"{name}: native restart lost the accepted final block")
            if scenario_id in ("MIX-18", "MIX-21", "mixed-survival-long") and name in ("returned_edit", "last_block"):
                if _block(live, name).get("name") != _block(after, name).get("name"):
                    failures.append(f"{name}: live recovery block disagrees with native restart")
                if _object(_object(live.get("blocks")).get(name)).get("update_count", 0) < 1:
                    failures.append(f"{name}: no live accepted final edit update was received")
        for entity in expected.get("entities", []):
            for label, image in (("live recovery", live), ("native restart", after)):
                rows = _samples(image, "minecraft:armor_stand") + image.get("absence_probes", [])
                rows = [row for row in rows if row.get("uuid") == entity["uuid"]]
                if len(rows) != 1 or rows[0].get("active") is not entity["active"] or (entity["active"] and rows[0].get("final_position") != entity["position"]):
                    failures.append(f"{label}: final entity {entity['uuid']} differs from pinned image")
        if expected.get("entities"):
            readers = live.get("players", [])
            resident_count = expected["clients"] - cohort_clients
            if len(readers) != resident_count:
                failures.append("per-reader final mutation images missing")
            for index, reader in enumerate(readers):
                if _object(_object(reader.get("final_blocks")).get("last_block")).get("name") != _block(after, "last_block").get("name"):
                    failures.append(f"reader {index}: final block differs from native restart")
                observed = reader.get("final_observed_entities", [])
                for entity in expected["entities"]:
                    rows = [row for row in observed if row.get("uuid") == entity["uuid"]]
                    if (entity["active"] and (len(rows) != 1 or rows[0].get("final_position") != entity["position"])) or (not entity["active"] and rows):
                        failures.append(f"reader {index}: final entity {entity['uuid']} differs from native restart")
        if expected.get("finite_blasts"):
            explosions = {tuple(row["center"]) for row in mechanics.get("explosions", [])
                          if isinstance(row, dict) and isinstance(row.get("center"), list) and len(row["center"]) == 3}
            if len(explosions) < expected["finite_blasts"] or _samples(native, "minecraft:tnt"):
                failures.append("finite declared blast stock did not complete with distinct received explosion centers")
            completed["unique_explosions"] = len(explosions)
        completed["same_world_restart"] = restart.get("same_world")
    if scenario_id == "MIX-18":
        chunks = initial.get("persisted_chunks")
        if not isinstance(chunks, list):
            failures.append("outbound route lacks the initial native chunk index")
        else:
            for waypoint in expected["route"]:
                if any(row.get("position") == waypoint["center"] for row in chunks):
                    failures.append(f"outbound route chunk {waypoint['center']} was already persisted before measurement")
                    break
        players = mechanics.get("players", [])
        trace = iter(players[0].get("chunk_center_trace", []) if players else [])
        for waypoint in expected["route"]:
            if not any(row.get("window") == waypoint["window"] and row.get("center") == waypoint["center"]
                       for row in trace):
                leg = "outbound" if waypoint["window"] == "steady" else "return"
                failures.append(f"{leg} route never received ordered chunk center {waypoint['center']}")
                break
    if scenario_id == "MIX-08":
        players = live.get("players", [])
        if len(players) != expected["clients"]:
            failures.append("per-reader live completion images missing")
        else:
            slow = players[1]
            for index, player in enumerate(players):
                if index == 1:
                    continue
                progress = _object(_object(player.get("reader_progress_by_window")).get("burst"))
                if progress.get("frames", 0) <= 0 or progress.get("decoded_bytes", 0) <= 0 or progress.get("position_updates", 0) <= 0:
                    failures.append(f"normal reader {index} did not progress while the slow reader was limited")
            if slow.get("deaths", 0) < 1 or slow.get("respawns", 0) < 1 or slow.get("reader_bytes_per_second") is not None:
                failures.append("slow reader did not restore and complete a real death/respawn")
            actual = slow.get("final_observed_entities", [])
            durable = {row["uuid"] for row in _samples(native, "minecraft:cow")}
            known = set(expected.get("mob_uuids", durable))
            identities = [row.get("uuid") for row in actual]
            if (len(identities) != len(set(identities)) or set(identities) - durable
                    or not known.issubset(identities) or len(known & durable) != expected["known_mobs"]):
                failures.append("slow reader final entity identities do not converge to the native population")
        if live.get("ghost_updates", 0) or live.get("duplicate_spawns", 0):
            failures.append("slow reader retained stale or duplicate entities after respawn")
    return {"passed": not failures, "failures": failures, "infra_errors": [], "completed": completed,
            "oracle": deepcopy(_PREREQUISITES[scenario_id])["invariants"]}
