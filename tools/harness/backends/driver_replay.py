"""Core replay manifest validation, real-client execution, and provenance."""

from __future__ import annotations

import hashlib
import json
import os
import platform
import re
import subprocess
from pathlib import Path
from typing import Any

if __package__:
    from .driver_client import AgentClient, call_and_record, wait_for_existing_or_explicit_connection, wait_for_valid_png
else:
    from driver_client import AgentClient, call_and_record, wait_for_existing_or_explicit_connection, wait_for_valid_png


CORE_REPLAY_SCENARIO_SCHEMA = "solaris.core_replay.scenario.v1"
CORE_REPLAY_RESULT_SCHEMA = "solaris.core_replay.result.v1"
CORE_REPLAY_DRIVERS = {"solaris_protocol", "vanilla_oracle", "real_client"}
CORE_REPLAY_EVIDENCE_KINDS = {
    "unit",
    "harness",
    "oracle",
    "real_client",
    "performance",
    "soak",
}
CORE_REPLAY_IDENTIFIER = re.compile(r"[a-z0-9][a-z0-9._-]{0,127}\Z")
MAX_CORE_REPLAY_ACTIONS = 10_000
MAX_CORE_REPLAY_CHECKS = 128


def load_core_replay_manifest(path: Path, expected_scenario_id: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"failed to read core replay manifest {path}: {exc}") from exc
    require_object(value, "core replay manifest")
    require_exact_fields(
        value,
        {"schema", "id", "seed", "actions", "lanes", "expected_invariants"},
        "core replay manifest",
    )
    if value["schema"] != CORE_REPLAY_SCENARIO_SCHEMA:
        raise ValueError(f"unsupported core replay schema: {value['schema']!r}")
    require_identifier(value["id"], "core replay scenario id")
    if value["id"] != expected_scenario_id:
        raise ValueError(
            f"core replay manifest id {value['id']!r} does not match scenario {expected_scenario_id!r}"
        )
    require_bounded_int(value["seed"], "core replay seed", 0, (1 << 64) - 1)

    actions = value["actions"]
    if not isinstance(actions, list) or not actions:
        raise ValueError("core replay actions must be a non-empty array")
    if len(actions) > MAX_CORE_REPLAY_ACTIONS:
        raise ValueError("core replay has too many actions")
    for index, action in enumerate(actions):
        validate_core_replay_action(action, index)

    lanes = value["lanes"]
    if not isinstance(lanes, list) or not lanes:
        raise ValueError("core replay lanes must be a non-empty array")
    seen_drivers: set[str] = set()
    real_client_lane = None
    for index, lane in enumerate(lanes):
        require_object(lane, f"core replay lane {index}")
        require_exact_fields(lane, {"driver", "required_gates"}, f"core replay lane {index}")
        driver = lane["driver"]
        if driver not in CORE_REPLAY_DRIVERS:
            raise ValueError(f"core replay lane {index} has unsupported driver {driver!r}")
        if driver in seen_drivers:
            raise ValueError(f"duplicate core replay lane {driver}")
        seen_drivers.add(driver)
        gates = lane["required_gates"]
        if not isinstance(gates, list) or not gates or len(gates) > MAX_CORE_REPLAY_CHECKS:
            raise ValueError(f"core replay lane {driver} must have 1..{MAX_CORE_REPLAY_CHECKS} gates")
        seen_gate_ids: set[str] = set()
        for gate_index, gate in enumerate(gates):
            require_object(gate, f"core replay lane {driver} gate {gate_index}")
            require_exact_fields(
                gate,
                {"id", "evidence_kind"},
                f"core replay lane {driver} gate {gate_index}",
            )
            require_identifier(gate["id"], "core replay gate id")
            if gate["id"] in seen_gate_ids:
                raise ValueError(f"duplicate core replay gate id {gate['id']}")
            seen_gate_ids.add(gate["id"])
            if gate["evidence_kind"] not in CORE_REPLAY_EVIDENCE_KINDS:
                raise ValueError(
                    f"core replay gate {gate['id']} has unsupported evidence kind {gate['evidence_kind']!r}"
                )
        primary_evidence = {
            "solaris_protocol": "harness",
            "vanilla_oracle": "oracle",
            "real_client": "real_client",
        }[driver]
        if not any(gate["evidence_kind"] == primary_evidence for gate in gates):
            raise ValueError(
                f"core replay lane {driver} must require {primary_evidence} evidence"
            )
        if driver == "real_client":
            real_client_lane = lane
    if real_client_lane is None:
        raise ValueError("core replay manifest has no real_client lane")

    invariants = value["expected_invariants"]
    if not isinstance(invariants, list) or not invariants or len(invariants) > MAX_CORE_REPLAY_CHECKS:
        raise ValueError(
            f"core replay expected_invariants must contain 1..{MAX_CORE_REPLAY_CHECKS} entries"
        )
    seen_invariant_ids: set[str] = set()
    for index, invariant in enumerate(invariants):
        require_object(invariant, f"core replay invariant {index}")
        require_exact_fields(
            invariant,
            {"id", "description"},
            f"core replay invariant {index}",
        )
        require_identifier(invariant["id"], "core replay invariant id")
        if invariant["id"] in seen_invariant_ids:
            raise ValueError(f"duplicate core replay invariant id {invariant['id']}")
        seen_invariant_ids.add(invariant["id"])
        if not isinstance(invariant["description"], str) or not invariant["description"].strip():
            raise ValueError(f"core replay invariant {invariant['id']} description is empty")

    if any(action["type"] == "reconnect" for action in actions):
        raise ValueError("real-client replay adapter does not support reconnect actions")
    return value


