#!/usr/bin/env python3
"""Generate summon-seeded farm parity scenarios (no natural spawning, no player).

Natural mob spawning needs a player within range, and this oracle runs with no
player connected, so every farm here is seed-driven: the setup/phase commands
summon the animals/mobs, vanilla mechanics (egg laying, fall damage, loot
tables, hopper pickup) do the rest, and production is read back from hoppers
via the `container_items` rate metric.

What this emits (into scenarios/ next to this file):
  farm-chicken-eggs      16 adult chickens in a hopper-floored glass pen.
                         Vanilla Chicken eggTime = rand(6000)+6000, so one egg
                         per ~9000 ticks per chicken; 42000 measured ticks =>
                         ~75 eggs. Metric: container_items minecraft:egg.
  farm-mob-kill-chamber  16 phases x 8 skeletons summoned at y=90 over a
                         hopper-floored pad (fall ~25 => damage ~22 >= 20 HP).
                         Skeletons (not zombies) so no reinforcement spawns
                         can survive the fall and pollute the kill accounting.
                         Metric: container_items minecraft:bone.
  farm-gold-nuggets      Same drop chamber, zombified piglins instead of
                         skeletons. Summon-seeded piglin slaughter: the only
                         gold-route farm achievable without natural spawning
                         (portal-spawned piglin farms need a player).
                         Metric: container_items minecraft:gold_nugget.

Deliberately NOT generated: slime-chunk/swamp slime farms, portal-based gold
farms, dark-room/spawner-cage mob farms - all need natural spawning or a
spawner within 16 blocks of a player, neither of which this oracle has.

Both servers receive identical command strings; no SNBT anywhere (plain
summons) so Solaris summon-NBT coverage is never the question. Night is
frozen (`time set 18000` over the prelude's `advance_time false`) so burnable
mobs never burn.

Regenerate:  python3 tools/harness/quirk_parity/farmgen_mobs.py
Run (vanilla oracle):  python3 -m tools.harness.quirk_parity --out DIR \\
                         --scenario farm-chicken-eggs --vanilla-only
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

SCENARIO_DIR = Path(__file__).with_name("scenarios")

CHICKEN_SEEDS = [11, 12, 13, 14, 15]
MOB_SEEDS = [21, 22, 23, 24, 25]
GOLD_SEEDS = [31, 32, 33, 34, 35]

COMPARE = {"blocks": "exact", "entities": "counts", "containers": "exact"}


def pen_setup(floor_y: int = 64, wall_top: int = 66, ceiling: bool = True) -> list[str]:
    """Hopper floor + stone base + glass walls (+ optional ceiling) over x/z 1..6."""
    x1, z1, x2, z2 = 1, 1, 6, 6
    ix1, iz1, ix2, iz2 = 2, 2, 5, 5  # 4x4 interior
    setup = [
        f"fill {x1} {floor_y - 1} {z1} {x2} {floor_y - 1} {z2} minecraft:stone",
        f"fill {ix1} {floor_y} {iz1} {ix2} {floor_y} {iz2} minecraft:hopper[facing=down]",
    ]
    for y in range(floor_y + 1, wall_top + 1):
        setup.append(f"fill {x1} {y} {z1} {x2} {y} {z2} minecraft:glass")
        setup.append(f"fill {ix1} {y} {iz1} {ix2} {y} {iz2} minecraft:air")
    if ceiling:
        setup.append(f"fill {x1} {wall_top + 1} {z1} {x2} {wall_top + 1} {z2} minecraft:glass")
    return setup


def chicken_scenario() -> dict:
    setup = pen_setup(ceiling=True)
    for x in range(2, 6):
        for z in range(2, 6):
            setup.append(f"summon minecraft:chicken {x + 0.5} 65 {z + 0.5}")
    phases = [{"name": "warmup", "commands": [], "ticks": 6000}]
    for run in range(1, 5):
        phases.append({"name": f"run{run}", "commands": [], "ticks": 10500})
    return {
        "id": "farm-chicken-eggs",
        "family": "farm",
        "description": (
            "Chicken egg farm: 16 summoned adult chickens penned on a 4x4 hopper "
            "floor (glass walls + ceiling, one chicken per hopper cell). Eggs are "
            "laid by the vanilla egg timer (~1 per 9000 ticks/chicken), land on the "
            "hoppers and are sucked in. 6000 warmup + 4x10500 run ticks "
            "(48000 total, ~75 expected eggs). No natural spawning, no player."
        ),
        "box": [0, 60, 0, 15, 75, 15],
        "seeds": CHICKEN_SEEDS,
        "setup": setup,
        "phases": phases,
        "compare": COMPARE,
        "vanilla_expect": [
            {"phase": "run4", "check": "count", "key": "minecraft:chicken", "min": 16, "max": 16}
        ],
        "rate": {
            "metric": {"kind": "container_items", "item": "minecraft:egg"},
            "baseline_phase": "warmup",
            "phase": "run4",
            "tolerance_pct": 5,
            "min_vanilla": 20,
        },
    }


DROP_SPOTS = [
    (2.5, 2.5), (3.5, 2.5), (4.5, 2.5), (5.5, 2.5),
    (2.5, 5.5), (3.5, 5.5), (4.5, 5.5), (5.5, 5.5),
]
DROP_Y = 90  # fall to the pad top (~65) ~= 25 blocks => ~22 damage, kills 20 HP mobs


def kill_chamber(
    scenario_id: str,
    mob: str,
    item: str,
    seeds: list[int],
    description: str,
    mob_key: str,
) -> dict:
    setup = ["time set 18000", "difficulty normal"] + pen_setup(ceiling=False)
    phases = []
    for wave in range(16):
        commands = [f"summon {mob} {x} {DROP_Y} {z}" for x, z in DROP_SPOTS]
        phases.append({"name": f"kill{wave:02d}", "commands": commands, "ticks": 2000})
    return {
        "id": scenario_id,
        "family": "farm",
        "description": description,
        "box": [0, 60, 0, 15, 95, 15],
        "seeds": seeds,
        "setup": setup,
        "phases": phases,
        "compare": COMPARE,
        "vanilla_expect": [
            {"phase": "kill15", "check": "count", "key": mob_key, "min": 0, "max": 0}
        ],
        "known_divergence": (
            "Mobs steer horizontally while airborne in Solaris (the idle-wander drive is applied "
            "mid-fall; vanilla creates no new wander path in the air, so an idle mob drops "
            "straight). Trace of farm-mob-kill-chamber wave 0 (seed 22): one skeleton drifts "
            "x 2.45->1.95, z 2.54->2.95 during the 25-block fall, lands on the glass rim (y=67) "
            "instead of the hopper pad, and its drops never reach a hopper (items rest there "
            "until the 6000-tick despawn): Solaris collects ~101 of ~128 bones (vanilla 123 "
            "+- 6.4). Fall damage, loot tables and hopper pickup match (the same chamber with "
            "100-tick waves collects 123 bones / 114 arrows)."
        ),
        "rate": {
            "metric": {"kind": "container_items", "item": item},
            "baseline_phase": "setup",
            "phase": "kill15",
            "tolerance_pct": 5,
            "min_vanilla": 20,
        },
    }


def mob_scenario() -> dict:
    return kill_chamber(
        "farm-mob-kill-chamber",
        "minecraft:skeleton",
        "minecraft:bone",
        MOB_SEEDS,
        (
            "Mob kill chamber: 16 waves of 8 skeletons summoned at y=90 fall onto "
            "a 4x4 hopper-floored pad (open drop column, glass rim walls). Fall "
            "~25 blocks kills (20 HP) via vanilla fall damage; bones/arrows are "
            "sucked into the hoppers. Skeletons, not zombies, so no reinforcement "
            "spawns can survive and pollute the kill accounting; midnight is "
            "frozen so nothing burns. Exercises death, skeleton loot tables, fall "
            "damage and hopper item pickup. ~128 kills, ~128 expected bones."
        ),
        "minecraft:skeleton",
    )


def gold_scenario() -> dict:
    return kill_chamber(
        "farm-gold-nuggets",
        "minecraft:zombified_piglin",
        "minecraft:gold_nugget",
        GOLD_SEEDS,
        (
            "Summon-seeded gold route: 16 waves of 8 zombified piglins through the "
            "same drop chamber as farm-mob-kill-chamber. Portal-based gold farms "
            "need natural nether spawns near a player, which this oracle cannot "
            "do; this covers the zombified-piglin loot table (0-1 nuggets each) "
            "plus fall death and hopper pickup instead. ~128 kills, ~64 nuggets."
        ),
        "minecraft:zombified_piglin",
    )


GENERATORS = {
    "farm-chicken-eggs": chicken_scenario,
    "farm-mob-kill-chamber": mob_scenario,
    "farm-gold-nuggets": gold_scenario,
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", action="append", default=[],
                        help="only regenerate these scenario ids")
    args = parser.parse_args()
    wanted = args.scenario or sorted(GENERATORS)
    for scenario_id in wanted:
        scenario = GENERATORS[scenario_id]()
        path = SCENARIO_DIR / f"{scenario_id}.json"
        path.write_text(json.dumps(scenario, indent=2) + "\n")
        print(f"wrote {path}")


if __name__ == "__main__":
    main()
