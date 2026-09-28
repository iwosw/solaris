# Fail-closed artifact, automation-driver, and server-log validation sourced by regression.sh.

validate_run_dir() {
  local run_dir observations core_replay_source
  run_dir="$1"
  require_dir "$run_dir"
  for required in manifest.json client.log server.log observations.json screenshots git.txt toolchain.txt automation-driver.txt; do
    if [[ "$required" == screenshots ]]; then
      require_dir "$run_dir/$required"
    else
      require_file "$run_dir/$required"
    fi
  done
  core_replay_source="$(manifest_core_replay_source "$run_dir/manifest.json")"
  if [[ -n "$core_replay_source" ]]; then
    require_file "$run_dir/core-replay-manifest.json"
    require_file "$run_dir/core-replay-result.json"
    (
      cd "$REPO_ROOT"
      cargo run --quiet -p mc-test-harness --bin core-replay-validate -- \
        "$run_dir/core-replay-manifest.json" \
        "$run_dir/core-replay-result.json"
    )
  fi
  observations="$(tr -d '\n[:space:]' < "$run_dir/observations.json")"
  if [[ "$observations" != *'"client_gate":"agent-run-real-client"'* ]]; then
    printf 'error: observations.json client_gate must be agent-run-real-client\n' >&2
    exit 1
  fi
  if [[ "$observations" == *'"result":"not-run"'* || "$observations" == *'"result":"prepared"'* ]]; then
    printf 'error: observations.json is still a prepared/not-run template\n' >&2
    exit 1
  fi
  validate_automation_driver "$run_dir/automation-driver.txt"
  validate_automation_driver_for_observations "$run_dir/automation-driver.txt" "$observations" "$run_dir/observations.json" "$run_dir/manifest.json"
  validate_server_log "$run_dir/server.log" "$observations"
  python3 - "$run_dir" <<'PY'
import binascii
import json
import sys
from pathlib import Path

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
EXPECTED_OBSERVATIONS_SCHEMA = "solaris.real_client_observations.v1"
MIN_SCREENSHOT_WIDTH = 320
MIN_SCREENSHOT_HEIGHT = 180


def png_validation_error(path):
    try:
        data = path.read_bytes()
    except OSError as exc:
        return f"could not read file: {exc}"

    if not data.startswith(PNG_SIGNATURE):
        return "missing PNG signature"
    offset = len(PNG_SIGNATURE)
    saw_ihdr = False
    while True:
        if offset + 12 > len(data):
            return "truncated PNG chunk header"
        length = int.from_bytes(data[offset : offset + 4], "big")
        chunk_type = data[offset + 4 : offset + 8]
        chunk_start = offset + 8
        chunk_end = chunk_start + length
        crc_end = chunk_end + 4
        if crc_end > len(data):
            return f"truncated {chunk_type.decode('ascii', errors='replace')} chunk"
        expected_crc = int.from_bytes(data[chunk_end:crc_end], "big")
        actual_crc = binascii.crc32(chunk_type + data[chunk_start:chunk_end]) & 0xFFFFFFFF
        if actual_crc != expected_crc:
            return f"{chunk_type.decode('ascii', errors='replace')} CRC mismatch"
        if not saw_ihdr:
            if chunk_type != b"IHDR":
                return "first PNG chunk is not IHDR"
            if length != 13:
                return "IHDR chunk has invalid length"
            width = int.from_bytes(data[chunk_start : chunk_start + 4], "big")
            height = int.from_bytes(data[chunk_start + 4 : chunk_start + 8], "big")
            if width < MIN_SCREENSHOT_WIDTH or height < MIN_SCREENSHOT_HEIGHT:
                return (
                    f"screenshot size {width}x{height} is smaller than "
                    f"{MIN_SCREENSHOT_WIDTH}x{MIN_SCREENSHOT_HEIGHT}"
                )
            saw_ihdr = True
        if chunk_type == b"IEND":
            if length != 0:
                return "IEND chunk has invalid length"
            if crc_end != len(data):
                return "trailing bytes after IEND"
            return None
        offset = crc_end


run_dir = Path(sys.argv[1])
manifest = json.loads((run_dir / "manifest.json").read_text())
observations = json.loads((run_dir / "observations.json").read_text())
if not isinstance(observations, dict):
    print("error: observations.json must be a JSON object", file=sys.stderr)
    sys.exit(1)
if observations.get("schema") != EXPECTED_OBSERVATIONS_SCHEMA:
    print(
        f"error: observations.json schema must be {EXPECTED_OBSERVATIONS_SCHEMA}",
        file=sys.stderr,
    )
    sys.exit(1)
manifest_quality_label = manifest.get("quality_label")
if not isinstance(manifest_quality_label, str) or not manifest_quality_label:
    print(
        "error: manifest.json quality_label must be a non-empty string",
        file=sys.stderr,
    )
    sys.exit(1)
if observations.get("quality_label") != manifest_quality_label:
    print(
        f"error: observations.json quality_label must match manifest quality_label {manifest_quality_label}",
        file=sys.stderr,
    )
    sys.exit(1)
if observations.get("result") != "passed":
    print(
        f"error: observations.json result must be passed, got {observations.get('result')!r}",
        file=sys.stderr,
    )
    sys.exit(1)
observed_scenarios = observations.get("scenarios")
if not isinstance(observed_scenarios, list) or not observed_scenarios:
    print(
        "error: observations.json scenarios must contain at least one passed scenario",
        file=sys.stderr,
    )
    sys.exit(1)
screenshots_dir = run_dir / "screenshots"
screenshots_root = screenshots_dir.resolve()

required_screenshots = {
    scenario.get("id")
    for scenario in manifest.get("scenarios", [])
    if scenario.get("screenshots_required") is True
}
manifest_scenarios = {
    scenario.get("id")
    for scenario in manifest.get("scenarios", [])
    if isinstance(scenario.get("id"), str)
}
for scenario in observed_scenarios:
    scenario_id = scenario.get("id")
    if not isinstance(scenario_id, str) or scenario_id not in manifest_scenarios:
        print(
            f"error: observations.json contains unknown scenario id: {scenario_id!r}",
            file=sys.stderr,
        )
        sys.exit(1)
    if scenario.get("result") != "passed":
        print(
            f"error: scenario {scenario_id} result must be passed, got {scenario.get('result')!r}",
            file=sys.stderr,
        )
        sys.exit(1)
    screenshots = scenario.get("screenshots")
    if scenario_id in required_screenshots and (not isinstance(screenshots, list) or not screenshots):
        print(
            f"error: scenario {scenario_id} requires at least one screenshots/ artifact",
            file=sys.stderr,
        )
        sys.exit(1)
    if screenshots is None:
        continue
    if not isinstance(screenshots, list):
        print(
            f"error: scenario {scenario_id} screenshots must be a list",
            file=sys.stderr,
        )
        sys.exit(1)
    for screenshot in screenshots:
        if not isinstance(screenshot, str) or not screenshot.startswith("screenshots/"):
            print(
                f"error: scenario {scenario_id} screenshot must live under screenshots/: {screenshot!r}",
                file=sys.stderr,
            )
            sys.exit(1)
        screenshot_path = (run_dir / screenshot).resolve()
        try:
            screenshot_path.relative_to(screenshots_root)
        except ValueError:
            print(
                f"error: scenario {scenario_id} screenshot escapes screenshots/: {screenshot!r}",
                file=sys.stderr,
            )
            sys.exit(1)
        if not screenshot_path.is_file():
            print(
                f"error: scenario {scenario_id} screenshot is missing on disk: {screenshot}",
                file=sys.stderr,
            )
            sys.exit(1)
        png_error = png_validation_error(screenshot_path)
        if png_error is not None:
            print(
                f"error: scenario {scenario_id} screenshot is invalid PNG: {screenshot}: {png_error}",
                file=sys.stderr,
            )
            sys.exit(1)
PY
  printf 'validated %s\n' "$run_dir"
}


