# Operating Solaris

The current binary is `solaris-v2` (`mc-edge` + `mc-core`). Start from
[`example.toml`](../example.toml). This guide covers the native runtime after
[ADR 0014](decisions/0014-v1-core-retirement.md), not the published historical
v0.0.7 preliminary binary.

```sh
cargo run -p mc-edge --bin solaris-v2 -- --config example.toml
# Direct Cargo-built binary (release packaging may name it `solaris`):
solaris-v2 --config server.toml
```

`--config` defaults to `server.toml`. Unknown sections/keys warn and are ignored;
a warning is not proof that a legacy setting took effect. There is no `--check`,
offline `operator` CLI, TUI/profile command, `check-world`, or rectangle
`pregenerate` command. Stdin is enabled unless `--no-console` is supplied.

## Vanilla content cache

Solaris does not distribute Minecraft data. Derive a complete cache from your
licensed 26.1.2 server bundle with JDK 25 (`JAVA`/`JAVAC` may select it):

```sh
solaris-v2 content import --from /path/to/server.jar --cache /path/to/cache
# Alternative: download Mojang's pinned artifact, verifying size and SHA-1.
solaris-v2 content import --version 26.1.2 --download --cache /path/to/cache
solaris-v2 content verify --cache /path/to/cache
```

Import stages, validates and publishes the cache; a failed derivation preserves
the previous cache. Verify is offline and checks the same content used by
startup, not the complete server/plugin deployment. Serving never downloads
implicitly. Missing or incomplete content fails before world/listener startup.

Selection order: `[data].vanilla_data_dir`, `SOLARIS_CONTENT_CACHE`,
`$XDG_DATA_HOME/solaris/content/26.1.2` (otherwise
`~/.local/share/solaris/content/26.1.2`), then `data/vanilla` in the working
directory and checkout ancestors. An explicit invalid override fails without
silently choosing another source. `--cache` affects only that offline command.

## Supported configuration

```toml
[server]
name = "Solaris"
motd = "Solaris native server"
max_players = 20
view_distance = 8
simulation_distance = 6

[network]
bind_address = "127.0.0.1"
port = 25565

[data]
world_dir = "world"
seed = 42
# vanilla_data_dir = "/path/to/cache"

[auth]
online_mode = true

# Omit to run without component packages.
# [plugins]
# directory = "plugins"
```

Relative world, content and plugin paths resolve against the process working
directory, not the TOML directory. Access-list files such as `ops.json` live
beside the selected config. Missing sections use native defaults: loopback TCP
25565, world `world`, seed 0, view distance 8, simulation distance 6, max players
20 and offline authentication. Use explicit operator-controlled values.

`view_distance` is the server's upper bound, not a forced client radius. Each
connection uses the smaller of its requested view distance and this limit
(minimum two chunks, without exceeding the server limit). Changing the client's
render distance during Play updates its cache radius and streams or forgets
chunks without reconnecting. World residency unions those per-player views,
the simulation radius and the existing extra chunk margin; explicit tickets
and dirty-chunk pins can keep additional chunks resident without sending them.

The old `[admin]`, `[simulation]`, `[autoscale]`, `[chunk_pipeline]`, worldgen-mode,
per-package grants/expected roster and reload knobs are not supported settings.
Do not carry them forward expecting old behavior. Use the source `EdgeConfig`
and its key list when adding new deployment settings.

## Network bind and port

Clients connect to `host:port`. `127.0.0.1` is local-only; an interface address
accepts clients on that interface; `0.0.0.0` listens on all IPv4 interfaces.
Choose an unused port and independently configure firewall/NAT for that TCP port.
Authentication does not alter listener exposure. Offline identities can be
impersonated, including operator names; restrict offline servers to trusted
players or a correctly authenticated network boundary.

## Server console and access control

Stdin lines, with or without a leading `/`, use the native command parser and
admin path at operator level four. Feedback goes to stdout; tracing goes to
stderr, filtered by `RUST_LOG`. There is no separate TUI or automatic
`logs/profile.json` diagnostic capture.

The console source has no player entity/inventory. Relative world coordinates
and nearest selectors originate at world spawn; executor-dependent commands
need an explicit player/selector. Player-only component commands report their
normal admission error rather than inventing a console player.

