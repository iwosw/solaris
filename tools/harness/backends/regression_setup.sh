# Manifest, adapter, bridge, and per-run configuration helpers sourced by regression.sh.

require_file() {
  if [[ ! -f "$1" ]]; then
    printf 'error: missing required file: %s\n' "$1" >&2
    exit 1
  fi
}


require_dir() {
  if [[ ! -d "$1" ]]; then
    printf 'error: missing required directory: %s\n' "$1" >&2
    exit 1
  fi
}


manifest_quality_label() {
  python3 - "$MANIFEST" <<'PY'
import json
import sys
from pathlib import Path

manifest = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
quality_label = manifest.get("quality_label")
if not isinstance(quality_label, str) or not quality_label:
    print("error: manifest quality_label must be a non-empty string", file=sys.stderr)
    sys.exit(1)
print(quality_label)
PY
}


requested_scenario_no_debug() {
  require_file "$MANIFEST"
  python3 - "$MANIFEST" "$AGENT_SCENARIO" <<'PY'
import json
import sys
from pathlib import Path

manifest_path = Path(sys.argv[1])
scenario_id = sys.argv[2]
manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
scenarios = manifest.get("scenarios")
if not isinstance(scenarios, list):
    print("error: manifest scenarios must be a list", file=sys.stderr)
    sys.exit(1)
matches = [
    scenario
    for scenario in scenarios
    if isinstance(scenario, dict) and scenario.get("id") == scenario_id
]
if not matches:
    print(
        f"error: requested scenario {scenario_id!r} is missing from manifest {manifest_path}",
        file=sys.stderr,
    )
    sys.exit(1)
if len(matches) != 1:
    print(
        f"error: requested scenario {scenario_id!r} is duplicated in manifest {manifest_path}",
        file=sys.stderr,
    )
    sys.exit(1)
no_debug = matches[0].get("no_debug_commands")
if not isinstance(no_debug, bool):
    print(
        f"error: requested scenario {scenario_id!r} must declare boolean no_debug_commands",
        file=sys.stderr,
    )
    sys.exit(1)
print("1" if no_debug else "0")
PY
}


manifest_core_replay_source() {
  local manifest_path
  manifest_path="${1:-$MANIFEST}"
  python3 - "$manifest_path" <<'PY'
import json
import sys
from pathlib import Path, PurePosixPath

manifest = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
value = manifest.get("core_replay_manifest")
if value is None:
    sys.exit(0)
if not isinstance(value, str) or not value:
    print("error: core_replay_manifest must be a non-empty relative path", file=sys.stderr)
    sys.exit(1)
path = PurePosixPath(value)
if (
    path.is_absolute()
    or value != path.as_posix()
    or any(part in {"", ".", ".."} for part in path.parts)
    or path.parts[:2] != ("tools", "core-replay-scenarios")
    or path.suffix != ".json"
):
    print(
        "error: core_replay_manifest must name a JSON file under tools/core-replay-scenarios",
        file=sys.stderr,
    )
    sys.exit(1)
print(value)
PY
}


pick_free_loopback_port() {
  python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}


bridge_port_from_url() {
  python3 - "$1" <<'PY'
import sys
from urllib.parse import urlparse

parsed = urlparse(sys.argv[1])
if parsed.port is None:
    sys.exit(1)
print(parsed.port)
PY
}


configure_primary_agent_bridge() {
  if [[ -n "$AGENT_BRIDGE_URL" ]]; then
    if [[ -z "$AGENT_PORT" ]]; then
      if ! AGENT_PORT="$(bridge_port_from_url "$AGENT_BRIDGE_URL")"; then
        printf 'error: SOLARIS_REAL_CLIENT_AGENT_PORT is required when SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL has no explicit port\n' >&2
        exit 1
      fi
    fi
    return 0
  fi
  if [[ -n "$AGENT_SECRET" ]]; then
    if [[ -z "$AGENT_PORT" ]]; then
      AGENT_PORT="$(pick_free_loopback_port)"
    fi
    AGENT_BRIDGE_URL="http://127.0.0.1:${AGENT_PORT}/rpc"
  fi
}


