"""Colony villager live-QA scenario: exact chat contract, follow motion, and refusal probes."""

from __future__ import annotations

import time
from pathlib import Path
from typing import Any

if __package__:
    from .driver_client import (
        AgentClient,
        call_and_record,
        capture_screenshot,
        is_in_play,
        wait_for_existing_or_explicit_connection,
    )
else:
    from driver_client import (
        AgentClient,
        call_and_record,
        capture_screenshot,
        is_in_play,
        wait_for_existing_or_explicit_connection,
    )


def run_colony_villager_live_qa_scenario(
    client: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)
    exact_commands: list[str] = []
    command_observations: list[dict[str, Any]] = []
    exploratory_checks: list[dict[str, Any]] = []

    def recent_chat(observation: dict[str, Any]) -> list[str]:
        value = observation.get("recent_chat")
        if not isinstance(value, list) or not all(isinstance(line, str) for line in value):
            raise RuntimeError("client observation did not contain string recent_chat entries")
        return value

    def observe_client() -> dict[str, Any]:
        observation = call_and_record(
            client,
            transcript,
            "observe",
            {},
            timeout_seconds,
        )
        if not observation.get("in_play"):
            raise RuntimeError("client left Play state during colony QA")
        return observation

    def wait_for_exact_chat(
        expected: list[str],
        baseline_counts: dict[str, int],
    ) -> dict[str, Any]:
        deadline = time.monotonic() + timeout_seconds
        while True:
            observation = observe_client()
            lines = recent_chat(observation)
            if all(lines.count(message) > baseline_counts[message] for message in expected):
                return observation
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                missing = [
                    message
                    for message in expected
                    if lines.count(message) <= baseline_counts[message]
                ]
                raise TimeoutError(f"exact colony chat did not arrive: {missing}")
            state_version = observation.get("state_version")
            if not isinstance(state_version, int):
                raise RuntimeError("client observation did not contain integer state_version")
            event_timeout = min(remaining, 10.0)
            call_and_record(
                client,
                transcript,
                "wait_state_change",
                {
                    "observed_version": state_version,
                    "timeout_seconds": event_timeout,
                },
                event_timeout + 2.0,
            )

    def issue_command(command: str, expected: list[str]) -> dict[str, Any]:
        before = observe_client()
        before_lines = recent_chat(before)
        baseline_counts = {message: before_lines.count(message) for message in expected}
        exact_commands.append("/" + command)
        call_and_record(
            client,
            transcript,
            "send_chat",
            {"message": command, "command": True},
            timeout_seconds,
        )
        call_and_record(
            client,
            transcript,
            "wait_ticks",
            {"ticks": 25},
            timeout_seconds,
        )
        after = wait_for_exact_chat(expected, baseline_counts)
        command_observations.append(
            {
                "command": "/" + command,
                "expected_chat": expected,
                "observed_chat": [
                    line for line in recent_chat(after) if line in expected
                ],
            }
        )
        return after

    def visible_villagers(payload: dict[str, Any]) -> list[dict[str, Any]]:
        entities = payload.get("entities")
        if not isinstance(entities, list):
            raise RuntimeError("list_entities did not return entities")
        return [
            entity
            for entity in entities
            if isinstance(entity, dict)
            and entity.get("entity_type") == "minecraft:villager"
        ]

    call_and_record(client, transcript, "ping", {}, timeout_seconds)
    play_state = wait_for_existing_or_explicit_connection(
        client,
        transcript,
        server_addr,
        timeout_seconds,
    )
    if not is_in_play(play_state):
        raise RuntimeError("client did not reach Play state for colony QA")
    initial_observation = wait_for_exact_chat(
        ["Starter Colony plugin ready."],
        {"Starter Colony plugin ready.": 0},
    )
    call_and_record(
        client,
        transcript,
        "wait_ticks",
        {"ticks": 40},
        timeout_seconds,
    )

    summon_message = "Summoned minecraft:villager"
    issue_command("summon villager", [summon_message])
    issue_command("summon villager", [summon_message])
    summoned_entities = call_and_record(
        client,
        transcript,
        "list_entities",
        {"radius": 24.0, "limit": 64},
        timeout_seconds,
    )
    summoned_villagers = visible_villagers(summoned_entities)
    if len(summoned_villagers) < 2:
        raise RuntimeError(
            f"two summoned villagers were not client-visible: count={len(summoned_villagers)}"
        )

    durable_message = "Villager recruitment recorded durably by the Luau plugin."
    issue_command("colony recruit worker", [durable_message])
    issue_command("colony recruit guard", [durable_message])
    member_one_initial = (
        "Starter Colony member 1: status=active, role=worker, "
        "order=home, generation=2."
    )
    member_two_initial = (
        "Starter Colony member 2: status=active, role=guard, "
        "order=follow, generation=2."
    )
    issue_command("colony status", [member_one_initial, member_two_initial])

    invalid_action = "Colony command rejected: invalid bounded action."
    issue_command("colony role guard 1", ["Applied Luau order follow."])
    issue_command("colony order home 1", ["Applied Luau order home."])
    requested_command_contract_passed = True

    issue_command("colony role farmer 1", ["Applied Luau order home."])
    issue_command("colony order hold 1", ["Applied Luau order hold."])
    issue_command("colony dismiss 2", ["Member 2 dismissed: slot released."])

    member_one_held = (
        "Starter Colony member 1: status=active, role=farmer, "
        "order=hold, generation=6."
    )
    before_post_dismiss = observe_client()
    member_two_count_before = recent_chat(before_post_dismiss).count(member_two_initial)
    post_dismiss = issue_command("colony status", [member_one_held])
    member_two_count_after = recent_chat(post_dismiss).count(member_two_initial)
    if member_two_count_after != member_two_count_before:
        raise RuntimeError("dismissed member 2 was still emitted by colony status")

    issue_command("colony recruit guard", [durable_message])
    member_two_recruited = (
        "Starter Colony member 2: status=active, role=guard, "
        "order=follow, generation=5."
    )
    issue_command("colony status", [member_one_held, member_two_recruited])

    follow_before_move = call_and_record(
        client,
        transcript,
        "list_entities",
        {"radius": 24.0, "limit": 64},
        timeout_seconds,
    )
    follow_candidates = visible_villagers(follow_before_move)
    if len(follow_candidates) < 2:
        raise RuntimeError("recruited villagers were not visible before follow check")
    call_and_record(
        client,
        transcript,
        "move_by",
        {"dx_cm": 800, "dz_cm": 0},
        timeout_seconds,
    )
    call_and_record(
        client,
        transcript,
        "wait_ticks",
        {"ticks": 25},
        timeout_seconds,
    )
    follow_at_new_position = call_and_record(
        client,
        transcript,
        "list_entities",
        {"radius": 24.0, "limit": 64},
        timeout_seconds,
    )
    follow_at_new_position_by_id = {
        int(entity["entity_id"]): entity
        for entity in visible_villagers(follow_at_new_position)
    }
    issue_command("colony order follow 2", ["Applied Luau order follow."])
    follow_after = call_and_record(
        client,
        transcript,
        "list_entities",
        {"radius": 24.0, "limit": 64},
        timeout_seconds,
    )
    follow_motion_candidates: list[dict[str, Any]] = []
    for entity in visible_villagers(follow_after):
        entity_id = int(entity["entity_id"])
        before = follow_at_new_position_by_id.get(entity_id)
        if before is None:
            continue
        dx = float(entity["x"]) - float(before["x"])
        dz = float(entity["z"]) - float(before["z"])
        follow_motion_candidates.append(
            {
                "entity_id": entity_id,
                "entity_uuid": entity["entity_uuid"],
                "entity_type": entity["entity_type"],
                "horizontal_distance": (dx * dx + dz * dz) ** 0.5,
                "distance_to_player_before": before["distance"],
                "distance_to_player_after": entity["distance"],
                "start_x": before["x"],
                "start_z": before["z"],
                "end_x": entity["x"],
                "end_z": entity["z"],
            }
        )
    if not follow_motion_candidates:
        raise RuntimeError("no villager identity survived the follow observation")
    follow_motion = max(
        follow_motion_candidates,
        key=lambda motion: float(motion["horizontal_distance"]),
    )
    if (
        float(follow_motion["horizontal_distance"]) < 0.25
        or float(follow_motion["distance_to_player_after"])
        >= float(follow_motion["distance_to_player_before"])
    ):
        raise RuntimeError(
            "no rebound villager approached the moved player after follow refresh: "
            f"{follow_motion}"
        )
    call_and_record(
        client,
        transcript,
        "look",
        {"yaw_deg": 90, "pitch_deg": 0},
        timeout_seconds,
    )
    call_and_record(
        client,
        transcript,
        "close_screen",
        {},
        timeout_seconds,
    )
    call_and_record(
        client,
        transcript,
        "wait_ticks",
        {"ticks": 5},
        timeout_seconds,
    )
    screenshot = capture_screenshot(
        client,
        transcript,
        run_dir,
        screenshots_dir,
        f"{scenario_id}-follow",
        timeout_seconds,
    )

    exploratory_checks.append(
        {
            "check": "unsupported role",
            "command": "/colony role lumberjack 1",
            "observation": issue_command(
                "colony role lumberjack 1",
                [invalid_action],
            ),
        }
    )
    exploratory_checks.append(
        {
            "check": "unsupported order",
            "command": "/colony order patrol 1",
            "observation": issue_command(
                "colony order patrol 1",
                [invalid_action],
            ),
        }
    )
    exploratory_checks.append(
        {
            "check": "out-of-range dismiss",
            "command": "/colony dismiss 99",
            "observation": issue_command(
                "colony dismiss 99",
                [invalid_action],
            ),
        }
    )
    exploratory_checks.append(
        {
            "check": "dismiss empty slot",
            "command": "/colony dismiss 3",
            "observation": issue_command(
                "colony dismiss 3",
                ["Update rejected: member 3 is not active."],
            ),
        }
    )
    away_observation = observe_client()
    away_player = away_observation.get("player")
    if not isinstance(away_player, dict):
        raise RuntimeError("client observation did not contain player coordinates")
    away_x = int(round(float(away_player["x"]))) + 64
    away_y = int(round(float(away_player["y"])))
    away_z = int(round(float(away_player["z"]))) + 64
    issue_command(
        f"tp {away_x} {away_y} {away_z}",
        [f"Teleported to {away_x} {away_y} {away_z}"],
    )
    exploratory_checks.append(
        {
            "check": "recruit with no unbound villager nearby",
            "command": "/colony recruit worker",
            "observation": issue_command(
                "colony recruit worker",
                ["Villager binding failed: not_found."],
            ),
        }
    )

    final_state = call_and_record(client, transcript, "state", {}, timeout_seconds)
    final_observation = observe_client()
    call_and_record(client, transcript, "disconnect", {}, timeout_seconds)
    scenario_report = {
        "result": "passed",
        "id": scenario_id,
        "requested_command_contract_passed": requested_command_contract_passed,
        "exact_commands": exact_commands,
        "command_observations": command_observations,
        "initial_observation": initial_observation,
        "summoned_villagers": summoned_villagers,
        "post_dismiss_member_2_status_count": member_two_count_after,
        "rerecruit_generation": 5,
        "follow_before_move": follow_before_move,
        "follow_motion": follow_motion,
        "follow_after": follow_after,
        "exploratory_checks": exploratory_checks,
        "final_observation": final_observation,
    }
    return "passed", final_state, [screenshot], scenario_report