Examples in the running console:

```text
help
list
op PlayerName
deop PlayerName
save-all
stop
```

V2 uses vanilla `ops.json` beside the config. There is no standalone
`operator add|remove|list` command or local-dev operator configuration.
Vanilla command behavior and permission levels are owned by the native admin
path. `stop` requests normal shutdown and a final durable native checkpoint;
SIGINT/SIGTERM also checkpoint on exit. `--no-console` disables stdin admission
entirely and leaves the HTTP dashboard independent.

## Native worlds and Anvil interchange

Serving persists native `native/state.redb` and tick WAL, not Anvil region files.
Player, world and plugin effects recover at the same tick boundary. An
acknowledged operation is not an fsync guarantee: an unflushed tail may roll back
consistently after a crash. Corrupt/incompatible saved state fails closed.
Retain the same seed/content/deployment configuration and back up alpha worlds
before changing binaries or world identity. Never bypass identity validation by
hand-editing saved metadata.

Stop source worlds before conversion. Destinations must not exist; conversion
keeps the source unchanged and emits a receipt:

```sh
solaris-v2 --config server.toml world import-anvil \
  --source /srv/anvil-world --destination /srv/native-world --receipt /srv/import.json
solaris-v2 --config server.toml world export-anvil \
  --source /srv/native-world --destination /srv/exported-world --receipt /srv/export.json
```

Anvil is a compatibility format, not live fallback or dual-write storage.
Earth WorldPlan preparation and offline rectangular pregeneration are not
ported. Simulation tickets are not an offline generator: using them advances
simulation and persists ticket ownership.

## Offline native backup and restore

Stop the server first. Commands use the same exclusive world-directory lease
as startup, also lock the native database and refuse live worlds:

```sh
solaris-v2 backup --world-dir /srv/solaris/world --out /backups/world-2026-10-15
solaris-v2 restore --from /backups/world-2026-10-15 --world-dir /srv/solaris/restored
solaris-v2 --config restored-server.toml
```

The backup contains native database, WAL/journal and other world files with a
versioned SHA-256 manifest; lease files are excluded. Restore verifies the full
inventory and hashes before writing world data. The target must be empty or
absent, separate from the backup and exclusively leased. Missing/changed/extra
files, incompatible manifests and symlinks are refused. This is not a hot backup;
retain TOML, plugin artifacts and access lists separately.

## Plugins

```toml
[plugins]
directory = "plugins"
```

Each child is a deployable package with `plugin.toml`, `plugin.wasm`, optional
`config.toml` and declared resources. The directory is an exact deployment:
malformed entries, duplicate ids/command roots, failed startup and unsupported
declarations refuse startup. Selecting it authorizes manifest-declared ordinary
capabilities. There are no current strict/expected/grants or hot-reload settings.

Manifest `before-build` hooks register in deterministic package-id order with
fail-closed policy. Unsupported hooks, including `before-damage`, fail startup.
Server-only packages accept vanilla clients; client bundles require the matching
Loader and player permission approval. See [PLUGINS.md](PLUGINS.md) and
[SOLARIS_LOADER.md](SOLARIS_LOADER.md). Production builds do not need sibling
plugin source or compile guests at startup.

## Dashboard

The default-off dashboard is read-only and unauthenticated:

```toml
[dashboard]
enabled = true
bind_address = "127.0.0.1"
port = 8080
allow_remote = false
```

Open `http://127.0.0.1:8080/` while serving. Non-loopback addresses require
`allow_remote = true`; otherwise the dashboard is not started and a warning is
logged. Prefer an authenticated reverse proxy on a trusted network rather than
exposing it directly. Dashboard metrics are observations, not gameplay or
client-rendering evidence, and do not implement the deleted autoscale policy.

## Evidence boundaries

Use [AGENT_TOOLING.md](AGENT_TOOLING.md#harness-entrypoint) for registered
validation profiles. Historical preliminary-release instructions, regional-owner
benchmarks, multi-journal recovery receipts and prior console profile captures
retain their original meaning; they are not current operator commands or proof
of native-client acceptance. Maturity remains **draft**.
