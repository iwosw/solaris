# Revision 30 acceptance receipts

The isolated `pauline/worldgen` branch changes deepslate separately from the output-preserving climate optimization. The 26.1.2 Overworld noise settings specify a deepslate `vertical_gradient` certain at absolute Y=0 and false at absolute Y=8. Solaris used −8..0 and a reciprocal probability. Revision 30 uses the correct 0..8 bounds and a linear probability, retaining the existing coordinate hash: this is not a claim of vanilla RNG bit identity.

## Regression and focused gates

Before the correction, both `deepslate_gradient` regressions failed with stone instead of deepslate (`BlockStateId(2)` versus `BlockStateId(7)`); the focused command exited 101. After the correction:

| Gate | Result | Receipt |
|---|---|---|
| `cargo test --release -p mc-worldgen --lib -- --test-threads=2` | 402 passed, 0 failed, 21 explicitly ignored | [worldgen-tests.txt](worldgen-tests.txt) |
| `cargo test --release -p mc-core --lib older_revision_world_is_refused_then_upgraded_and_opens -- --nocapture` | 1 passed, 0 failed | [upgrade29-30.txt](upgrade29-30.txt) |
| `cargo clippy --release -p mc-worldgen --all-targets --no-deps -- -D warnings` | exit 0 | [worldgen-clippy.txt](worldgen-clippy.txt) |
| `cargo build --release -p mc-edge --bin solaris-v2` | exit 0 | [server-build.txt](server-build.txt) |

Every Cargo command used `/home/kaiserroman/solaris/.analysis/tools/mem-run.sh -m 6G env RUSTC_WRAPPER= CARGO_TARGET_DIR=/run/media/kaiserroman/games/solaris-work/targets/worldgen`; no exclusive/perf lane or private Cargo job limit was used. [validation.json](validation.json) records the complete command arrays and exit codes. The worldgen test invocation also set `SOLARIS_LOD_PROGRAM_FIXTURE` to export the final far-program fixture.

The existing upgrade test creates a real native world, rewrites its generation contract to revision 29, verifies default startup refuses it with the explicit upgrade hint, and invokes the same `upgrade_worldgen_revision` implementation as `world upgrade-worldgen`. It asserts revision **29 → 30**, a rewritten generation contract, unchanged stored checkpoint inspection, and successful subsequent `Core::open` startup/shutdown. This is the requested focused-test proof; it is not presented as a separately exercised CLI or network-listener run.

## Native-client far-terrain fixture

The native client's fixture decoder reads a seed-free SLOD program and `(x, z, height, biome-name)` rows, not subsurface block states. Its `Program::column` evaluates cave-free density and climate; deepslate selection and `WORLDGEN_REVISION` are not part of that program.

More strongly, the revision-30 server's newly exported fixture is **byte-identical after decompression** to the existing native-client fixture pinned to core `1aaa08dd`:

- 180,401 decompressed bytes.
- SHA-256: `910bb02370cd868fe4c235898c8e374e3a99eb3aa044568527e73f45648d8d02`.
- Compared fixture: `/run/media/kaiserroman/games/solaris-work/wt/client-input/tests/fixtures/far-program-seed81.zlib`.

The primary native-client checkout lacked its fixture file, so this comparison used the existing `client-input` worktree copy without modifying either client checkout. No native-client test suite or graphical client gate is claimed here.

## Scope limits

The full saved-block comparison and its structure metadata cover three spawn-centered Overworld rectangles, not all seeds, dimensions, structure contents or gameplay. Performance results and their shared-host uncertainty are documented separately in [PERFORMANCE.md](PERFORMANCE.md). The output-preserving optimization fence deliberately predates the intentional deepslate/revision-30 change.

## Independent read-only review

Main reported **WorldgenReview: PASS, no findings** for commits `364315ec`, `db0ba531` and `85df594b`. The non-Muse reviewer checked cache keys, revision wiring, datapack gradient endpoints and absence of committed Mojang data. No Rust changes followed that review.
