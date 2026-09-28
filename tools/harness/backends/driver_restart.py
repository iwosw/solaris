"""Two-client save/restart invariant snapshots and assertions."""

from __future__ import annotations

import json
import os
import re
from pathlib import Path
from typing import Any

if __package__:
    from .driver_client import utc_now
    from .driver_replay import CORE_REPLAY_IDENTIFIER, require_exact_fields, require_identifier, require_object
else:
    from driver_client import utc_now
    from driver_replay import CORE_REPLAY_IDENTIFIER, require_exact_fields, require_identifier, require_object


RESTART_INVARIANT_SCHEMA = "solaris.restart_invariants.v1"
RESTART_INVARIANT_FILE = "restart-invariants.json"
RESTART_INVARIANT_CATEGORIES = {"player", "inventory", "world", "container", "entity", "time"}
RESTART_INVARIANT_TYPES = {"string", "integer", "boolean", "record"}
RESTART_INVARIANT_MODES = {"stable", "transition"}
MAX_RESTART_INVARIANTS = 32
MAX_RESTART_SNAPSHOT_BYTES = 32 * 1024
MAX_RESTART_MARKER_BYTES = 4 * 1024
P45_RESTART_BEFORE_SCENARIO = "playable-45-two-client-shared-chest-save-restart-before"
P45_RESTART_AFTER_SCENARIO = "playable-45-two-client-shared-chest-save-restart-after"
P45_SHARED_CHEST_MARKER = "playable-31-shared-chest-marker.properties"




def require_restart_scalar(value: Any, label: str) -> None:
    if isinstance(value, bool):
        return
    if isinstance(value, int):
        return
    if isinstance(value, str) and value and len(value) <= 256:
        return
    raise ValueError(f"{label} must be a bounded string, integer, or boolean")


def validate_restart_typed_value(type_name: str, value: Any, label: str) -> None:
    if type_name == "string":
        if not isinstance(value, str) or not value or len(value) > 256:
            raise ValueError(f"{label} must be a non-empty bounded string")
        return
    if type_name == "integer":
        if isinstance(value, bool) or not isinstance(value, int):
            raise ValueError(f"{label} must be an integer")
        return
    if type_name == "boolean":
        if not isinstance(value, bool):
            raise ValueError(f"{label} must be a boolean")
        return
    if type_name == "record":
        require_object(value, label)
        if not value or len(value) > 16:
            raise ValueError(f"{label} must contain 1..16 fields")
        for key, field in value.items():
            if not isinstance(key, str) or not CORE_REPLAY_IDENTIFIER.fullmatch(key):
                raise ValueError(f"{label} has invalid field name {key!r}")
            require_restart_scalar(field, f"{label}.{key}")
        return
    raise ValueError(f"{label} has unsupported type {type_name!r}")


def validate_restart_invariant_snapshot(value: Any) -> dict[str, Any]:
    require_object(value, "restart invariant snapshot")
    require_exact_fields(
        value,
        {"schema", "producer_scenario", "created_at", "invariants"},
        "restart invariant snapshot",
    )
    if value["schema"] != RESTART_INVARIANT_SCHEMA:
        raise ValueError(f"unsupported restart invariant schema: {value['schema']!r}")
    require_identifier(value["producer_scenario"], "restart invariant producer scenario")
    if not isinstance(value["created_at"], str) or not value["created_at"]:
        raise ValueError("restart invariant created_at must be a non-empty string")
    invariants = value["invariants"]
    if not isinstance(invariants, list) or not invariants or len(invariants) > MAX_RESTART_INVARIANTS:
        raise ValueError(f"restart invariants must contain 1..{MAX_RESTART_INVARIANTS} entries")
    seen: set[str] = set()
    for index, invariant in enumerate(invariants):
        label = f"restart invariant {index}"
        require_object(invariant, label)
        require_exact_fields(
            invariant,
            {"id", "category", "type", "mode", "before", "expected_after"},
            label,
        )
        require_identifier(invariant["id"], f"{label} id")
        if invariant["id"] in seen:
            raise ValueError(f"duplicate restart invariant id {invariant['id']}")
        seen.add(invariant["id"])
        if invariant["category"] not in RESTART_INVARIANT_CATEGORIES:
            raise ValueError(f"{label} has unsupported category {invariant['category']!r}")
        if invariant["type"] not in RESTART_INVARIANT_TYPES:
            raise ValueError(f"{label} has unsupported type {invariant['type']!r}")
        if invariant["mode"] not in RESTART_INVARIANT_MODES:
            raise ValueError(f"{label} has unsupported mode {invariant['mode']!r}")
        validate_restart_typed_value(invariant["type"], invariant["before"], f"{label}.before")
        validate_restart_typed_value(
            invariant["type"], invariant["expected_after"], f"{label}.expected_after"
        )
    return value


def restart_invariant_path(run_dir: Path) -> Path:
    return run_dir / RESTART_INVARIANT_FILE


