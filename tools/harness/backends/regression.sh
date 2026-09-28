#!/usr/bin/env bash
# Prepare or run the M94+ real-client regression pack.
# This path is intentionally fail-closed: protocol bots and mocks are rejected,
# and prepared artifacts do not count as passed client evidence.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
MANIFEST="${SOLARIS_REAL_CLIENT_MANIFEST:-$REPO_ROOT/docs/real-client-regression/manifests/m94-regression-pack.json}"
RUN_ROOT="${SOLARIS_REAL_CLIENT_RUN_ROOT:-$REPO_ROOT/.analysis/real-client-runs}"
AGENT_ROOT="${SOLARIS_LOADER_ROOT:-$REPO_ROOT/../solaris-loader}"
GRADLE_RUNCLIENT_TASK=":fabric-agent:runClientAgent"
CLIENT_ADAPTER_SOURCE="auto-gradle-runclient"
CLIENT_KIND="gradle-runclient"
SECOND_CLIENT_ADAPTER_SOURCE="auto-gradle-runclient"
SECOND_CLIENT_KIND="gradle-runclient"
CLIENT_TIMEOUT_SECONDS="${SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS:-180}"
SERVER_READY_TIMEOUT_SECONDS="${SOLARIS_REAL_CLIENT_SERVER_READY_TIMEOUT_SECONDS:-120}"
SERVER_CONFIG="${SOLARIS_REAL_CLIENT_SERVER_CONFIG:-example.toml}"
FRESH_WORLD="${SOLARIS_REAL_CLIENT_FRESH_WORLD:-0}"
SERVER_SEED="${SOLARIS_REAL_CLIENT_SERVER_SEED:-}"
SERVER_ADDR="${SOLARIS_REAL_CLIENT_SERVER_ADDR:-127.0.0.1:25565}"
SCENARIO_SERVER_PORT="${SCENARIO_SERVER_PORT:-}"
AGENT_DRIVER="${SOLARIS_REAL_CLIENT_AGENT_DRIVER:-$REPO_ROOT/tools/harness/backends/driver.py}"
AGENT_BRIDGE_URL="${SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL:-}"
AGENT_SECRET="${SOLARIS_REAL_CLIENT_AGENT_SECRET:-}"
AGENT_PORT="${SOLARIS_REAL_CLIENT_AGENT_PORT:-}"
AGENT_SCENARIO="${SOLARIS_REAL_CLIENT_AGENT_SCENARIO:-m94-02b-rejected-block-resync}"
SECOND_AGENT_SECRET="${SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET:-}"
SECOND_AGENT_PORT="${SOLARIS_REAL_CLIENT_SECOND_AGENT_PORT:-}"
SECOND_AGENT_BRIDGE_URL="${SOLARIS_REAL_CLIENT_SECOND_AGENT_BRIDGE_URL:-}"
MODE="prepare"
VALIDATE_RUN_DIR=""
SCENARIO_NO_DEBUG="0"