validate_automation_driver() {
  local automation_driver legacy_matches legacy_primary_env legacy_m94_env bad_status_matches bad_bridge_matches phase_status_count
  automation_driver="$1"
  legacy_primary_env="$(printf '%s_%s_%s_%s' SOLARIS REAL CLIENT COMMAND)"
  legacy_m94_env="$(printf '%s_%s_%s' M94 CLIENT COMMAND)"

  if ! grep -Fxq 'client_kind=gradle-runclient' "$automation_driver"; then
    printf 'error: automation-driver.txt must record client_kind=gradle-runclient\n' >&2
    exit 1
  fi
  if ! grep -Fxq 'client_adapter_source=auto-gradle-runclient' "$automation_driver"; then
    printf 'error: automation-driver.txt must record client_adapter_source=auto-gradle-runclient\n' >&2
    exit 1
  fi
  if ! grep -Fxq 'client_adapter_task=:fabric-agent:runClientAgent' "$automation_driver"; then
    printf 'error: automation-driver.txt must record client_adapter_task=:fabric-agent:runClientAgent\n' >&2
    exit 1
  fi
  if grep -Eq '^client_agent_jar(_injected)?(_(primary|secondary))?=' "$automation_driver"; then
    printf 'error: automation-driver.txt must not claim an irrelevant java-agent jar\n' >&2
    exit 1
  fi

  validate_runtime_provenance() {
    local label
    label="$1"
    if ! grep -Fxq "client_agent_runtime_kind_${label}=compiled-classes" "$automation_driver"; then
      printf 'error: automation-driver.txt must record %s runtime classes provenance\n' "$label" >&2
      exit 1
    fi
    if ! grep -Eq "^client_agent_runtime_path_${label}=.+/fabric-agent/build/classes/java/main$" "$automation_driver"; then
      printf 'error: automation-driver.txt must record the %s Gradle runtime classes path\n' "$label" >&2
      exit 1
    fi
    if ! grep -Eq "^client_agent_runtime_sha256_${label}=[0-9a-f]{64}$" "$automation_driver"; then
      printf 'error: automation-driver.txt must record the %s runtime classes digest\n' "$label" >&2
      exit 1
    fi
    if ! grep -Eq "^client_agent_runtime_file_count_${label}=[1-9][0-9]*$" "$automation_driver"; then
      printf 'error: automation-driver.txt must record the %s runtime classes file count\n' "$label" >&2
      exit 1
    fi
    if ! grep -Fxq "client_agent_runtime_validation_${label}=verified" "$automation_driver"; then
      printf 'error: automation-driver.txt must record verified %s runtime classes provenance\n' "$label" >&2
      exit 1
    fi
  }
  validate_runtime_provenance primary

  legacy_matches="$(
    grep -En \
      "^(client_command|second_client_command)=|${legacy_primary_env}|${legacy_m94_env}|client_adapter_source=legacy-command|client_kind=(prismlauncher|command)" \
      "$automation_driver" || true
  )"
  if [[ -n "$legacy_matches" ]]; then
    printf 'error: automation-driver.txt contains legacy client-command adapter metadata\n' >&2
    sed -n '1,20p' <<< "$legacy_matches" >&2
    exit 1
  fi

  if ! grep -Fxq 'client_agent_driver_exit_status=0' "$automation_driver"; then
    printf 'error: automation-driver.txt must record client_agent_driver_exit_status=0\n' >&2
    exit 1
  fi
  phase_status_count="$(grep -Ec '^client_agent_phase_exit_status_[^=]+=0$' "$automation_driver" || true)"
  if [[ "$phase_status_count" -lt 1 ]]; then
    printf 'error: automation-driver.txt must record at least one client-agent phase exit status 0\n' >&2
    exit 1
  fi
  bad_status_matches="$(
    awk -F= '/^client_agent_.*exit_status/ && $2 != "0" { print NR ":" $0 }' "$automation_driver" || true
  )"
  if [[ -n "$bad_status_matches" ]]; then
    printf 'error: automation-driver.txt contains nonzero client-agent exit status\n' >&2
    sed -n '1,20p' <<< "$bad_status_matches" >&2
    exit 1
  fi
  if ! grep -Fxq 'client_agent_bridge_wait_status_primary=ready' "$automation_driver"; then
    printf 'error: automation-driver.txt must record primary client-agent bridge wait ready\n' >&2
    exit 1
  fi
  bad_bridge_matches="$(
    awk -F= '/^client_agent_bridge_wait_status_/ && $2 != "ready" { print NR ":" $0 }' "$automation_driver" || true
  )"
  if [[ -n "$bad_bridge_matches" ]]; then
    printf 'error: automation-driver.txt contains non-ready client-agent bridge wait status\n' >&2
    sed -n '1,20p' <<< "$bad_bridge_matches" >&2
    exit 1
  fi
  if grep -Fxq 'second_client_enabled=1' "$automation_driver"; then
    if ! grep -Fxq 'second_client_kind=gradle-runclient' "$automation_driver"; then
      printf 'error: automation-driver.txt must record second_client_kind=gradle-runclient when second_client_enabled=1\n' >&2
      exit 1
    fi
    if ! grep -Fxq 'second_client_adapter_source=auto-gradle-runclient' "$automation_driver"; then
      printf 'error: automation-driver.txt must record second_client_adapter_source=auto-gradle-runclient when second_client_enabled=1\n' >&2
      exit 1
    fi
    if ! grep -Fxq 'second_client_adapter_task=:fabric-agent:runClientAgent' "$automation_driver"; then
      printf 'error: automation-driver.txt must record second_client_adapter_task=:fabric-agent:runClientAgent when second_client_enabled=1\n' >&2
      exit 1
    fi
    if ! grep -Fxq 'second_client_agent_secret=SET_REDACTED' "$automation_driver"; then
      printf 'error: automation-driver.txt must record second_client_agent_secret=SET_REDACTED when second_client_enabled=1\n' >&2
      exit 1
    fi
    if ! grep -Fxq 'client_agent_bridge_wait_status_secondary=ready' "$automation_driver"; then
      printf 'error: automation-driver.txt must record secondary client-agent bridge wait ready when second_client_enabled=1\n' >&2
      exit 1
    fi
    validate_runtime_provenance secondary
  fi
}


