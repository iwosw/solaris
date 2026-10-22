# Single-writer regions for Minecraft entity simulation

Status: historical publication draft about the retired v1 model in ADR 0005.
ADR 0013/0014 supersede region ownership with the native data-parallel model;
the descriptions below do not describe the current server. No comparison with
other servers is claimed.

## The problem

A vanilla-style server ticks one world on one thread. The obvious way to use
more cores is to let several threads touch the same world, which turns every
entity update into a lock-ordering problem (`Arc<Mutex<World>>` and its
descendants). Solaris takes the other route: no mutable entity state is shared.

## The design

- The world is cut into fixed **8×8-chunk regions** (`RegionKey`, Euclidean
  division, so negative coordinates tile the same way).
- Every active region has exactly **one lease** and one lane-owned
  `EntityStore` (the regional ECS). A lane may own several regions; a region is
  never executed by two lanes at once.
- Mutable ECS state never leaves its lane. What crosses lanes is immutable tick
  input, compact results and versioned publications.
- Region keys, leases and owner handles are internal. Plugin authors never see
  them.

## One entity tick

```text
immutable tick inputs
  -> owner lane: selection, AI/goals, bounded pathing, ECS mutation
  -> compact goal motion + semantic ids + region leases
  -> central transactions (hostile, breeding, villager)
  -> owner lane: re-read current ECS, collision physics, ECS mutation
  -> compact committed motion
  -> publication to clients, only while the regional fence is still current
```

Lanes process their regions in deterministic `RegionKey` order.

## Cross-region work

Anything that touches more than one owner is a transaction with a recorded
decision: **prepare** (validate leases, identities, expected state and the whole
write set, expose nothing), **commit** (apply in deterministic lane order),
**finalize** (now externally publishable), or **rollback** while the rejection
is still known. An indeterminate commit outcome stops the runtime instead of
guessing. Transfers keep entity id and UUID, never expose the entity on both
sides, and move vehicle/passenger groups atomically. Save barriers capture all
lanes at one completed phase.

## What is not parallel

This is not a fully parallel world tick. Players, blocks, containers and block
entities still commit through one ordered `SimulationOwner`
([ADR 0004](../decisions/0004-staged-single-writer-simulation.md)). Chunk
crossings, vehicles, projectiles, items, experience orbs and falling blocks use
the central exception path.

## Measuring it

The workloads exist and are reproducible: 60 real TCP clients plus 40,000
hostile entities over 16 regions
([`ENTITY_SCALE_PROFILE.md`](../performance/ENTITY_SCALE_PROFILE.md)), and a
living-world profile through `python3 -m tools.harness run living-world-scale`.
Publishing a scaling curve (1, 2, 4, 8 workers) and a comparison against other
servers needs an exclusive, quiet machine and equivalent workloads on the other
servers; until that run exists, treat performance claims about Solaris as
unproven.
