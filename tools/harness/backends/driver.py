#!/usr/bin/env python3
"""Drive a Solaris real-client bridge and write fail-closed observations."""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any

if __package__:
    from .driver_client import (
        AgentClient,
        call_and_record,
        capture_screenshot,
        horizontal_distance,
        is_in_play,
        state_observation,
        utc_now,
        validate_loopback_server_addr,
        wait_for_existing_or_explicit_connection,
        wait_for_interactive_play,
        wait_for_server_session_release,
        wait_for_valid_png,
        wait_until_not_in_play,
    )
    from .driver_multiplayer_player import (
        run_playable_two_client_chat_message_scenario,
        run_playable_two_client_movement_soak_scenario,
        run_playable_two_client_player_death_respawn_visibility_scenario,
        run_playable_two_client_player_disconnect_removal_scenario,
        run_playable_two_client_player_reconnect_cleanup_scenario,
        run_playable_two_client_player_visibility_movement_scenario,
    )
    from .driver_multiplayer_world import (
        run_playable_two_client_earned_shared_chest_scenario,
        run_playable_two_client_earned_torch_block_edit_scenario,
        run_playable_two_client_inventory_drop_handoff_scenario,
        run_playable_two_client_shared_chest_save_restart_after_scenario,
        run_playable_two_client_shared_chest_save_restart_before_scenario,
        run_playable_two_client_shared_log_drop_pickup_scenario,
        run_two_client_shared_chest_live_update_scenario,
        run_two_client_shared_chest_scenario,
        run_two_client_shared_drop_scenario,
        run_two_client_shared_pickup_scenario,
        run_two_client_visibility_scenario,
    )
    from .driver_colony import run_colony_villager_live_qa_scenario
    from .driver_replay import load_core_replay_manifest, run_core_replay_scenario
    from .driver_results import combine_results, scenario_report_result
    from .wasm_inventory_probe import (
        WASM_INVENTORY_MENU_SCENARIOS,
        WASM_INVENTORY_STORAGE_SCENARIOS,
        WASM_ZONE_MARKET_SCENARIOS,
        TradeScenarioHooks,
    )
    from .wasm_inventory_trade import run_wasm_inventory_storage_scenario
    from .wasm_inventory_menu import run_wasm_inventory_menu_scenario
    from .wasm_zone_market import run_wasm_zone_market_scenario
    from .wasm_plugin_join_hello import (
        WASM_PLUGIN_JOIN_HELLO_SCENARIOS,
        HarnessHooks,
        run_wasm_plugin_join_hello_scenario,
    )
else:
    # Direct scripts get this path automatically; importlib/runpy file loaders
    # do not. Both entry modes need the same sibling scenario modules.
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from driver_client import (
        AgentClient,
        call_and_record,
        capture_screenshot,
        horizontal_distance,
        is_in_play,
        state_observation,
        utc_now,
        validate_loopback_server_addr,
        wait_for_existing_or_explicit_connection,
        wait_for_interactive_play,
        wait_for_server_session_release,
        wait_for_valid_png,
        wait_until_not_in_play,
    )
    from driver_multiplayer_player import (
        run_playable_two_client_chat_message_scenario,
        run_playable_two_client_movement_soak_scenario,
        run_playable_two_client_player_death_respawn_visibility_scenario,
        run_playable_two_client_player_disconnect_removal_scenario,
        run_playable_two_client_player_reconnect_cleanup_scenario,
        run_playable_two_client_player_visibility_movement_scenario,
    )
    from driver_multiplayer_world import (
        run_playable_two_client_earned_shared_chest_scenario,
        run_playable_two_client_earned_torch_block_edit_scenario,
        run_playable_two_client_inventory_drop_handoff_scenario,
        run_playable_two_client_shared_chest_save_restart_after_scenario,
        run_playable_two_client_shared_chest_save_restart_before_scenario,
        run_playable_two_client_shared_log_drop_pickup_scenario,
        run_two_client_shared_chest_live_update_scenario,
        run_two_client_shared_chest_scenario,
        run_two_client_shared_drop_scenario,
        run_two_client_shared_pickup_scenario,
        run_two_client_visibility_scenario,
    )
    from driver_replay import load_core_replay_manifest, run_core_replay_scenario
    from driver_colony import run_colony_villager_live_qa_scenario
    from driver_results import combine_results, scenario_report_result
    from wasm_inventory_probe import (
        WASM_INVENTORY_MENU_SCENARIOS,
        WASM_INVENTORY_STORAGE_SCENARIOS,
        WASM_ZONE_MARKET_SCENARIOS,
        TradeScenarioHooks,
    )
    from wasm_inventory_trade import run_wasm_inventory_storage_scenario
    from wasm_inventory_menu import run_wasm_inventory_menu_scenario
    from wasm_zone_market import run_wasm_zone_market_scenario
    from wasm_plugin_join_hello import (
        WASM_PLUGIN_JOIN_HELLO_SCENARIOS,
        HarnessHooks,
        run_wasm_plugin_join_hello_scenario,
    )