configure_second_agent_bridge() {
  if [[ -z "$SECOND_AGENT_SECRET" && -z "$SECOND_AGENT_BRIDGE_URL" && -z "$SECOND_AGENT_PORT" ]]; then
    return 0
  fi
  if [[ -n "$SECOND_AGENT_BRIDGE_URL" ]]; then
    if [[ -z "$SECOND_AGENT_PORT" ]]; then
      if ! SECOND_AGENT_PORT="$(bridge_port_from_url "$SECOND_AGENT_BRIDGE_URL")"; then
        printf 'error: SOLARIS_REAL_CLIENT_SECOND_AGENT_PORT is required when SOLARIS_REAL_CLIENT_SECOND_AGENT_BRIDGE_URL has no explicit port\n' >&2
        exit 1
      fi
    fi
    return 0
  fi
  if [[ -n "$SECOND_AGENT_SECRET" ]]; then
    if [[ -z "$SECOND_AGENT_PORT" ]]; then
      SECOND_AGENT_PORT="$(pick_free_loopback_port)"
      while [[ -n "$AGENT_PORT" && "$SECOND_AGENT_PORT" == "$AGENT_PORT" ]]; do
        SECOND_AGENT_PORT="$(pick_free_loopback_port)"
      done
    fi
    SECOND_AGENT_BRIDGE_URL="http://127.0.0.1:${SECOND_AGENT_PORT}/rpc"
  fi
}


gradle_runclient_adapter_available() {
  local build_file
  build_file="$AGENT_ROOT/fabric-agent/build.gradle.kts"

  [[ -x "$AGENT_ROOT/gradlew" && -f "$build_file" ]] || return 1
  grep -Fq 'net.neoforged.moddev' "$build_file" || return 1
  grep -Fq 'create("clientAgent")' "$build_file" || return 1
  grep -Fq 'tasks.named("runClientAgent")' "$build_file" || return 1
  grep -Fq 'validateClientAgentRunProperties' "$build_file" || return 1
  grep -Fq 'gameDirectory.set(file(clientAgentGameDir.get()))' "$build_file" || return 1
  grep -Fq 'programArgument("--username")' "$build_file" || return 1
  grep -Fq 'programArgument(clientAgentUsername.get())' "$build_file"
}


resolve_client_adapter() {
  if gradle_runclient_adapter_available; then
    CLIENT_ADAPTER_SOURCE="auto-gradle-runclient"
    CLIENT_KIND="gradle-runclient"
    return 0
  fi

  CLIENT_ADAPTER_SOURCE="missing"
  return 1
}