def write_restart_invariant_snapshot(run_dir: Path, snapshot: dict[str, Any]) -> None:
    validate_restart_invariant_snapshot(snapshot)
    encoded = (json.dumps(snapshot, indent=2, sort_keys=True) + "\n").encode("utf-8")
    if len(encoded) > MAX_RESTART_SNAPSHOT_BYTES:
        raise ValueError("restart invariant snapshot exceeds the bounded size limit")
    path = restart_invariant_path(run_dir)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_bytes(encoded)
    os.replace(temporary, path)


def load_restart_invariant_snapshot(run_dir: Path, expected_scenario: str) -> dict[str, Any]:
    path = restart_invariant_path(run_dir)
    if not path.is_file():
        raise RuntimeError(f"missing restart invariant snapshot: {path}")
    size = path.stat().st_size
    if size <= 0 or size > MAX_RESTART_SNAPSHOT_BYTES:
        raise RuntimeError(f"restart invariant snapshot has invalid size {size}: {path}")
    try:
        snapshot = json.loads(path.read_text(encoding="utf-8"))
        validate_restart_invariant_snapshot(snapshot)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        raise RuntimeError(f"invalid restart invariant snapshot {path}: {exc}") from exc
    if snapshot["producer_scenario"] != expected_scenario:
        raise RuntimeError(
            f"restart invariant snapshot producer {snapshot['producer_scenario']!r} "
            f"does not match {expected_scenario!r}"
        )
    return snapshot


def load_p45_shared_chest_marker(run_dir: Path) -> dict[str, Any]:
    path = run_dir / P45_SHARED_CHEST_MARKER
    if not path.is_file():
        raise RuntimeError(f"missing P45 shared chest marker: {path}")
    size = path.stat().st_size
    if size <= 0 or size > MAX_RESTART_MARKER_BYTES:
        raise RuntimeError(f"P45 shared chest marker has invalid size {size}: {path}")
    values: dict[str, str] = {}
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        if not raw_line:
            continue
        if "=" not in raw_line:
            raise RuntimeError(f"invalid P45 shared chest marker line: {raw_line!r}")
        key, value = raw_line.split("=", 1)
        if key in values:
            raise RuntimeError(f"duplicate P45 shared chest marker field: {key}")
        values[key] = value
    required = {"x", "y", "z", "face", "item", "count"}
    if set(values) != required:
        missing = sorted(required - set(values))
        extra = sorted(set(values) - required)
        raise RuntimeError(f"invalid P45 shared chest marker fields: missing={missing} extra={extra}")
    try:
        marker = {
            "x": int(values["x"]),
            "y": int(values["y"]),
            "z": int(values["z"]),
            "face": values["face"],
            "item": values["item"],
            "count": int(values["count"]),
        }
    except ValueError as exc:
        raise RuntimeError(f"invalid numeric P45 shared chest marker field: {path}") from exc
    if not marker["face"] or len(marker["face"]) > 32:
        raise RuntimeError("P45 shared chest marker face must be a bounded non-empty string")
    if not re.fullmatch(r"[a-z0-9_.-]+:[a-z0-9_./-]+", marker["item"]):
        raise RuntimeError(f"P45 shared chest marker has invalid item id {marker['item']!r}")
    if marker["count"] < 1 or marker["count"] > 64:
        raise RuntimeError(f"P45 shared chest marker count is outside 1..64: {marker['count']}")
    return marker


def require_play_dimension(state: dict[str, Any], actor: str) -> str:
    require_object(state, f"{actor} client state")
    if state.get("in_play") is not True:
        raise RuntimeError(f"{actor} client is not in Play while capturing restart invariants")
    dimension = state.get("dimension")
    if not isinstance(dimension, str) or not dimension:
        raise RuntimeError(f"{actor} client state is missing dimension")
    return dimension


def p45_restart_snapshot(
    primary_state: dict[str, Any], secondary_state: dict[str, Any], marker: dict[str, Any]
) -> dict[str, Any]:
    primary_dimension = require_play_dimension(primary_state, "primary")
    secondary_dimension = require_play_dimension(secondary_state, "secondary")
    stack = {"item": marker["item"], "count": marker["count"]}
    marker_record = {
        "x": marker["x"],
        "y": marker["y"],
        "z": marker["z"],
        "face": marker["face"],
        "item": marker["item"],
        "count": marker["count"],
    }
    snapshot = {
        "schema": RESTART_INVARIANT_SCHEMA,
        "producer_scenario": P45_RESTART_BEFORE_SCENARIO,
        "created_at": utc_now(),
        "invariants": [
            {
                "id": "player.primary.dimension",
                "category": "player",
                "type": "string",
                "mode": "stable",
                "before": primary_dimension,
                "expected_after": primary_dimension,
            },
            {
                "id": "player.secondary.dimension",
                "category": "player",
                "type": "string",
                "mode": "stable",
                "before": secondary_dimension,
                "expected_after": secondary_dimension,
            },
            {
                "id": "inventory.deposited_stack",
                "category": "inventory",
                "type": "record",
                "mode": "transition",
                "before": stack,
                "expected_after": {
                    "owner": "secondary",
                    "item": marker["item"],
                    "count": marker["count"],
                },
            },
            {
                "id": "world.shared_chest_marker",
                "category": "world",
                "type": "record",
                "mode": "stable",
                "before": marker_record,
                "expected_after": marker_record,
            },
            {
                "id": "container.shared_chest.slot0",
                "category": "container",
                "type": "record",
                "mode": "transition",
                "before": {"slot": 0, "item": marker["item"], "count": marker["count"]},
                "expected_after": {"slot": 0, "empty": True},
            },
            {
                "id": "entity.shared_chest.block",
                "category": "entity",
                "type": "string",
                "mode": "stable",
                "before": "minecraft:chest",
                "expected_after": "minecraft:chest",
            },
        ],
    }
    return validate_restart_invariant_snapshot(snapshot)