def run_bridge_scenario(
    client: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
    secondary_client: AgentClient | None = None,
    replay_manifest: dict[str, Any] | None = None,
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    if replay_manifest is not None:
        if secondary_client is not None:
            raise ValueError("core replay currently supports exactly one real client")
        return run_core_replay_scenario(
            client,
            run_dir,
            replay_manifest,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id in WASM_PLUGIN_JOIN_HELLO_SCENARIOS:
        if secondary_client is not None:
            raise ValueError(f"{scenario_id} drives exactly one real client")
        return run_wasm_plugin_join_hello_scenario(
            client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
            HarnessHooks(
                call=call_and_record,
                capture_screenshot=capture_screenshot,
                connect_or_confirm_play=wait_for_existing_or_explicit_connection,
                is_in_play=is_in_play,
            ),
        )
    if scenario_id in WASM_INVENTORY_STORAGE_SCENARIOS:
        if secondary_client is not None:
            raise ValueError(f"{scenario_id} drives exactly one real client")
        return run_wasm_inventory_storage_scenario(
            client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
            TradeScenarioHooks(
                call=call_and_record,
                capture_screenshot=capture_screenshot,
                connect_or_confirm_play=wait_for_existing_or_explicit_connection,
                is_in_play=is_in_play,
                leave_play=wait_until_not_in_play,
                await_session_release=wait_for_server_session_release,
                await_interactive_play=wait_for_interactive_play,
            ),
        )
    if scenario_id in WASM_INVENTORY_MENU_SCENARIOS:
        if secondary_client is not None:
            raise ValueError(f"{scenario_id} drives exactly one real client")
        return run_wasm_inventory_menu_scenario(
            client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
            TradeScenarioHooks(
                call=call_and_record,
                capture_screenshot=capture_screenshot,
                connect_or_confirm_play=wait_for_existing_or_explicit_connection,
                is_in_play=is_in_play,
                leave_play=wait_until_not_in_play,
                await_session_release=wait_for_server_session_release,
                await_interactive_play=wait_for_interactive_play,
            ),
        )
    if scenario_id in WASM_ZONE_MARKET_SCENARIOS:
        if secondary_client is not None:
            raise ValueError(f"{scenario_id} drives exactly one real client")
        return run_wasm_zone_market_scenario(
            client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
            TradeScenarioHooks(
                call=call_and_record,
                capture_screenshot=capture_screenshot,
                connect_or_confirm_play=wait_for_existing_or_explicit_connection,
                is_in_play=is_in_play,
                leave_play=wait_until_not_in_play,
                await_session_release=wait_for_server_session_release,
                await_interactive_play=wait_for_interactive_play,
            ),
        )
    if scenario_id == "m94-01-join-rejoin-chunks-movement":
        return run_join_rejoin_movement_scenario(
            client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "m94-colony-villager-live-qa":
        return run_colony_villager_live_qa_scenario(
            client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "m94-03b-two-client-shared-chest":
        if secondary_client is None:
            raise RuntimeError("m94-03b two-client shared chest requires a secondary bridge")
        return run_two_client_shared_chest_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "m94-03c-two-client-shared-chest-live-update":
        if secondary_client is None:
            raise RuntimeError("m94-03c two-client shared chest live update requires a secondary bridge")
        return run_two_client_shared_chest_live_update_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "m94-06-two-client-live-visibility":
        if secondary_client is None:
            raise RuntimeError("m94-06 two-client visibility requires a secondary bridge")
        return run_two_client_visibility_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "m94-06-two-client-shared-drop":
        if secondary_client is None:
            raise RuntimeError("m94-06 two-client shared drop requires a secondary bridge")
        return run_two_client_shared_drop_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "m94-06-two-client-shared-pickup":
        if secondary_client is None:
            raise RuntimeError("m94-06 two-client shared pickup requires a secondary bridge")
        return run_two_client_shared_pickup_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-30-two-client-shared-log-drop-pickup":
        if secondary_client is None:
            raise RuntimeError("playable-30 two-client shared log drop/pickup requires a secondary bridge")
        return run_playable_two_client_shared_log_drop_pickup_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-31-two-client-earned-shared-chest":
        if secondary_client is None:
            raise RuntimeError("playable-31 two-client earned shared chest requires a secondary bridge")
        return run_playable_two_client_earned_shared_chest_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-45-two-client-shared-chest-save-restart-before":
        if secondary_client is None:
            raise RuntimeError("playable-45 before-restart phase requires a secondary bridge")
        return run_playable_two_client_shared_chest_save_restart_before_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-45-two-client-shared-chest-save-restart-after":
        if secondary_client is None:
            raise RuntimeError("playable-45 after-restart phase requires a secondary bridge")
        return run_playable_two_client_shared_chest_save_restart_after_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id in {
        "playable-46-generated-ruin-cache-before",
        "playable-46-generated-ruin-cache-after",
    }:
        return run_playable_generated_ruin_cache_scenario(
            client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-32-two-client-earned-torch-block-edit":
        if secondary_client is None:
            raise RuntimeError("playable-32 two-client earned torch block edit requires a secondary bridge")
        return run_playable_two_client_earned_torch_block_edit_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-33-two-client-player-visibility-movement":
        if secondary_client is None:
            raise RuntimeError("playable-33 two-client player visibility/movement requires a secondary bridge")
        return run_playable_two_client_player_visibility_movement_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-34-two-client-chat-message":
        if secondary_client is None:
            raise RuntimeError("playable-34 two-client chat message requires a secondary bridge")
        return run_playable_two_client_chat_message_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-35-two-client-player-disconnect-removal":
        if secondary_client is None:
            raise RuntimeError("playable-35 two-client player disconnect removal requires a secondary bridge")
        return run_playable_two_client_player_disconnect_removal_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-36-two-client-player-reconnect-cleanup":
        if secondary_client is None:
            raise RuntimeError("playable-36 two-client player reconnect cleanup requires a secondary bridge")
        return run_playable_two_client_player_reconnect_cleanup_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-37-two-client-player-death-respawn-visibility":
        if secondary_client is None:
            raise RuntimeError("playable-37 two-client player death/respawn visibility requires a secondary bridge")
        return run_playable_two_client_player_death_respawn_visibility_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-38-two-client-inventory-drop-handoff":
        if secondary_client is None:
            raise RuntimeError("playable-38 two-client inventory drop handoff requires a secondary bridge")
        return run_playable_two_client_inventory_drop_handoff_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-39-two-client-short-soak":
        if secondary_client is None:
            raise RuntimeError("playable-39 two-client short soak requires a secondary bridge")
        return run_playable_two_client_movement_soak_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
            observation_label="short soak",
            pulse_count=6,
            min_chunk_delta=0,
        )
    if scenario_id == "playable-40-two-client-chunk-stream-crossing":
        if secondary_client is None:
            raise RuntimeError("playable-40 two-client chunk-stream crossing requires a secondary bridge")
        return run_playable_two_client_movement_soak_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
            observation_label="chunk crossing",
            pulse_count=12,
            min_chunk_delta=1,
        )
    if scenario_id == "playable-41-two-client-chunk-prewarm-crossing":
        if secondary_client is None:
            raise RuntimeError("playable-41 two-client chunk prewarm crossing requires a secondary bridge")
        return run_playable_two_client_movement_soak_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
            observation_label="chunk prewarm crossing",
            pulse_count=12,
            min_chunk_delta=1,
        )
    if scenario_id == "playable-42-two-client-opposite-chunk-crossing":
        if secondary_client is None:
            raise RuntimeError("playable-42 two-client opposite chunk crossing requires a secondary bridge")
        return run_playable_two_client_movement_soak_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
            observation_label="opposite chunk crossing",
            pulse_count=12,
            min_chunk_delta=1,
            secondary_move_command="move_backward",
        )

    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(client, transcript, "ping", {}, timeout_seconds)
    wait_for_existing_or_explicit_connection(
        client,
        transcript,
        server_addr,
        timeout_seconds,
    )
    scenario_report = call_and_record(
        client,
        transcript,
        "run_scenario",
        {
            "id": scenario_id,
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
    )
    scenario_result = scenario_report.get("result")
    if scenario_result not in {"passed", "failed", "blocked"}:
        raise RuntimeError("run_scenario did not return passed, failed, or blocked")
    final_state = call_and_record(client, transcript, "state", {}, timeout_seconds)

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

    call_and_record(client, transcript, "disconnect", {}, timeout_seconds)
    reject_blocked_only_pass(scenario_id, scenario_result)
    return scenario_result, final_state, [screenshot_path.relative_to(run_dir).as_posix()], scenario_report

def run_playable_generated_ruin_cache_scenario(
    client: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(client, transcript, "ping", {}, timeout_seconds)
    wait_for_existing_or_explicit_connection(
        client,
        transcript,
        server_addr,
        timeout_seconds,
    )
    scenario_report = call_and_record(
        client,
        transcript,
        "run_scenario",
        {
            "id": scenario_id,
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
    )
    scenario_result = scenario_report_result(scenario_report)
    final_state = call_and_record(client, transcript, "state", {}, timeout_seconds)
    call_and_record(client, transcript, "disconnect", {}, timeout_seconds)
    reject_blocked_only_pass(scenario_id, scenario_result)
    return scenario_result, final_state, [], scenario_report
















def run_join_rejoin_movement_scenario(
    client: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)
    observations: list[str] = []
    screenshots: list[str] = []

    call_and_record(client, transcript, "ping", {}, timeout_seconds)
    initial_play = wait_for_existing_or_explicit_connection(
        client,
        transcript,
        server_addr,
        timeout_seconds,
    )
    observations.append(state_observation("initial play", initial_play))
    before_move = call_and_record(client, transcript, "state", {}, timeout_seconds)

    move_ticks = 15
    call_and_record(
        client,
        transcript,
        "move_forward",
        {"ticks": move_ticks},
        min(timeout_seconds, 10.0),
    )
    after_move = call_and_record(client, transcript, "state", {}, timeout_seconds)
    movement_distance = horizontal_distance(before_move, after_move)
    movement_passed = movement_distance >= 0.05
    observations.append(
        "movement probe: "
        + ("passed" if movement_passed else "failed")
        + f" ticks={move_ticks} horizontal_delta={movement_distance:.3f}"
    )

    screenshots.append(capture_screenshot(client, transcript, run_dir, screenshots_dir, f"{scenario_id}-after-move", timeout_seconds))
    call_and_record(client, transcript, "disconnect", {}, timeout_seconds)
    wait_until_not_in_play(client, transcript, timeout_seconds)
    observations.append(wait_for_server_session_release(run_dir, timeout_seconds))
    call_and_record(client, transcript, "connect", {"server_addr": server_addr}, timeout_seconds)
    rejoin_play = call_and_record(
        client,
        transcript,
        "wait_play",
        {"timeout_seconds": timeout_seconds},
        timeout_seconds,
    )
    rejoin_play = wait_for_interactive_play(
        client,
        transcript,
        rejoin_play,
        timeout_seconds,
    )
    if not is_in_play(rejoin_play):
        raise RuntimeError("client did not reach Play state after reconnect")
    observations.append(state_observation("rejoin play", rejoin_play))
    final_state = call_and_record(client, transcript, "state", {}, timeout_seconds)
    screenshots.append(capture_screenshot(client, transcript, run_dir, screenshots_dir, f"{scenario_id}-after-rejoin", timeout_seconds))
    call_and_record(client, transcript, "disconnect", {}, timeout_seconds)

    result = "passed" if movement_passed and is_in_play(initial_play) and is_in_play(rejoin_play) else "failed"
    scenario_report = {
        "result": result,
        "id": scenario_id,
        "observations": observations,
    }
    return result, final_state, screenshots, scenario_report



BLOCKED_ONLY_BROAD_SCENARIOS = {
    "m94-02-blocks-fluids-farming-drops",
    "m94-03-inventory-crafting-containers-stations",
    "m94-04-signs-beds-campfires-and-block-entities",
    "m94-05-entities-combat-death-respawn",
    "m94-06-save-restart-two-client-visibility",
    "m94-07-m40-m41-route-with-metrics",
}




def reject_blocked_only_pass(scenario_id: str, scenario_result: str) -> None:
    if scenario_result != "passed" or scenario_id not in BLOCKED_ONLY_BROAD_SCENARIOS:
        return
    raise RuntimeError(
        f"scenario {scenario_id} is blocked-only and must report blocked until broad M94 evidence is complete"
    )


def write_observations(
    run_dir: Path,
    scenario_id: str,
    transcript: list[dict[str, Any]],
    result: str,
    *,
    server_addr: str,
    final_state: dict[str, Any] | None = None,
    screenshots: list[str] | None = None,
    scenario_report: dict[str, Any] | None = None,
    error_message: str | None = None,
    append_observations: bool = False,
) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    quality_label = run_quality_label(run_dir)
    scenario: dict[str, Any] = {
        "id": scenario_id,
        "result": result,
        "commands": transcript,
        "final_state": final_state or {},
        "screenshots": screenshots or [],
    }
    if scenario_report is not None:
        scenario["agent_report"] = scenario_report
    observations: dict[str, Any] = {
        "schema": "solaris.real_client_observations.v1",
        "client_gate": "agent-run-real-client",
        "quality_label": quality_label,
        "result": result,
        "server_addr": server_addr,
        "generated_at": utc_now(),
        "scenarios": [scenario],
    }
    if error_message is not None:
        error_payload = {"message": error_message}
        observations["error"] = error_payload
        scenario["error"] = error_payload
    observations_path = run_dir / "observations.json"
    if append_observations:
        observations = append_to_existing_observations(observations_path, observations)
    observations_path.write_text(
        json.dumps(observations, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def run_quality_label(run_dir: Path) -> str:
    manifest_path = run_dir / "manifest.json"
    if not manifest_path.is_file():
        return "draft"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    quality_label = manifest.get("quality_label")
    if not isinstance(quality_label, str) or not quality_label:
        raise RuntimeError("manifest.json quality_label must be a non-empty string")
    return quality_label


def append_to_existing_observations(path: Path, next_observations: dict[str, Any]) -> dict[str, Any]:
    if not path.is_file():
        return next_observations

    existing = json.loads(path.read_text(encoding="utf-8"))
    for field in ["schema", "client_gate", "quality_label"]:
        if existing.get(field) != next_observations.get(field):
            raise RuntimeError(f"cannot append observations with mismatched {field}")

    existing_scenarios = existing.get("scenarios")
    next_scenarios = next_observations.get("scenarios")
    if not isinstance(existing_scenarios, list) or not isinstance(next_scenarios, list):
        raise RuntimeError("cannot append observations without scenario arrays")

    combined = dict(existing)
    combined["generated_at"] = next_observations["generated_at"]
    combined["server_addr"] = next_observations["server_addr"]
    combined["scenarios"] = existing_scenarios + next_scenarios
    combined["result"] = combine_results(
        scenario.get("result", "failed") for scenario in combined["scenarios"]
    )
    if "error" in next_observations:
        combined["error"] = next_observations["error"]
    return combined




def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Drive a loopback real-client bridge and write Solaris observations."
    )
    parser.add_argument("--bridge-url", required=True)
    parser.add_argument("--secret", required=True)
    parser.add_argument("--run-dir", required=True)
    parser.add_argument("--scenario", default="m94-02b-rejected-block-resync")
    parser.add_argument(
        "--replay-manifest",
        type=Path,
        help="Checked solaris.core_replay.scenario.v1 manifest for the real-client lane.",
    )
    parser.add_argument("--server-addr", required=True)
    parser.add_argument("--timeout-seconds", type=float, default=30.0)
    parser.add_argument("--secondary-bridge-url")
    parser.add_argument("--secondary-secret")
    parser.add_argument(
        "--append-observations",
        action="store_true",
        help="Append this scenario result to an existing observations.json and recompute the top-level result.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    run_dir = Path(args.run_dir)
    server_addr = args.server_addr
    transcript: list[dict[str, Any]] = []

    try:
        if not args.secret:
            raise ValueError("bridge secret must not be empty")
        if args.timeout_seconds <= 0:
            raise ValueError("timeout must be positive")
        server_addr = validate_loopback_server_addr(args.server_addr)
        replay_manifest = None
        if args.replay_manifest is not None:
            replay_manifest = load_core_replay_manifest(args.replay_manifest, args.scenario)
        client = AgentClient(args.bridge_url, args.secret)
        secondary_client = None
        if bool(args.secondary_bridge_url) != bool(args.secondary_secret):
            raise ValueError("secondary bridge URL and secret must be provided together")
        if args.secondary_bridge_url:
            secondary_client = AgentClient(args.secondary_bridge_url, args.secondary_secret)
        result, final_state, screenshots, scenario_report = run_bridge_scenario(
            client,
            run_dir,
            args.scenario,
            server_addr,
            args.timeout_seconds,
            transcript,
            secondary_client=secondary_client,
            replay_manifest=replay_manifest,
        )
        write_observations(
            run_dir,
            args.scenario,
            transcript,
            result,
            server_addr=server_addr,
            final_state=final_state,
            screenshots=screenshots,
            scenario_report=scenario_report,
            append_observations=args.append_observations,
        )
        print(f"wrote real-client agent observations: {run_dir / 'observations.json'}")
        return 0 if result == "passed" else 1
    except Exception as exc:
        write_observations(
            run_dir,
            args.scenario,
            transcript,
            "failed",
            server_addr=server_addr,
            error_message=str(exc),
            append_observations=args.append_observations,
        )
        print(f"real-client agent driver failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
