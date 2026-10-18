# Playable manifests and profiles

Run the playable gate through the canonical harness (`python3 -m tools.harness`
profiles; see [agent tooling](../AGENT_TOOLING.md#harness-entrypoint)).
Protocol harnesses, wire probes and prepared artifacts are not passing evidence:
a real vanilla 26.1.2 client must
connect through the repo-native Gradle `runClient` adapter and record the
required artifacts (`manifest.json`, `client.log`, `server.log`,
`observations.json`, screenshots where required, plus run metadata).

## Current inputs

- [`playable.toml`](../../playable.toml): default manual profile. Copy it
  to a per-run config rather than editing it in place.
- [`real-client-playable-loop.json`](real-client-playable-loop.json): bounded
  natural-progression real-client scenario pack (join, wood to tools, crafting,
  save/restart/rejoin, furnace smelt, food, storage, sleep, combat and two-client
  visibility routes). Every scenario is `manual-pending` until a real-client
  run records passing observations.
- [`real-client-v2-java-qa.json`](real-client-v2-java-qa.json): bounded core
  v2 Java-client smoke pack. Select its manifest and a declared scenario with
  the environment overrides in the tooling guide; no server-variant flag is needed.
- [`worldgen-seed-712816-preflight.json`](worldgen-seed-712816-preflight.json):
  seed-712816 graphical worldgen preflight input.

Do not treat these packs as shorthand for readiness, M100 progress, or the
VALIDATION_LEDGER numerator. Keep run evidence in ignored
`.analysis/real-client-runs/<run_id>/` directories, not in tracked docs.

## Target loop

The packs above cover one ordinary 20-minute vanilla-client survival session:
quick join; walk/jump/collide without rage bugs; break/place without
ghost/desync; drops/pickup/inventory/hotbar through wood to tools; crafting
table and basic recipes; save/restart/rejoin preserving player, inventory and
edited blocks; no crash, disconnect or catastrophic tick stall.

## Furnace startup contract

The v2 core's full-content-cache startup must attach vanilla furnace fuel
values to the resolved tags using the same item registry as the inventory
rules. Loading wire tags alone leaves the burn-time table empty: even an item
in `#minecraft:planks` is then rejected by the fuel slot and cannot burn.
The embedded-data test fixture initializes the same table; it is not an
operator-selectable content-cache fallback.

`inventory::furnace_tests::full_data_furnace_accepts_acacia_planks_and_cooks_charcoal`
covers full-data startup, hotbar-to-furnace pickup clicks, acacia planks' 300-tick
burn duration, and charcoal output after 200 cooking ticks.
