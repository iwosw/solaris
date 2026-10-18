"""Offline city census and deterministic public-wire preparation for core v2.

Census a stopped native world's canonical export, never its live database. All
third-party worlds, exports, plans and receipts belong under ignored .analysis/.
Preparation JSON is consumed by player-scale-bots' existing 32-command fences.
"""
from __future__ import annotations

import argparse
from collections import Counter
import gzip
import json
import math
from pathlib import Path
import random
import struct
import uuid
import zlib

AIR = {"air", "cave_air", "void_air"}
CROPS = ("wheat", "carrots", "potatoes", "beetroots")
FARMLAND_PLANTS = set(CROPS) | {"melon_stem", "attached_melon_stem", "pumpkin_stem",
                               "attached_pumpkin_stem", "torchflower_crop", "pitcher_crop"}
JOBS = {"blast_furnace": "armorer", "smoker": "butcher",
        "cartography_table": "cartographer", "brewing_stand": "cleric",
        "composter": "farmer", "barrel": "fisherman", "fletching_table": "fletcher",
        "cauldron": "leatherworker", "water_cauldron": "leatherworker",
        "lava_cauldron": "leatherworker", "powder_snow_cauldron": "leatherworker",
        "lectern": "librarian", "stonecutter": "mason", "loom": "shepherd",
        "smithing_table": "toolsmith", "grindstone": "weaponsmith"}
PROFESSIONS = tuple((block, profession) for block, profession in JOBS.items()
                    if not block.endswith("_cauldron"))
GREEN = {"grass_block", "dirt", "coarse_dirt", "podzol", "moss_block"}
# Deliberately conservative full cubes, shared with the protocol driver's terrain
# contract. Partial blocks, leaves, foliage, liquids and snow layers are excluded.
SUPPORT = {"stone", "dirt", "grass_block", "sand", "red_sand", "gravel",
           "deepslate", "granite", "diorite", "andesite", "tuff", "calcite",
           "snow_block", "podzol", "mycelium", "coarse_dirt", "rooted_dirt",
           "clay", "terracotta", "cobblestone", "moss_block", "bedrock",
           "stone_bricks", "mossy_stone_bricks", "cracked_stone_bricks",
           "chiseled_stone_bricks", "mossy_cobblestone", "bricks", "sandstone",
           "smooth_sandstone", "cut_sandstone", "chiseled_sandstone",
           "red_sandstone", "smooth_red_sandstone", "cut_red_sandstone",
           "chiseled_red_sandstone", "smooth_stone", "polished_andesite",
           "polished_diorite", "polished_granite", "quartz_block",
           "smooth_quartz", "quartz_bricks", "chiseled_quartz_block"}
NAMESPACE = uuid.UUID("ae2ce27b-2e6d-5088-bab4-cbc19b97d4a3")


def full_cube(name):
    return name in SUPPORT or name.endswith(("_planks", "_concrete", "_terracotta", "_wool"))


def nbt(data):
    """Read-only named NBT; same bounded take pattern as vanilla_world_roundtrip."""
    data, offset = memoryview(data), 0

    def take(size):
        nonlocal offset
        if size < 0 or offset + size > len(data):
            raise ValueError("truncated NBT")
        value = data[offset:offset + size]
        offset += size
        return value

    def number(fmt):
        return struct.unpack(fmt, take(struct.calcsize(fmt)))[0]

    def text():
        # NBT uses modified UTF-8. Names and identifiers here are ASCII; retain
        # unrelated Java strings without allowing malformed UTF-8 to stop census.
        return bytes(take(number(">H"))).decode("utf-8", errors="surrogateescape")

    def payload(kind):
        formats = {1: ">b", 2: ">h", 3: ">i", 4: ">q", 5: ">f", 6: ">d"}
        if kind in formats:
            return number(formats[kind])
        if kind == 7:
            return bytes(take(number(">i")))
        if kind == 8:
            return text()
        if kind == 9:
            child, count = number(">B"), number(">i")
            if count < 0:
                raise ValueError("negative NBT list length")
            return [payload(child) for _ in range(count)]
        if kind == 10:
            result = {}
            while (child := number(">B")) != 0:
                name = text()
                if name in result:
                    raise ValueError(f"duplicate NBT name {name}")
                result[name] = payload(child)
            return result
        if kind in (11, 12):
            count = number(">i")
            if count < 0:
                raise ValueError("negative NBT array length")
            return [number(">i" if kind == 11 else ">q") for _ in range(count)]
        raise ValueError(f"unsupported NBT tag {kind}")

    kind = number(">B")
    text()
    result = payload(kind)
    if offset != len(data):
        raise ValueError("trailing named NBT bytes")
    return result


def region_chunks(path):
    """Stream modern Anvil records (gzip/zlib/raw, including external .mcc)."""
    _, rx, rz, _ = path.name.split(".")
    with path.open("rb") as stream:
        header = stream.read(8192)
        # Vanilla leaves unused region placeholders with no records/header.
        if not header:
            return
        if len(header) != 8192:
            raise ValueError(f"truncated region {path}")
        for index in range(1024):
            location = int.from_bytes(header[index * 4:index * 4 + 4], "big")
            if not location:
                continue
            sector, sectors = location >> 8, location & 255
            if sector < 2 or not sectors:
                raise ValueError(f"invalid location {path}:{index}")
            stream.seek(sector * 4096)
            size = int.from_bytes(stream.read(4), "big")
            compression = stream.read(1)[0]
            if size < 1 or size > sectors * 4096 - 4:
                raise ValueError(f"invalid record size {path}:{index}")
            cx, cz = int(rx) * 32 + index % 32, int(rz) * 32 + index // 32
            if compression & 128:
                payload = (path.parent / f"c.{cx}.{cz}.mcc").read_bytes()
            else:
                payload = stream.read(size - 1)
                if len(payload) != size - 1:
                    raise ValueError(f"truncated record {path}:{index}")
            compression &= 127
            if compression == 1:
                payload = gzip.decompress(payload)
            elif compression == 2:
                payload = zlib.decompress(payload)
            elif compression != 3:
                raise ValueError(f"unsupported Anvil compression {compression}")
            yield cx, cz, nbt(payload)


