"""Generic received-world navigation proof; the canonical harness owns lifecycle.

Export unchanged as tools/harness/backends/native_navigation.py. Call only in a
fresh isolated operator-authorized world with the windowed native client. The
explicit first-party /fill and /setblock commands are PRIVILEGED fixtures, never
earned acquisition. Navigation itself uses only minecraft_navigate_to_block;
there is no teleport, direct world access, sleep, or readiness polling here.
"""
from __future__ import annotations

import math
import uuid
from pathlib import Path
from typing import Any

from . import native_client
from .probe_steps import ProbeSteps


TOOL = "minecraft_navigate_to_block"
SUCCESS_KEYS = {"arrived", "x", "y", "z", "player_x", "player_y", "player_z"}


def _xyz(position: tuple[int, int, int]) -> dict[str, int]:
    return dict(zip(("x", "y", "z"), position))


def _read(steps: ProbeSteps, position: tuple[int, int, int]) -> dict[str, Any]:
    return steps.call("minecraft_read_block", _xyz(position))


def _scan(steps: ProbeSteps, minimum: tuple[int, int, int],
          maximum: tuple[int, int, int]) -> list[dict[str, Any]]:
    volume = math.prod(b - a + 1 for a, b in zip(minimum, maximum))
    result = steps.call("minecraft_scan_blocks", {
        **dict(zip(("min_x", "min_y", "min_z"), minimum)),
        **dict(zip(("max_x", "max_y", "max_z"), maximum)), "max_blocks": volume,
    })
    rows = result["blocks"]
    steps.check("complete_received_scan", result["count"] == volume and len(rows) == volume)
    return rows


def _echo(steps: ProbeSteps, label: str) -> dict[str, Any]:
    marker = "navigation-fixture-" + uuid.uuid4().hex
    steps.call("minecraft_send_chat", {"message": marker, "command": False})
    return steps.wait(lambda s: any(isinstance(line, str) and marker in line
                                   for line in s["recent_chat"]), label)


def _fixture(steps: ProbeSteps, command: str, witness: tuple[int, int, int],
             block_id: str, label: str) -> None:
    steps.fixture(command)
    steps.call("minecraft_wait_for_block_state", {
        **_xyz(witness), "block_id": block_id, "timeout_seconds": steps.remaining(),
    })
    _echo(steps, label + "_ordered_server_echo")


def _released(steps: ProbeSteps, label: str, *, stationary: bool) -> dict[str, Any]:
    # Public pose and passive state-event waits never send input/release commands.
    # The canonical surface does not expose internal held-key state.
    before = steps.observe()
    after = steps.wait(lambda state: state["client_tick"] >= before["client_tick"] + 8,
                       label + "_tick_advanced")
    displacement = math.dist([before["player"][k] for k in ("x", "y", "z")],
                             [after["player"][k] for k in ("x", "y", "z")])
    steps.check(label + "_no_continued_movement", displacement <= (0.05 if stationary else 0.5))
    steps.check(label + "_settled_grounded", after["player"]["on_ground"] is True)
    return {"before": before, "after": after, "displacement": displacement,
            "evidence": "at least eight received client ticks using readonly observation/state-event waits, no input/release cleanup",
            "internal_held_inputs_observed": False}


def _clear_body(steps: ProbeSteps, state: dict[str, Any], label: str) -> list[dict[str, Any]]:
    p = state["player"]
    steps.check(label + "_grounded", p["on_ground"] is True)
    # The fixture uses only full stone cubes/air. Requiring every intersecting
    # received cell to be collision-empty is deliberately conservative.
    epsilon = 1e-6
    minimum = (math.floor(p["x"] - 0.3 + epsilon), math.floor(p["y"] + epsilon),
               math.floor(p["z"] - 0.3 + epsilon))
    maximum = (math.floor(p["x"] + 0.3 - epsilon), math.floor(p["y"] + 1.8 - epsilon),
               math.floor(p["z"] + 0.3 - epsilon))
    rows = _scan(steps, minimum, maximum)
    steps.check(label + "_received_body_collision_free",
                all(row["collision_empty"] is True and row["fluid"]["empty"] is True for row in rows))
    support = _read(steps, (math.floor(p["x"]), round(p["y"]) - 1, math.floor(p["z"])))
    steps.check(label + "_received_stone_support", support["block_id"] == "minecraft:stone"
                and support["collision_empty"] is False and abs(p["y"] - round(p["y"])) < 0.02)
    return rows + [support]