def restart_invariant_index(snapshot: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {invariant["id"]: invariant for invariant in snapshot["invariants"]}


def p45_marker_record(marker: dict[str, Any]) -> dict[str, Any]:
    return {
        "x": marker["x"],
        "y": marker["y"],
        "z": marker["z"],
        "face": marker["face"],
        "item": marker["item"],
        "count": marker["count"],
    }


def validate_p45_restart_preflight(
    snapshot: dict[str, Any], marker: dict[str, Any]
) -> dict[str, dict[str, Any]]:
    invariants = restart_invariant_index(snapshot)
    expected_ids = {
        "player.primary.dimension",
        "player.secondary.dimension",
        "inventory.deposited_stack",
        "world.shared_chest_marker",
        "container.shared_chest.slot0",
        "entity.shared_chest.block",
    }
    if set(invariants) != expected_ids:
        raise RuntimeError(
            "P45 restart invariant ids do not match the declared scenario contract: "
            f"missing={sorted(expected_ids - set(invariants))} extra={sorted(set(invariants) - expected_ids)}"
        )
    actual_marker = p45_marker_record(marker)
    expected_marker = invariants["world.shared_chest_marker"]["expected_after"]
    if actual_marker != expected_marker:
        raise RuntimeError(
            "restart invariant world.shared_chest_marker mismatch: "
            f"expected={expected_marker!r} actual={actual_marker!r}"
        )
    return invariants


def validate_p45_restart_after(
    snapshot: dict[str, Any],
    primary_state: dict[str, Any],
    secondary_state: dict[str, Any],
    marker: dict[str, Any],
    secondary_withdraw_result: str,
    primary_empty_result: str,
) -> list[dict[str, Any]]:
    invariants = validate_p45_restart_preflight(snapshot, marker)
    primary_dimension = require_play_dimension(primary_state, "primary")
    secondary_dimension = require_play_dimension(secondary_state, "secondary")
    actual_marker = p45_marker_record(marker)
    checks = [
        (
            "player.primary.dimension",
            primary_dimension,
            invariants["player.primary.dimension"]["expected_after"],
        ),
        (
            "player.secondary.dimension",
            secondary_dimension,
            invariants["player.secondary.dimension"]["expected_after"],
        ),
        (
            "world.shared_chest_marker",
            actual_marker,
            invariants["world.shared_chest_marker"]["expected_after"],
        ),
        (
            "entity.shared_chest.block",
            "minecraft:chest",
            invariants["entity.shared_chest.block"]["expected_after"],
        ),
    ]
    results: list[dict[str, Any]] = []
    for invariant_id, actual, expected in checks:
        if actual != expected:
            raise RuntimeError(
                f"restart invariant {invariant_id} mismatch: expected={expected!r} actual={actual!r}"
            )
        results.append({"id": invariant_id, "status": "passed", "actual": actual})

    deposited = invariants["inventory.deposited_stack"]
    expected_stack = deposited["before"]
    if expected_stack != {"item": marker["item"], "count": marker["count"]}:
        raise RuntimeError("restart inventory invariant no longer matches the shared chest marker")
    if secondary_withdraw_result != "passed":
        raise RuntimeError("restart inventory invariant failed: secondary withdraw did not pass")
    results.append(
        {
            "id": "inventory.deposited_stack",
            "status": "passed",
            "actual": deposited["expected_after"],
        }
    )

    container = invariants["container.shared_chest.slot0"]
    if container["before"] != {"slot": 0, "item": marker["item"], "count": marker["count"]}:
        raise RuntimeError("restart container invariant no longer matches the shared chest marker")
    if secondary_withdraw_result != "passed" or primary_empty_result != "passed":
        raise RuntimeError("restart container invariant failed: withdraw/empty observation did not pass")
    results.append(
        {
            "id": "container.shared_chest.slot0",
            "status": "passed",
            "actual": container["expected_after"],
        }
    )
    return results