validate_automation_driver_for_observations() {
  local automation_driver observations observations_file manifest_file restart_observation two_client_observation
  automation_driver="$1"
  observations="$2"
  observations_file="$3"
  manifest_file="$4"
  restart_observation=0
  two_client_observation=0

  if [[ "$observations" == *'"id":"m94-06-save-restart-after"'* \
    || "$observations" == *'"id":"playable-03-save-restart-after"'* \
    || "$observations" == *'"id":"playable-06-stone-tool-save-restart-after"'* \
    || "$observations" == *'"id":"playable-13-chest-storage-save-restart-after"'* \
    || "$observations" == *'"id":"playable-25-iron-sword-save-restart-after"'* \
    || "$observations" == *'"id":"playable-29-iron-chestplate-save-restart-mitigation-after"'* \
    || "$observations" == *'"id":"playable-45-two-client-shared-chest-save-restart-after"'* \
    || "$observations" == *'"id":"playable-46-generated-ruin-cache-after"'* ]]; then
    restart_observation=1
  fi

  if [[ "$observations" == *'"id":"playable-30-two-client-'* \
    || "$observations" == *'"id":"playable-31-two-client-'* \
    || "$observations" == *'"id":"playable-32-two-client-'* \
    || "$observations" == *'"id":"playable-33-two-client-'* \
    || "$observations" == *'"id":"playable-34-two-client-'* \
    || "$observations" == *'"id":"playable-35-two-client-'* \
    || "$observations" == *'"id":"playable-36-two-client-'* \
    || "$observations" == *'"id":"playable-37-two-client-'* \
    || "$observations" == *'"id":"playable-38-two-client-'* \
    || "$observations" == *'"id":"playable-39-two-client-'* \
    || "$observations" == *'"id":"playable-40-two-client-'* \
    || "$observations" == *'"id":"playable-41-two-client-'* \
    || "$observations" == *'"id":"playable-42-two-client-'* \
    || "$observations" == *'"id":"playable-45-two-client-'* ]]; then
    two_client_observation=1
  fi

  if [[ "$two_client_observation" -eq 1 ]] && ! grep -Fxq 'second_client_enabled=1' "$automation_driver"; then
    printf 'error: automation-driver.txt must record second_client_enabled=1 for two-client observations\n' >&2
    exit 1
  fi
  if [[ "$restart_observation" -eq 1 ]]; then
    if ! grep -Eq '^server_restart_count=[1-9][0-9]*$' "$automation_driver"; then
      printf 'error: automation-driver.txt must record server_restart_count>=1 for after-restart observations\n' >&2
      exit 1
    fi
    if ! grep -Eq '^server_stop_phase=.* signal=INT$' "$automation_driver"; then
      printf 'error: automation-driver.txt must record a graceful server_stop_phase for after-restart observations\n' >&2
      exit 1
    fi
    if ! grep -Eq '^server_exit_phase=.* status=0$' "$automation_driver"; then
      printf 'error: automation-driver.txt must record server_exit_phase with status=0 for after-restart observations\n' >&2
      exit 1
    fi
    if ! grep -Eq '^server_start_phase=.*after' "$automation_driver"; then
      printf 'error: automation-driver.txt must record an after-restart server_start_phase for after-restart observations\n' >&2
      exit 1
    fi
  fi
  if [[ "$observations" == *'"id":"playable-46-generated-ruin-cache-'* ]] \
    && ! grep -Eq '^server_world_dir=.+/world$' "$automation_driver"; then
    printf 'error: playable-46 observations require a runner-created isolated server_world_dir\n' >&2
    exit 1
  fi
  if [[ "$observations" == *'"id":"playable-46-generated-ruin-cache-'* ]] \
    && ! grep -Fxq 'server_op_users=NONE' "$automation_driver"; then
    printf 'error: playable-46 observations require server_op_users=NONE\n' >&2
    exit 1
  fi
  python3 - "$automation_driver" "$observations_file" "$manifest_file" <<'PY'
import json
import sys
from pathlib import Path

automation_driver = Path(sys.argv[1])
observations_path = Path(sys.argv[2])
manifest_path = Path(sys.argv[3])
phase_status_prefix = "client_agent_phase_exit_status_"
passed_phases = set()
for line in automation_driver.read_text().splitlines():
    if not line.startswith(phase_status_prefix) or not line.endswith("=0"):
        continue
    passed_phases.add(line[len(phase_status_prefix) : -2])

manifest = json.loads(manifest_path.read_text())
manifest_scenarios = {
    scenario.get("id")
    for scenario in manifest.get("scenarios", [])
    if isinstance(scenario, dict) and isinstance(scenario.get("id"), str)
}
observations = json.loads(observations_path.read_text())
passed_observed_scenarios = set()
for scenario in observations.get("scenarios", []):
    if not isinstance(scenario, dict) or scenario.get("result") != "passed":
        continue
    scenario_id = scenario.get("id")
    if not isinstance(scenario_id, str):
        continue
    if scenario_id not in manifest_scenarios:
        continue
    passed_observed_scenarios.add(scenario_id)
    if scenario_id not in passed_phases:
        print(
            f"error: automation-driver.txt must record client_agent_phase_exit_status_{scenario_id}=0 for passed observations",
            file=sys.stderr,
        )
        sys.exit(1)
restart_prerequisites = {
    "m94-06-save-restart-after": {"m94-06-save-restart-before"},
    "playable-03-save-restart-after": {
        "playable-03-save-restart-before",
        "playable-03-save-restart-rejoin",
        "playable-04-twenty-minute-survival-loop",
    },
    "playable-06-stone-tool-save-restart-after": {
        "playable-06-stone-tool-save-restart-before",
    },
    "playable-13-chest-storage-save-restart-after": {
        "playable-13-chest-storage-save-restart-before",
    },
    "playable-25-iron-sword-save-restart-after": {
        "playable-25-iron-sword-save-restart-before",
    },
    "playable-29-iron-chestplate-save-restart-mitigation-after": {
        "playable-29-iron-chestplate-save-restart-mitigation-before",
    },
    "playable-45-two-client-shared-chest-save-restart-after": {
        "playable-45-two-client-shared-chest-save-restart-before",
    },
    "playable-46-generated-ruin-cache-after": {
        "playable-46-generated-ruin-cache-before",
    },
}
for after_scenario, required_before_scenarios in restart_prerequisites.items():
    if after_scenario not in passed_observed_scenarios:
        continue
    if passed_observed_scenarios.isdisjoint(required_before_scenarios):
        required = ", ".join(sorted(required_before_scenarios))
        print(
            f"error: observations.json must include a passed before-restart scenario for {after_scenario}: one of {required}",
            file=sys.stderr,
        )
        sys.exit(1)
PY
  python3 "$REPO_ROOT/tools/harness/backends/restart_evidence.py" \
    "$automation_driver" "$observations_file" "$manifest_file"
}