def _navigate(steps: ProbeSteps, artifact: Path, label: str,
              target: tuple[int, int, int], identity: tuple[Any, Any]) -> dict[str, Any]:
    before = steps.observe()
    p = before["player"]
    distance_before = math.hypot(p["x"] - target[0] - 0.5, p["z"] - target[2] - 0.5)
    steps.check(label + "_not_initially_arrived", distance_before > 2.0)
    before_frame = native_client.capture(steps.client, artifact, label + "-before")
    result = steps.call(TOOL, {**_xyz(target), "timeout_seconds": min(8.0, steps.remaining())})
    after = steps.observe()
    q = after["player"]
    steps.check(label + "_exact_success_response", set(result) == SUCCESS_KEYS
                and result["arrived"] is True and all(type(result[k]) is int and result[k] == value
                    for k, value in zip(("x", "y", "z"), target))
                and all(type(result["player_" + k]) in (int, float)
                        and math.isfinite(result["player_" + k]) for k in ("x", "y", "z")))
    for pose_label, pose in (("response", {k: result["player_" + k] for k in ("x", "y", "z")}),
                             ("observed", q)):
        steps.check(label + "_" + pose_label + "_arrival",
                    math.hypot(pose["x"] - target[0] - 0.5, pose["z"] - target[2] - 0.5) <= 1.5
                    and abs(pose["y"] - target[1]) <= 1.25)
    displacement = math.hypot(q["x"] - p["x"], q["z"] - p["z"])
    steps.check(label + "_actual_displacement", displacement > 1.0)
    steps.check(label + "_same_player_dimension", (q["entity_uuid"], after["dimension"]) == identity)
    steps.check(label + "_response_matches_observed_pose",
                all(abs(q[k] - result["player_" + k]) <= 0.15 for k in ("x", "y", "z")))
    collision = _clear_body(steps, after, label)
    release = _released(steps, label, stationary=False)
    after_frame = native_client.capture(steps.client, artifact, label + "-after")
    steps.check(label + "_fresh_completed_window_frames", int(after_frame["frame_id"]) > int(before_frame["frame_id"]))
    return {"target": _xyz(target), "before": before, "result": result, "after": after,
            "horizontal_displacement": displacement, "received_collision": collision,
            "release": release, "captures": {"before": before_frame, "after": after_frame}}


def _failure(steps: ProbeSteps, label: str, target: tuple[int, int, int],
             expected: str, *, seconds: float = 8.0, stationary: bool = True) -> dict[str, Any]:
    before = steps.observe()
    try:
        result = steps.call(TOOL, {**_xyz(target), "timeout_seconds": min(seconds, steps.remaining())})
    except RuntimeError as error:
        message = str(error)
        steps.check(label + "_deterministic_error", expected in message)
    else:
        steps.check(label + "_must_not_succeed", False)
        raise RuntimeError(f"unexpected navigation response: {result}")
    released = _released(steps, label, stationary=stationary)
    steps.check(label + "_same_player_dimension",
                before["player"]["entity_uuid"] == released["after"]["player"]["entity_uuid"]
                and before["dimension"] == released["after"]["dimension"])
    return {"target": _xyz(target), "timeout_seconds": seconds, "error": message,
            "before": before, "release": released}


