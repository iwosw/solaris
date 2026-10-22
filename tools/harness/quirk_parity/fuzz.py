"""Seeded differential fuzz cells for piston/TNT/rail/carpet/coral contraptions.

Each fuzz scenario packs many independent 16x16 cells (48 blocks apart, so
blasts never meet) that all run on the same tick timeline. Vanilla is the
oracle: cells where vanilla ends with more TNT (blocks + primed entities) than
it started with are classified as duplication cells and reported separately,
because those are the player-relied quirks Solaris must keep.
"""
from __future__ import annotations

import random

CELL_SPACING = 48
GRID = 8
INNER = 6  # contraption volume is INNER x INNER x HEIGHT inside the cell
HEIGHT = 4
BASE_Y = 64
FACINGS = ["north", "south", "east", "west", "up", "down"]
HORIZONTAL = ["north", "south", "east", "west"]

# (weight, builder). Builders return a block string.
def _piston(rng): return f"minecraft:piston[facing={rng.choice(FACINGS)}]"
def _sticky(rng): return f"minecraft:sticky_piston[facing={rng.choice(FACINGS)}]"
def _observer(rng): return f"minecraft:observer[facing={rng.choice(FACINGS)}]"
def _plain(name): return lambda rng: f"minecraft:{name}"
def _rail(rng): return f"minecraft:rail[shape={rng.choice(['north_south', 'east_west'])}]"
def _powered_rail(rng): return f"minecraft:powered_rail[shape={rng.choice(['north_south', 'east_west'])}]"
def _activator_rail(rng): return f"minecraft:activator_rail[shape={rng.choice(['north_south', 'east_west'])}]"
def _fan(rng): return f"minecraft:tube_coral_wall_fan[facing={rng.choice(HORIZONTAL)}]"

PALETTE = [
    (10, _piston), (8, _sticky), (5, _observer),
    (12, _plain("tnt")), (8, _plain("slime_block")), (5, _plain("honey_block")),
    (9, _plain("redstone_block")), (10, _plain("stone")),
    (5, _plain("white_carpet")), (6, _rail), (3, _powered_rail), (3, _activator_rail),
    (4, _plain("tube_coral_fan")), (3, _fan), (3, _plain("sand")),
    (3, _plain("redstone_lamp")), (3, _plain("glass")),
]
# Dense style: biased to the duper ingredients (pistons pushing TNT with slime,
# a support-popping coral fan, power sources and update relays).
DENSE_PALETTE = [
    (10, _piston), (10, _sticky), (14, _plain("tnt")), (10, _plain("slime_block")),
    (4, _plain("honey_block")), (8, _plain("redstone_block")), (5, _observer),
    (6, _plain("dead_tube_coral_fan")), (5, lambda rng: f"minecraft:dead_tube_coral_wall_fan[facing={rng.choice(HORIZONTAL)}]"),
    (6, _plain("stone")), (3, _rail), (2, _powered_rail), (3, _plain("white_carpet")),
    (2, _plain("detector_rail")),
]
PALETTES = {"mixed": PALETTE, "dense": DENSE_PALETTE}


def _pick(rng: random.Random, palette) -> str:
    roll = rng.randrange(sum(weight for weight, _ in palette))
    for weight, build in palette:
        if roll < weight:
            return build(rng)
        roll -= weight
    raise AssertionError


