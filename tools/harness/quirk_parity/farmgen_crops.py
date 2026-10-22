#!/usr/bin/env python3
"""Generate farm-crop-wheat.json + farm-crop-cane.json (+ beetroot/carrot rows).

farm-crop-wheat: 6x11 wheat[age=0] on farmland[moisture=7] with water strips,
random_tick_speed 3, prelude:false (needs random ticks + advancing time for
light; weather clear). Metric: block_state_count minecraft:wheat[age=7]
per seed vs baseline. Horizon 24000 ticks: at ~50% mature vanilla-like pace,
half the rows should be mature; the oracle decides the truth.

Layout (box [0,60,0,17,70,11], y=64 ground):
- rows z=1..9 planted: farmland strip x=1..12, wheat x=2..11 on top (10 per
  row x 6 rows = 60 heads), water at x=0 and x=13 (moisture stays 7),
  glowstone-free: open sky, time advances so light stays day-ish? Time IS
  advancing from noon; over 24000 ticks a full day passes, nights slow
  growth on both sides equally (parity compares constructions, not clocks).
- beetroot row z=10 and carrot row z=0 (age=0) for aux rate texture; wheat
  metric is the gate, the others are snapshot texture for divergence hunts.

farm-crop-cane: sugar cane (needs water-adjacent dirt/sand) + cactus +
bamboo rows, random_tick_speed 3. Cane grows to 3 high per vanilla age-15
cycle; metric block_count minecraft:sugar_cane (new segments raise count).
Cactus: sand base rows; bamboo: sapling on dirt. All in one box, separated
in z so survival shapes (sand vs farmland, water adjacency) hold.
"""
import json
import sys
from pathlib import Path

HERE = Path(__file__).parent / "scenarios"

SEEDS = [11, 22, 33, 44, 55]

# wheat: 12 rows (z=0..11), 10 heads each (x=2..11); 120 heads, age=0.
# Farmland rows are contiguous into the box edge so no dirt dries them.
WHEAT_BOX = [0, 60, 0, 13, 70, 11]
WHEAT_ROWS = list(range(0, 12))
WHEAT_XS = list(range(2, 12))

CANE_BOX = [0, 60, 0, 17, 70, 11]


def wheat_setup() -> list[str]:
    # 12 wheat rows (z=0..11), 10 heads each (x=2..11): 120 heads age=0.
    # Water columns every 4 rows (z=1,5,9 at x=0 and x=13) keep moisture 7
    # over the 4-block hydration radius; every ground cell inside x=1..12 is
    # wet farmland so no dirt edge dries the field.
    cmds = [
        "gamerule spawn_mobs false",
        "gamerule mob_griefing false",
        "gamerule random_tick_speed 3",
        "gamerule advance_weather false",
        "weather clear",
        "time set 1000",
        "fill 0 63 0 13 63 11 minecraft:dirt",
        "fill 0 64 0 13 64 11 minecraft:air",
    ]
    for z in (1, 5, 9):
        for x in (0, 13):
            cmds.append(f"setblock {x} 63 {z} minecraft:water")
            cmds.append(f"setblock {x} 64 {z} minecraft:air")
    for z in WHEAT_ROWS:
        for x in range(1, 13):
            cmds.append(f"setblock {x} 63 {z} minecraft:farmland[moisture=7]")
        for x in WHEAT_XS:
            cmds.append(f"setblock {x} 64 {z} minecraft:wheat[age=0]")
    return cmds