server_config_path() {
  if [[ "$SERVER_CONFIG" = /* ]]; then
    printf '%s\n' "$SERVER_CONFIG"
  else
    printf '%s/%s\n' "$REPO_ROOT" "$SERVER_CONFIG"
  fi
}


client_username_for_label() {
  case "$1" in
    primary)
      printf 'SolarisPrimary\n'
      ;;
    secondary)
      printf 'SolarisSecondary\n'
      ;;
    *)
      printf 'SolarisClient\n'
      ;;
  esac
}


client_game_dir_for_label() {
  printf '%s/clients/%s\n' "$run_dir" "$1"
}


real_client_operator_list() {
  if [[ "$SCENARIO_NO_DEBUG" == "1" ]]; then
    printf '[]\n'
    return
  fi
  printf '["%s", "%s"]\n' "$(client_username_for_label primary)" "$(client_username_for_label secondary)"
}


write_real_client_server_config() {
  local source_config target_config world_dir server_port operators
  source_config="$1"
  target_config="$2"
  world_dir="$3"
  server_port="${4:-}"
  operators="$(real_client_operator_list)"
  if [[ -n "$world_dir" && "$MODE" == "run" ]]; then
    mkdir -p "$world_dir"
  fi
  awk -v world_dir="$world_dir" \
    -v seed_override="$SERVER_SEED" \
    -v server_port="$server_port" '
    BEGIN {
      section = ""
      replaced_world_dir = 0
      replaced_seed = 0
      replaced_port = 0
      escaped_world_dir = world_dir
      gsub(/\\/, "\\\\", escaped_world_dir)
      gsub(/"/, "\\\"", escaped_world_dir)
    }
    /^[[:space:]]*\[[^]]+\][[:space:]]*$/ {
      section = $0
      gsub(/^[[:space:]]*\[/, "", section)
      gsub(/\][[:space:]]*$/, "", section)
      print
      next
    }
    section == "network" && server_port != "" && /^[[:space:]]*port[[:space:]]*=/ && replaced_port == 0 {
      print "port = " server_port
      replaced_port = 1
      next
    }
    section == "data" && world_dir != "" && /^[[:space:]]*world_dir[[:space:]]*=/ && replaced_world_dir == 0 {
      print "world_dir = \"" escaped_world_dir "\""
      replaced_world_dir = 1
      next
    }
    section == "data" && seed_override != "" && /^[[:space:]]*seed[[:space:]]*=/ && replaced_seed == 0 {
      print "seed = " seed_override
      replaced_seed = 1
      next
    }
    { print }
    END {
      if (world_dir != "" && replaced_world_dir == 0) {
        print "error: server config has no data.world_dir setting" > "/dev/stderr"
        exit 1
      }
      if (seed_override != "" && replaced_seed == 0) {
        print "error: server config has no data.seed setting" > "/dev/stderr"
        exit 1
      }
      if (server_port != "" && replaced_port == 0) {
        print "error: server config has no network.port setting" > "/dev/stderr"
        exit 1
      }
    }
  ' "$source_config" > "$target_config"
  # solaris-v2 reads config-adjacent vanilla ops.json.
  python3 - "$target_config" "$operators" <<'PY'
import hashlib
import json
import sys
import uuid
from pathlib import Path

names = json.loads(sys.argv[2])
entries = [{
    "uuid": str(uuid.UUID(bytes=hashlib.md5(("OfflinePlayer:" + name).encode()).digest(), version=3)),
    "name": name,
    "level": 4,
    "bypassesPlayerLimit": False,
} for name in names]
Path(sys.argv[1]).with_name("ops.json").write_text(json.dumps(entries, indent=2) + "\n")
PY
}


client_adapter_status() {
  if ! resolve_client_adapter; then
    printf 'degraded: auto-discovery found no supported real-client launcher; repo-native Gradle runClient adapter is missing\n'
    return 1
  fi

  printf 'agent-run real-client configured: gradle-runclient\n'
  return 0
}


validate_second_client_config() {
  if [[ -z "$SECOND_AGENT_SECRET" && -z "$SECOND_AGENT_BRIDGE_URL" ]]; then
    return 1
  fi
  if [[ -z "$SECOND_AGENT_SECRET" || -z "$SECOND_AGENT_BRIDGE_URL" ]]; then
    printf 'error: second real-client mode requires SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET plus bridge URL or port\n' >&2
    exit 1
  fi
  if ! gradle_runclient_adapter_available; then
    printf 'error: second real-client mode requires the repo-native Gradle runClient adapter\n' >&2
    exit 1
  fi
  SECOND_CLIENT_ADAPTER_SOURCE="auto-gradle-runclient"
  SECOND_CLIENT_KIND="gradle-runclient"
  return 0
}


write_run_templates() {
  local run_dir status manifest_sha quality_label core_replay_source core_replay_sha
  run_dir="$1"
  status="$2"
  manifest_sha="$(sha256sum "$MANIFEST" | cut -d ' ' -f 1)"
  quality_label="$(manifest_quality_label)"
  core_replay_source="$(manifest_core_replay_source)"

  mkdir -p "$run_dir/screenshots"
  cp "$MANIFEST" "$run_dir/manifest.json"
  if [[ -n "$core_replay_source" ]]; then
    require_file "$REPO_ROOT/$core_replay_source"
    cp "$REPO_ROOT/$core_replay_source" "$run_dir/core-replay-manifest.json"
    core_replay_sha="$(sha256sum "$run_dir/core-replay-manifest.json" | cut -d ' ' -f 1)"
  else
    core_replay_sha=""
  fi

  {
    printf 'branch=%s\n' "$(git -C "$REPO_ROOT" branch --show-current)"
    printf 'commit=%s\n' "$(git -C "$REPO_ROOT" rev-parse HEAD)"
    printf 'status_short_begin\n'
    git -C "$REPO_ROOT" status --short --branch
    printf 'status_short_end\n'
    printf 'manifest=%s\n' "$MANIFEST"
    printf 'manifest_sha256=%s\n' "$manifest_sha"
    if [[ -n "$core_replay_source" ]]; then
      printf 'core_replay_source=%s\n' "$core_replay_source"
      printf 'core_replay_sha256=%s\n' "$core_replay_sha"
    fi
  } > "$run_dir/git.txt"

  {
    printf 'rustc=%s\n' "$(rustc --version)"
    printf 'cargo=%s\n' "$(cargo --version)"
    printf 'java=%s\n' "$(java --version 2>&1 | tr '\n' ' ')"
  } > "$run_dir/toolchain.txt"

  {
    printf 'client_gate=%s\n' "$status"
    printf 'client_kind=%s\n' "${CLIENT_KIND:-UNSET}"
    printf 'client_adapter_source=%s\n' "$CLIENT_ADAPTER_SOURCE"
    printf 'client_adapter_root=%s\n' "$AGENT_ROOT"
    printf 'client_adapter_gradlew=%s\n' "$AGENT_ROOT/gradlew"
    printf 'client_adapter_task=%s\n' "$GRADLE_RUNCLIENT_TASK"
    printf 'client_adapter_args=%s\n' "--no-daemon --no-configuration-cache $GRADLE_RUNCLIENT_TASK"
    printf 'client_adapter_sha256=%s\n' "$(printf '%s' "$AGENT_ROOT/gradlew --no-daemon --no-configuration-cache $GRADLE_RUNCLIENT_TASK" | sha256sum | cut -d ' ' -f 1)"
    printf 'client_username=%s\n' "$(client_username_for_label primary)"
    printf 'client_game_dir=%s\n' "$(client_game_dir_for_label primary)"
    if [[ -n "${SOLARIS_SERVER_BINARY:-}" ]]; then
      printf 'server_command=%s\n' "$SOLARIS_SERVER_BINARY --config $(server_config_path)"
      printf 'server_binary=%s\n' "$SOLARIS_SERVER_BINARY"
      printf 'server_binary_sha256=%s\n' "$(sha256sum -- "$SOLARIS_SERVER_BINARY" | cut -d ' ' -f 1)"
    else
      printf 'server_command=%s\n' "cargo run --bin solaris-v2 -p mc-edge -- --config $(server_config_path)"
    fi
    printf 'server_addr=%s\n' "$SERVER_ADDR"
    printf 'client_agent_driver=%s\n' "$AGENT_DRIVER"
    printf 'client_agent_runtime_provenance_primary=PENDING_GRADLE_RUNCLIENT\n'
    printf 'client_agent_port=%s\n' "$AGENT_PORT"
    printf 'client_agent_scenario=%s\n' "$AGENT_SCENARIO"
    if [[ -n "$core_replay_source" ]]; then
      printf 'core_replay_manifest=core-replay-manifest.json\n'
      printf 'core_replay_manifest_sha256=%s\n' "$core_replay_sha"
    fi
    if [[ -n "$AGENT_BRIDGE_URL" ]]; then
      printf 'client_agent_bridge_url=%s\n' "$AGENT_BRIDGE_URL"
    else
      printf 'client_agent_bridge_url=UNSET_PREPARED_OWNER_RUN\n'
    fi
    if [[ -n "$AGENT_SECRET" ]]; then
      printf 'client_agent_secret=SET_REDACTED\n'
    else
      printf 'client_agent_secret=UNSET_PREPARED_OWNER_RUN\n'
    fi
    printf 'second_client_kind=%s\n' "${SECOND_CLIENT_KIND:-UNSET}"
    printf 'second_client_adapter_source=%s\n' "$SECOND_CLIENT_ADAPTER_SOURCE"
    printf 'second_client_adapter_root=%s\n' "$AGENT_ROOT"
    printf 'second_client_adapter_gradlew=%s\n' "$AGENT_ROOT/gradlew"
    printf 'second_client_adapter_task=%s\n' "$GRADLE_RUNCLIENT_TASK"
    printf 'second_client_adapter_args=%s\n' "--no-daemon --no-configuration-cache $GRADLE_RUNCLIENT_TASK"
    printf 'second_client_adapter_sha256=%s\n' "$(printf '%s' "$AGENT_ROOT/gradlew --no-daemon --no-configuration-cache $GRADLE_RUNCLIENT_TASK" | sha256sum | cut -d ' ' -f 1)"
    printf 'second_client_username=%s\n' "$(client_username_for_label secondary)"
    printf 'second_client_game_dir=%s\n' "$(client_game_dir_for_label secondary)"
    printf 'second_client_agent_port=%s\n' "$SECOND_AGENT_PORT"
    if [[ -n "$SECOND_AGENT_BRIDGE_URL" ]]; then
      printf 'second_client_agent_bridge_url=%s\n' "$SECOND_AGENT_BRIDGE_URL"
    else
      printf 'second_client_agent_bridge_url=UNSET\n'
    fi
    if [[ -n "$SECOND_AGENT_SECRET" ]]; then
      printf 'second_client_agent_secret=SET_REDACTED\n'
    else
      printf 'second_client_agent_secret=UNSET\n'
    fi
    printf 'forbidden_clients=wire-probe,mc-test-harness,protocol-only bot,headless mock client\n'
  } > "$run_dir/automation-driver.txt"

  cat > "$run_dir/observations.json" <<EOF
{
  "schema": "solaris.real_client_observations.v1",
  "client_gate": "prepared-owner-run",
  "quality_label": "$quality_label",
  "result": "not-run",
  "scenarios": []
}
EOF

  touch "$run_dir/client.log" "$run_dir/server.log"
}


prepare_run_dir() {
  local timestamp pack_id status run_dir
  require_file "$MANIFEST"
  require_file "$(server_config_path)"
  resolve_client_adapter || true

  timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
  pack_id="$(basename "$MANIFEST" .json)"
  mkdir -p "$RUN_ROOT"
  run_dir="$(mktemp -d "$RUN_ROOT/$timestamp-$pack_id-XXXXXX")"
  status="$(client_adapter_status || true)"
  write_run_templates "$run_dir" "$status"
  printf '%s\n' "$run_dir"
}