def origin(index: int) -> tuple[int, int]:
    return (index % GRID) * CELL_SPACING, (index // GRID) * CELL_SPACING


def fuzz_scenario(seed: int, cells: int = 64, style: str = "mixed") -> dict:
    density = {"mixed": 0.22, "dense": 0.42}[style]
    palette = PALETTES[style]
    rng = random.Random(seed)
    boxes, setup, power_on, power_off, meta = [], [], [], [], []
    for index in range(cells):
        ox, oz = origin(index)
        boxes.append([ox, BASE_Y - 4, oz, ox + 15, BASE_Y + 20, oz + 15])
        setup.append(f"fill {ox} {BASE_Y - 1} {oz} {ox + 15} {BASE_Y - 1} {oz + 15} minecraft:stone")
        placed = []
        for dy in range(HEIGHT):
            for dz in range(INNER):
                for dx in range(INNER):
                    if rng.random() >= density:
                        continue
                    pos = (ox + 5 + dx, BASE_Y + dy, oz + 5 + dz)
                    block = _pick(rng, palette)
                    placed.append((pos, block))
        rng.shuffle(placed)  # construction order is part of the case, identical on both sides
        for (x, y, z), block in placed:
            setup.append(f"setblock {x} {y} {z} {block}")
        meta.append({"index": index, "origin": [ox, oz],
                     "blocks": [[list(pos), block] for pos, block in placed]})
        # Toggle power: strip every redstone block, later put new ones at random cells.
        power_off.append(f"fill {ox + 5} {BASE_Y} {oz + 5} {ox + 10} {BASE_Y + HEIGHT - 1} {oz + 10} minecraft:air replace minecraft:redstone_block")
        for _ in range(3):
            x, y, z = ox + 5 + rng.randrange(INNER), BASE_Y + rng.randrange(HEIGHT), oz + 5 + rng.randrange(INNER)
            power_on.append(f"setblock {x} {y} {z} minecraft:redstone_block")
    return {
        "id": f"fuzz-{style}-{seed}",
        "family": "fuzz-dupers",
        "description": f"Seeded differential fuzz: {cells} cells of piston/TNT/rail/carpet/coral contraptions",
        "boxes": boxes,
        "setup": setup,
        "phases": [
            {"name": "settle", "commands": [], "ticks": 20},
            {"name": "unpower", "commands": power_off, "ticks": 20},
            {"name": "repower", "commands": power_on, "ticks": 30},
        ],
        "compare": {"blocks": "exact", "entities": "counts", "phases": ["settle", "unpower", "repower"],
                    "entity_types": ["minecraft:tnt", "minecraft:falling_block"]},
        "fuzz": {"seed": seed, "style": style, "cells": meta},
    }


LINE_CHOICES = [
    ("air", 2), ("stone", 2), ("slime_block", 3), ("honey_block", 1), ("tnt", 2),
    ("white_carpet", 1), ("dead_tube_coral_fan", 1), ("tube_coral_fan", 1), ("redstone_block", 1),
    ("observer[facing=east]", 1), ("observer[facing=west]", 1), ("observer[facing=east,powered=true]", 1),
    ("observer[facing=west,powered=true]", 1), ("observer[facing=up]", 1),
    ("rail[shape=east_west]", 1), ("powered_rail[shape=east_west,powered=true]", 1),
    ("detector_rail[shape=east_west]", 1), ("activator_rail[shape=east_west]", 1),
    ("redstone_lamp", 1), ("sand", 1), ("piston[facing=east]", 1), ("sticky_piston[facing=east]", 1),
    ("redstone_torch", 1), ("lever[face=floor,facing=east,powered=true]", 1), ("repeater[facing=east,powered=true]", 1),
    ("repeater[facing=west,powered=true]", 1), ("redstone_wire[east=side,west=side,power=15]", 1),
]
ATTACH = [
    ("air", 14), ("dead_tube_coral_fan", 3), ("tube_coral_fan", 2), ("white_carpet", 2),
    ("redstone_block", 1), ("tnt", 1), ("slime_block", 1), ("redstone_torch", 1),
    ("dead_tube_coral_wall_fan[facing=north]", 1), ("dead_tube_coral_wall_fan[facing=south]", 1),
]


def _weighted(rng, table):
    roll = rng.randrange(sum(w for _, w in table))
    for name, weight in table:
        if roll < weight:
            return name
        roll -= weight
    raise AssertionError


SLIME_LINE = [("slime_block", 8), ("honey_block", 1), ("stone", 1), ("air", 1)]
UPPER = [("air", 5), ("tnt", 5), ("dead_tube_coral_fan", 3), ("tube_coral_fan", 1), ("white_carpet", 1),
         ("slime_block", 1), ("stone", 1), ("redstone_block", 1)]
SIDE = [("air", 4), ("redstone_block", 4), ("dead_tube_coral_fan", 1), ("stone", 1), ("slime_block", 1),
        ("dead_tube_coral_wall_fan[facing=north]", 1), ("dead_tube_coral_wall_fan[facing=south]", 1)]


def bud_scenario(seed: int, cells: int = 128, style: str = "line") -> dict:
    if style == "slime3d":
        return _slime3d_scenario(seed, cells)
    return _bud_line_scenario(seed, cells)


def _slime3d_scenario(seed: int, cells: int) -> dict:
    """Slime line with TNT/fans stuck on top: the Mytteri/Kades duper family, found by search."""
    rng = random.Random(seed)
    boxes, setup, fire, meta = [], [], [], []
    for index in range(cells):
        ox, oz = origin(index)
        boxes.append([ox, BASE_Y - 4, oz, ox + 15, BASE_Y + 20, oz + 15])
        setup.append(f"fill {ox} {BASE_Y - 1} {oz} {ox + 15} {BASE_Y - 1} {oz + 15} minecraft:stone")
        placed = [((ox + 3, BASE_Y, oz + 6), f"minecraft:{rng.choice(['piston', 'sticky_piston'])}[facing=east]")]
        length = rng.randrange(2, 5)
        for dx in range(4, 4 + length):
            line = _weighted(rng, SLIME_LINE)
            placed.append(((ox + dx, BASE_Y, oz + 6), f"minecraft:{line}"))
        for dx in range(3, 5 + length):
            up = _weighted(rng, UPPER)
            placed.append(((ox + dx, BASE_Y + 1, oz + 6), f"minecraft:{up}"))
            if rng.random() < 0.35:
                down = _weighted(rng, UPPER)
                placed.append(((ox + dx, BASE_Y - 1, oz + 6), f"minecraft:{down}"))
            for dz in (-1, 1):
                side = _weighted(rng, SIDE)
                placed.append(((ox + dx, BASE_Y + 1, oz + 6 + dz), f"minecraft:{side}"))
        # Keep only the last placement per position, drop sources that would QC-power the piston early.
        final = {}
        for pos, block in placed:
            final[pos] = block
        banned = {(ox + 3, BASE_Y + 1, oz + 6), (ox + 3, BASE_Y, oz + 5), (ox + 3, BASE_Y, oz + 7), (ox + 3, BASE_Y - 1, oz + 6)}
        for dx, dy, dz in [(2, 1, 0), (4, 1, 0), (3, 2, 0), (3, 1, -1), (3, 1, 1)]:
            banned.add((ox + dx, BASE_Y + dy, oz + 6 + dz))  # neighbours of the QC point above the piston
        ordered = [(pos, block) for pos, block in final.items()
                   if not (block == "minecraft:redstone_block" and pos in banned)]
        rng.shuffle(ordered)
        for (x, y, z), block in ordered:
            setup.append(f"setblock {x} {y} {z} {block}")
        fire.append(f"setblock {ox + 3} {BASE_Y} {oz + 5} minecraft:redstone_block")
        meta.append({"index": index, "origin": [ox, oz],
                     "blocks": [[list(pos), block] for pos, block in ordered]})
    return {
        "id": f"fuzz-slime3d-{seed}", "family": "fuzz-dupers",
        "description": f"Slime-line BUD TNT duper search, {cells} cells (seed {seed})",
        "boxes": boxes, "setup": ["gamerule tnt_explodes false"] + setup,
        "phases": [
            {"name": "armed", "commands": ["gamerule tnt_explodes true"], "ticks": 2},
            {"name": "fired", "commands": fire, "ticks": 8},
        ],
        "compare": {"blocks": "exact", "entities": "counts", "phases": ["armed", "fired"],
                    "entity_types": ["minecraft:tnt", "minecraft:item", "minecraft:falling_block"]},
        "fuzz": {"seed": seed, "style": "slime3d", "cells": meta},
    }


def _bud_line_scenario(seed: int, cells: int = 128) -> dict:
    """Piston-push search for BUD-TNT duplication: a powered, never-updated TNT in a push line.

    The TNT is powered while the tnt_explodes gamerule is off (powered but unlit),
    then the piston fires with the rule on. Any cell where vanilla ends with more
    TNT (blocks + primed entities) than it was built with is a duplication cell.
    """
    rng = random.Random(seed)
    boxes, setup, fire, meta = [], [], [], []
    for index in range(cells):
        ox, oz = origin(index)
        boxes.append([ox, BASE_Y - 4, oz, ox + 15, BASE_Y + 20, oz + 15])
        setup.append(f"fill {ox} {BASE_Y - 1} {oz} {ox + 15} {BASE_Y - 1} {oz + 15} minecraft:stone")
        placed = [((ox + 4, BASE_Y, oz + 6), f"minecraft:{rng.choice(['piston', 'sticky_piston'])}[facing=east]")]
        tnt_x = 5 if rng.random() < 0.7 else rng.randrange(5, 8)
        row = {}
        for dx in range(5, 9):
            row[dx] = "tnt" if dx == tnt_x else _weighted(rng, LINE_CHOICES)
        for dx, name in row.items():
            placed.append(((ox + dx, BASE_Y, oz + 6), f"minecraft:{name}"))
            for dy in (1, -1):
                attach = _weighted(rng, ATTACH)
                if attach != "air" and not (dy == -1 and not rng.random() < 0.3):
                    placed.append(((ox + dx, BASE_Y + dy, oz + 6), f"minecraft:{attach}"))
        for dz in (-1, 1):
            if rng.random() < 0.25:
                placed.append(((ox + tnt_x, BASE_Y, oz + 6 + dz), f"minecraft:{_weighted(rng, ATTACH)}"))
        placed.append(((ox + tnt_x, BASE_Y - 1, oz + 6), "minecraft:redstone_block"))
        rng.shuffle(placed)
        # Piston first would fire during build: order irrelevant under tnt_explodes=false for TNT,
        # but the piston must stay unpowered until the fire phase, so drop power sources next to it.
        for (x, y, z), block in placed:
            setup.append(f"setblock {x} {y} {z} {block}")
        fire.append(f"setblock {ox + 4} {BASE_Y} {oz + 5} minecraft:redstone_block")
        meta.append({"index": index, "origin": [ox, oz],
                     "blocks": [[list(pos), block] for pos, block in placed]})
    return {
        "id": f"fuzz-bud-{seed}", "family": "fuzz-dupers",
        "description": f"BUD TNT push-line search, {cells} cells (seed {seed})",
        "boxes": boxes, "setup": ["gamerule tnt_explodes false"] + setup,
        "phases": [
            {"name": "armed", "commands": ["gamerule tnt_explodes true"], "ticks": 2},
            {"name": "fired", "commands": fire, "ticks": 8},
        ],
        "compare": {"blocks": "exact", "entities": "counts", "phases": ["armed", "fired"],
                    "entity_types": ["minecraft:tnt", "minecraft:item", "minecraft:falling_block"]},
        "fuzz": {"seed": seed, "style": "bud", "cells": meta},
    }


def dupe_cells(scenario: dict, vanilla: dict) -> dict[int, list[str]]:
    """Cell index -> phases where vanilla holds more TNT than the cell was built with.

    The placed count comes from the scenario, not the setup snapshot: TNT lit
    while building is already a primed entity (or gone) by then.
    """
    phases = vanilla["phases"]
    found: dict[int, list[str]] = {}
    for cell in scenario["fuzz"]["cells"]:
        index = cell["index"]
        initial = sum(1 for _, block in cell["blocks"] if block == "minecraft:tnt")
        for phase in phases[1:]:
            if tnt_total(phase["snapshot"]["cells"][index]) > initial:
                found.setdefault(index, []).append(phase["name"])
    return found


def tnt_total(snapshot: dict) -> int:
    blocks = sum(1 for name in snapshot["blocks"].values() if name == "minecraft:tnt")
    entities = sum(1 for e in snapshot["entities"] if e["type"] == "minecraft:tnt")
    return blocks + entities


def timing_scenario(seed: int, cells: int = 128, ticks: int = 8) -> dict:
    """Redstone-timing differential fuzz: piston/sticky piston/observer driven by random on/off waveforms.

    Every tick (phase) randomly applies power toggles, QC-point toggles, neighbour-update nudges and
    zero-tick pairs (on+off or off+on inside ONE tick). The block state of every cell is compared
    after every tick, so any 0-tick, quasi-connectivity or update-order difference shows up as the
    first diverging tick.
    """
    rng = random.Random(seed)
    boxes, setup, meta = [], [], []
    phase_cmds: list[list[str]] = [[] for _ in range(ticks)]
    for index in range(cells):
        ox, oz = origin(index)
        boxes.append([ox, BASE_Y - 4, oz, ox + 15, BASE_Y + 20, oz + 15])
        setup.append(f"fill {ox} {BASE_Y - 1} {oz} {ox + 15} {BASE_Y - 1} {oz + 15} minecraft:stone")
        kind = rng.choice(["piston", "sticky_piston", "piston", "sticky_piston", "observer_piston"])
        px, py, pz = ox + 6, BASE_Y, oz + 6
        extra = rng.choice([[], [(1, 0, 0, "cobblestone")], [(1, 0, 0, "slime_block"), (2, 0, 0, "cobblestone")],
                            [(1, 0, 0, "white_carpet")], [(1, 0, 0, "tnt")] if rng.random() < 0.2 else []])
        placed = []
        if kind == "observer_piston":
            placed.append(((px, py, pz), "minecraft:piston[facing=east]"))
            placed.append(((px - 1, py, pz), "minecraft:observer[facing=east]"))
            sources = [("trigger", (px - 2, py, pz), "minecraft:stone")]
        else:
            placed.append(((px, py, pz), f"minecraft:{kind}[facing=east]"))
            sources = [
                ("direct", (px, py, pz - 1), "minecraft:redstone_block"),
                ("qc", (px + 1, py + 2, pz), "minecraft:redstone_block"),
                ("qc-side", (px - 1, py + 1, pz), "minecraft:redstone_block"),
            ]
            sources = [rng.choice(sources)] + ([rng.choice(sources)] if rng.random() < 0.3 else [])
        for dx, dy, dz, block in extra:
            placed.append(((px + dx, py + dy, pz + dz), f"minecraft:{block}"))
        for (x, y, z), block in placed:
            setup.append(f"setblock {x} {y} {z} {block}")
        nudge = (px, py, pz + 1)  # a block update next to the piston
        wave = []
        for t in range(ticks):
            ops = []
            for name, pos, block in sources:
                roll = rng.random()
                if roll < 0.25:
                    ops.append(("on", name))
                elif roll < 0.5:
                    ops.append(("off", name))
                elif roll < 0.6:
                    ops += [("on", name), ("off", name)]
                elif roll < 0.7:
                    ops += [("off", name), ("on", name)]
            if rng.random() < 0.25:
                ops.append(("nudge", "n"))
            wave.append(ops)
            for op, name in ops:
                if op == "nudge":
                    phase_cmds[t].append(f"setblock {nudge[0]} {nudge[1]} {nudge[2]} "
                                         f"minecraft:{'stone' if (t % 2 == 0) else 'air'}")
                    continue
                pos, block = next((p, b) for n, p, b in sources if n == name)
                state_block = block if op == "on" else "minecraft:air"
                phase_cmds[t].append(f"setblock {pos[0]} {pos[1]} {pos[2]} {state_block}")
        meta.append({"index": index, "origin": [ox, oz], "kind": kind,
                     "blocks": [[list(pos), block] for pos, block in placed], "wave": wave})
    return {
        "id": f"fuzz-timing-{seed}", "family": "zero-tick",
        "description": f"Redstone waveform differential fuzz, {cells} cells x {ticks} ticks (seed {seed})",
        "boxes": boxes, "setup": setup,
        "phases": [{"name": f"t{t + 1}", "commands": phase_cmds[t], "ticks": 1} for t in range(ticks)],
        "compare": {"blocks": "exact", "entities": "counts",
                    "entity_types": ["minecraft:tnt", "minecraft:item"]},
        "fuzz": {"seed": seed, "style": "timing", "cells": meta},
    }
