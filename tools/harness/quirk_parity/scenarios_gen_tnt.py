"""Generator for the oracle-discovered TNT-duper scenario (run once, output checked in).

`python3 -m tools.harness.quirk_parity.scenarios_gen_tnt` rewrites
scenarios/tnt-duper-found-bud.json from the fuzz seeds that found the cells.
"""
import json
from pathlib import Path

Y = 64
OUT = Path(__file__).with_name("scenarios") / "tnt-duper-found-bud.json"


def found_duper_cells():
    """Duplication cells discovered by the oracle search (fuzz-bud seeds), isolated one per cell.

    Each came out of `--fuzz-style bud` (192..1024 cells); vanilla ended with more TNT than it was
    built with. Construction order is kept exactly (it is part of the case).
    """
    from . import fuzz
    found = {(11, 61): "bud-seed11-cell61", (12, 90): "bud-seed12-cell90", (12, 224): "bud-seed12-cell224"}
    cells = []
    for (seed, index), name in found.items():
        scenario = fuzz.bud_scenario(seed, 1024, "line")
        meta = scenario["fuzz"]["cells"][index]
        ox, oz = meta["origin"]
        blocks = [((x - ox, y, z - oz), b) for (x, y, z), b in meta["blocks"]]
        fire_pos = (4, fuzz.BASE_Y, 5)
        cells.append((name, blocks, fire_pos))
    return cells


def build_found():
    boxes, setup, fire, tags, expect = [], [], [], [], []
    for i, (name, blocks, fire_pos) in enumerate(found_duper_cells()):
        ox = i * 16
        boxes.append([ox, 60, 0, ox + 15, 72, 15])
        setup.append(f"fill {ox} 63 0 {ox + 15} 63 15 minecraft:stone")
        for (x, y, z), block in blocks:
            setup.append(f"setblock {ox + x} {y} {z} {block}")
        fire.append(f"setblock {ox + fire_pos[0]} {fire_pos[1]} {fire_pos[2]} minecraft:redstone_block")
        tags.append(name)
        built = sum(1 for _, b in blocks if b == "minecraft:tnt")
        expect.append({"phase": "fired", "check": "count", "cell": i, "key": "minecraft:tnt", "min": built + 1, "max": built + 1,
                       "note": f"{built} TNT built"})
    return {
        "id": "tnt-duper-found-bud", "family": "tnt-duping",
        "known_divergence": (
            "TNT duplication is NOT reproducible in Solaris: pistons move blocks instantly (no "
            "moving_piston block / 2-tick travel), so the vanilla window in which a BUD TNT is "
            "pushed while its replaced cell is still updated does not exist. Repro (oracle "
            "cells 0-2, settle_ticks 0): vanilla ends with 2 primed TNT from 1 built, Solaris "
            "ignites the BUD TNT at the first update ('armed' phase tnt entity 1 vs 0) and "
            "ends with 1. Architectural (piston movement model), not a small fix."
        ),
        "description": "Vanilla-26.1.2 TNT duplication cells found by the oracle search (--fuzz-style bud): powered-but-never-updated (BUD, built under tnt_explodes=false) TNT in a piston push line next to observers/torches/coral fans. Vanilla ends with more TNT (blocks + primed entities) than were built.",
        "boxes": boxes, "setup": ["gamerule tnt_explodes false"] + setup,
        "settle_ticks": 0,
        "phases": [{"name": "armed", "commands": ["gamerule tnt_explodes true"], "ticks": 2},
                   {"name": "fired", "commands": fire, "ticks": 8}],
        "compare": {"blocks": "exact", "entities": "counts", "phases": ["armed", "fired"],
                    "entity_types": ["minecraft:tnt", "minecraft:item"]},
        "cells": tags,
        "vanilla_expect": expect,
    }


if __name__ == "__main__":
    OUT.write_text(json.dumps(build_found(), indent=1))
    print(OUT)
