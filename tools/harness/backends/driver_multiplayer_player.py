"""Two-client player visibility, chat, disconnect, and respawn scenarios."""

from __future__ import annotations

from pathlib import Path
from typing import Any

if __package__:
    from .driver_client import (
        AgentClient, call_and_record, capture_screenshot, chunk_delta, horizontal_distance,
        is_in_play, wait_for_existing_or_explicit_connection, wait_for_server_session_release,
        wait_until_not_in_play,
    )
    from .driver_multiplayer_shared import prepare_two_client_scenario, capture_two_client_screenshots
    from .driver_results import combine_results, scenario_report_result
else:
    from driver_client import (
        AgentClient, call_and_record, capture_screenshot, chunk_delta, horizontal_distance,
        is_in_play, wait_for_existing_or_explicit_connection, wait_for_server_session_release,
        wait_until_not_in_play,
    )
    from driver_multiplayer_shared import prepare_two_client_scenario, capture_two_client_screenshots
    from driver_results import combine_results, scenario_report_result


def run_playable_two_client_player_visibility_movement_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = prepare_two_client_scenario(
        primary, secondary, run_dir, server_addr, timeout_seconds, transcript,
    )
    secondary_observe_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-33-two-client-player-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_observe_result = scenario_report_result(secondary_observe_report)
    primary_before_move_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    move_ticks = 20
    call_and_record(
        primary,
        transcript,
        "move_forward",
        {"ticks": move_ticks},
        min(timeout_seconds, 10.0),
        "primary",
    )
    primary_after_move_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_moved_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-33-two-client-player-moved-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_moved_result = scenario_report_result(secondary_moved_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        secondary_observe_result,
        secondary_moved_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"secondary bridge scenario=playable-33-two-client-player-observe result={secondary_observe_result}",
            f"primary bridge move_forward ticks={move_ticks}",
            f"secondary bridge scenario=playable-33-two-client-player-moved-observe result={secondary_moved_result}",
        ],
        "secondary_observe_report": secondary_observe_report,
        "primary_before_move_state": primary_before_move_state,
        "primary_after_move_state": primary_after_move_state,
        "secondary_moved_report": secondary_moved_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_chat_message_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = prepare_two_client_scenario(
        primary, secondary, run_dir, server_addr, timeout_seconds, transcript,
    )
    primary_send_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-34-two-client-chat-send",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_send_result = scenario_report_result(primary_send_report)
    secondary_observe_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-34-two-client-chat-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_observe_result = scenario_report_result(secondary_observe_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_send_result,
        secondary_observe_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=playable-34-two-client-chat-send result={primary_send_result}",
            f"secondary bridge scenario=playable-34-two-client-chat-observe result={secondary_observe_result}",
        ],
        "primary_send_report": primary_send_report,
        "secondary_observe_report": secondary_observe_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_player_disconnect_removal_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = prepare_two_client_scenario(
        primary, secondary, run_dir, server_addr, timeout_seconds, transcript,
    )
    secondary_visible_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-35-two-client-player-disconnect-visible",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_visible_result = scenario_report_result(secondary_visible_report)
    primary_before_disconnect_state = call_and_record(
        primary,
        transcript,
        "state",
        {},
        timeout_seconds,
        "primary",
    )
    primary_screenshot = capture_screenshot(
        primary,
        transcript,
        run_dir,
        screenshots_dir,
        f"{scenario_id}-primary-before-disconnect",
        timeout_seconds,
        actor="primary",
    )
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")
    server_release = wait_for_server_session_release(run_dir, timeout_seconds)
    secondary_gone_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-35-two-client-player-gone-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_gone_result = scenario_report_result(secondary_gone_report)
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        primary_screenshot,
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")

    result = combine_results([
        secondary_visible_result,
        secondary_gone_result,
    ])
    final_state = {
        "primary_before_disconnect": primary_before_disconnect_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"secondary bridge scenario=playable-35-two-client-player-disconnect-visible result={secondary_visible_result}",
            "primary bridge disconnect: sent",
            server_release,
            f"secondary bridge scenario=playable-35-two-client-player-gone-observe result={secondary_gone_result}",
        ],
        "secondary_visible_report": secondary_visible_report,
        "primary_before_disconnect_state": primary_before_disconnect_state,
        "secondary_gone_report": secondary_gone_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_player_reconnect_cleanup_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = prepare_two_client_scenario(
        primary, secondary, run_dir, server_addr, timeout_seconds, transcript,
    )
    secondary_visible_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-36-two-client-player-reconnect-visible",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_visible_result = scenario_report_result(secondary_visible_report)
    primary_before_disconnect_state = call_and_record(
        primary,
        transcript,
        "state",
        {},
        timeout_seconds,
        "primary",
    )
    primary_before_disconnect_screenshot = capture_screenshot(
        primary,
        transcript,
        run_dir,
        screenshots_dir,
        f"{scenario_id}-primary-before-disconnect",
        timeout_seconds,
        actor="primary",
    )
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")
    primary_disconnected_state = wait_until_not_in_play(primary, transcript, timeout_seconds, actor="primary")
    server_release = wait_for_server_session_release(run_dir, timeout_seconds)
    secondary_gone_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-36-two-client-player-reconnect-gone-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_gone_result = scenario_report_result(secondary_gone_report)
    primary_reconnected_state = wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    secondary_reconnected_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-36-two-client-player-reconnected-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_reconnected_result = scenario_report_result(secondary_reconnected_report)
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        primary_before_disconnect_screenshot,
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary-after-reconnect",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        secondary_visible_result,
        secondary_gone_result,
        secondary_reconnected_result,
    ])
    final_state = {
        "primary_before_disconnect": primary_before_disconnect_state,
        "primary_disconnected": primary_disconnected_state,
        "primary_reconnected": primary_reconnected_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"secondary bridge scenario=playable-36-two-client-player-reconnect-visible result={secondary_visible_result}",
            "primary bridge disconnect: sent",
            server_release,
            f"secondary bridge scenario=playable-36-two-client-player-reconnect-gone-observe result={secondary_gone_result}",
            "primary bridge reconnect: reached Play state",
            f"secondary bridge scenario=playable-36-two-client-player-reconnected-observe result={secondary_reconnected_result}",
        ],
        "secondary_visible_report": secondary_visible_report,
        "primary_before_disconnect_state": primary_before_disconnect_state,
        "primary_disconnected_state": primary_disconnected_state,
        "primary_reconnected_state": primary_reconnected_state,
        "secondary_gone_report": secondary_gone_report,
        "secondary_reconnected_report": secondary_reconnected_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_player_death_respawn_visibility_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = prepare_two_client_scenario(
        primary, secondary, run_dir, server_addr, timeout_seconds, transcript,
    )
    secondary_baseline_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-37-two-client-player-death-baseline",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_baseline_result = scenario_report_result(secondary_baseline_report)
    primary_death_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-37-two-client-campfire-death-respawn",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_death_result = scenario_report_result(primary_death_report)
    primary_after_respawn_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    move_ticks = 20
    if primary_death_result == "passed":
        call_and_record(
            primary,
            transcript,
            "move_forward",
            {"ticks": move_ticks},
            min(timeout_seconds, 10.0),
            "primary",
        )
        primary_after_move_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
        secondary_moved_report = call_and_record(
            secondary,
            transcript,
            "run_scenario",
            {
                "id": "playable-37-two-client-player-post-respawn-moved-observe",
                "screenshots_dir": str(screenshots_dir),
            },
            timeout_seconds,
            "secondary",
        )
        secondary_moved_result = scenario_report_result(secondary_moved_report)
    else:
        primary_after_move_state = primary_after_respawn_state
        secondary_moved_report = {
            "id": "playable-37-two-client-player-post-respawn-moved-observe",
            "result": "failed",
            "observations": ["skipped: primary campfire death/respawn phase failed"],
        }
        secondary_moved_result = "failed"
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        secondary_baseline_result,
        primary_death_result,
        secondary_moved_result,
    ])
    final_state = {
        "primary_after_respawn": primary_after_respawn_state,
        "primary_after_move": primary_after_move_state,
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"secondary bridge scenario=playable-37-two-client-player-death-baseline result={secondary_baseline_result}",
            f"primary bridge scenario=playable-37-two-client-campfire-death-respawn result={primary_death_result}",
            f"primary bridge post-respawn move_forward ticks={move_ticks}",
            f"secondary bridge scenario=playable-37-two-client-player-post-respawn-moved-observe result={secondary_moved_result}",
        ],
        "secondary_baseline_report": secondary_baseline_report,
        "primary_death_report": primary_death_report,
        "primary_after_respawn_state": primary_after_respawn_state,
        "primary_after_move_state": primary_after_move_state,
        "secondary_moved_report": secondary_moved_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_movement_soak_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
    *,
    observation_label: str,
    pulse_count: int,
    min_chunk_delta: int,
    primary_move_command: str = "move_forward",
    secondary_move_command: str = "move_forward",
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    primary_play = wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    secondary_play = wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_initial = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_initial = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")

    move_ticks = 15
    primary_states: list[dict[str, Any]] = []
    secondary_states: list[dict[str, Any]] = []
    for _ in range(pulse_count):
        call_and_record(
            primary,
            transcript,
            primary_move_command,
            {"ticks": move_ticks},
            min(timeout_seconds, 10.0),
            "primary",
        )
        primary_states.append(call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary"))
        call_and_record(
            secondary,
            transcript,
            secondary_move_command,
            {"ticks": move_ticks},
            min(timeout_seconds, 10.0),
            "secondary",
        )
        secondary_states.append(call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary"))

    primary_final = primary_states[-1] if primary_states else primary_initial
    secondary_final = secondary_states[-1] if secondary_states else secondary_initial
    primary_delta = horizontal_distance(primary_initial, primary_final)
    secondary_delta = horizontal_distance(secondary_initial, secondary_final)
    primary_chunk_delta = chunk_delta(primary_initial, primary_final)
    secondary_chunk_delta = chunk_delta(secondary_initial, secondary_final)
    all_states_in_play = all(
        is_in_play(state)
        for state in [primary_play, secondary_play, primary_initial, secondary_initial]
        + primary_states
        + secondary_states
    )
    movement_passed = (
        primary_delta >= 0.05
        and secondary_delta >= 0.05
        and primary_chunk_delta >= min_chunk_delta
        and secondary_chunk_delta >= min_chunk_delta
    )
    result = "passed" if all_states_in_play and movement_passed else "failed"

    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    final_state = {
        "primary": primary_final,
        "secondary": secondary_final,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            "two-client "
            + observation_label
            + ": "
            + result
            + f" pulses={pulse_count} min_chunk_delta={min_chunk_delta}"
            + f" ticks={move_ticks}"
            + f" primary_move_command={primary_move_command}"
            + f" secondary_move_command={secondary_move_command}"
            + f" primary_horizontal_delta={primary_delta:.3f}"
            + f" secondary_horizontal_delta={secondary_delta:.3f}"
            + f" primary_chunk_delta={primary_chunk_delta}"
            + f" secondary_chunk_delta={secondary_chunk_delta}"
            + f" all_states_in_play={str(all_states_in_play).lower()}",
        ],
        "primary_initial_state": primary_initial,
        "primary_final_state": primary_final,
        "secondary_initial_state": secondary_initial,
        "secondary_final_state": secondary_final,
    }
    return result, final_state, screenshots, scenario_report
