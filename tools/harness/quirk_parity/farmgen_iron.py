"""Generator for the farm-iron-golem parity scenario.

Farm design v15 (command-kill + hopper floor, ASCII layers; X = east,
Z = south). Lessons from vanilla runs v1..v14:

- v1 lava single-deck: villagers walked into lava. v2 zombie burned at
  daybreak; 1 golem spawned, never died, GOLEM_DETECTED_RECENTLY stalled
  the rest. v3 water pod flooded the sleep cell. v4 permanent panic
  starved sleep. v5 time-split SOLVED sleep (NBT last_slept on all 5).
- v6-v11: cast 100% stable, all slept, all carry golem_detected_recently
  (set BY a successful trySpawnMob or by GolemSensor near a live golem),
  but 0 golems in the box, 0 ingots, 0 deaths: the tiny box hid
  escapees (v12 proved this: wide box caught 1 live golem on 5/5 seeds).
- v12 sealed room + wide box: SPAWN PROVEN on 5/5 seeds (1 live golem each,
  full HP, parked on the deck). Daylight froze the clock at day,
  GOLEM_DETECTED_RECENTLY never clears (golem never dies), so exactly 1
  golem/seed: the farm spawns but never kills.
- v13 sealed capsule around cactus: golems never path into the box (5/5
  parked at full HP, 0 deaths). Kill mechanisms the golem must walk into
  do not work; the kill must come to the golem.

v15 (per QuirkParity decision): the kill step is a COMMAND. Every census
phase opens with `kill @e[type=minecraft:iron_golem]` (vanilla drops
normal loot at the golem's position: 3-5 iron_ingot + 0-2 poppy, same
loot table as any death). Mob-kill mechanics themselves are covered by
farm-mob-kill-chamber; this gate measures SPAWN RATE (panic -> golem),
not kill contraptions.

    Single room (x 0..10, z 0..6), wide census box around it:
      y=68  GGGGGGGGGGG  glass roof (seal; LEGACY refuses glass floors)
      y=67  #.........#  air buffer
      y=66  #.........#  air
      y=65  #V.......D#  villagers (west) + open drowned post (east)
      y=64  #BBBbBSSSS#  dormitory beds (west half) + deck (east half)
      y=63  #HHHHHHHHH#  HOPPER bed across the whole interior -> chest
      y=62  ###########  foundation

    The whole interior floor at y=63 is hoppers (funnel row -> one chest),
    so kill drops land on hoppers wherever the golem dies in the room.
    Deck at y=64 is solid stone (valid LEGACY spawn floor: solid +
    air above). Glass roof seals the room (golems cannot leave; spawns
    cannot land on top).

    Phase 1 "night": no hostile, `time set 12500`, `advance_time true`,
    13000 ticks. Villagers claim HOME and sleep (REST 12000-24000),
    setting LAST_SLEPT.
    Phase 2 "panic": in-phase commands freeze the clock at day and summon
    the NoAI drowned onto the open post. Panic rolls every 100 ticks.
    Phase 3: 60 x 600-tick census phases, each opening with the kill
    command. GOLEM_DETECTED_RECENTLY is 599 ticks, so each 600-tick cycle
    admits ~1 golem: expected yield ~3-5 ingots/golem x up to 60 golems.
    Rate metric: container_items iron_ingot, baseline night -> census59.

Mechanics (vanilla 26.1.2 sources):
- VillagerPanicTrigger.tick() -> spawnGolemIfNeeded(level, ts, 3) only when
  `timestamp % 100 == 0`.
- spawnGolemIfNeeded needs >=3 villagers with wantsToSpawnGolem() in a
  10-block box: LAST_SLEPT within 24000 ticks + no GOLEM_DETECTED_RECENTLY
  (599-tick expiry; the per-phase kill clears it).
- SpawnUtil LEGACY_IRON_GOLEM: 10 attempts, dx/dz in [-8, 8] from the
  villager; floor must be non-glass solid (hoppers are NOT solid: legacy
  isSolid via collision bounds fails the 0.729 size test, so the deck
  stays stone and hoppers sit one below), air/liquid above.
- `kill` drops normal loot at the entity (loot table
  data/minecraft/loot_table/entities/iron_golem.json: 3-5 ingots).
- Villager brain schedule (data/minecraft/timeline/villager_schedule.json):
  REST 12000-24000. LAST_SLEPT set by SleepInBed.start on real bed sleep.

Layout contract: identical command strings on vanilla and Solaris;
natural spawning off in prelude; summoned villagers/drowned only. The
kill + clock commands are in-phase `commands` entries on both sides.
"""

from __future__ import annotations

import json
from pathlib import Path

SCENARIO_ID = "farm-iron-golem"

# Room origin. Room is (PW+1) x (W+1) blocks, y 62..68 (glass roof y=68).
X0, Y0, Z0 = 0, 62, 0
PW, W = 10, 6

# Wide census box: catches anything that still escapes (down to y=-64).
BOX = [X0 - 12, -64, Z0 - 12, X0 + PW + 12, 80, Z0 + W + 12]

# Dormitory: 5 beds in a row along the west wall behind a sill.
# Sill: stone step y=64 along x=3 (z=1,2,4,5), doorway gap at z=3; beds at
# y=65 heads-east in the alcove.
BEDS = [((1, 65, 1), (2, 65, 1)), ((1, 65, 2), (2, 65, 2)),
        ((1, 65, 4), (2, 65, 4)), ((1, 65, 5), (2, 65, 5)),
        ((1, 65, 3), (2, 65, 3))]