def section_cells(section):
    states = section.get("block_states")
    if states is None:
        return None
    palette = states["palette"]
    if not palette:
        raise ValueError("empty block palette")
    if len(palette) == 1:
        return palette, [0] * 4096
    bits = max(4, (len(palette) - 1).bit_length())
    per_word, mask = 64 // bits, (1 << bits) - 1
    words = states["data"]
    if len(words) != (4096 + per_word - 1) // per_word:
        raise ValueError("invalid padded Anvil block-state storage")
    cells = [(words[i // per_word] >> ((i % per_word) * bits)) & mask for i in range(4096)]
    if max(cells) >= len(palette):
        raise ValueError("palette index out of range")
    return palette, cells


def bounds(points):
    return {"min": list(map(min, zip(*points))), "max": list(map(max, zip(*points)))} if points else None


def stored_item_total(record, path):
    """Physical stored items, including legacy equipment; not derived views."""
    def stack(item, where):
        if not isinstance(item, dict):
            raise ValueError(f"{where}: expected item compound")
        if not item or item.get("id") in ("air", "minecraft:air"):
            return 0
        amount = item.get("count", item.get("Count"))
        if not isinstance(amount, int) or amount < 0 or "id" not in item:
            raise ValueError(f"{where}: expected item id and nonnegative integral count")
        total = amount
        legacy = item.get("tag", {})
        if not isinstance(legacy, dict):
            raise ValueError(f"{where}.tag: expected compound")
        total += stored_item_total(legacy.get("BlockEntityTag", {}), where + ".tag.BlockEntityTag")
        for key in ("Items", "ChargedProjectiles"):
            total += stacks(legacy, key, where + ".tag")
        components = item.get("components", {})
        total += component_items(components, where + ".components")
        return total

    def stacks(owner, key, where):
        values = owner.get(key, [])
        if not isinstance(values, list):
            raise ValueError(f"{where}.{key}: expected item list")
        return sum(stack(item, f"{where}.{key}[{i}]") for i, item in enumerate(values))

    def component_items(components, where):
        if not isinstance(components, dict):
            raise ValueError(f"{where}: expected components compound")
        total = 0
        if "minecraft:block_entity_data" in components:
            total += stored_item_total(components["minecraft:block_entity_data"],
                                       where + ".minecraft:block_entity_data")
        for key in ("minecraft:bundle_contents", "minecraft:charged_projectiles"):
            total += stacks(components, key, where)
        entries = components.get("minecraft:container", [])
        if not isinstance(entries, list):
            raise ValueError(f"{where}.minecraft:container: expected container list")
        for i, entry in enumerate(entries):
            if not isinstance(entry, dict) or "item" not in entry:
                raise ValueError(f"{where}.minecraft:container[{i}]: missing item compound")
            total += stack(entry["item"], f"{where}.minecraft:container[{i}].item")
        return total

    if not isinstance(record, dict):
        raise ValueError(f"{path}: expected inventory owner compound")
    total = sum(stacks(record, key, path) for key in
                ("Inventory", "EnderItems", "Items", "ArmorItems", "HandItems"))
    # HorseBodyArmorItemFix clears the legacy chest-slot copy of ArmorItem.
    # Exclude only an identical view; distinct payloads must still expose loss.
    armor = record.get("ArmorItems", [])
    if (record.get("id") in ("horse", "minecraft:horse") and len(armor) > 2
            and armor[2] == record.get("ArmorItem")):
        total -= stack(armor[2], path + ".ArmorItems[2]")
    for key in ("Item", "item", "ArmorItem", "DecorItem", "SaddleItem",
                "body_armor_item", "saddle", "weapon", "RecordItem", "Book", "Trident"):
        if key in record:
            total += stack(record[key], path + "." + key)
    equipment = record.get("equipment", {})
    if not isinstance(equipment, dict):
        raise ValueError(f"{path}.equipment: expected compound")
    total += sum(stack(item, path + ".equipment." + key) for key, item in equipment.items())
    # SaddleEquipmentSlotFix materializes this old attached-item flag as a stack.
    flag = record.get("Saddle")
    if (record.get("id") in ("pig", "minecraft:pig", "strider", "minecraft:strider")
            and isinstance(flag, int) and flag != 0
            and "saddle" not in record and "saddle" not in equipment):
        total += 1
    return total + component_items(record.get("components", {}), path + ".components")


def preservation_inventory(world):
    """Read-only record counts across every dimension, without terrain decoding."""
    counts = Counter(entities=0, block_entities=0, container_items=0, players=0, player_items=0)
    entity_types, players = Counter(), {}
    chunks = 0

    def records(owner, key, path):
        values = owner.get(key, [])
        if not isinstance(values, list) or any(not isinstance(v, dict) for v in values):
            raise ValueError(f"{path}.{key}: expected compound list")
        return values

    def entity(record, path):
        if not isinstance(record.get("id"), str):
            raise ValueError(f"{path}: missing entity id")
        counts["entities"] += 1
        entity_types[record["id"]] += 1
        counts["container_items"] += stored_item_total(record, path)
        for i, passenger in enumerate(records(record, "Passengers", path)):
            entity(passenger, f"{path}.Passengers[{i}]")
        if "Riding" in record:
            entity(record["Riding"], path + ".Riding")

    for path in sorted(world.rglob("r.*.*.mca")):
        if path.parent.name not in ("region", "entities"):
            continue
        for cx, cz, root in region_chunks(path):
            where = f"{path}#chunk[{cx},{cz}]"
            if path.parent.name == "entities":
                for i, body in enumerate(records(root, "Entities", where)):
                    entity(body, f"{where}.Entities[{i}]")
                continue
            chunks += 1
            chunk = root.get("Level", root)
            for key in ("TileEntities", "block_entities"):
                for i, block in enumerate(records(chunk, key, where)):
                    counts["block_entities"] += 1
                    counts["container_items"] += stored_item_total(block, f"{where}.{key}[{i}]")
            for key in ("Entities", "entities"):
                for i, body in enumerate(records(chunk, key, where)):
                    entity(body, f"{where}.{key}[{i}]")
    if not chunks:
        raise ValueError(f"{world}: no stored terrain chunks")

    def read_record(path):
        raw = path.read_bytes()
        return nbt(gzip.decompress(raw) if raw.startswith(b"\x1f\x8b") else raw)

    def player_id(record, fallback):
        identity = record.get("UUID")
        if isinstance(identity, list) and len(identity) == 4:
            bits = sum((word & 0xffffffff) << (96 - 32 * i) for i, word in enumerate(identity))
            return str(uuid.UUID(int=bits))
        if "UUIDMost" in record and "UUIDLeast" in record:
            return str(uuid.UUID(int=((record["UUIDMost"] & ((1 << 64) - 1)) << 64) |
                                 (record["UUIDLeast"] & ((1 << 64) - 1))))
        if fallback is None:
            raise ValueError(f"{world}/level.dat#Data.Player: missing player UUID")
        return str(uuid.UUID(fallback))

    level = read_record(world / "level.dat")
    embedded = level.get("Data", {}).get("Player")
    owner = player_id(embedded, None) if embedded is not None else None
    if embedded is not None:
        players[owner] = stored_item_total(embedded, f"{world}/level.dat#Data.Player")
    for directory in (world / "playerdata", world / "players/data"):
        for path in sorted(directory.glob("*.dat")):
            record = read_record(path)
            identity = player_id(record, path.stem)
            if identity == owner:
                continue  # Same authoritative embedded-owner rule as canonical import.
            if identity in players:
                raise ValueError(f"{path}: duplicate active player UUID {identity}")
            players[identity] = stored_item_total(record, str(path))
    counts["players"], counts["player_items"] = len(players), sum(players.values())
    return {"world": str(world.resolve()), "counts": dict(counts), "stored_chunks": chunks,
            "entities_by_type": dict(sorted(entity_types.items())), "player_items_by_uuid": dict(sorted(players.items()))}


def preservation(source, world):
    before, after = preservation_inventory(source), preservation_inventory(world)
    differences = {key: {"source": before["counts"][key], "native": after["counts"][key]}
                   for key in before["counts"] if before["counts"][key] != after["counts"][key]}
    if before["player_items_by_uuid"] != after["player_items_by_uuid"]:
        differences["player_items_by_uuid"] = {"source": before["player_items_by_uuid"],
                                              "native": after["player_items_by_uuid"]}
    return {"schema_version": 1, "status": "FAIL" if differences else "PASS",
            "source": before, "native_export": after, "differences": differences,
            "item_definition": "physical stored item units, including nested container/bundle/projectile stacks and equipment; counts legacy pig/strider Saddle flags and excludes identical horse ArmorItems[2]/ArmorItem views, trade templates and derived SelectedItem; embedded owner wins over its external copy"}


def census(world, *, collect_terrain=True):
    """Every stored overworld chunk; not a claim that every chunk is resident."""
    region = world / "dimensions/minecraft/overworld/region"
    if not region.is_dir():
        region = world / "region"
    paths = sorted(region.glob("r.*.*.mca"))
    if not paths:
        raise ValueError(f"no overworld Anvil regions in {world}")
    counts = Counter(beds=0, workstations=0, street_cells=0)
    blocks, jobs = Counter(), Counter()
    terrain = {"beds": [], "workstations": [], "farmland": [], "outdoor": []}
    chunks, regions = [], []
    for path in paths:
        _, rx, rz, _ = path.name.split(".")
        regions.append((int(rx), int(rz)))
        for cx, cz, chunk in region_chunks(path):
            if (chunk.get("xPos"), chunk.get("zPos")) != (cx, cz):
                raise ValueError(f"chunk location mismatch {path}:{cx},{cz}")
            if chunk.get("DataVersion", 0) < 2860:
                raise ValueError("census requires Java 1.18+; use the canonical offline upgrade")
            chunks.append((cx, cz))
            top = [None] * 256
            for section in sorted(chunk.get("sections", []), key=lambda s: s["Y"], reverse=True):
                decoded = section_cells(section)
                if decoded is None:
                    continue
                palette, cells = decoded
                names = [state["Name"].removeprefix("minecraft:") for state in palette]
                histogram = Counter(cells)
                for index, amount in histogram.items():
                    name = names[index]
                    blocks[name] += amount
                    if name == "water" or palette[index].get("Properties", {}).get("waterlogged") == "true":
                        counts["water_blocks"] += amount
                relevant = {i for i, name in enumerate(names) if name.endswith("_bed") or name in JOBS or
                            (collect_terrain and name == "farmland")}
                if relevant:
                    for i, index in enumerate(cells):
                        if index not in relevant:
                            continue
                        position = [cx * 16 + i % 16, section["Y"] * 16 + i // 256, cz * 16 + (i // 16) % 16]
                        name, props = names[index], palette[index].get("Properties", {})
                        if name.endswith("_bed") and props.get("part") == "head":
                            counts["beds"] += 1
                            if collect_terrain:
                                terrain["beds"].append({"position": position, "facing": props["facing"]})
                        elif name in JOBS:
                            counts["workstations"] += 1
                            if collect_terrain:
                                terrain["workstations"].append({"position": position, "profession": JOBS[name]})
                            jobs[JOBS[name]] += 1
                        elif name == "farmland":
                            above = i + 256
                            if above < 4096:
                                crop = names[cells[above]]
                            else:
                                upper = next((s for s in chunk["sections"] if s["Y"] == section["Y"] + 1), {})
                                upper_states = section_cells(upper)
                                crop = ("air" if upper_states is None else
                                        upper_states[0][upper_states[1][i % 256]]["Name"].removeprefix("minecraft:"))
                            terrain["farmland"].append({"position": position, "above": crop})
                if all(name in AIR for name in names):
                    continue
                for column in range(256):
                    if top[column] is not None:
                        continue
                    for y in range(15, -1, -1):
                        name = names[cells[y * 256 + column]]
                        if name not in AIR:
                            top[column] = (section["Y"] * 16 + y + 1, name)
                            break
            for column, surface in enumerate(top):
                if surface is not None and full_cube(surface[1]):
                    counts["street_cells"] += 1
                    if collect_terrain:
                        terrain["outdoor"].append([cx * 16 + column % 16, surface[0], cz * 16 + column // 16, surface[1]])
    entities = Counter()
    terrain["existing_villagers"] = []
    entity_region = region.parent / "entities"
    if not entity_region.is_dir():
        raise ValueError("canonical export entities directory missing; cannot infer preserved population")
    for path in sorted(entity_region.glob("r.*.*.mca")):
        for _, _, record in region_chunks(path):
            pending = list(record.get("Entities", []))
            while pending:
                entity = pending.pop()
                pending.extend(entity.get("Passengers", []))
                entities[entity["id"]] += 1
                if entity["id"] == "minecraft:villager" and collect_terrain:
                    identity = entity["UUID"]
                    bits = sum((word & 0xffffffff) << (96 - 32 * i) for i, word in enumerate(identity))
                    memories = entity.get("Brain", {}).get("memories", {})
                    point = lambda name: memories.get("minecraft:" + name, {}).get("value", {}).get("pos")
                    terrain["existing_villagers"].append({"uuid": str(uuid.UUID(int=bits)), "position": entity["Pos"],
                                                         "home": point("home"), "job_site": point("job_site")})
    for value in terrain.values():
        value.sort(key=lambda p: p["position"] if isinstance(p, dict) else p[:3])
    counts.update(chunks=len(chunks), regions=len(paths), farmland=blocks["farmland"],
                  crop_blocks=sum(blocks[name] for name in FARMLAND_PLANTS))
    counts["existing_villagers"] = entities["minecraft:villager"]
    counts["existing_entities"] = sum(entities.values())
    return {"schema_version": 1, "world": str(world.resolve()), "counts": dict(counts),
            "chunk_bounds": bounds(chunks), "region_bounds": bounds(regions),
            "workstations_by_profession": dict(sorted(jobs.items())),
            "existing_entities_by_type": dict(sorted(entities.items())),
            "street_definition": "topmost non-air full-cube terrain with sky open above; conservative canonical bot support whitelist; roofs count, partial shapes/foliage/water excluded",
            "resident_chunks": None, "resident_note": "stored chunks, not simultaneous runtime residency",
            "blocks": dict(sorted(blocks.items()))}, terrain if collect_terrain else None


def neighbors(point, cells):
    x, y, z = point
    for dx, dz in ((-1, 0), (0, -1), (0, 1), (1, 0)):
        other = cells.get((x + dx, z + dz))
        if other is not None and abs(other[1] - y) <= 1:
            yield other[:3]


def population_plan(terrain, villagers, animals, bots, seed, center=(0, 0), radius=None):
    if villagers < 0 or animals < 0 or bots < 1:
        raise ValueError("nonnegative population and at least one bot required")
    existing = sorted(terrain.get("existing_villagers", []), key=lambda v: v["uuid"])
    target_villagers = villagers
    villagers -= len(existing)
    if villagers < 0:
        raise ValueError("target is below the preserved existing villager population")
    rng = random.Random(seed)
    outdoor = [p for p in terrain["outdoor"] if radius is None or
               max(abs(p[0] - center[0]), abs(p[2] - center[1])) <= radius]
    cells = {(p[0], p[2]): p for p in outdoor}
    if len(cells) != len(outdoor):
        raise ValueError("duplicate outdoor columns")
    if not cells:
        raise ValueError("no outdoor walkable cells")
    # Long connected streets first; no graph edges can cross water or a missing
    # column. Edge height changes are <= one full block, as in the wire driver.
    unseen, components = set(cells), []
    while unseen:
        key = next(iter(unseen))
        todo, component = [cells[key][:3]], []
        unseen.remove(key)
        while todo:
            point = todo.pop()
            component.append(point)
            for neighbor in neighbors(point, cells):
                key = (neighbor[0], neighbor[2])
                if key in unseen:
                    unseen.remove(key)
                    todo.append(neighbor)
        if len(component) >= 16:
            components.append(sorted(component))
    components.sort(key=lambda c: (-len(c), c[0]))
    if not components:
        raise ValueError("no connected street component of at least 16 cells")
    connected = {tuple(p) for p in components[0]}
    # Preserve existing POIs; only add complete modules on untouched flat open
    # cells when the actual city has insufficient beds/workstations.
    reserved = {(math.floor(v["position"][0]), math.floor(v["position"][2])) for v in existing}
    commands = ["gamerule doMobSpawning false", "gamerule doDaylightCycle false", "time set 3000", "weather clear"]
    residents = []
    claimed_beds = {tuple(v["home"]) for v in existing if v.get("home") is not None}
    claimed_jobs = {tuple(v["job_site"]) for v in existing if v.get("job_site") is not None}
    existing_beds = sorted((b for b in terrain["beds"] if tuple(b["position"]) not in claimed_beds and
                            (radius is None or max(abs(b["position"][0] - center[0]), abs(b["position"][2] - center[1])) <= radius)),
                           key=lambda b: b["position"])
    existing_jobs = sorted((b for b in terrain["workstations"] if tuple(b["position"]) not in claimed_jobs and
                            (radius is None or max(abs(b["position"][0] - center[0]), abs(b["position"][2] - center[1])) <= radius)),
                           key=lambda b: b["position"])
    available_jobs = list(existing_jobs)
    # Green sites precede other outdoor cubes; stable seeded shuffling avoids a
    # dependence on region traversal or input JSON ordering.
    candidates = sorted(outdoor, key=lambda p: p[:3])
    rng.shuffle(candidates)
    candidates.sort(key=lambda p: p[3] not in GREEN)
    candidate_cursor = 0
    added = 0
    for index in range(villagers):
        job = bed = spawn = None
        if index < len(existing_beds):
            home = existing_beds[index]["position"]
            nearby = [j for j in available_jobs if max(abs(j["position"][a] - home[a]) for a in (0, 2)) <= 32]
            if nearby:
                nearby.sort(key=lambda j: (sum((j["position"][a] - home[a]) ** 2 for a in range(3)), j["position"]))
                job = nearby[0]
                possible = [p for p in outdoor if (p[0], p[2]) not in reserved and
                            abs(p[0] - home[0]) + abs(p[2] - home[2]) <= 3 and abs(p[1] - home[1]) <= 1]
                if possible:
                    spawn = min(possible, key=lambda p: p[:3])[:3]
                    reserved.add((spawn[0], spawn[2]))
                    bed = home
                    available_jobs.remove(job)
        if bed is None:
            while candidate_cursor < len(candidates):
                p = candidates[candidate_cursor]
                candidate_cursor += 1
                x, y, z = p[:3]
                footprint = [(x + dx, z + dz) for dx in range(3) for dz in range(3)]
                if any(q in reserved or q not in cells or cells[q][1] != y for q in footprint):
                    continue
                # Keep modules off the chosen bot paths below by removing all
                # occupied module cells from the navigation graph.
                reserved.update(footprint)
                block, profession = PROFESSIONS[index % len(PROFESSIONS)]
                bed, job, spawn = [x + 1, y, z], {"position": [x + 2, y, z + 2], "profession": profession}, [x, y, z + 2]
                commands.extend((f"setblock {x} {y} {z} minecraft:red_bed[facing=east,part=foot]",
                                 f"setblock {x + 1} {y} {z} minecraft:red_bed[facing=east,part=head]",
                                 f"setblock {x + 2} {y} {z + 2} minecraft:{block}"))
                added += 1
                break
            if bed is None:
                raise ValueError(f"only {index} villagers fit: no more disjoint flat 3x3 open sites")
        identity = str(uuid.uuid5(NAMESPACE, f"{seed}:villager:{index}"))
        # Summon intentionally leaves vanilla brain/POI acquisition live. The
        # canonical summon codec does not admit VillagerData/Brain authoring.
        commands.append(f'summon minecraft:villager {spawn[0] + .5} {spawn[1]} {spawn[2] + .5} {{UUID:"{identity}",PersistenceRequired:1b,CustomName:\'{{"text":"constantinople-resident"}}\'}}')
        residents.append({"uuid": identity, "position": spawn, "home": bed, "job_site": job["position"], "planned_profession": job["profession"]})
    for i, field in enumerate(terrain["farmland"]):
        x, y, z = field["position"]
        above = field["above"]
        if above is not None and above not in AIR and above not in FARMLAND_PLANTS and above != "water":
            raise ValueError(f"farmland at {x},{y},{z} is obstructed by {above}")
        crop = CROPS[i % len(CROPS)]
        commands.append(f"setblock {x} {y + 1} {z} minecraft:{crop}[age=0]")
    animal_cells = sorted((p for p in outdoor if p[3] in GREEN and (p[0], p[2]) not in reserved), key=lambda p: p[:3])
    rng.shuffle(animal_cells)
    if len(animal_cells) < animals:
        raise ValueError("not enough distinct open green animal cells")
    herd = []
    for index, p in enumerate(animal_cells[:animals]):
        kind = ("sheep", "chicken", "pig", "cow")[index % 4]
        identity = str(uuid.uuid5(NAMESPACE, f"{seed}:{kind}:{index}"))
        commands.append(f'summon minecraft:{kind} {p[0] + .5} {p[1]} {p[2] + .5} {{UUID:"{identity}",PersistenceRequired:1b}}')
        herd.append({"uuid": identity, "kind": kind, "position": p[:3]})
    graph_cells = {k: p for k, p in cells.items() if k not in reserved and tuple(p[:3]) in connected}
    graph_nodes = sorted((p[:3] for p in graph_cells.values()))
    node_index = {tuple(p): i for i, p in enumerate(graph_nodes)}
    edges = [[node_index[tuple(q)] for q in neighbors(p, graph_cells)] for p in graph_nodes]
    route_starts = [i for i, adjacent in enumerate(edges) if adjacent]
    if len(route_starts) < bots:
        raise ValueError("insufficient connected street starts after placing POIs")
    rng.shuffle(route_starts)
    routes = []
    for start in route_starts[:bots]:
        route, previous, current = [start], None, start
        for _ in range(127):
            choices = [n for n in edges[current] if n != previous] or edges[current]
            if not choices:
                break
            following = choices[rng.randrange(len(choices))]
            route.append(following)
            previous, current = current, following
        # Retracing makes a closed cardinal route without inventing a shortcut
        # from its last point back to its first point.
        closed = route + route[-2:0:-1]
        routes.append([graph_nodes[i] for i in closed])
    commands.append("save-all flush")
    resident_chunks = sorted({(p[0] // 16, p[2] // 16) for p in (r["position"] for r in residents + herd)})
    regional = {}
    for command in commands[4:-1]:
        words = command.split()
        offset = 1 if words[0] == "setblock" else 2
        x, z = float(words[offset]), float(words[offset + 2])
        regional.setdefault((int(x // 128), int(z // 128)), []).append(command)
    preparation_regions = []
    safe = {(p[0], p[2]): p for p in terrain["outdoor"] if (p[0], p[2]) not in reserved}
    for (rx, rz), group in sorted(regional.items()):
        anchors = sorted((p for p in safe.values() if rx * 128 <= p[0] < (rx + 1) * 128 and
                          rz * 128 <= p[2] < (rz + 1) * 128),
                         key=lambda p: ((p[0] - rx * 128 - 64) ** 2 + (p[2] - rz * 128 - 64) ** 2, p[:3]))
        anchor = next((p for p in anchors if any(neighbors(p[:3], safe))), None)
        if anchor is None:
            raise ValueError(f"preparation region {rx},{rz} has no connected dry anchor")
        neighbor = next(neighbors(anchor[:3], safe))
        preparation_regions.append({"name": f"city_{rx}_{rz}", "anchor": [anchor[0], anchor[2]],
                                    "street_routes": [[anchor[:3], neighbor]], "commands": commands[:4] + group})
    return {"schema_version": 1, "seed": seed, "villagers": residents, "animals": herd,
            "target_villagers": target_villagers, "existing_villagers": existing,
            "added_poi_modules": added, "seeded_farmland": len(terrain["farmland"]),
            "assignment_scope": "exclusive planned home/job pairs; vanilla live brain must acquire and verify claims; no fake Brain SNBT",
            "population_chunks": [list(p) for p in resident_chunks],
            "street_graph": {"nodes": graph_nodes, "edges": edges}, "street_routes": routes,
            "bot_anchors": [[r[0][0], r[0][2]] for r in routes],
            "preparation_regions": preparation_regions,
            "commands": {"setup": commands, "recurring": []}}


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")

async def smoke(args):
    """Use the catalog's ordinary process ownership, wire fences and snapshots."""
    import asyncio
    import os
    from tools.harness.backends import catalog_load as catalog
    if not os.environ.get("SOLARIS_HARNESS_CPU_SCOPE"):
        raise RuntimeError("run the harness under .analysis/tools/mem-run.sh")
    plan = json.loads((args.plan / "plan.json").read_text())
    if plan["target_villagers"] > 5000 or len(plan["animals"]) > 500 or len(plan["street_routes"]) > 10:
        raise ValueError("phase-1 smoke is capped at 5000 villagers, 500 animals, 10 bots")
    args.out.mkdir(parents=True, exist_ok=False)
    clients = len(plan["street_routes"])
    args.readiness_seconds = args.timeout_seconds
    report = {"status": "FAIL", "performance_claim": False, "plan": str(args.plan),
              "commands_sha256": catalog.digest(plan["commands"]), "phases": []}
    expected = {row["uuid"] for row in plan["villagers"] + plan["animals"] + plan["existing_villagers"]}
    initial = None
    try:
        for name in ("populate", "reopen"):
            directory = args.out / name
            directory.mkdir()
            config, url, game = catalog.config(args, directory, args.world, clients,
                                                operators=True, name_prefix="city")
            session = catalog.Session(args, directory)
            if name == "populate":
                access = json.loads((directory / "ops.json").read_text())
                username = "load_setup_0000"
                import hashlib
                offline = uuid.UUID(bytes=hashlib.md5(("OfflinePlayer:" + username).encode()).digest(), version=3)
                access.append({"uuid": str(offline), "name": username, "level": 4, "bypassesPlayerLimit": False})
                dump(directory / "ops.json", access)
            row = {"phase": name}
            try:
                await session.spawn("server", [str(args.server_bin), "--config", str(config), "--no-console"])
                row["ready"] = await catalog.wait_dashboard(session, url, args.timeout_seconds)
                row["baseline_processes"] = session.process_samples()
                if name == "populate":
                    row["preparation_regions"] = []
                    for region in plan["preparation_regions"]:
                        row["preparation_regions"].append(await catalog.public_region(
                            args, directory / region["name"], game, region, save_command="save-all flush"))
                command = catalog.bot_command(args, directory, game, clients, {"group_size": 1})
                command += ["--no-edits", "--street-routes", str(args.plan / "street-routes.json"),
                            "--name-prefix", "city", "--open-loop"]
                schedule = {"windows": [{"name": "streets", "duration_ms": args.walk_seconds * 1000,
                                          "movement_hz": 20, "chat_hz": 1}]}
                dump(directory / "schedule.json", schedule)
                command += ["--schedule-file", str(directory / "schedule.json")]
                await session.spawn("bots", command, events=True)
                row["bots_ready"] = await session.ready(clients, args.timeout_seconds)
                proc = session.processes["bots"]
                proc.stdin.write(b'{"event":"run"}\n')
                await proc.stdin.drain()
                row["schedule_complete"] = await catalog._event(session, "schedule_complete", args.timeout_seconds)
                row["processes"] = session.process_samples()
                if name == "populate" and plan["villagers"]:
                    delta = row["processes"]["server"]["rss_bytes"] - row["baseline_processes"]["server"]["rss_bytes"]
                    row["memory_info_only"] = {"rss_delta_bytes": delta,
                        "rss_delta_per_added_villager_bytes": delta / len(plan["villagers"]),
                        "scope": "includes changed chunk residency, block preparation, animals, clients and allocator retention; not intrinsic per-villager memory or a 500k projection"}
                row["stats"] = await session.http(url + "/stats", 10)
                dump(directory / "profile-info-only.json", await session.http(url + "/profile", 10))
            finally:
                await session.stop()
                row["shutdown"] = session.shutdown
                report["phases"].append(row)
                dump(args.out / "receipt.json", report)
            if any(item["forced"] or item["returncode"] for item in session.shutdown.values()):
                raise RuntimeError(f"{name}: unclean child shutdown")
            bot_report = json.loads((directory / "bots-report.json").read_text())
            if bot_report.get("disconnects", 0) or bot_report.get("walking_distance_blocks", 0) <= 0:
                raise RuntimeError(f"{name}: bots did not walk the received streets")
            row["walking_distance_blocks"] = bot_report["walking_distance_blocks"]
            walkers = bot_report.get("open_loop", {}).get("bots", [])
            if len(walkers) != clients or any(bot.get("walking_distance_blocks", 0) <= 0 or
                                              bot.get("street_waypoints_reached", 0) < 2 for bot in walkers):
                raise RuntimeError(f"{name}: not every requested bot walked an authored street edge")
            row["walkers"] = walkers
            snapshot = await catalog.native_snapshot(args, directory / "native-snapshot", args.world,
                                                      config, {"entities": [], "blocks": [], "containers": []}, initial)
            observed = {sample["uuid"] for group in snapshot["entities"].values() for sample in group["samples"]}
            missing = sorted(expected - observed)
            row["persisted_cohort"] = len(expected - set(missing))
            row["missing_uuids"] = missing
            villagers = [v for v in snapshot["villagers"] if v["uuid"] in expected]
            row["actual_villagers"] = len(villagers)
            row["villagers_moved_between_checkpoints"] = sum(
                sample["initial_position"] != sample["final_position"]
                for sample in snapshot["entities"].get("minecraft:villager", {}).get("samples", [])
                if sample["uuid"] in expected) if initial is not None else None
            row["actual_professions"] = dict(Counter(v["profession"] for v in villagers))
            row["actual_home_claims"] = sum(v["home"] is not None for v in villagers)
            row["actual_job_claims"] = sum(v["job_site"] is not None for v in villagers)
            if missing:
                raise RuntimeError(f"{name}: {len(missing)} authored bodies missing from durable checkpoint")
            initial = snapshot
        report["status"] = "PASS"
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        dump(args.out / "receipt.json", report)
        print(json.dumps({"status": report["status"], "receipt": str(args.out / "receipt.json")}), flush=True)



def native_settlement_commands(population):
    """Existing native operator commands, not fixture-created memberships."""
    anchors, cohort = population["settlement_anchors"], population["cohort"]
    if not 0 < len(cohort) <= 5000 or len(cohort) != population["count"]:
        raise ValueError("native smoke requires the complete bounded population cohort")
    commands = []
    for anchor in anchors:
        commands.append("setworldspawn " + " ".join(map(str, anchor["anchor"])))
        commands.append(f'settlement create {anchor["name"]} {anchor["radius"]}')
    commands.extend(f'settlement bind {row["settlement"]} {row["uuid"]}' for row in cohort)
    commands.append("setworldspawn " + " ".join(map(str, population["spawn_position"])))
    commands.extend(f'settlement info {anchor["name"]}' for anchor in anchors)
    return commands


async def native_census(args, directory, config, name):
    import asyncio
    receipt = directory / f"{name}.json"
    command = [str(args.server_bin), "--config", str(config), "world", "census",
               "--source", str(args.world), "--receipt", str(receipt)]
    with (directory / f"{name}.log").open("wb") as log:
        process = await asyncio.create_subprocess_exec(*command, stdout=log, stderr=log)
        try:
            await asyncio.wait_for(process.wait(), args.timeout_seconds)
        except BaseException:
            if process.returncode is None:
                process.kill()
                await process.wait()
            raise
    if process.returncode:
        raise RuntimeError(f"native census failed; see {directory / (name + '.log')}")
    result = json.loads(receipt.read_text())
    if result.get("status") != "PASS" or result.get("read_only") is not True:
        raise RuntimeError(f"native census did not report read-only PASS: {receipt}")
    return result


def native_rss_event(session, event, **context):
    """Persist canonical /proc samples at existing lifecycle/feedback events."""
    import time
    process = session.processes["server"]
    sample = {"event": event, "phase": session.directory.name,
              "unix_ns": time.time_ns(), "monotonic_ns": time.monotonic_ns(), **context}
    if process.returncode is None:
        sample.update(session.process_samples()["server"])
    else:
        sample.update(pid=process.pid, returncode=process.returncode, rss_bytes=None)
    with (session.directory / "rss-events.jsonl").open("a") as stream:
        stream.write(json.dumps(sample, sort_keys=True) + "\n")
    return sample


async def native_console_setup(session, population, timeout, url):
    import asyncio
    import re
    import time
    from tools.harness import runtime
    expected = {row["uuid"]: row["settlement"] for row in population["cohort"]}
    founded, bound = set(), set()
    names = {row["name"] for row in population["settlement_anchors"]}
    last = population["settlement_anchors"][-1]["name"] + " | owner=Server "
    commands = native_settlement_commands(population)
    dump(session.directory / "native-commands.json", commands)
    stream = (session.directory / "server.log").open(errors="replace")
    remainder = ""
    finished = False

    def received(done):
        nonlocal remainder, finished
        session.alive()
        remainder += stream.read()
        lines = remainder.split("\n")
        remainder = lines.pop()
        for line in lines:
            if "Command failed:" in line or "Console input failed:" in line or \
                    re.search(r"\bERROR\b|panicked at", line):
                raise RuntimeError(f"native console error: {line}")
            if line.startswith("checkpoint capture ms:"):
                native_rss_event(session, "checkpoint_capture", bound_count=len(bound),
                                 detail=line)
            created = re.search(r"Founded (\S+) \(revision \d+\)\.", line)
            if created and created[1] in names:
                founded.add(created[1])
                native_rss_event(session, "founded", name=created[1],
                                 founded_count=len(founded), bound_count=len(bound))
            member = re.search(r"Bound ([0-9a-f-]{36}) to (\S+)\.", line)
            if member and expected.get(member[1]) == member[2]:
                bound.add(member[1])
                if len(bound) == 1 or len(bound) % 100 == 0:
                    native_rss_event(session, "bound", bound_count=len(bound),
                                     settlement=member[2], uuid=member[1])
            finished |= last in line
        return (done(),
                {"founded": sorted(founded), "bound_villagers": len(bound)})

    process = session.processes["server"]

    async def exchange(batch, done):
        process.stdin.write(("\n".join(batch) + "\n").encode())
        _, result = await asyncio.gather(process.stdin.drain(), asyncio.to_thread(
            runtime.wait_for_path_condition, session.directory / "server.log", timeout,
            lambda: received(done)))
        return result

    try:
        async with asyncio.timeout(timeout):
            first_bind = 2 * len(population["settlement_anchors"])
            after_binds = first_bind + len(expected)
            await exchange(commands[:first_bind], lambda: founded == names)
            start = await native_game_clock(session, url, timeout)
            started = time.monotonic()
            await exchange(commands[first_bind:after_binds], lambda: len(bound) == len(expected))
            elapsed = time.monotonic() - started
            end = await native_game_clock(session, url, timeout)
            ticks = end["latest_tick"] - start["latest_tick"]
            if ticks <= 0 or end["instance_started_unix_ns"] != start["instance_started_unix_ns"]:
                raise RuntimeError("native bind clock did not advance in the same instance")
            result = await exchange(commands[after_binds:], lambda: finished)
            result.update(bind_seconds=elapsed, start_tick=start["latest_tick"],
                          end_tick=end["latest_tick"], bind_ticks=ticks,
                          binds_per_second=len(bound) / elapsed, binds_per_tick=len(bound) / ticks)
        return result
    finally:
        stream.close()


async def native_game_clock(session, url, timeout):
    import time
    deadline = time.monotonic() + timeout
    while True:
        session.alive()
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("native game clock unavailable")
        sample = await session.http(
            url + "/load-metrics?wait=true&min_samples=1", remaining)
        if sample["latest_tick"] is not None:
            return sample
        if sample.get("wait_timed_out") is not True:
            raise RuntimeError("native game clock unavailable")


async def native_cohort_snapshot(args, directory, config, population):
    """Prove exact durable UUID membership with the existing read-only probe."""
    from tools.harness.backends import catalog_load as catalog
    snapshot = await catalog.native_snapshot(
        args, directory, args.world, config, {"entities": [], "blocks": [], "containers": []})
    names = {row["payload"]["value"]["id"]: row["payload"]["value"]["name"]
             for key, row in snapshot["globals"].items() if key.startswith("settlement/")}
    expected = {row["uuid"]: row["settlement"] for row in population["cohort"]}
    actual, seen = {}, set()
    for row in snapshot["entity_records"]:
        if row["uuid"] in seen:
            raise RuntimeError(f"duplicate native UUID: {row['uuid']}")
        seen.add(row["uuid"])
        if row["active"] and row["uuid"] in expected:
            payload = row["payload"]
            if payload["id"] == "minecraft:villager":
                actual[row["uuid"]] = names.get(
                    payload.get("solaris:VillagerRuntime", {}).get("settlement"))
    if actual != expected:
        wrong = [identity for identity in expected if actual.get(identity) != expected[identity]]
        raise RuntimeError(f"native bound UUID membership mismatch: {wrong[:5]}")
    return {"verified_bound_uuids": len(actual), "snapshot": str(directory / "result.json"),
            "source_world_sha256": snapshot["source_world"]["sha256"]}


async def native_smoke(args):
    """Current native server, real create/bind commands, game-tick fence, reopen."""
    import asyncio
    import os
    import time
    from tools.harness.backends import catalog_load as catalog
    if not os.environ.get("SOLARIS_HARNESS_CPU_SCOPE"):
        raise RuntimeError("run the harness under .analysis/tools/mem-run.sh")
    population = json.loads((args.world / "population.json").read_text())
    args.readiness_seconds = args.timeout_seconds
    args.out.mkdir(parents=True, exist_ok=False)
    plugins = args.out / "empty-plugins"
    plugins.mkdir()
    report = {"status": "FAIL", "performance_claim": False, "server_bin": str(args.server_bin),
              "world": str(args.world), "plugins": str(plugins), "phases": []}
    baseline = previous = None
    try:
        for phase in (("reopen",) if args.reopen_only else ("run", "reopen")):
            directory = args.out / phase
            directory.mkdir()
            config, url, _ = catalog.config(args, directory, args.world, 0,
                                            operators=False, plugins_dir=plugins)
            if baseline is None:
                baseline = await native_census(args, directory, config, "before")
                report["before"] = baseline
                previous = baseline
            session = catalog.Session(args, directory)
            row = {"phase": phase, "rss_events": str(directory / "rss-events.jsonl")}
            try:
                await session.spawn("server", [str(args.server_bin), "--config", str(config)],
                                    console=True)
                native_rss_event(session, "spawned")
                row["ready"] = await catalog.wait_dashboard(session, url, args.timeout_seconds)
                native_rss_event(session, "dashboard_ready")
                if "plugin" not in row["ready"] or row["ready"]["plugin"] is not None:
                    raise RuntimeError("native smoke requires an empty plugin runtime")
                if phase == "run":
                    row["native_commands"] = await native_console_setup(
                        session, population, args.setup_timeout_seconds, url)
                    native_rss_event(session, "bind_fence", bound_count=population["count"])
                start = await native_game_clock(session, url, args.timeout_seconds)
                cursor = start["latest_tick"]
                target = cursor + (args.ticks if phase == "run" else 2)
                row.update(start_tick=cursor, target_tick=target)
                native_rss_event(session, "tick_fence_start", start_tick=cursor, target_tick=target)
                deadline = time.monotonic() + args.timeout_seconds
                while cursor < target:
                    session.alive()
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise TimeoutError(f"{phase}: native game tick fence expired")
                    sample = await session.http(
                        url + f"/load-metrics?after_tick={cursor}&wait=true&min_samples=20", remaining)
                    if sample["instance_started_unix_ns"] != start["instance_started_unix_ns"] or \
                            sample.get("overrun"):
                        raise RuntimeError("native game clock changed instance or lost its cursor")
                    cursor = sample["latest_tick"] or cursor
                row["observed_tick"] = cursor
                native_rss_event(session, "tick_fence_end", observed_tick=cursor)
                row["stats"] = await session.http(url + "/stats", 12)
            finally:
                stop_started = time.monotonic()
                try:
                    if "server" in session.processes:
                        native_rss_event(session, "shutdown_requested")
                finally:
                    await session.stop(server_timeout=args.timeout_seconds)
                    row["shutdown_seconds"] = time.monotonic() - stop_started
                    row["shutdown"] = session.shutdown
                    report["phases"].append(row)
                    dump(args.out / "receipt.json", report)
                if "server" in session.processes:
                    native_rss_event(session, "process_exit", shutdown_seconds=row["shutdown_seconds"])
            if any(child["forced"] or child["returncode"] for child in session.shutdown.values()):
                raise RuntimeError(f"{phase}: unclean server shutdown")
            after = await native_census(args, directory, config, "after")
            row["census"] = after
            row["entity_count_delta"] = after["counts"]["entities"] - previous["counts"]["entities"]
            # Live loot may change totals during the long run. Reopen must
            # preserve the actual stopped run, not the pre-bind population.
            for key in (("entities", "villagers", "chunks") if phase == "reopen"
                        else ("villagers", "chunks")):
                if after["counts"][key] != previous["counts"][key]:
                    raise RuntimeError(f"{phase}: native {key} count changed")
            row["cohort"] = await native_cohort_snapshot(
                args, directory / "cohort", config, population)
            if after["counts"]["settlements"] != len(population["settlement_anchors"]) or \
                    after["counts"]["settlement_members"] != population["count"] or \
                    any(after["counts"][key] != baseline["counts"][key]
                        for key in ("households", "household_members")) or \
                    after["counts"]["settlement_orphan_members"] != 0:
                raise RuntimeError(f"{phase}: settlement membership/household census mismatch")
            groups = Counter(row["settlement"] for row in population["cohort"])
            details = {row["name"]: row for row in after["settlement_details"]}
            initial_details = {row["name"]: row for row in baseline["settlement_details"]}
            for name, count in groups.items():
                if details[name]["members"] != count or any(
                        details[name][key] != initial_details.get(name, {}).get(key, 0)
                        for key in ("households", "household_members")):
                    raise RuntimeError(f"{phase}: {name} household/member count mismatch")
            previous = after
            errors = [line for line in (directory / "server.log").read_text(errors="replace").splitlines()
                      if "ERROR" in line or "panicked at" in line or "Command failed:" in line]
            if errors:
                raise RuntimeError(f"{phase}: server errors: {errors[:5]}")
        report["status"] = "PASS"
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        dump(args.out / "receipt.json", report)
        print(json.dumps({"status": report["status"], "receipt": str(args.out / "receipt.json")}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    scan = sub.add_parser("census", help="scan canonical export of a stopped native world")
    scan.add_argument("--world", type=Path, required=True)
    scan.add_argument("--output", type=Path, required=True)
    scan.add_argument("--counts-only", action="store_true", help="do not retain/write per-cell terrain for large comparison worlds")
    keep = sub.add_parser("preservation", help="compare stopped source records with canonical native export")
    keep.add_argument("--source", type=Path, required=True)
    keep.add_argument("--world", type=Path, required=True)
    keep.add_argument("--output", type=Path, required=True)
    plan = sub.add_parser("plan", help="emit deterministic canonical preparation and street routes")
    plan.add_argument("--terrain", type=Path, required=True)
    plan.add_argument("--output", type=Path, required=True)
    plan.add_argument("--villagers", type=int, required=True)
    plan.add_argument("--animals", type=int, default=64)
    plan.add_argument("--bots", type=int, default=10)
    plan.add_argument("--seed", type=int, default=1453)
    plan.add_argument("--radius", type=int)
    run = sub.add_parser("smoke", help="bounded public preparation, street walk, native save/reopen")
    run.add_argument("--plan", type=Path, required=True)
    run.add_argument("--world", type=Path, required=True)
    run.add_argument("--server-bin", type=Path, required=True)
    run.add_argument("--bots-bin", type=Path, required=True)
    run.add_argument("--vanilla-data-dir", type=Path, required=True)
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--seed", type=int, required=True)
    run.add_argument("--view-distance", type=int, default=8)
    run.add_argument("--walk-seconds", type=int, default=20)
    run.add_argument("--timeout-seconds", type=int, default=600)
    native = sub.add_parser("native-smoke", help="native offline-cohort create/bind, game ticks and reopen")
    native.add_argument("--world", type=Path, required=True)
    native.add_argument("--server-bin", type=Path, required=True)
    native.add_argument("--bots-bin", type=Path, required=True,
                        help="existing read-only native snapshot probe; does not connect any bots")
    native.add_argument("--vanilla-data-dir", type=Path, required=True)
    native.add_argument("--out", type=Path, required=True)
    native.add_argument("--seed", type=int, required=True)
    native.add_argument("--view-distance", type=int, default=1)
    native.add_argument("--ticks", type=int, default=2400)
    native.add_argument("--timeout-seconds", type=int, default=1800)
    native.add_argument("--setup-timeout-seconds", type=int, default=3600,
                        help="native serial create/bind batch deadline; separate from tick/census fences")
    native.add_argument("--reopen-only", action="store_true",
                        help="check an already-bound stopped world without repeating setup")
    args = parser.parse_args()
    if args.operation == "census":
        summary, terrain = census(args.world, collect_terrain=not args.counts_only)
        dump(args.output, summary)
        if terrain is not None:
            dump(args.output.with_name("terrain.json"), terrain)
        print(json.dumps(summary["counts"], sort_keys=True))
    elif args.operation == "preservation":
        try:
            result = preservation(args.source, args.world)
        except (ValueError, OSError) as error:
            result = {"schema_version": 1, "status": "FAIL", "error": str(error)}
        dump(args.output, result)
        print(json.dumps({"status": result["status"], "receipt": str(args.output)}))
        if result["status"] != "PASS":
            raise SystemExit(1)
    elif args.operation == "plan":
        result = population_plan(json.loads(args.terrain.read_text()), args.villagers, args.animals, args.bots, args.seed, radius=args.radius)
        dump(args.output / "plan.json", result)
        dump(args.output / "commands.json", result["commands"])
        dump(args.output / "anchors.json", result["bot_anchors"])
        dump(args.output / "street-routes.json", result["street_routes"])
        dump(args.output / "street-graph.json", result["street_graph"])
        print(json.dumps({"target_villagers": result["target_villagers"], "preserved_villagers": len(result["existing_villagers"]), "added_villagers": len(result["villagers"]), "animals": len(result["animals"]), "bots": len(result["street_routes"]), "added_poi_modules": result["added_poi_modules"], "population_chunks": len(result["population_chunks"]), "commands": len(result["commands"]["setup"])}))
    elif args.operation == "native-smoke":
        import asyncio
        for key in ("world", "server_bin", "bots_bin", "vanilla_data_dir", "out"):
            setattr(args, key, getattr(args, key).resolve())
        asyncio.run(native_smoke(args))
    else:
        import asyncio
        for key in ("plan", "world", "server_bin", "bots_bin", "vanilla_data_dir", "out"):
            setattr(args, key, getattr(args, key).resolve())
        asyncio.run(smoke(args))


if __name__ == "__main__":
    main()
