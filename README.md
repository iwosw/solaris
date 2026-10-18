# Solaris

A Minecraft Java Edition **26.1.2** server written in Rust. The current server is
**`solaris-v2`**, composed by `mc-edge` over the native `mc-core` runtime.

- **Data-parallel simulation:** immutable snapshots, entity intents and a
  deterministic merge; spatial locality does not assign entity writer lanes.
- **Native persistence:** one tick WAL and native checkpoints. Anvil is an
  explicit offline import/export format, not the serving storage backend.
- **Vanilla clients:** server-only packages need no client mod; optional authored
  content uses Solaris Loader.
- **Wasmtime components:** sandboxed packages implementing `solaris:plugin@0.7.0`,
  running outside the tick through bounded admission.
- **Evidence:** harness runs record commands, scope and results under
  `.analysis/validation/`; a preparation receipt is not a gameplay pass.

[ADR 0013](docs/decisions/0013-core-model-v2.md) defines the current model.
[ADR 0014](docs/decisions/0014-v1-core-retirement.md) records the completed v1
retirement. Older regional-owner benchmarks and preliminary-release receipts
are historical evidence, not claims about this runtime.

> **Development status:** the workspace targets **v0.0.8** and remains **draft**.
> The published **v0.0.7** preliminary release predates this cutover. Solaris is
> for testing, plugin development and bounded field tests, not a production-safe
> replacement for vanilla, Paper, Fabric, Forge or NeoForge. Worlds, APIs and
> Loader contracts may change without migration.

## Build and run the development tree

Use the debug profile for normal development:

```sh
cargo build -p mc-edge --bin solaris-v2
cargo run -p mc-edge --bin solaris-v2 -- content import --from /path/to/server.jar
cargo run -p mc-edge --bin solaris-v2 -- content verify
cargo run -p mc-edge --bin solaris-v2 -- --config example.toml
```

Content import requires the operator's licensed 26.1.2 server bundle and JDK 25;
`--download` can select Mojang's pinned artifact instead of `--from`. Solaris
never distributes Mojang data or downloads it implicitly during serving.

`--config` defaults to `server.toml`. Unknown configuration keys warn and are
ignored; they do not enable old features. There is no `--check`, offline
`operator` subcommand, pregeneration rectangle command or console TUI/profile
command. `content verify` checks the content cache only, not a complete server
or plugin deployment. See [the operating guide](docs/OPERATING.md).

For the locked release server build used by release gates:

```sh
python3 -m tools.harness run build --release
```

This builds `mc-edge --bin solaris-v2`, not every workspace target.

## Configuration essentials

Start with [`example.toml`](example.toml):

```toml
[network]
bind_address = "127.0.0.1"
port = 25565

[server]
view_distance = 8
simulation_distance = 6
max_players = 20

[data]
world_dir = "world"
seed = 42

[auth]
online_mode = true
```

- Clients connect to `host:port`. Remote listening requires an appropriate bind
  address and matching firewall/NAT policy. Offline identities can be impersonated.
- Relative world/content/plugin paths resolve against the process working
  directory. Vanilla access lists, including `ops.json`, live beside the config.
- Content cache selection is `[data].vanilla_data_dir`, `SOLARIS_CONTENT_CACHE`,
  then XDG/home discovery and checkout `data/vanilla`. Explicit invalid overrides
  fail rather than falling back.
- Back up the whole stopped native world and retain config, access lists and
  plugin packages separately. Corrupt or incompatible state fails closed.
- `[plugins].directory` selects an exact directory-only deployment. The old
  strict/expected/grants configuration and autoscale knobs are not current settings.

## Console, operators and dashboard

Stdin accepts ordinary vanilla-style commands, with or without `/`: `help`,
`list`, `op PlayerName`, `deop PlayerName`, `save-all` and `stop`. The server
source has permission level four but no player inventory or entity. Use explicit
players/selectors for commands needing a player. `stop`, SIGINT and SIGTERM
request a final native checkpoint. `--no-console` disables stdin admission;
feedback goes to stdout and tracing logs to stderr (`RUST_LOG` selects filtering).

The optional default-off read-only HTTP dashboard is configured with
`[dashboard]`; it is unauthenticated. Prefer loopback or an authenticated reverse
proxy. See [operating details](docs/OPERATING.md#dashboard).

## Offline world operations

Stop the server before backup, restore or conversion:

```sh
solaris-v2 backup --world-dir world --out /backups/world-copy
solaris-v2 restore --from /backups/world-copy --world-dir restored-world
solaris-v2 --config example.toml world import-anvil \
  --source /worlds/anvil --destination /worlds/native --receipt /worlds/import.json
solaris-v2 --config example.toml world export-anvil \
  --source /worlds/native --destination /worlds/export --receipt /worlds/export.json
```

Conversion destinations must not exist; source worlds are not modified. A backup
is not a hot snapshot and does not include deployment files outside the world.

## Plugins and Solaris Loader

Deploy independently built `plugin.toml` / `plugin.wasm` packages under
`[plugins].directory`. The out-of-tree `sdk/rust/` workspace contains the generic
Rust SDK and examples; first-party product source belongs to
`../solaris-default-plugins`. Production core builds need neither sibling.

Packages without client bundles accept vanilla clients. Packages with bundles
require a matching Solaris Loader adapter and player permission approval.
Core uses Loader wire **3**, artifact schema **2**, through `mc-loader`.

- [Plugin operator/author guide](docs/PLUGINS.md)
- [Fabric/NeoForge/Forge client installation](docs/SOLARIS_LOADER.md)
- [Independent Loader repository](https://github.com/kaiserproger/solaris-loader)

## Validation and limits

Ordinary survival, entities, crafting, persistence and plugin integrations are
implemented for field testing, not complete vanilla parity. Protocol and Anvil
layouts require exact evidence; gameplay/worldgen target vanilla-like behavior.
A historical gate pass never establishes current native-client acceptance or a
whole-server performance envelope. Owner field testing followed by varied-seed
exploration remains necessary.

```sh
python3 -m tools.harness list
python3 -m tools.harness run correctness
python3 -m tools.harness run inventory
python3 -m tools.harness run playable --check  # preparation only
```

`correctness` runs formatter, strict workspace Clippy, code-health and
workspace/all-target tests. Release and graphical runs have their own scope and
prerequisites; read [harness wiring](docs/AGENT_TOOLING.md#validation-harness).

## Documentation map

- [Architecture](docs/ARCHITECTURE.md), [runtime map](docs/architecture/runtime.md),
  [state ownership](docs/architecture/state-ownership.md), [mutations](docs/architecture/mutations.md)
- [Architecture program closeout](docs/CORE_ARCHITECTURE_PLAN.md), [ADRs](docs/decisions/README.md)
- [Operating](docs/OPERATING.md), [project scope](docs/PROJECT_SPEC.md),
  [village generation](docs/VILLAGE_GENERATION.md)
- [Agent tooling](docs/AGENT_TOOLING.md), [readiness definitions](docs/DEFINITION_OF_DONE.md)
- [Memory/evidence cursor](docs/MEMORY.md); milestone, memory, night-run, dated
  evidence, performance and release records preserve their original runtime era.
- [Published preliminary v0.0.7](https://github.com/kaiserproger/solaris/releases/tag/v0.0.7)
  and its [historical release notes](docs/releases/v0.0.7.md); those release
  binaries and commands are not the current development server.

## License

Dual-licensed under either Apache-2.0 ([`LICENSE-APACHE`](LICENSE-APACHE)) or MIT
([`LICENSE-MIT`](LICENSE-MIT)), at your option.