usage() {
  cat <<'EOF'
Usage: python3 -m tools.harness run regression [--check|--prepare|--run|--validate-run DIR]

Environment:
	  Primary client adapter is auto-selected as the repo-native
	                                Gradle runClient adapter:
	                                ../solaris-loader/gradlew
	                                --no-configuration-cache :fabric-agent:runClientAgent.
  SOLARIS_REAL_CLIENT_MANIFEST  Regression pack manifest path.
  SOLARIS_REAL_CLIENT_RUN_ROOT  Local-only artifact root. Defaults to .analysis/real-client-runs.
  SOLARIS_REAL_CLIENT_SERVER_CONFIG
                                Server config path. Defaults to example.toml.
  SOLARIS_REAL_CLIENT_FRESH_WORLD
                                If 1, copy the server config into the run dir
                                and point data.world_dir at a fresh per-run world.
                                --run always uses a per-run server.toml that
                                grants the Gradle client usernames operator
                                access for local debug commands, except
                                playable-46-generated-ruin-cache which uses
                                no operators.
  SOLARIS_REAL_CLIENT_SERVER_SEED
                                Optional signed decimal integer used to override
                                data.seed in the prepared server config.
  SOLARIS_REAL_CLIENT_SERVER_ADDR
                                Server address passed to the in-client agent driver.
                                Defaults to 127.0.0.1:25565.
  SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS
                                Timeout for the Gradle runClient adapter.
                                Defaults to 180.
  SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL
                                Loopback JSON bridge URL inside the real client.
  SOLARIS_REAL_CLIENT_AGENT_SECRET
                                Per-run bridge secret. Required for agent-driver mode.
  SOLARIS_REAL_CLIENT_AGENT_PORT
                                Java agent bridge port. Defaults to a free
                                loopback port selected per run.
  SOLARIS_REAL_CLIENT_AGENT_DRIVER
                                Internal driver path. Defaults to tools/harness/backends/driver.py.
  SOLARIS_REAL_CLIENT_AGENT_SCENARIO
                                Scenario id. Defaults to m94-02b-rejected-block-resync.
  SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET
                                Separate per-run bridge secret for the second client.
  SOLARIS_REAL_CLIENT_SECOND_AGENT_PORT
                                Second Java agent bridge port. Defaults to a
                                free loopback port selected per run.
  SOLARIS_REAL_CLIENT_SECOND_AGENT_BRIDGE_URL
                                Optional explicit loopback bridge URL for the second client.

Modes:
  --check         Validate that the Gradle runClient adapter is available.
  --prepare       Create a local run directory and observation templates.
  --run           Start Solaris, launch Gradle runClient, and record logs.
  --validate-run  Check that a run directory has the required artifact shape.

Protocol bots, wire-probe, mc-test-harness clients, and mocks are rejected.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check)
      MODE="check"
      ;;
    --prepare)
      MODE="prepare"
      ;;
    --run)
      MODE="run"
      ;;
    --validate-run)
      MODE="validate-run"
      shift
      if [[ $# -eq 0 ]]; then
        printf 'error: --validate-run requires a run directory\n' >&2
        exit 2
      fi
      VALIDATE_RUN_DIR="$1"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'error: unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

if [[ -n "$SERVER_SEED" ]]; then
  if [[ ! "$SERVER_SEED" =~ ^[+-]?[0-9]+$ ]] \
    || ! SERVER_SEED="$(python3 - "$SERVER_SEED" <<'PY'
import sys

value = int(sys.argv[1], 10)
if value < -(1 << 63) or value > (1 << 63) - 1:
    raise SystemExit(1)
print(value)
PY
)"; then
    printf 'error: SOLARIS_REAL_CLIENT_SERVER_SEED must be a signed 64-bit decimal integer\n' >&2
    exit 2
  fi
fi

. "$REPO_ROOT/tools/harness/backends/regression_setup.sh"
. "$REPO_ROOT/tools/harness/backends/regression_validation.sh"
. "$REPO_ROOT/tools/harness/backends/regression_scenarios.sh"

if [[ "$MODE" == "check" || "$MODE" == "run" ]]; then
  SCENARIO_NO_DEBUG="$(requested_scenario_no_debug)"
fi

if [[ "$MODE" == "run" && -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
  printf 'error: --run requires a graphical display; set DISPLAY or WAYLAND_DISPLAY\n' >&2
  exit 1
fi

if [[ "$MODE" == "check" ]]; then
  client_adapter_status
  if [[ "$SCENARIO_NO_DEBUG" == "1" ]]; then
    printf 'scenario policy: no-debug, no operator privileges\n'
  else
    printf 'scenario policy: debug commands allowed by manifest\n'
  fi
  exit 0
fi

if [[ "$MODE" == "validate-run" ]]; then
  validate_run_dir "$VALIDATE_RUN_DIR"
  exit 0
fi

if [[ "$MODE" == "prepare" ]]; then
  configure_primary_agent_bridge
  configure_second_agent_bridge
  SCENARIO_NO_DEBUG="$(requested_scenario_no_debug 2>/dev/null || printf '1\n')"
  run_dir="$(prepare_run_dir)"
  server_config_source="$(server_config_path)"
  server_config="$run_dir/server.toml"
  fresh_world_dir=""
  if [[ "$FRESH_WORLD" == "1" || "$AGENT_SCENARIO" == "playable-46-generated-ruin-cache" ]]; then
    fresh_world_dir="$run_dir/world"
  fi
  write_real_client_server_config \
    "$server_config_source" "$server_config" "$fresh_world_dir" "$SCENARIO_SERVER_PORT"
  {
    printf 'server_config_source=%s\n' "$server_config_source"
    printf 'server_config_effective=%s\n' "$server_config"
    if [[ -n "$fresh_world_dir" ]]; then
      printf 'server_world_dir=%s\n' "$fresh_world_dir"
    fi
    if [[ -n "$SERVER_SEED" ]]; then
      printf 'server_seed_override=%s\n' "$SERVER_SEED"
    fi
    if [[ -n "$SCENARIO_SERVER_PORT" ]]; then
      printf 'server_port_effective=%s\n' "$SCENARIO_SERVER_PORT"
    fi
  } >> "$run_dir/automation-driver.txt"
  printf '%s\n' "$run_dir"
  exit 0
fi

if [[ "$MODE" == "run" ]]; then
  client_adapter_status >/dev/null
  if [[ "$CLIENT_KIND" == "gradle-runclient" && -z "$AGENT_SECRET" ]]; then
    AGENT_SECRET="$(python3 - <<'PY'
import secrets
print("s_" + secrets.token_urlsafe(24))
PY
)"
  fi
  if [[ ( "$AGENT_SCENARIO" == "playable-30-two-client-shared-log-drop-pickup" || "$AGENT_SCENARIO" == "playable-31-two-client-earned-shared-chest" || "$AGENT_SCENARIO" == "playable-32-two-client-earned-torch-block-edit" || "$AGENT_SCENARIO" == "playable-33-two-client-player-visibility-movement" || "$AGENT_SCENARIO" == "playable-34-two-client-chat-message" || "$AGENT_SCENARIO" == "playable-35-two-client-player-disconnect-removal" || "$AGENT_SCENARIO" == "playable-36-two-client-player-reconnect-cleanup" || "$AGENT_SCENARIO" == "playable-37-two-client-player-death-respawn-visibility" || "$AGENT_SCENARIO" == "playable-38-two-client-inventory-drop-handoff" || "$AGENT_SCENARIO" == "playable-39-two-client-short-soak" || "$AGENT_SCENARIO" == "playable-40-two-client-chunk-stream-crossing" || "$AGENT_SCENARIO" == "playable-41-two-client-chunk-prewarm-crossing" || "$AGENT_SCENARIO" == "playable-42-two-client-opposite-chunk-crossing" || "$AGENT_SCENARIO" == "playable-45-two-client-shared-chest-save-restart" ) && -z "$SECOND_AGENT_SECRET" ]]; then
    SECOND_AGENT_SECRET="$(python3 - <<'PY'
import secrets
print("s_" + secrets.token_urlsafe(24))
PY
)"
  fi
  configure_primary_agent_bridge
  configure_second_agent_bridge
  run_dir="$(prepare_run_dir)"
  server_config_source="$(server_config_path)"
  server_config="$run_dir/server.toml"
  fresh_world_dir=""
  if [[ "$FRESH_WORLD" == "1" || "$AGENT_SCENARIO" == "playable-46-generated-ruin-cache" ]]; then
    fresh_world_dir="$run_dir/world"
  fi
  if [[ "$AGENT_SCENARIO" == "playable-46-generated-ruin-cache" ]] \
    && ! mkdir "$fresh_world_dir"; then
    printf 'error: playable-46 refuses to reuse an existing world directory: %s\n' "$fresh_world_dir" >&2
    exit 1
  fi
  write_real_client_server_config \
    "$server_config_source" "$server_config" "$fresh_world_dir" "$SCENARIO_SERVER_PORT"
  {
    printf 'server_config_source=%s\n' "$server_config_source"
    printf 'server_config_effective=%s\n' "$server_config"
    printf 'server_command_effective=%s\n' "cargo run --bin mc-server -- --config $server_config"
    if [[ "$SCENARIO_NO_DEBUG" == "1" ]]; then
      printf 'server_op_users=NONE\n'
    else
      printf 'server_op_users=%s,%s\n' "$(client_username_for_label primary)" "$(client_username_for_label secondary)"
    fi
    if [[ -n "$fresh_world_dir" ]]; then
      printf 'server_world_dir=%s\n' "$fresh_world_dir"
    else
      printf 'server_world_dir=UNCHANGED_FROM_SOURCE\n'
    fi
    if [[ -n "$SERVER_SEED" ]]; then
      printf 'server_seed_override=%s\n' "$SERVER_SEED"
    fi
    if [[ -n "$SCENARIO_SERVER_PORT" ]]; then
      printf 'server_port_effective=%s\n' "$SCENARIO_SERVER_PORT"
    fi
  } >> "$run_dir/automation-driver.txt"
  printf 'running real-client regression into %s\n' "$run_dir"

  server_pid=""
  client_pid=""
  second_client_pid=""
  driver_timeout_seconds=$((CLIENT_TIMEOUT_SECONDS + 15))
  server_restart_count=0
  agent_phase_status=0
  second_client_enabled=0

  start_server() {
    local phase ready_fifo ready_fd ready_result
    phase="$1"
    printf 'server_start_phase=%s\n' "$phase" >> "$run_dir/automation-driver.txt"
    ready_fifo="$run_dir/.server-ready-$phase.fifo"
    rm -f "$ready_fifo"
    mkfifo "$ready_fifo"
    exec {ready_fd}<>"$ready_fifo"
    (
      cd "$REPO_ROOT"
      exec cargo run --bin mc-server -- --config "$server_config"
    ) > >(
      ready_sent=0
      while IFS= read -r line; do
        printf '%s\n' "$line" >> "$run_dir/server.log"
        if [[ "$ready_sent" -eq 0 && "$line" == *"Solaris is listening"* ]]; then
          printf 'ready\n' > "$ready_fifo"
          ready_sent=1
        fi
      done
      if [[ "$ready_sent" -eq 0 ]]; then
        printf 'exited-before-ready\n' > "$ready_fifo"
      fi
    ) 2>&1 &
    server_pid="$!"
    printf 'server_pid_%s=%s\n' "$phase" "$server_pid" >> "$run_dir/automation-driver.txt"
    ready_result=""
    if ! IFS= read -r -t "$SERVER_READY_TIMEOUT_SECONDS" ready_result <&"$ready_fd"; then
      ready_result="timeout"
    fi
    exec {ready_fd}>&-
    rm -f "$ready_fifo"
    printf 'server_ready_phase=%s status=%s\n' "$phase" "$ready_result" >> "$run_dir/automation-driver.txt"
    if [[ "$ready_result" != "ready" ]]; then
      printf 'error: server did not publish readiness for phase %s: %s\n' "$phase" "$ready_result" >&2
      return 1
    fi
  }

  stop_server_gracefully() {
    local phase require_clean_stop status
    phase="$1"
    require_clean_stop="${2:-0}"
    if [[ -z "${server_pid:-}" ]]; then
      [[ "$require_clean_stop" -eq 0 ]]
      return
    fi
    if kill -0 "$server_pid" >/dev/null 2>&1; then
      printf 'server_stop_phase=%s signal=INT\n' "$phase" >> "$run_dir/automation-driver.txt"
      kill -INT "$server_pid" >/dev/null 2>&1 || true
    fi
    set +e
    wait "$server_pid" >/dev/null 2>&1
    status="$?"
    set -e
    printf 'server_exit_phase=%s status=%s\n' "$phase" "$status" >> "$run_dir/automation-driver.txt"
    server_pid=""
    if [[ "$require_clean_stop" -eq 1 ]]; then
      return "$status"
    fi
    return 0
  }

  run_agent_driver_phase() {
    local scenario phase_status
    local -a replay_args
    scenario="$1"
    shift
    replay_args=()
    if [[ -f "$run_dir/core-replay-manifest.json" ]]; then
      replay_args+=(--replay-manifest "$run_dir/core-replay-manifest.json")
    fi
    printf 'client_agent_phase_scenario=%s\n' "$scenario" >> "$run_dir/automation-driver.txt"
    set +e
    timeout "$driver_timeout_seconds" python3 "$AGENT_DRIVER" \
      --bridge-url "$AGENT_BRIDGE_URL" \
      --secret="$AGENT_SECRET" \
      --run-dir "$run_dir" \
      --scenario "$scenario" \
      --server-addr "$SERVER_ADDR" \
      --timeout-seconds "$CLIENT_TIMEOUT_SECONDS" \
      "${replay_args[@]}" \
      "$@" >> "$run_dir/automation-driver.txt" 2>&1
    phase_status="$?"
    set -e
    agent_phase_status="$phase_status"
    printf 'client_agent_phase_exit_status_%s=%s\n' "$scenario" "$phase_status" >> "$run_dir/automation-driver.txt"
    return 0
  }

  validate_client_agent_runtime_provenance() {
    local label provenance_file expected_classes_dir provenance_result provenance_status
    local artifact_kind artifact_path artifact_sha256 artifact_file_count
    label="$1"
    provenance_file="$run_dir/client-agent-runtime-$label.txt"
    expected_classes_dir="$AGENT_ROOT/fabric-agent/build/classes/java/main"
    set +e
    provenance_result="$(python3 - "$provenance_file" "$expected_classes_dir" <<'PY'
import hashlib
import sys
from pathlib import Path

provenance_file = Path(sys.argv[1])
expected_classes_dir = Path(sys.argv[2]).resolve()
required = {
    "runtime_artifact_kind",
    "runtime_artifact_path",
    "runtime_artifact_sha256",
    "runtime_artifact_file_count",
}

try:
    entries = {}
    for line in provenance_file.read_text(encoding="utf-8").splitlines():
        key, separator, value = line.partition("=")
        if not separator or key in entries:
            raise ValueError(f"invalid runtime provenance line: {line!r}")
        entries[key] = value
    if set(entries) != required:
        raise ValueError("runtime provenance fields do not match the required schema")
    if entries["runtime_artifact_kind"] != "compiled-classes":
        raise ValueError("runtime artifact kind must be compiled-classes")
    artifact_path = Path(entries["runtime_artifact_path"]).resolve()
    if artifact_path != expected_classes_dir:
        raise ValueError(f"runtime artifact path must be {expected_classes_dir}, got {artifact_path}")
    if not artifact_path.is_dir():
        raise ValueError(f"runtime artifact directory is missing: {artifact_path}")
    files = sorted(
        (path for path in artifact_path.rglob("*") if path.is_file()),
        key=lambda path: path.relative_to(artifact_path).as_posix(),
    )
    if not files:
        raise ValueError("runtime artifact directory is empty")
    if entries["runtime_artifact_file_count"] != str(len(files)):
        raise ValueError("runtime artifact file count does not match the compiled classes directory")
    digest = hashlib.sha256()
    for path in files:
        digest.update(path.relative_to(artifact_path).as_posix().encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
    if entries["runtime_artifact_sha256"] != digest.hexdigest():
        raise ValueError("runtime artifact digest does not match the compiled classes directory")
except (OSError, ValueError) as exc:
    print(f"error: {exc}")
    sys.exit(1)

for key in sorted(required):
    print(f"{key}={entries[key]}")
PY
)"
    provenance_status="$?"
    set -e
    if [[ "$provenance_status" -ne 0 ]]; then
      printf 'client_agent_runtime_validation_%s=failed\n' "$label" >> "$run_dir/automation-driver.txt"
      printf 'client_agent_runtime_validation_error_%s=%s\n' "$label" "$provenance_result" >> "$run_dir/automation-driver.txt"
      printf 'error: %s client runtime classes provenance validation failed: %s\n' "$label" "$provenance_result" >&2
      return 1
    fi
    artifact_kind="$(sed -n 's/^runtime_artifact_kind=//p' <<< "$provenance_result")"
    artifact_path="$(sed -n 's/^runtime_artifact_path=//p' <<< "$provenance_result")"
    artifact_sha256="$(sed -n 's/^runtime_artifact_sha256=//p' <<< "$provenance_result")"
    artifact_file_count="$(sed -n 's/^runtime_artifact_file_count=//p' <<< "$provenance_result")"
    {
      printf 'client_agent_runtime_kind_%s=%s\n' "$label" "$artifact_kind"
      printf 'client_agent_runtime_path_%s=%s\n' "$label" "$artifact_path"
      printf 'client_agent_runtime_sha256_%s=%s\n' "$label" "$artifact_sha256"
      printf 'client_agent_runtime_file_count_%s=%s\n' "$label" "$artifact_file_count"
      printf 'client_agent_runtime_validation_%s=verified\n' "$label"
    } >> "$run_dir/automation-driver.txt"
  }

  wait_for_agent_bridge() {
    local label bridge_url secret timeout wait_result wait_status wait_seconds
    label="$1"
    bridge_url="$2"
    secret="$3"
    timeout="$4"
    set +e
    wait_result="$(python3 - "$bridge_url" "$secret" "$timeout" <<'PY'
import json
import sys
import time
from urllib import request

bridge_url = sys.argv[1]
secret = sys.argv[2]
timeout_seconds = float(sys.argv[3])
started = time.monotonic()
payload = json.dumps({
    "id": 1,
    "secret": secret,
    "command": "ping",
    "payload": {},
}, separators=(",", ":")).encode("utf-8")
rpc_request = request.Request(
    bridge_url,
    data=payload,
    headers={"Content-Type": "application/json"},
    method="POST",
)
try:
    with request.urlopen(rpc_request, timeout=timeout_seconds) as response:
        decoded = json.loads(response.read().decode("utf-8"))
except Exception as exc:
    print(f"unavailable {time.monotonic() - started:.3f} {exc}")
    sys.exit(1)
if decoded.get("ok") is not True:
    print(f"invalid {time.monotonic() - started:.3f} {json.dumps(decoded, sort_keys=True)}")
    sys.exit(1)
print(f"ready {time.monotonic() - started:.3f}")
PY
)"
    wait_status="$?"
    set -e
    wait_seconds="$(printf '%s\n' "$wait_result" | awk '{print $2}')"
    if [[ "$wait_status" -eq 0 ]]; then
      if ! validate_client_agent_runtime_provenance "$label"; then
        return 1
      fi
      printf 'client_agent_bridge_wait_status_%s=ready\n' "$label" >> "$run_dir/automation-driver.txt"
      printf 'client_agent_bridge_wait_seconds_%s=%s\n' "$label" "$wait_seconds" >> "$run_dir/automation-driver.txt"
      return 0
    fi
    printf 'client_agent_bridge_wait_status_%s=timeout\n' "$label" >> "$run_dir/automation-driver.txt"
    printf 'client_agent_bridge_wait_seconds_%s=%s\n' "$label" "${wait_seconds:-$timeout}" >> "$run_dir/automation-driver.txt"
    printf 'client_agent_bridge_wait_error_%s=%s\n' "$label" "$wait_result" >> "$run_dir/automation-driver.txt"
    return 1
  }

  launch_gradle_runclient() {
    local label port secret log_file pid_var jdk_options client_game_dir client_username runtime_provenance_file
    local ready_fifo ready_fd ready_result
    label="$1"
    port="$2"
    secret="$3"
    log_file="$4"
    pid_var="$5"
    jdk_options="${JDK_JAVA_OPTIONS:-}"
    client_game_dir="$(client_game_dir_for_label "$label")"
    client_username="$(client_username_for_label "$label")"
    mkdir -p "$client_game_dir"
    : > "$log_file"
    ready_fifo="$run_dir/.client-agent-ready-$label.fifo"
    rm -f "$ready_fifo"
    mkfifo "$ready_fifo"
    exec {ready_fd}<>"$ready_fifo"
    runtime_provenance_file="$run_dir/client-agent-runtime-$label.txt"
    rm -f "$runtime_provenance_file"
    printf 'client_agent_runtime_entrypoint_%s=GRADLE_RUNCLIENT_ENTRYPOINT\n' "$label" >> "$run_dir/automation-driver.txt"
    printf 'client_game_dir_%s=%s\n' "$label" "$client_game_dir" >> "$run_dir/automation-driver.txt"
    printf 'client_username_%s=%s\n' "$label" "$client_username" >> "$run_dir/automation-driver.txt"
    (
      cd "$AGENT_ROOT"
      SOLARIS_CLIENT_AGENT_SECRET="$secret" \
        SOLARIS_CLIENT_AGENT_PORT="$port" \
        SOLARIS_CLIENT_AGENT_RUN_DIR="$run_dir" \
        SOLARIS_CLIENT_AGENT_GAME_DIR="$client_game_dir" \
        SOLARIS_CLIENT_AGENT_USERNAME="$client_username" \
        JDK_JAVA_OPTIONS="$jdk_options" \
        ./gradlew --no-configuration-cache "$GRADLE_RUNCLIENT_TASK" \
          -Psolaris.clientAgent.secret="$secret" \
          -Psolaris.clientAgent.port="$port" \
          -Psolaris.clientAgent.runDir="$run_dir" \
          -Psolaris.clientAgent.gameDir="$client_game_dir" \
          -Psolaris.clientAgent.runtimeProvenanceFile="$runtime_provenance_file" \
          -Psolaris.clientAgent.username="$client_username"
    ) > >(
      ready_sent=0
      while IFS= read -r line; do
        printf '%s\n' "$line" >> "$log_file"
        if [[ "$ready_sent" -eq 0 \
          && "$line" == *"Solaris client agent bridge listening on http://127.0.0.1:$port/rpc"* ]]; then
          printf 'ready\n' > "$ready_fifo"
          ready_sent=1
        fi
      done
      if [[ "$ready_sent" -eq 0 ]]; then
        printf 'exited-before-ready\n' > "$ready_fifo"
      fi
    ) 2>&1 &
    printf -v "$pid_var" '%s' "$!"
    printf 'client_pid_%s=%s\n' "$label" "${!pid_var}" >> "$run_dir/automation-driver.txt"
    ready_result=""
    if ! IFS= read -r -t "$CLIENT_TIMEOUT_SECONDS" ready_result <&"$ready_fd"; then
      ready_result="timeout"
    fi
    exec {ready_fd}>&-
    rm -f "$ready_fifo"
    printf 'client_agent_process_ready_%s=%s\n' "$label" "$ready_result" >> "$run_dir/automation-driver.txt"
    if [[ "$ready_result" != "ready" ]]; then
      printf 'error: %s client bridge process did not publish readiness: %s\n' "$label" "$ready_result" >&2
      return 1
    fi
  }

  cleanup() {
    if [[ -n "${second_client_pid:-}" ]] && kill -0 "$second_client_pid" >/dev/null 2>&1; then
      kill "$second_client_pid" >/dev/null 2>&1 || true
      wait "$second_client_pid" >/dev/null 2>&1 || true
    fi
    if [[ -n "${client_pid:-}" ]] && kill -0 "$client_pid" >/dev/null 2>&1; then
      kill "$client_pid" >/dev/null 2>&1 || true
      wait "$client_pid" >/dev/null 2>&1 || true
    fi
    if [[ -n "${server_pid:-}" ]] && kill -0 "$server_pid" >/dev/null 2>&1; then
      kill "$server_pid" >/dev/null 2>&1 || true
      wait "$server_pid" >/dev/null 2>&1 || true
    fi
  }
  trap cleanup EXIT

  : > "$run_dir/server.log"
  start_server "initial"
  if [[ -n "$AGENT_BRIDGE_URL" || -n "$AGENT_SECRET" ]]; then
    if [[ -z "$AGENT_BRIDGE_URL" || -z "$AGENT_SECRET" ]]; then
      printf 'error: agent-driver mode requires SOLARIS_REAL_CLIENT_AGENT_SECRET plus SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL or SOLARIS_REAL_CLIENT_AGENT_PORT\n' >&2
      exit 1
    fi
    require_file "$AGENT_DRIVER"
    if validate_second_client_config; then
      second_client_enabled=1
      printf 'second_client_enabled=1\n' >> "$run_dir/automation-driver.txt"
    else
      printf 'second_client_enabled=0\n' >> "$run_dir/automation-driver.txt"
    fi

    launch_gradle_runclient "primary" "$AGENT_PORT" "$AGENT_SECRET" "$run_dir/client.log" client_pid
    if ! wait_for_agent_bridge "primary" "$AGENT_BRIDGE_URL" "$AGENT_SECRET" "$CLIENT_TIMEOUT_SECONDS"; then
      driver_status=1
    else

    run_regression_scenario
    fi

    set +e
    second_client_status=0
    if [[ -n "${second_client_pid:-}" ]]; then
      if kill -0 "$second_client_pid" >/dev/null 2>&1; then
        kill "$second_client_pid" >/dev/null 2>&1 || true
        wait "$second_client_pid" >/dev/null 2>&1
        second_client_status="$?"
      else
        wait "$second_client_pid" >/dev/null 2>&1
        second_client_status="$?"
      fi
    fi
    if kill -0 "$client_pid" >/dev/null 2>&1; then
      kill "$client_pid" >/dev/null 2>&1 || true
      wait "$client_pid" >/dev/null 2>&1
      client_status="$?"
    else
      wait "$client_pid" >/dev/null 2>&1
      client_status="$?"
    fi
    set -e

    {
      printf 'client_exit_status=%s\n' "$client_status"
      printf 'client_timeout_seconds=%s\n' "$CLIENT_TIMEOUT_SECONDS"
      printf 'second_client_exit_status=%s\n' "$second_client_status"
      printf 'client_agent_driver_timeout_seconds=%s\n' "$driver_timeout_seconds"
      printf 'client_agent_driver_exit_status=%s\n' "$driver_status"
      printf 'server_restart_count=%s\n' "$server_restart_count"
    } >> "$run_dir/automation-driver.txt"

    printf 'run artifacts: %s\n' "$run_dir"
    validate_run_dir "$run_dir"
    printf 'validate with: python3 -m tools.harness run regression --validate-run %s\n' "$run_dir"
    exit "$driver_status"
  fi

  printf 'error: primary Gradle runClient adapter did not configure the in-client bridge\n' >&2
  exit 1
fi