def cane_setup() -> list[str]:
    # Proven pool design (oracle probe Oct 2026): cane/cactus bases are sand
    # islands in a full water pool (y=63 water over stone), so every base has
    # adjacent water and nothing pops. 12 cane stalks z=5 (2 silo stalks at
    # age=15 in glass 1x1 columns y=64..69, 10 at age=0); spaced cactus row
    # z=1; bamboo saplings z=9. Metric: segment-count growth over 12000 ticks.
    cmds = [
        "gamerule spawn_mobs false",
        "gamerule mob_griefing false",
        "gamerule random_tick_speed 3",
        "gamerule advance_weather false",
        "weather clear",
        "time set 1000",
        "fill 0 62 0 13 62 11 minecraft:stone",
        "fill 0 63 0 13 63 11 minecraft:water",
        "fill 0 64 0 13 70 11 minecraft:air",
    ]
    for sx in (3, 9):
        cmds.append(f"setblock {sx} 63 5 minecraft:sand")
        cmds.append(f"setblock {sx} 64 5 minecraft:sugar_cane[age=15]")
        for y in range(64, 70):
            for dx, dz in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                cmds.append(f"setblock {sx + dx} {y} {5 + dz} minecraft:glass")
    for x in [x for x in range(1, 13) if x not in (3, 9)]:
        cmds.append(f"setblock {x} 63 5 minecraft:sand")
        cmds.append(f"setblock {x} 64 5 minecraft:sugar_cane[age=0]")
    for x in range(1, 13, 2):  # cactus needs empty neighbours: every other column
        cmds.append(f"setblock {x} 63 1 minecraft:sand")
        cmds.append(f"setblock {x} 64 1 minecraft:cactus[age=0]")
    for x in range(1, 13):
        cmds.append(f"setblock {x} 63 9 minecraft:dirt")
        cmds.append(f"setblock {x} 64 9 minecraft:bamboo_sapling")
    return cmds


def wheat_scenario() -> dict:
    return {
        "id": "farm-crop-wheat",
        "family": "farm",
        "description": (
            "Wheat farm: 120 wheat[age=0] on wet farmland, "
            "random_tick_speed 3, 200 + 72000 ticks. Metric: mature wheat[age=7] count. "
            "Vanilla CropBlock.randomTick: 1/(25/growthspeed+1) per hit, growthspeed "
            "from wet farmland 3x3; nights pass on both sides equally."
        ),
        "box": WHEAT_BOX,
        "prelude": False,
        "seeds": SEEDS,
        "setup": wheat_setup(),
        "phases": [
            {"name": "warmup", "commands": [], "ticks": 200},
            {"name": "run", "commands": [], "ticks": 72000},
        ],
        "compare": {"blocks": "exact", "entities": "counts"},
        "rate": {
            "metric": {"kind": "block_state_count", "equals": "minecraft:wheat[age=7]"},
            "baseline_phase": "warmup",
            "phase": "run",
            "tolerance_pct": 5,
            "min_vanilla": 20,
        },
        "layout": {
            "ground_y": 63,
            "wheat": f"rows z={WHEAT_ROWS}, x={WHEAT_XS} (120 heads age=0)",
            "water": "columns x=0,13 at z=1,5,9 (hydration radius 4 keeps moisture 7)",
            "horizon": "200 + 72000 ticks (~60 min game time)",
        },
    }


def cane_scenario() -> dict:
    return {
        "id": "farm-crop-cane",
        "family": "farm",
        "description": (
            "Sugar cane: 12 stalks on sand (z=5), 2 silo stalks at age=15 in "
            "glass 1x1 columns, 10 at age=0; spaced cactus + bamboo rows as "
            "texture. random_tick_speed 3, 200 + 12000 ticks. Metric: sugar_cane "
            "segment count growth (age 15 -> new block above)."
        ),
        "box": CANE_BOX,
        "prelude": False,
        "seeds": SEEDS,
        "setup": cane_setup(),
        "phases": [
            {"name": "warmup", "commands": [], "ticks": 200},
            {"name": "run", "commands": [], "ticks": 12000},
        ],
        "compare": {"blocks": "exact", "entities": "counts"},
        "rate": {
            "metric": {"kind": "block_count", "block": "minecraft:sugar_cane"},
            "baseline_phase": "warmup",
            "phase": "run",
            "tolerance_pct": 50,
            "min_vanilla": 1,
        },
        "layout": {
            "ground_y": 63,
            "base": "full water pool y=63 over stone; sand islands under cane/cactus, dirt under bamboo",
            "cane": "z=5, x=1..12 on sand (x=3,9 silos age=15 in glass, rest age=0)",
            "cactus": "z=1, odd x only (6, spaced, sand)",
            "bamboo": "z=9, x=1..12 saplings on dirt",
            "horizon": "200 + 12000 ticks",
        },
    }


def main() -> None:
    for name, scenario in (("farm-crop-wheat.json", wheat_scenario()),
                           ("farm-crop-cane.json", cane_scenario())):
        out = HERE / name
        out.write_text(json.dumps(scenario, indent=2) + "\n")
        print(f"wrote {out} ({len(scenario['setup'])} setup commands)")


if __name__ == "__main__":
    sys.exit(main())