def validate_core_replay_action(action: Any, index: int) -> None:
    require_object(action, f"core replay action {index}")
    action_type = action.get("type")
    if action_type == "wait_ticks":
        require_exact_fields(action, {"type", "ticks"}, f"core replay action {index}")
        require_bounded_int(action["ticks"], "wait_ticks ticks", 1, 255)
    elif action_type == "move_by":
        require_exact_fields(action, {"type", "dx_cm", "dz_cm"}, f"core replay action {index}")
        require_bounded_int(action["dx_cm"], "move_by dx_cm", -32768, 32767)
        require_bounded_int(action["dz_cm"], "move_by dz_cm", -32768, 32767)
    elif action_type == "look":
        require_exact_fields(
            action,
            {"type", "yaw_deg", "pitch_deg"},
            f"core replay action {index}",
        )
        require_bounded_int(action["yaw_deg"], "look yaw_deg", -180, 180)
        require_bounded_int(action["pitch_deg"], "look pitch_deg", -90, 90)
    elif action_type == "reconnect":
        require_exact_fields(action, {"type"}, f"core replay action {index}")
    else:
        raise ValueError(f"core replay action {index} has unsupported type {action_type!r}")


def require_object(value: Any, label: str) -> None:
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be an object")


def require_exact_fields(value: dict[str, Any], expected: set[str], label: str) -> None:
    actual = set(value)
    if actual != expected:
        missing = sorted(expected - actual)
        unknown = sorted(actual - expected)
        raise ValueError(f"{label} fields mismatch: missing={missing} unknown={unknown}")


def require_identifier(value: Any, label: str) -> None:
    if not isinstance(value, str) or CORE_REPLAY_IDENTIFIER.fullmatch(value) is None:
        raise ValueError(f"{label} is not a bounded lowercase identifier: {value!r}")