validate_server_log() {
  local server_log observations matches catastrophic_tick_matches slow_matches
  server_log="$1"
  observations="$2"
  matches="$(
    grep -En \
      'lock wait exceeded.*lock="chunk_prepare"|degraded_delivery=true|teleport confirmation id mismatch|WARN .*dirty chunk cache pressure|dirty pressure flush failed|region changed before replace|chunk preparation abandoned after repeated dirty chunk cache pressure' \
      "$server_log" || true
  )"
  if [[ -n "$matches" ]]; then
    printf 'error: server.log contains playable degradation warning(s)\n' >&2
    sed -n '1,20p' <<< "$matches" >&2
    exit 1
  fi

  catastrophic_tick_matches="$(python3 - "$server_log" <<'PY'
import re
import sys
from pathlib import Path

threshold_us = 500_000
for line_number, line in enumerate(Path(sys.argv[1]).read_text(encoding="utf-8").splitlines(), 1):
    if "runtime tick exceeded performance budget" not in line:
        continue
    match = re.search(r"(?:^|\s)tick_us=(\d+)(?:\s|$)", line)
    if match is None or int(match.group(1)) >= threshold_us:
        print(f"{line_number}:{line}")
PY
)"
  if [[ -n "$catastrophic_tick_matches" ]]; then
    printf 'error: server.log contains catastrophic runtime tick(s) at or above 500000 us\n' >&2
    sed -n '1,20p' <<< "$catastrophic_tick_matches" >&2
    exit 1
  fi

  if [[ "$observations" == *'"id":"playable-41-two-client-chunk-prewarm-crossing"'* || "$observations" == *'"id":"playable-42-two-client-opposite-chunk-crossing"'* ]]; then
    slow_matches="$(
      grep -En \
        'view-distance window flushed.*(slow_fetch_chunks=[1-9][0-9]*|slow_light_compute_chunks=[1-9][0-9]*)' \
        "$server_log" || true
    )"
    if [[ -n "$slow_matches" ]]; then
      printf 'error: server.log contains slow chunk window(s) for prewarm crossing scenario\n' >&2
      sed -n '1,20p' <<< "$slow_matches" >&2
      exit 1
    fi
  fi
}
