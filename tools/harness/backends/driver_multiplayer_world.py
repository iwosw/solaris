"""Two-client shared world, container, and item-transfer scenarios."""

from __future__ import annotations

from pathlib import Path
from typing import Any

if __package__:
    from .driver_client import AgentClient, call_and_record, is_in_play, wait_for_existing_or_explicit_connection
    from .driver_multiplayer_shared import prepare_two_client_scenario, capture_two_client_screenshots
    from .driver_results import combine_results, scenario_report_result
    from .driver_restart import (
        P45_RESTART_BEFORE_SCENARIO,
        RESTART_INVARIANT_SCHEMA,
        load_p45_shared_chest_marker,
        load_restart_invariant_snapshot,
        p45_restart_snapshot,
        validate_p45_restart_after,
        validate_p45_restart_preflight,
        write_restart_invariant_snapshot,
    )
else:
    from driver_client import AgentClient, call_and_record, is_in_play, wait_for_existing_or_explicit_connection
    from driver_multiplayer_shared import prepare_two_client_scenario, capture_two_client_screenshots
    from driver_results import combine_results, scenario_report_result
    from driver_restart import (
        P45_RESTART_BEFORE_SCENARIO,
        RESTART_INVARIANT_SCHEMA,
        load_p45_shared_chest_marker,
        load_restart_invariant_snapshot,
        p45_restart_snapshot,
        validate_p45_restart_after,
        validate_p45_restart_preflight,
        write_restart_invariant_snapshot,
    )


def run_two_client_visibility_scenario(
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
    primary_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-place",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_result = scenario_report_result(primary_report)
    secondary_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_result = scenario_report_result(secondary_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([primary_result, secondary_result])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=m94-06-two-client-place result={primary_result}",
            f"secondary bridge scenario=m94-06-two-client-observe result={secondary_result}",
        ],
        "primary_report": primary_report,
        "secondary_report": secondary_report,
    }
    return result, final_state, screenshots, scenario_report


def run_two_client_shared_chest_scenario(
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
    primary_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-03b-two-client-shared-chest-deposit",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_result = scenario_report_result(primary_report)
    secondary_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "m94-03b-two-client-shared-chest-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_result = scenario_report_result(secondary_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([primary_result, secondary_result])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=m94-03b-two-client-shared-chest-deposit result={primary_result}",
            f"secondary bridge scenario=m94-03b-two-client-shared-chest-observe result={secondary_result}",
        ],
        "primary_report": primary_report,
        "secondary_report": secondary_report,
    }
    return result, final_state, screenshots, scenario_report


def run_two_client_shared_chest_live_update_scenario(
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
    primary_open_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-03c-two-client-shared-chest-open-with-dirt",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_open_result = scenario_report_result(primary_open_report)
    secondary_withdraw_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "m94-03c-two-client-shared-chest-withdraw",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_withdraw_result = scenario_report_result(secondary_withdraw_report)
    primary_observe_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-03c-two-client-shared-chest-observe-empty",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_observe_result = scenario_report_result(primary_observe_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_open_result,
        secondary_withdraw_result,
        primary_observe_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=m94-03c-two-client-shared-chest-open-with-dirt result={primary_open_result}",
            f"secondary bridge scenario=m94-03c-two-client-shared-chest-withdraw result={secondary_withdraw_result}",
            f"primary bridge scenario=m94-03c-two-client-shared-chest-observe-empty result={primary_observe_result}",
        ],
        "primary_open_report": primary_open_report,
        "secondary_withdraw_report": secondary_withdraw_report,
        "primary_observe_report": primary_observe_report,
    }
    return result, final_state, screenshots, scenario_report


def run_two_client_shared_drop_scenario(
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
    primary_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-drop-break",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_result = scenario_report_result(primary_report)
    secondary_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-drop-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_result = scenario_report_result(secondary_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([primary_result, secondary_result])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=m94-06-two-client-drop-break result={primary_result}",
            f"secondary bridge scenario=m94-06-two-client-drop-observe result={secondary_result}",
        ],
        "primary_report": primary_report,
        "secondary_report": secondary_report,
    }
    return result, final_state, screenshots, scenario_report