def require_bounded_int(value: Any, label: str, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= maximum:
        raise ValueError(f"{label} must be an integer in [{minimum}, {maximum}]")
    return value


def core_action_summary(action: dict[str, Any]) -> str:
    action_type = action["type"]
    if action_type == "wait_ticks":
        return f"wait:{action['ticks']}"
    if action_type == "move_by":
        return f"move:{action['dx_cm']},{action['dz_cm']}"
    if action_type == "look":
        return f"look:{action['yaw_deg']},{action['pitch_deg']}"
    return "reconnect"


def repo_root() -> Path:
    return Path(__file__).resolve().parents[3]


def core_replay_provenance(run_dir: Path) -> dict[str, Any]:
    config_path = run_dir / "server.toml"
    if not config_path.is_file():
        raise ValueError("core replay requires the runner-captured server.toml")
    commit = subprocess.run(
        ["git", "-C", str(repo_root()), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if re.fullmatch(r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", commit) is None:
        raise ValueError("git rev-parse did not return a full object id")

    properties_path = Path(
        os.environ.get("SOLARIS_LOADER_ROOT", repo_root().parent / "solaris-loader")
    ).expanduser().resolve() / "gradle.properties"
    client_version = ""
    for line in properties_path.read_text(encoding="utf-8").splitlines():
        if line.startswith("minecraftVersion="):
            client_version = line.split("=", 1)[1].strip()
            break
    if not client_version:
        raise ValueError("client Gradle properties do not record minecraftVersion")

    logical_cpus = os.cpu_count() or 1
    memory_mib = physical_memory_mib()
    return {
        "git_commit": commit,
        "config_sha256": hashlib.sha256(config_path.read_bytes()).hexdigest(),
        "build_profile": "debug",
        "sidecar_version": client_version,
        "hardware": {
            "os": platform.system().lower() or "unknown",
            "arch": platform.machine() or "unknown",
            "cpu_model": cpu_model(),
            "logical_cpus": logical_cpus,
            "memory_mib": memory_mib,
        },
    }


def physical_memory_mib() -> int:
    try:
        pages = os.sysconf("SC_PHYS_PAGES")
        page_size = os.sysconf("SC_PAGE_SIZE")
        memory_mib = int(pages) * int(page_size) // (1024 * 1024)
        if memory_mib > 0:
            return memory_mib
    except (OSError, ValueError):
        pass
    return 1


def cpu_model() -> str:
    try:
        for line in Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines():
            if line.lower().startswith("model name") and ":" in line:
                model = line.split(":", 1)[1].strip()
                if model:
                    return model
    except (OSError, UnicodeDecodeError):
        pass
    return platform.processor() or "unknown"


def run_core_replay_scenario(
    client: AgentClient,
    run_dir: Path,
    manifest: dict[str, Any],
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    scenario_id = manifest["id"]
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(client, transcript, "ping", {}, timeout_seconds)
    wait_for_existing_or_explicit_connection(
        client,
        transcript,
        server_addr,
        timeout_seconds,
    )
    initial_state = call_and_record(client, transcript, "state", {}, timeout_seconds)
    require_replay_play_state(initial_state, "initial")

    action_observations: list[str] = []
    final_state = initial_state
    for index, action in enumerate(manifest["actions"]):
        action_type = action["type"]
        if action_type == "wait_ticks":
            command = "wait_ticks"
            payload = {"ticks": action["ticks"]}
        elif action_type == "move_by":
            command = "move_by"
            payload = {"dx_cm": action["dx_cm"], "dz_cm": action["dz_cm"]}
        elif action_type == "look":
            command = "look"
            payload = {"yaw_deg": action["yaw_deg"], "pitch_deg": action["pitch_deg"]}
        else:
            raise ValueError(f"unsupported real-client replay action: {action_type}")
        call_and_record(client, transcript, command, payload, timeout_seconds)
        final_state = call_and_record(client, transcript, "state", {}, timeout_seconds)
        require_replay_play_state(final_state, f"post-action {index}")
        action_observations.append(f"action.{index}={core_action_summary(action)}")

    expected_dx_cm = sum(
        action.get("dx_cm", 0) for action in manifest["actions"] if action["type"] == "move_by"
    )
    expected_dz_cm = sum(
        action.get("dz_cm", 0) for action in manifest["actions"] if action["type"] == "move_by"
    )
    actual_dx_cm = (state_coordinate(final_state, "x") - state_coordinate(initial_state, "x")) * 100.0
    actual_dz_cm = (state_coordinate(final_state, "z") - state_coordinate(initial_state, "z")) * 100.0
    movement_tolerance_cm = 5.0
    if (
        abs(actual_dx_cm - expected_dx_cm) > movement_tolerance_cm
        or abs(actual_dz_cm - expected_dz_cm) > movement_tolerance_cm
    ):
        raise RuntimeError(
            "real-client replay movement did not converge: "
            f"expected=({expected_dx_cm},{expected_dz_cm})cm "
            f"actual=({actual_dx_cm:.1f},{actual_dz_cm:.1f})cm"
        )

    screenshot_path = screenshots_dir / f"{scenario_id}.png"
    call_and_record(
        client,
        transcript,
        "screenshot",
        {"path": str(screenshot_path)},
        timeout_seconds,
    )
    png_error = wait_for_valid_png(screenshot_path, min(timeout_seconds, 5.0))
    if png_error is not None:
        raise RuntimeError(f"screenshot command wrote invalid PNG {screenshot_path}: {png_error}")
    screenshot_relative = screenshot_path.relative_to(run_dir).as_posix()

    scenario_report = {
        "result": "passed",
        "id": scenario_id,
        "observations": action_observations
        + [
            f"actions_executed={len(manifest['actions'])}",
            "post_action_liveness=client_play_state",
            (
                "movement_delta_cm="
                f"{round(actual_dx_cm)},{round(actual_dz_cm)}"
            ),
        ],
    }
    write_core_replay_result(
        run_dir,
        manifest,
        final_state,
        screenshot_relative,
    )
    call_and_record(client, transcript, "disconnect", {}, timeout_seconds)
    return "passed", final_state, [screenshot_relative], scenario_report


def require_replay_play_state(state: Any, label: str) -> None:
    require_object(state, f"{label} client state")
    if state.get("in_play") is not True:
        raise RuntimeError(f"{label} client state is not in Play")
    dimension = state.get("dimension")
    if not isinstance(dimension, str) or not dimension:
        raise RuntimeError(f"{label} client state has no dimension")
    state_coordinate(state, "x")
    state_coordinate(state, "y")
    state_coordinate(state, "z")


def state_coordinate(state: dict[str, Any], key: str) -> float:
    value = state.get(key)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise RuntimeError(f"client state {key} is not numeric")
    return float(value)


def write_core_replay_result(
    run_dir: Path,
    manifest: dict[str, Any],
    final_state: dict[str, Any],
    screenshot_relative: str,
) -> None:
    client_log = run_dir / "client.log"
    if not client_log.is_file():
        raise ValueError("core replay requires the runner-captured client.log")
    lane = next(lane for lane in manifest["lanes"] if lane["driver"] == "real_client")
    gates = []
    for requirement in lane["required_gates"]:
        if requirement["evidence_kind"] == "real_client":
            gates.append({
                "id": requirement["id"],
                "evidence_kind": requirement["evidence_kind"],
                "status": "passed",
                "artifacts": ["client.log", screenshot_relative],
            })
        else:
            gates.append({
                "id": requirement["id"],
                "evidence_kind": requirement["evidence_kind"],
                "status": "skipped",
                "reason": "the real-client adapter did not execute this evidence kind",
                "artifacts": [],
            })

    invariants = []
    for expected in manifest["expected_invariants"]:
        invariant_id = expected["id"]
        if invariant_id == "post-action-liveness":
            invariants.append({"id": invariant_id, "status": "passed"})
        elif invariant_id == "deterministic-normalized-state":
            invariants.append({
                "id": invariant_id,
                "status": "degraded",
                "reason": "one real-client phase does not establish repeated-run determinism",
            })
        else:
            invariants.append({
                "id": invariant_id,
                "status": "skipped",
                "reason": "the real-client adapter has no evaluator for this invariant",
            })

    statuses = [gate["status"] for gate in gates] + [
        invariant["status"] for invariant in invariants
    ]
    if "failed" in statuses:
        outcome = "failed"
    elif "blocked" in statuses:
        outcome = "blocked"
    elif any(status in {"degraded", "skipped"} for status in statuses):
        outcome = "degraded"
    else:
        outcome = "passed"

    notes = [
        {"type": "note", "key": f"action.{index}", "value": core_action_summary(action)}
        for index, action in enumerate(manifest["actions"])
    ]
    notes.extend([
        {
            "type": "note",
            "key": "actions_executed",
            "value": str(len(manifest["actions"])),
        },
        {
            "type": "note",
            "key": "final.dimension",
            "value": final_state["dimension"],
        },
        {
            "type": "note",
            "key": "final.position_cm",
            "value": (
                f"{round(state_coordinate(final_state, 'x') * 100)},"
                f"{round(state_coordinate(final_state, 'y') * 100)},"
                f"{round(state_coordinate(final_state, 'z') * 100)}"
            ),
        },
        {
            "type": "note",
            "key": "post_action_liveness",
            "value": "client_play_state",
        },
    ])
    notes.sort(key=lambda fact: (fact["key"], fact["value"]))

    result = {
        "schema": CORE_REPLAY_RESULT_SCHEMA,
        "scenario_id": manifest["id"],
        "seed": manifest["seed"],
        "driver": "real_client",
        "outcome": outcome,
        "actions": manifest["actions"],
        "provenance": core_replay_provenance(run_dir),
        "gates": gates,
        "invariants": invariants,
        "observations": [{
            "subject": "real_client",
            "phase": manifest["id"],
            "facts": notes,
        }],
    }
    (run_dir / "core-replay-result.json").write_text(
        json.dumps(result, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
