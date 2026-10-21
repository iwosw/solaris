#!/usr/bin/env python3
"""Check, cost and preview Solaris settlement blueprints.

    tools/blueprints.py check                  # structural validation of every blueprint
    tools/blueprints.py materials [id ...]     # item totals and base-resource totals per level
    tools/blueprints.py preview [id ...]       # isometric PNGs under .analysis/codex-logs/millenaire/blueprints/

Blueprint files live in crates/mc-core/assets/blueprints/*.json (format: docs/SETTLEMENTS_GUIDE.md).
The authoritative gate is the Rust test `settlements::blueprints` (certification of every level);
`check` only catches authoring mistakes without building the workspace.
"""
import json
import math
import re
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIR = ROOT / "crates/mc-core/assets/blueprints"
OUT = ROOT / ".analysis/codex-logs/millenaire/blueprints"
MAX_SIZE = (11, 9, 11)
WOODS = ("oak", "spruce", "birch", "dark_oak")
WOOD_PARTS = ("planks", "log", "stairs", "slab", "fence", "fence_gate", "door", "trapdoor", "pressure_plate")
PLAIN = {
    "chest", "barrel", "crafting_table", "composter", "ladder", "torch", "wall_torch", "cobblestone",
    "cobblestone_wall", "cobblestone_stairs", "cobblestone_slab", "furnace", "hay_block", "wheat",
    "farmland", "dirt", "stonecutter", "smithing_table", "fletching_table",
}
STATIONS = {"stonecutter", "smithing_table", "fletching_table"}
FREE_HALVES = (("half", "upper"), ("part", "head"))


def parse_state(text):
    name, _, rest = text.partition("[")
    props = dict(p.split("=", 1) for p in rest.rstrip("]").split(",") if p)
    return name.removeprefix("minecraft:"), props


def load(path):
    data = json.loads(path.read_text())
    palette = {ch: parse_state(s) for ch, s in data["palette"].items()}
    levels = []
    for level in data["levels"]:
        layers = level["layers"]
        cells = {}
        for y, layer in enumerate(layers):
            for z, row in enumerate(layer):
                for x, ch in enumerate(row):
                    if ch != ".":
                        cells[(x, y, z)] = palette[ch]
        levels.append(
            {
                "cert": level["cert"],
                "entrance": level["entrance"],
                "cells": cells,
                "size": (len(layers[0][0]), len(layers), len(layers[0])),
            }
        )
    return data, levels


def allowed_block(name):
    if name in PLAIN or name.endswith("_bed"):
        return True
    return any(name == f"{w}_{p}" for w in WOODS for p in WOOD_PARTS)


def check(path):
    errors = []
    data, levels = load(path)
    if path.stem != data["id"]:
        errors.append("id must equal file name")
    prev = None
    for n, level in enumerate(levels, 1):
        tag = f"{data['id']} L{n}"
        size, cert, cells = level["size"], level["cert"], level["cells"]
        if any(s > m for s, m in zip(size, MAX_SIZE)):
            errors.append(f"{tag}: size {size} exceeds {MAX_SIZE}")
        if cert[1] > size[1] or cert[0] > size[0] or cert[2] > size[2]:
            errors.append(f"{tag}: cert {cert} outside size {size}")
        if prev and any(a < b for a, b in zip(size, prev["size"])) or prev and any(a < b for a, b in zip(cert, prev["cert"])):
            errors.append(f"{tag}: must contain previous level")
        ex, ey, ez = level["entrance"]
        if not all(0 <= v < s for v, s in zip(level["entrance"], size)):
            errors.append(f"{tag}: entrance {level['entrance']} outside size {size}")
        for (x, y, z), (name, props) in cells.items():
            if not allowed_block(name):
                errors.append(f"{tag}: block {name} violates the material rule")
            if name.endswith("_door") and props.get("half") == "lower":
                up = cells.get((x, y + 1, z))
                if not up or up[0] != name or up[1].get("half") != "upper":
                    errors.append(f"{tag}: door at {(x, y, z)} lacks upper half")
            if name.endswith("_bed") and props.get("part") == "head":
                facing = props.get("facing")
                dx, dz = {"north": (0, 1), "south": (0, -1), "east": (-1, 0), "west": (1, 0)}[facing]
                foot = cells.get((x + dx, y, z + dz))
                if not foot or foot[0] != name or foot[1].get("part") != "foot" or foot[1].get("facing") != facing:
                    errors.append(f"{tag}: bed head at {(x, y, z)} lacks its foot")
        floor = [(x, z) for x in range(cert[0]) for z in range(cert[2]) if (x, 0, z) not in cells]
        if floor and data["kind"] != "mine" and data["kind"] != "wall":
            errors.append(f"{tag}: floor layer has gaps inside cert box, e.g. {floor[0]}")
        prev = level
    return errors


# ----- materials -----------------------------------------------------------

def item_of(name, props):
    if any(props.get(k) == v for k, v in FREE_HALVES) and (name.endswith("_door") or name.endswith("_bed")):
        return None, 0
    count = 2 if props.get("type") == "double" else 1
    item = {"wall_torch": "torch", "wheat": "wheat_seeds", "farmland": "dirt"}.get(name, name)
    return item, count


def diffs(levels):
    prev = {}
    for level in levels:
        cells = level["cells"]
        placed = [(pos, st) for pos, st in cells.items() if prev.get(pos) != st]
        yield placed
        prev = cells


