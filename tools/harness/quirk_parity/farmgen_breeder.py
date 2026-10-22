#!/usr/bin/env python3
"""Generate farm-villager-breeder.json (farmer harvest parity gate).

Probes on the vanilla 26.1.2 oracle (Oct 2026) showed that a command-driven
oracle CANNOT make villagers breed from floor food alone: 2 adults + beds +
bread/carrot/wheat item entities at their feet, farmer or not, bell or not,
noon-locked or full-day, produce ZERO babies over 24k-96k ticks on every
seed, although villagers pick food up (~6k ticks), sleep in beds at night,
and a 6-villager crowd spawns an iron golem (village mechanics work). Root
cause: summon-spawned villagers in a bare stone cell never enter courtship
(breed-target pairing needs village gossip/POI context the oracle's void
cell cannot provide; vanilla `VillagerMakeLove` needs mutual BREED_TARGET +
free bed, which never assembles here).

So this gate covers the breeder's food-supply leg: a farmer with a
composter job site harvests mature carrots (field visibly depletes) and
replants. Metric: mature-carrot count DROP from warmup to run (harvest
churn); vanilla is the oracle. Both servers run the identical
construction.

Layout (box [0,60,0,13,70,11], y=63 ground):
- job row z=4: composter at x=7, dirt elsewhere.
- crop rows z=1..2 and z=7..8: farmland[moisture=7] x=1..12 with mature
  carrots[age=7] x=2..11 (40 heads, farmer harvest target).
- water columns x=0,13 at z=1,2,4,7,8 (moisture 7 via radius-4 hydration).
- 1 farmer (VillagerData profession farmer) + 2 unemployed villagers.
- prelude:false: needs random_tick_speed 3 (crop regrow after harvest),
  advancing time (farmer works by schedule), weather clear.

Rate compares the run-phase mature count against the warmup baseline; a
negative production (harvest) is the signal, so min_vanilla is 1 on |drop|.
Seeds 5. Known limitation recorded in-scenario: birth rate is not
oracle-drivable without a player.
"""
import json
import sys
from pathlib import Path

OUT = Path(__file__).parent / "scenarios" / "farm-villager-breeder.json"

SEEDS = [11, 22, 33, 44, 55]

FARM_XS = list(range(2, 12))
CROP_ROWS = [1, 2, 7, 8]


def setup() -> list[str]:
    cmds = [
        "gamerule spawn_mobs false",
        "gamerule mob_griefing true",
        "gamerule random_tick_speed 3",
        "gamerule advance_weather false",
        "weather clear",
        "time set 2000",
        "fill 0 63 0 13 63 11 minecraft:dirt",
        "fill 0 64 0 13 70 11 minecraft:air",
    ]
    for z in (1, 2, 4, 7, 8):
        for x in (0, 13):
            cmds.append(f"setblock {x} 63 {z} minecraft:water")
            cmds.append(f"setblock {x} 64 {z} minecraft:air")
    cmds.append("setblock 7 63 4 minecraft:composter")
    cmds.append("setblock 7 64 4 minecraft:air")
    for z in CROP_ROWS:
        for x in range(1, 13):
            cmds.append(f"setblock {x} 63 {z} minecraft:farmland[moisture=7]")
        for x in FARM_XS:
            cmds.append(f"setblock {x} 64 {z} minecraft:carrots[age=7]")
    cmds.append(
        "summon minecraft:villager 7.5 65 5.5 {VillagerData:{profession:'minecraft:farmer'}}"
    )
    cmds.append("summon minecraft:villager 6.5 65 6.5")
    cmds.append("summon minecraft:villager 8.5 65 6.5")
    return cmds


def main() -> None:
    scenario = {
        "id": "farm-villager-breeder",
        "family": "farm",
        "description": (
            "Villager farmer harvest gate (breeder food-supply leg): 1 farmer "
            "(composter job site) + 2 unemployed + 40 mature carrots; "
            "random_tick_speed 3, full work-day schedule. The farmer harvests "
            "mature carrots and replants (field depletes mid-run). Metric: "
            "mature-carrot count DROP from warmup to run. Birth rate itself "
            "is NOT oracle-drivable without a player (bare-cell pairs never "
            "court on vanilla 26.1.2); see known_limitation."
        ),
        "box": [0, 60, 0, 13, 70, 11],
        "prelude": False,
        "seeds": SEEDS,
        "setup": setup(),
        "phases": [
            {"name": "warmup", "commands": [], "ticks": 200},
            {"name": "run", "commands": [], "ticks": 24000},
        ],
        "compare": {"blocks": "exact", "entities": "counts"},
        "known_divergence": (
            "Solaris farmer villagers harvest ~2.4x faster than vanilla: carrots[age=7] drop 35 "
            "(identical across seeds) vs vanilla 14.8 +- 3.2 over one work day (24000 ticks, "
            "farmer + composter + 40 mature carrots). Solaris villager harvest is not paced by "
            "vanilla's HarvestFarmland walk/cooldown timing. Repro: python3 -m "
            "tools.harness.quirk_parity --out DIR --scenario farm-villager-breeder."
        ),
        "rate": {
            "metric": {"kind": "block_state_count", "equals": "minecraft:carrots[age=7]"},
            "direction": "drop",
            "baseline_phase": "warmup",
            "phase": "run",
            "tolerance_pct": 20,
            "min_vanilla": 5,
        },
        "known_limitation": (
            "Gates harvest churn only. Command-summoned pairs in a bare "
            "oracle cell never enter courtship on vanilla 26.1.2 (0 babies "
            "in 24k-96k ticks across bread/carrot/wheat, farmer/bell/"
            "noon-lock variants), so birth-rate parity is out of oracle "
            "scope until a player-driven village fixture exists."
        ),
        "layout": {
            "ground_y": 63,
            "composter": [7, 63, 4],
            "carrot_field": f"rows z={CROP_ROWS}, x={FARM_XS} (40 heads age=7)",
            "villagers": "farmer (7.5,65,5.5) + 2 unemployed (6.5/8.5,65,6.5)",
        },
    }
    OUT.write_text(json.dumps(scenario, indent=2) + "\n")
    print(f"wrote {OUT} ({len(scenario['setup'])} setup commands)")

if __name__ == "__main__":
    sys.exit(main())
