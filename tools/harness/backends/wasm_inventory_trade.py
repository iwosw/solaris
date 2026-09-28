"""Real-client inventory/storage trade scenario."""

from __future__ import annotations

from pathlib import Path
from typing import Any

if __package__:
    from .wasm_inventory_probe import (
        ClientProbe,
        COMMITTED_OUTCOME,
        MCP_OBSERVATION_GAPS,
        REFUSED_OUTCOME,
        TRADE_MARKER_PREFIX,
        TradeScenarioHooks,
        load_inventory_storage_expectation,
        require_int,
        require_mapping,
        require_str,
    )
else:
    from wasm_inventory_probe import (
        ClientProbe,
        COMMITTED_OUTCOME,
        MCP_OBSERVATION_GAPS,
        REFUSED_OUTCOME,
        TRADE_MARKER_PREFIX,
        TradeScenarioHooks,
        load_inventory_storage_expectation,
        require_int,
        require_mapping,
        require_str,
    )

def run_wasm_inventory_storage_scenario(
    client: Any,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
    hooks: TradeScenarioHooks,
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    expectation = load_inventory_storage_expectation(run_dir, scenario_id)
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    package_id = require_str(expectation["package_id"], "package id")
    mode = require_str(expectation["mode"], "fixture mode")
    player_username = require_str(expectation["player_username"], "player username")
    command_root = require_str(expectation["command_root"], "command root")
    ledger_key = require_str(expectation["ledger_key"], "ledger key")
    marker_prefix = require_str(expectation["marker_prefix"], "marker prefix")
    if marker_prefix != TRADE_MARKER_PREFIX:
        raise ValueError(f"{scenario_id} trade marker prefix must be {TRADE_MARKER_PREFIX}")
    setup = require_mapping(expectation["setup"], "setup")
    sequence = expectation["sequence"]
    if not isinstance(sequence, list) or not sequence:
        raise ValueError(f"{scenario_id} sequence must be a non-empty list")
    rejoin = require_mapping(expectation["rejoin"], "rejoin phase")
    rejoin_expected_emerald = require_int(rejoin["emerald"], "rejoin emerald count")
    rejoin_expected_apple = require_int(rejoin["apple"], "rejoin apple count")

    emerald = require_mapping(setup["emerald"], "setup emerald")
    apple = require_mapping(setup["apple"], "setup apple")
    tool = require_mapping(setup["tool"], "setup tool")
    emerald_item_id = require_str(emerald["item_id"], "emerald item id")
    apple_item_id = require_str(apple["item_id"], "apple item id")
    tool_item_id = require_str(tool["item_id"], "tool item id")

    probe = ClientProbe(
        client=client,
        transcript=transcript,
        hooks=hooks,
        scenario_id=scenario_id,
        run_dir=run_dir,
        screenshots_dir=screenshots_dir,
        timeout_seconds=timeout_seconds,
    )
    # The phase logic below keeps the probe's own names for the primitives it
    # reads, so it stays a description of the trade flow.
    call = probe.call
    recent_chat = probe.recent_chat
    line_counts = probe.line_counts
    observe_client = probe.observe_client
    submit_chat = probe.submit_chat
    inventory_fingerprint = probe.inventory_fingerprint
    item_count = probe.item_count
    tool_fingerprint = probe.tool_fingerprint
    await_inventory_counts = probe.await_inventory_counts
    capture_inventory_screenshot = probe.capture_inventory_screenshot

    phases: list[dict[str, Any]] = []
    adversarial_checks: list[dict[str, Any]] = []
    screenshots: list[str] = []

    def run_trade_phase(phase: dict[str, Any], label: str) -> dict[str, Any]:
        action = require_str(phase["action"], f"{label} action")
        args = require_str(phase["args"], f"{label} command arguments")
        outcome = require_str(phase["outcome"], f"{label} outcome")
        if outcome not in {COMMITTED_OUTCOME, REFUSED_OUTCOME}:
            raise ValueError(
                f"{label} outcome must be {COMMITTED_OUTCOME} or {REFUSED_OUTCOME}"
            )
        ledger = require_int(phase["ledger"], f"{label} ledger value")
        expected_emerald = require_int(phase["emerald"], f"{label} emerald count")
        expected_apple = require_int(phase["apple"], f"{label} apple count")
        before = observe_client()
        before_inventory = inventory_fingerprint(before)
        before_tool = tool_fingerprint(before, tool_item_id)
        baseline = line_counts(recent_chat(before))
        command = f"{command_root} {args}"
        submit_chat(command)
        settled, evidence = probe.require_trade_settlement(
            label=label,
            expected_marker=f"{marker_prefix} {action} {outcome} ledger={ledger}",
            outcome=outcome,
            expected_counts=(
                (emerald_item_id, expected_emerald),
                (apple_item_id, expected_apple),
            ),
            baseline=baseline,
            before_inventory=before_inventory,
            before_tool=before_tool,
            initial_tool=initial_tool,
        )
        record = {
            "label": label,
            "action": action,
            "command": "/" + command,
            "outcome": outcome,
            "ledger": ledger,
            **evidence,
        }
        phases.append(record)
        capture_name = phase.get("capture")
        if capture_name is not None:
            screenshots.append(
                capture_inventory_screenshot(require_str(capture_name, f"{label} capture name"))
            )
        adversarial = phase.get("adversarial")
        if adversarial is not None:
            adversarial_checks.append(
                {
                    "check": require_str(adversarial, f"{label} adversarial check"),
                    "command": "/" + command,
                    "expected": f"{outcome} ledger={ledger}",
                    "observed_marker": evidence["fresh_markers"][0],
                    "inventory": evidence["inventory"],
                    "inventory_unchanged": evidence["inventory_unchanged"],
                    "tool_fingerprint_unchanged": evidence["tool_fingerprint_unchanged"],
                }
            )
        return settled

    call("ping", {})
    play = hooks.connect_or_confirm_play(client, transcript, server_addr, timeout_seconds)
    if not hooks.is_in_play(play):
        raise RuntimeError("the real client did not enter Play")
    joined = observe_client()
    player = joined.get("player")
    if not isinstance(player, dict) or player.get("name") != player_username:
        raise RuntimeError(
            f"unexpected client identity: {player.get('name') if isinstance(player, dict) else player!r}"
        )

    seeded = probe.seed_trade_fixture(setup, emerald, apple, tool)
    setup_commands = seeded["commands"]
    initial_tool = seeded["tool_fingerprint"]
    screenshots.append(seeded["screenshot"])

    for index, phase in enumerate(sequence):
        run_trade_phase(require_mapping(phase, f"sequence phase {index}"), f"sequence[{index}]")

    pre_rejoin = observe_client()
    pre_rejoin_emerald = item_count(pre_rejoin, emerald_item_id)
    pre_rejoin_apple = item_count(pre_rejoin, apple_item_id)
    rejoin_info = probe.reconnect(server_addr, player_username)
    session_release = rejoin_info["session_release"]
    await_inventory_counts(
        (emerald_item_id, rejoin_expected_emerald),
        (apple_item_id, rejoin_expected_apple),
    )
    rejoined = observe_client()
    observed_rejoin_emerald = item_count(rejoined, emerald_item_id)
    observed_rejoin_apple = item_count(rejoined, apple_item_id)
    if (observed_rejoin_emerald, observed_rejoin_apple) != (pre_rejoin_emerald, pre_rejoin_apple):
        raise RuntimeError(
            "inventory did not survive the disconnect/rejoin: "
            f"before={pre_rejoin_emerald}/{pre_rejoin_apple} "
            f"after={observed_rejoin_emerald}/{observed_rejoin_apple}"
        )
    rejoin_tool = tool_fingerprint(rejoined, tool_item_id)
    if rejoin_tool != initial_tool:
        raise RuntimeError(
            f"the component-bearing tool changed across the disconnect/rejoin: {rejoin_tool}"
        )
    screenshots.append(capture_inventory_screenshot("rejoin-inventory"))

    run_trade_phase(rejoin, "rejoin")
    run_trade_phase(
        require_mapping(expectation["live_session_probe"], "live session probe"),
        "live-session-after-refusal",
    )

    final_state = call("state", {})
    call("disconnect", {})

    scenario_report = {
        "result": "passed",
        "id": scenario_id,
        "package_id": package_id,
        "mode": mode,
        "player": player_username,
        "command_root": "/" + command_root,
        "ledger_key": ledger_key,
        "setup": {
            "commands": setup_commands,
            "initial_inventory": seeded["inventory"],
            "tool_item_id": seeded["tool_item_id"],
            "tool_fingerprint": initial_tool,
            "durability_attack": seeded["durability_attack"],
        },
        "phases": phases,
        "rejoin": {
            "session_release": session_release,
            "inventory": {
                emerald_item_id: observed_rejoin_emerald,
                apple_item_id: observed_rejoin_apple,
            },
            "tool_fingerprint": rejoin_tool,
        },
        "adversarial_checks": adversarial_checks,
        "screenshots": screenshots,
        "mcp_observation_gaps": list(MCP_OBSERVATION_GAPS),
    }
    return "passed", final_state, screenshots, scenario_report