def base_resources(items):
    """Expand item counts to logs/cobblestone/coal/wool/crops/dirt/station blocks (vanilla recipes)."""
    planks = 0.0
    sticks = 0.0
    base = Counter()
    for item, n in items.items():
        w = next((w for w in WOODS if item.startswith(w + "_")), None)
        part = item[len(w) + 1:] if w else item
        if w and part == "planks":
            planks += n
        elif w and part == "log":
            base[f"{w}_log"] += n
        elif w and part == "slab":
            planks += n / 2
        elif w and part == "stairs":
            planks += 1.5 * n
        elif w and part == "fence":
            planks += 4 / 3 * n
            sticks += 2 / 3 * n
        elif w and part == "fence_gate":
            planks += 2 * n
            sticks += 4 * n
        elif w and part == "door":
            planks += 2 * n
        elif w and part == "trapdoor":
            planks += 3 * n
        elif w and part == "pressure_plate":
            planks += 2 * n
        elif item == "chest":
            planks += 8 * n
        elif item == "barrel":
            planks += 6 * n + 1 * n
        elif item == "crafting_table":
            planks += 4 * n
        elif item == "composter":
            planks += 3.5 * n
        elif item == "ladder":
            sticks += 7 / 3 * n
        elif item == "torch":
            sticks += n / 4
            base["coal"] += math.ceil(n / 4)
        elif item == "furnace":
            base["cobblestone"] += 8 * n
        elif item.endswith("_bed"):
            planks += 3 * n
            base["white_wool"] += 3 * n
        elif item == "cobblestone_wall":
            base["cobblestone"] += 1 * n
        elif item in ("cobblestone_stairs",):
            base["cobblestone"] += 1.5 * n
        elif item in ("cobblestone_slab",):
            base["cobblestone"] += 0.5 * n
        else:
            base[item] += n
    planks += sticks / 2
    base["oak_log"] += math.ceil(planks / 4)
    return {k: math.ceil(v) for k, v in base.items()}


def materials(path):
    data, levels = load(path)
    print(f"== {data['id']} ({data['kind']})")
    for n, placed in enumerate(diffs(levels), 1):
        items = Counter()
        for _, (name, props) in placed:
            item, count = item_of(name, props)
            if item:
                items[item] += count
        stations = sorted(set(items) & STATIONS)
        print(f"  L{n}: {sum(items.values())} items; base {dict(sorted(base_resources(items).items()))}" + (f"; stations {stations}" if stations else ""))
        print("      " + ", ".join(f"{k}x{v}" for k, v in sorted(items.items())))


# ----- preview ---------------------------------------------------------------

def color(name, props):
    table = [
        ("cobblestone", (128, 128, 128)), ("planks", (176, 140, 84)), ("log", (102, 81, 50)),
        ("stairs", (150, 115, 70)), ("slab", (160, 125, 75)), ("door", (130, 95, 50)),
        ("fence", (140, 105, 60)), ("chest", (150, 100, 30)), ("barrel", (110, 80, 45)),
        ("bed", (200, 60, 60)), ("torch", (255, 200, 60)), ("farmland", (95, 60, 35)),
        ("wheat", (200, 190, 60)), ("hay", (210, 180, 50)), ("ladder", (140, 110, 60)),
        ("furnace", (100, 100, 105)), ("table", (150, 110, 60)), ("composter", (110, 85, 50)),
        ("stonecutter", (130, 130, 140)), ("smithing", (60, 60, 70)), ("fletching", (190, 170, 120)),
        ("pressure", (170, 135, 80)), ("trapdoor", (135, 100, 55)), ("stone", (125, 125, 125)),
    ]
    for key, value in table:
        if key in name:
            return value
    return (255, 0, 255)


def preview(path):
    from PIL import Image, ImageDraw

    data, levels = load(path)
    OUT.mkdir(parents=True, exist_ok=True)
    for n, level in enumerate(levels, 1):
        sx, sy, sz = level["size"]
        s = 14
        w = (sx + sz) * s + 40
        h = (sx + sz) * s // 2 + sy * s + 40
        img = Image.new("RGB", (w, h), (30, 34, 40))
        draw = ImageDraw.Draw(img)
        ox, oy = sz * s + 20, sy * s + 20
        for y in range(sy):
            for z in range(sz):
                for x in range(sx):
                    cell = level["cells"].get((x, y, z))
                    if not cell:
                        continue
                    name, props = cell
                    r, g, b = color(name, props)
                    px = ox + (x - z) * s
                    py = oy + (x + z) * s // 2 - y * s
                    top = [(px, py - s // 2), (px + s, py), (px, py + s // 2), (px - s, py)]
                    left = [(px - s, py), (px, py + s // 2), (px, py + s // 2 + s), (px - s, py + s)]
                    right = [(px, py + s // 2), (px + s, py), (px + s, py + s), (px, py + s // 2 + s)]
                    draw.polygon(top, fill=(min(r + 30, 255), min(g + 30, 255), min(b + 30, 255)), outline=(20, 20, 20))
                    draw.polygon(left, fill=(int(r * 0.75), int(g * 0.75), int(b * 0.75)), outline=(20, 20, 20))
                    draw.polygon(right, fill=(int(r * 0.55), int(g * 0.55), int(b * 0.55)), outline=(20, 20, 20))
        target = OUT / f"{data['id']}_L{n}.png"
        img.save(target)
        print(target)


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "check"
    ids = sys.argv[2:]
    paths = [DIR / f"{i}.json" for i in ids] or sorted(DIR.glob("*.json"))
    failed = False
    for path in paths:
        if cmd == "check":
            errors = check(path)
            print(f"{path.stem}: {'ok' if not errors else ''}")
            for e in errors:
                print("  " + e)
            failed |= bool(errors)
        elif cmd == "materials":
            materials(path)
        elif cmd == "preview":
            preview(path)
        else:
            sys.exit(__doc__)
    sys.exit(1 if failed else 0)


main()