def run_two_client_shared_pickup_scenario(
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
    primary_drop_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-drop-break",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_drop_result = scenario_report_result(primary_drop_report)
    secondary_drop_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-drop-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_drop_result = scenario_report_result(secondary_drop_report)
    primary_pickup_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-pickup-collect",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_pickup_result = scenario_report_result(primary_pickup_report)
    secondary_gone_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-pickup-gone-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_gone_result = scenario_report_result(secondary_gone_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_drop_result,
        secondary_drop_result,
        primary_pickup_result,
        secondary_gone_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=m94-06-two-client-drop-break result={primary_drop_result}",
            f"secondary bridge scenario=m94-06-two-client-drop-observe result={secondary_drop_result}",
            f"primary bridge scenario=m94-06-two-client-pickup-collect result={primary_pickup_result}",
            f"secondary bridge scenario=m94-06-two-client-pickup-gone-observe result={secondary_gone_result}",
        ],
        "primary_drop_report": primary_drop_report,
        "secondary_drop_report": secondary_drop_report,
        "primary_pickup_report": primary_pickup_report,
        "secondary_gone_report": secondary_gone_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_shared_log_drop_pickup_scenario(
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
    primary_drop_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-30-two-client-shared-log-drop-break",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_drop_result = scenario_report_result(primary_drop_report)
    secondary_drop_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-30-two-client-shared-log-drop-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_drop_result = scenario_report_result(secondary_drop_report)
    primary_pickup_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-30-two-client-shared-log-pickup-collect",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_pickup_result = scenario_report_result(primary_pickup_report)
    secondary_gone_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-30-two-client-shared-log-pickup-gone-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_gone_result = scenario_report_result(secondary_gone_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_drop_result,
        secondary_drop_result,
        primary_pickup_result,
        secondary_gone_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=playable-30-two-client-shared-log-drop-break result={primary_drop_result}",
            f"secondary bridge scenario=playable-30-two-client-shared-log-drop-observe result={secondary_drop_result}",
            f"primary bridge scenario=playable-30-two-client-shared-log-pickup-collect result={primary_pickup_result}",
            f"secondary bridge scenario=playable-30-two-client-shared-log-pickup-gone-observe result={secondary_gone_result}",
        ],
        "primary_drop_report": primary_drop_report,
        "secondary_drop_report": secondary_drop_report,
        "primary_pickup_report": primary_pickup_report,
        "secondary_gone_report": secondary_gone_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_earned_shared_chest_scenario(
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
    primary_deposit_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-31-two-client-earned-shared-chest-deposit",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_deposit_result = scenario_report_result(primary_deposit_report)
    secondary_withdraw_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-31-two-client-earned-shared-chest-withdraw",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_withdraw_result = scenario_report_result(secondary_withdraw_report)
    primary_empty_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-31-two-client-earned-shared-chest-observe-empty",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_empty_result = scenario_report_result(primary_empty_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_deposit_result,
        secondary_withdraw_result,
        primary_empty_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=playable-31-two-client-earned-shared-chest-deposit result={primary_deposit_result}",
            f"secondary bridge scenario=playable-31-two-client-earned-shared-chest-withdraw result={secondary_withdraw_result}",
            f"primary bridge scenario=playable-31-two-client-earned-shared-chest-observe-empty result={primary_empty_result}",
        ],
        "primary_deposit_report": primary_deposit_report,
        "secondary_withdraw_report": secondary_withdraw_report,
        "primary_empty_report": primary_empty_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_shared_chest_save_restart_before_scenario(
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
    primary_deposit_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-31-two-client-earned-shared-chest-deposit",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_deposit_result = scenario_report_result(primary_deposit_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    clients_in_play = is_in_play(primary_state) and is_in_play(secondary_state)
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    marker = load_p45_shared_chest_marker(run_dir)
    invariant_snapshot = p45_restart_snapshot(primary_state, secondary_state, marker)
    write_restart_invariant_snapshot(run_dir, invariant_snapshot)

    result = combine_results([
        primary_deposit_result,
        "passed" if clients_in_play else "failed",
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary earned chest deposit result={primary_deposit_result}",
            f"both clients in Play before restart={str(clients_in_play).lower()}",
            "runner-managed restart pending after both clients disconnect cleanly",
        ],
        "primary_deposit_report": primary_deposit_report,
        "restart_invariant_snapshot": invariant_snapshot,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_shared_chest_save_restart_after_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)
    invariant_snapshot = load_restart_invariant_snapshot(run_dir, P45_RESTART_BEFORE_SCENARIO)
    marker = load_p45_shared_chest_marker(run_dir)
    validate_p45_restart_preflight(invariant_snapshot, marker)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    secondary_withdraw_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-31-two-client-earned-shared-chest-withdraw",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_withdraw_result = scenario_report_result(secondary_withdraw_report)
    primary_empty_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-31-two-client-earned-shared-chest-observe-empty",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_empty_result = scenario_report_result(primary_empty_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    clients_in_play = is_in_play(primary_state) and is_in_play(secondary_state)
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    invariant_checks = validate_p45_restart_after(
        invariant_snapshot,
        primary_state,
        secondary_state,
        marker,
        secondary_withdraw_result,
        primary_empty_result,
    )
    result = combine_results([
        secondary_withdraw_result,
        primary_empty_result,
        "passed" if clients_in_play else "failed",
        "passed" if invariant_checks else "failed",
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"secondary post-restart withdraw result={secondary_withdraw_result}",
            f"primary post-restart empty observe result={primary_empty_result}",
            f"both clients in Play after restart={str(clients_in_play).lower()}",
        ],
        "secondary_withdraw_report": secondary_withdraw_report,
        "primary_empty_report": primary_empty_report,
        "restart_invariant_validation": {
            "schema": RESTART_INVARIANT_SCHEMA,
            "status": "passed",
            "checks": invariant_checks,
        },
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_earned_torch_block_edit_scenario(
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
    primary_place_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-32-two-client-earned-torch-place",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_place_result = scenario_report_result(primary_place_report)
    secondary_observe_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-32-two-client-earned-torch-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_observe_result = scenario_report_result(secondary_observe_report)
    primary_break_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-32-two-client-earned-torch-break",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_break_result = scenario_report_result(primary_break_report)
    secondary_gone_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-32-two-client-earned-torch-gone-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_gone_result = scenario_report_result(secondary_gone_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_place_result,
        secondary_observe_result,
        primary_break_result,
        secondary_gone_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=playable-32-two-client-earned-torch-place result={primary_place_result}",
            f"secondary bridge scenario=playable-32-two-client-earned-torch-observe result={secondary_observe_result}",
            f"primary bridge scenario=playable-32-two-client-earned-torch-break result={primary_break_result}",
            f"secondary bridge scenario=playable-32-two-client-earned-torch-gone-observe result={secondary_gone_result}",
        ],
        "primary_place_report": primary_place_report,
        "secondary_observe_report": secondary_observe_report,
        "primary_break_report": primary_break_report,
        "secondary_gone_report": secondary_gone_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_inventory_drop_handoff_scenario(
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
    primary_drop_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-38-two-client-inventory-drop-primary",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_drop_result = scenario_report_result(primary_drop_report)
    if primary_drop_result == "passed":
        secondary_observe_report = call_and_record(
            secondary,
            transcript,
            "run_scenario",
            {
                "id": "playable-38-two-client-inventory-drop-observe",
                "screenshots_dir": str(screenshots_dir),
            },
            timeout_seconds,
            "secondary",
        )
        secondary_observe_result = scenario_report_result(secondary_observe_report)
    else:
        secondary_observe_report = {
            "id": "playable-38-two-client-inventory-drop-observe",
            "result": "failed",
            "observations": ["skipped: primary inventory drop phase failed"],
        }
        secondary_observe_result = "failed"
    if secondary_observe_result == "passed":
        secondary_pickup_report = call_and_record(
            secondary,
            transcript,
            "run_scenario",
            {
                "id": "playable-38-two-client-inventory-drop-secondary-pickup",
                "screenshots_dir": str(screenshots_dir),
            },
            timeout_seconds,
            "secondary",
        )
        secondary_pickup_result = scenario_report_result(secondary_pickup_report)
    else:
        secondary_pickup_report = {
            "id": "playable-38-two-client-inventory-drop-secondary-pickup",
            "result": "failed",
            "observations": ["skipped: secondary inventory drop observe phase failed"],
        }
        secondary_pickup_result = "failed"
    if secondary_pickup_result == "passed":
        primary_gone_report = call_and_record(
            primary,
            transcript,
            "run_scenario",
            {
                "id": "playable-38-two-client-inventory-drop-gone-observe",
                "screenshots_dir": str(screenshots_dir),
            },
            timeout_seconds,
            "primary",
        )
        primary_gone_result = scenario_report_result(primary_gone_report)
    else:
        primary_gone_report = {
            "id": "playable-38-two-client-inventory-drop-gone-observe",
            "result": "failed",
            "observations": ["skipped: secondary inventory drop pickup phase failed"],
        }
        primary_gone_result = "failed"
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = capture_two_client_screenshots(
        primary, secondary, transcript, run_dir, screenshots_dir, scenario_id, timeout_seconds,
    )
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_drop_result,
        secondary_observe_result,
        secondary_pickup_result,
        primary_gone_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=playable-38-two-client-inventory-drop-primary result={primary_drop_result}",
            f"secondary bridge scenario=playable-38-two-client-inventory-drop-observe result={secondary_observe_result}",
            f"secondary bridge scenario=playable-38-two-client-inventory-drop-secondary-pickup result={secondary_pickup_result}",
            f"primary bridge scenario=playable-38-two-client-inventory-drop-gone-observe result={primary_gone_result}",
        ],
        "primary_drop_report": primary_drop_report,
        "secondary_observe_report": secondary_observe_report,
        "secondary_pickup_report": secondary_pickup_report,
        "primary_gone_report": primary_gone_report,
    }
    return result, final_state, screenshots, scenario_report