# Villagers scattered in the room air (y=65), west half near the beds.
VILLAGERS = [(1, 65, 1), (3, 65, 2), (1, 65, 4), (3, 65, 5), (4, 65, 3)]
# Panic post at the east end: OPEN AIR. NoAI + PersistenceRequired.
PILLAR = (9, 64, 3)
DROWNED_Y = 65
CHEST = (5, 63, 3)

KILL = "kill @e[type=minecraft:iron_golem]"
N_CYCLES = 2
CYCLE_TICKS = 600


def _bed_commands() -> list[str]:
    cmds: list[str] = []
    for foot, head in BEDS:
        fx, fy, fz = foot
        hx, hy, hz = head
        facing = "east" if hx > fx else ("west" if hx < fx else ("south" if hz > fz else "north"))
        cmds.append(f"setblock {X0+fx} {Y0+fy-62} {Z0+fz} minecraft:white_bed[facing={facing},occupied=false,part=foot]")
        cmds.append(f"setblock {X0+hx} {Y0+hy-62} {Z0+hz} minecraft:white_bed[facing={facing},occupied=false,part=head]")
    return cmds


def build() -> dict:
    x1, _, z1 = X0, Y0, Z0
    x2, z2 = X0 + PW, Z0 + W
    px, _, pz = PILLAR
    setup: list[str] = [
        # Room shell: foundation y=62, walls y=63..67, GLASS roof y=68.
        f"fill {x1} {Y0} {z1} {x2} {Y0} {z2} minecraft:stone",
        f"fill {x1} {Y0+1} {z1} {x2} {Y0+5} {z1} minecraft:stone",
        f"fill {x1} {Y0+1} {z2} {x2} {Y0+5} {z2} minecraft:stone",
        f"fill {x1} {Y0+1} {z1} {x1} {Y0+5} {z2} minecraft:stone",
        f"fill {x2} {Y0+1} {z1} {x2} {Y0+5} {z2} minecraft:stone",
        f"fill {x1} {Y0+6} {z1} {x2} {Y0+6} {z2} minecraft:glass",
        # Hopper bed y=63 across the whole interior -> one chest.
        f"fill {x1+1} {Y0+1} {z1+1} {x2-1} {Y0+1} {z2-1} minecraft:hopper",
        f"setblock {X0+CHEST[0]} {Y0+CHEST[1]-62} {Z0+CHEST[2]} minecraft:chest",
        # Solid stone spawn deck y=64 (valid LEGACY floor) across interior.
        f"fill {x1+1} {Y0+2} {z1+1} {x2-1} {Y0+2} {z2-1} minecraft:stone",
        # Dormitory alcove sill: stone step y=64 along x=3 (z=1,2,4,5),
        # doorway gap at z=3.
        f"fill {X0+3} {Y0+2} {Z0+1} {X0+3} {Y0+2} {Z0+2} minecraft:stone",
        f"fill {X0+3} {Y0+2} {Z0+4} {X0+3} {Y0+2} {Z0+5} minecraft:stone",
        # Panic post: OPEN AIR at y=64..65.
        f"setblock {X0+px} {Y0+2} {Z0+pz} minecraft:air",
        f"setblock {X0+px} {Y0+3} {Z0+pz} minecraft:air",
        # Dormitory beds (sleep target for LAST_SLEPT).
        *_bed_commands(),
        # Clock: dusk; advance_time runs the night during phase 1.
        "time set 12500",
        "gamerule advance_time true",
        "gamerule mob_griefing false",
        "difficulty normal",
        # Cast phase 1: 5 villagers ONLY (no hostile, so they sleep).
        *[
            f"summon villager {X0+vx} {Y0+vy-62} {Z0+vz} {{PersistenceRequired:1b}}"
            for vx, vy, vz in VILLAGERS
        ],
    ]
    drowned_summon = (
        f"summon drowned {X0+px} {Y0+DROWNED_Y-62} {Z0+pz} "
        "{PersistenceRequired:1b,NoAI:1b}"
    )
    cycles = [
        {"name": f"cycle{i:02d}", "commands": [KILL], "ticks": CYCLE_TICKS}
        for i in range(N_CYCLES)
    ]
    return {
        "id": SCENARIO_ID,
        "family": "farm",
        "description": (
            "Sealed-room summon-seeded iron farm with command kill: phase 1 "
            "(night, no hostile) 5 villagers sleep in dormitory beds "
            "(LAST_SLEPT); phase 2 (day) a NoAI drowned is summoned onto the "
            "open post and the clock freezes at day; 60 x 600-tick cycles "
            "each open with kill @e[type=minecraft:iron_golem] so every "
            "spawned golem drops loot onto the hopper-bed floor. "
            "Metric: container iron_ingot."
        ),
        "box": BOX,
        "seed": 1,
        "seeds": [11, 22, 33, 44, 55],
        "setup": setup,
        "phases": (
            [{"name": "night", "commands": [], "ticks": 13000}]
            + [{"name": "panic", "commands": ["time set 1000", "gamerule advance_time false", drowned_summon], "ticks": 11000}]
            + cycles
        ),
        "compare": {"blocks": "exact", "entities": "counts"},
        "rate": {
            # The panicking villagers raise one golem per scenario (vanilla spawns no second one
            # without a fresh panic); the oracle kills it and counts the loot it dropped on the
            # sealed deck (3-5 ingots, uniform): ingot items alive after the first kill cycle.
            "metric": {"kind": "item_entities", "item": "minecraft:iron_ingot"},
            "baseline_phase": "night",
            "phase": "cycle00",
            "tolerance_pct": 40,
            "min_vanilla": 3,
        },
    }


def main() -> None:
    out = Path(__file__).with_name("scenarios") / f"{SCENARIO_ID}.json"
    out.write_text(json.dumps(build(), indent=1) + "\n")
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