def run_navigation(client: Any, artifact: Path, timeout: float) -> dict[str, Any]:
    """Prove navigation with the caller's sealed windowed instance; never launch it."""
    artifact = Path(artifact)
    with ProbeSteps(client, artifact / "navigation-proof", timeout, "native10-generic-navigation") as steps:
        steps.report.update(
            scope="generic ordinary navigation over received full-cube collision, not plugin routes",
            evidence_domain="windowed_native_received_world_and_reconciled_local_pose",
            pose_origin="locally_predicted_reconciled_by_server; server_position is correction-only",
            setup="PRIVILEGED first-party fill/setblock in isolated fresh harness world; no teleport",
            acquisition="no inventory or earned/quality-production claims",
            excludes=["arbitrary partial/fluid/dynamic collision parity", "continuous authoritative server pose track",
                      "monitor scanout verification", "disconnect/cancel during in-flight navigation"],
            cases={},
        )
        tools = client.list_tools()
        schema = next((t["inputSchema"] for t in tools if t["name"] == TOOL), None)
        expected_schema = {"type": "object", "properties": {
            "x": {"type": "integer"}, "y": {"type": "integer"}, "z": {"type": "integer"},
            "timeout_seconds": {"type": "number", "minimum": 0.1, "maximum": 120.0, "default": 8.0}},
            "required": ["x", "y", "z"], "additionalProperties": False}
        steps.report["navigation_input_schema"] = schema
        steps.check("unchanged_firstparty_navigation_schema", schema == expected_schema)
        initial = steps.wait(lambda s: s["player"]["on_ground"] is True, "initial_grounded")
        p = initial["player"]
        x, y, z = math.floor(p["x"]), round(p["y"]), math.floor(p["z"])
        steps.check("fixture_ground_level_without_teleport", abs(p["y"] - y) < 0.02)
        identity = (p["entity_uuid"], initial["dimension"])
        steps.report["initial_state"] = initial
        # Load/light readiness is causal and precedes any privileged world mutation.
        for cx, cz in ((x - 8, z - 6), (x + 8, z + 6)):
            steps.call("minecraft_wait_for_loaded_block", {
                "x": cx, "y": y, "z": cz, "timeout_seconds": steps.remaining()})
        _fixture(steps, f"fill {x-8} {y-1} {z-6} {x+8} {y-1} {z+6} minecraft:stone",
                 (x, y-1, z), "minecraft:stone", "fixture_floor")
        _fixture(steps, f"fill {x-8} {y} {z-6} {x+8} {y+4} {z+6} minecraft:air",
                 (x, y+4, z), "minecraft:air", "fixture_headroom")
        # Page the bounded fixture by layer to stay below ordinary JSON limits.
        received = [row for level in range(y-1, y+5)
                    for row in _scan(steps, (x-8, level, z-6), (x+8, level, z+6))]
        steps.check("received_complete_clear_fixture", all(
            row["block_id"] == ("minecraft:stone" if row["y"] == y-1 else "minecraft:air")
            for row in received))
        steps.report["received_fixture"] = received
        steps.call("minecraft_look", {"yaw_deg": 0, "pitch_deg": 15})
        steps.call("minecraft_wait_ticks", {"ticks": 8})
        steps.report["cases"]["clear"] = _navigate(steps, artifact, "clear", (x-5, y, z-3), identity)
        solid = (x-5, y, z+3)
        _fixture(steps, f"setblock {solid[0]} {solid[1]} {solid[2]} minecraft:stone",
                 solid, "minecraft:stone", "fixture_solid_endpoint")
        steps.report["solid_target_block"] = _read(steps, solid)
        steps.report["cases"]["solid_adjacent"] = _navigate(steps, artifact, "solid-adjacent", solid, identity)
        _fixture(steps, f"fill {x+3} {y} {z+1} {x+7} {y} {z+5} minecraft:stone",
                 (x+3, y, z+1), "minecraft:stone", "fixture_step_platform")
        platform = _scan(steps, (x+3, y, z+1), (x+7, y, z+5))
        steps.check("received_step_platform", all(row["block_id"] == "minecraft:stone"
                    and row["collision_empty"] is False for row in platform))
        steps.report["received_step_platform"] = platform
        stepped = _navigate(steps, artifact, "ordinary-step-up", (x+5, y+1, z+3), identity)
        steps.check("ordinary_step_up_observed_elevation", abs(stepped["after"]["player"]["y"] - y - 1) < 0.02)
        steps.report["cases"]["ordinary_step_up"] = stepped
        # No valid standable point can meet this target's 1.5/1.25 arrival envelope:
        # the whole envelope is inside received full cubes and the top is too high.
        _fixture(steps, f"fill {x-7} {y} {z-5} {x-3} {y+3} {z-1} minecraft:stone",
                 (x-5, y, z-3), "minecraft:stone", "fixture_unreachable")
        cube = _scan(steps, (x-7, y, z-5), (x-3, y+3, z-1))
        steps.check("received_sealed_unreachable_volume", all(row["block_id"] == "minecraft:stone"
                    and row["collision_empty"] is False for row in cube))
        steps.report["received_unreachable_volume"] = cube
        _released(steps, "before_failures", stationary=True)
        steps.report["cases"]["unreachable"] = _failure(steps, "unreachable", (x-5, y, z-3), "unreachable")
        steps.report["cases"]["timeout"] = _failure(steps, "timeout", (x, y, z-4),
                                                               "timed out before arrival", seconds=0.1, stationary=False)
        _released(steps, "after_timeout_settled", stationary=True)
        steps.report["cases"]["height_bounds"] = _failure(steps, "height-bounds", (x, 2147483647, z),
                                                                  "outside the client world height")
        # First-party precedence checks the target chunk before search radius.
        # This coordinate is valid world height but outside the received view.
        steps.report["cases"]["chunk_missing"] = _failure(
            steps, "chunk-missing", (x+2048, y, z), "target chunk is not client-loaded")
        steps.report["final_state"] = steps.observe()
    return steps.report
