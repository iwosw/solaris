"""Real-client inventory menu market scenario."""

from __future__ import annotations

from pathlib import Path
from typing import Any

if __package__:
    from .wasm_inventory_probe import (
        ClientProbe,
        COMMITTED_OUTCOME,
        MCP_MENU_OBSERVATION_GAPS,
        MENU_MARKER_PREFIX,
        REFUSED_OUTCOME,
        TRADE_MARKER_PREFIX,
        TradeScenarioHooks,
        load_inventory_menu_expectation,
        require_int,
        require_mapping,
        require_str,
    )
else:
    from wasm_inventory_probe import (
        ClientProbe,
        COMMITTED_OUTCOME,
        MCP_MENU_OBSERVATION_GAPS,
        MENU_MARKER_PREFIX,
        REFUSED_OUTCOME,
        TRADE_MARKER_PREFIX,
        TradeScenarioHooks,
        load_inventory_menu_expectation,
        require_int,
        require_mapping,
        require_str,
    )

def run_wasm_inventory_menu_scenario(
    client: Any,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
    hooks: TradeScenarioHooks,
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    """Drive the server-owned market window through the real client's menu surface.

    Every trade phase opens the window it needs and clicks the window's own slot
    through a real container click; the guest's marker only fences that request,
    so the committed/refused outcome is accepted together with the client-visible
    counts it implies. The stale probes keep the live next-menu open while the
    guest is asked to close a foreign menu and to act on the session the reconnect
    left behind, and a current-session purchase on that same kept window then
    proves it is still live.
    """

    expectation = load_inventory_menu_expectation(run_dir, scenario_id)
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
    menu_marker_prefix = require_str(expectation["menu_marker_prefix"], "menu marker prefix")
    if menu_marker_prefix != MENU_MARKER_PREFIX:
        raise ValueError(f"{scenario_id} menu marker prefix must be {MENU_MARKER_PREFIX}")
    menu_button_slot_count = require_int(
        expectation["menu_button_slot_count"], "menu button slot count"
    )
    container_slot_count = require_int(
        expectation["menu_container_slot_count"], "menu container slot count"
    )
    setup = require_mapping(expectation["setup"], "setup")
    market = require_mapping(expectation["market"], "market window")
    next_market = require_mapping(expectation["next_market"], "next market window")
    trades = expectation["trades"]
    if not isinstance(trades, list) or not trades:
        raise ValueError(f"{scenario_id} trades must be a non-empty list")
    close_slot = require_mapping(expectation["close_slot"], "close slot phase")
    close_request = require_mapping(expectation["close_request"], "close request phase")
    rejoin = require_mapping(expectation["rejoin"], "rejoin phase")
    stale_close = require_mapping(expectation["stale_close"], "stale close phase")
    stale_session = require_mapping(expectation["stale_session"], "stale session phase")
    live_purchase = require_mapping(expectation["live_purchase"], "live purchase phase")

    emerald = require_mapping(setup["emerald"], "setup emerald")
    apple = require_mapping(setup["apple"], "setup apple")
    tool = require_mapping(setup["tool"], "setup tool")
    emerald_item_id = require_str(emerald["item_id"], "emerald item id")
    apple_item_id = require_str(apple["item_id"], "apple item id")

    probe = ClientProbe(
        client=client,
        transcript=transcript,
        hooks=hooks,
        scenario_id=scenario_id,
        run_dir=run_dir,
        screenshots_dir=screenshots_dir,
        timeout_seconds=timeout_seconds,
    )
    phases: list[dict[str, Any]] = []
    menu_phases: list[dict[str, Any]] = []
    adversarial_checks: list[dict[str, Any]] = []
    screenshots: list[str] = []

    def record_adversarial(name: Any, label: str, detail: dict[str, Any]) -> None:
        adversarial_checks.append(
            {"check": require_str(name, f"{label} adversarial check"), **detail}
        )

    def inventory_counts(observation: dict[str, Any]) -> dict[str, int]:
        return {
            emerald_item_id: probe.item_count(observation, emerald_item_id),
            apple_item_id: probe.item_count(observation, apple_item_id),
        }

    def run_menu_trade(
        phase: dict[str, Any],
        label: str,
        menu: dict[str, Any],
        *,
        already_open: bool,
    ) -> dict[str, Any]:
        slot = require_int(phase["slot"], f"{label} slot")
        button = require_str(phase["button"], f"{label} click button")
        outcome = require_str(phase["outcome"], f"{label} outcome")
        if outcome not in {COMMITTED_OUTCOME, REFUSED_OUTCOME}:
            raise ValueError(f"{label} outcome must be {COMMITTED_OUTCOME} or {REFUSED_OUTCOME}")
        ledger = require_int(phase["ledger"], f"{label} ledger value")
        expected_marker = require_str(phase["marker"], f"{label} marker")
        if f"ledger={ledger}" not in expected_marker:
            raise ValueError(f"{label} marker {expected_marker!r} does not carry ledger={ledger}")
        expected_counts = (
            (emerald_item_id, require_int(phase["emerald"], f"{label} emerald count")),
            (apple_item_id, require_int(phase["apple"], f"{label} apple count")),
        )
        before = probe.observe_client()
        before_inventory = probe.inventory_fingerprint(before)
        before_tool = probe.tool_fingerprint(before, tool_item_id)
        if already_open:
            opened = before
            request: dict[str, Any] = {
                "command": None,
                "window": menu.get("id"),
                "request_only": "the previous phase opened this window and kept it open",
            }
        else:
            opened, request = probe.open_menu(command_root, menu, label)
        menu_fingerprint = probe.require_menu(
            opened,
            menu,
            menu_button_slot_count,
            container_slot_count,
            f"{label} window",
        )
        menu_capture = phase.get("menu_capture")
        if menu_capture is not None:
            screenshots.append(
                probe.capture_screen(require_str(menu_capture, f"{label} menu capture"))
            )
        baseline = probe.marker_baseline()
        click = probe.click_container_slot(slot, button, f"{label} slot {slot} {button} click")
        settled, evidence = probe.require_trade_settlement(
            label=label,
            expected_marker=expected_marker,
            outcome=outcome,
            expected_counts=expected_counts,
            baseline=baseline,
            before_inventory=before_inventory,
            before_tool=before_tool,
            initial_tool=initial_tool,
        )
        # Every trade click closes its accepted window before the transaction, so
        # the client must hold no server-owned window once the marker is out.
        probe.wait_for_closed_screen(f"{label} accepted window close")
        record = {
            "label": label,
            "menu": menu_fingerprint,
            "request": request,
            "click": click,
            "outcome": outcome,
            "ledger": ledger,
            "window_closed": True,
            **evidence,
        }
        phases.append(record)
        capture_name = phase.get("capture")
        if capture_name is not None:
            screenshots.append(
                probe.capture_inventory_screenshot(
                    require_str(capture_name, f"{label} capture name")
                )
            )
        adversarial = phase.get("adversarial")
        if adversarial is not None:
            record_adversarial(
                adversarial,
                label,
                {
                    "command": None,
                    "click": {"slot": slot, "button": button},
                    "expected": expected_marker,
                    "observed_marker": evidence["fresh_markers"][0],
                    "inventory": evidence["inventory"],
                    "inventory_unchanged": evidence["inventory_unchanged"],
                    "tool_fingerprint_unchanged": evidence["tool_fingerprint_unchanged"],
                    "window_closed": True,
                },
            )
        return record

    def run_close_slot_phase(phase: dict[str, Any], label: str) -> dict[str, Any]:
        slot = require_int(phase["slot"], f"{label} slot")
        button = require_str(phase["button"], f"{label} click button")
        fence = require_str(phase["fence"], f"{label} close fence")
        opened, request = probe.open_menu(command_root, market, label)
        menu_fingerprint = probe.require_menu(
            opened, market, menu_button_slot_count, container_slot_count, f"{label} window"
        )
        screenshots.append(
            probe.capture_screen(require_str(phase["menu_capture"], f"{label} menu capture"))
        )
        before_inventory = probe.inventory_fingerprint(probe.observe_client())
        baseline = probe.marker_baseline()
        click = probe.click_container_slot(slot, button, f"{label} slot {slot} {button} click")
        probe.wait_for_fresh_marker(fence, f"{label} request marker {fence!r}", baseline)
        closed = probe.wait_for_closed_screen(f"{label} window close")
        counts = inventory_counts(closed)
        if probe.inventory_fingerprint(closed) != before_inventory:
            raise RuntimeError(f"{label} changed the client-visible inventory while closing")
        record = {
            "label": label,
            "menu": menu_fingerprint,
            "request": request,
            "click": click,
            "request_marker": fence,
            "request_only": "the marker fences the close request; the closed window is the effect",
            "window_closed": True,
            "inventory": counts,
            "inventory_unchanged": True,
        }
        phases.append(record)
        screenshots.append(
            probe.capture_inventory_screenshot(
                require_str(phase["capture"], f"{label} capture name")
            )
        )
        adversarial = phase.get("adversarial")
        if adversarial is not None:
            record_adversarial(
                adversarial,
                label,
                {
                    "command": None,
                    "click": {"slot": slot, "button": button},
                    "expected": "the window closes without touching the inventory",
                    "request_marker": fence,
                    "window_closed": True,
                    "inventory": counts,
                    "inventory_unchanged": True,
                },
            )
        return record

    def run_close_request_phase(phase: dict[str, Any], label: str) -> dict[str, Any]:
        action = require_str(phase["action"], f"{label} close action")
        fence = require_str(phase["fence"], f"{label} close fence")
        opened, open_request = probe.open_menu(command_root, market, label)
        menu_fingerprint = probe.require_menu(
            opened, market, menu_button_slot_count, container_slot_count, f"{label} window"
        )
        screenshots.append(
            probe.capture_screen(require_str(phase["menu_capture"], f"{label} menu capture"))
        )
        before_inventory = probe.inventory_fingerprint(probe.observe_client())
        closed, request = probe.request_close(command_root, action, fence, label)
        counts = inventory_counts(closed)
        if probe.inventory_fingerprint(closed) != before_inventory:
            raise RuntimeError(f"{label} changed the client-visible inventory while closing")
        record = {
            "label": label,
            "menu": menu_fingerprint,
            "open_request": open_request,
            "request": request,
            "window_closed": True,
            "inventory": counts,
            "inventory_unchanged": True,
        }
        phases.append(record)
        screenshots.append(
            probe.capture_inventory_screenshot(
                require_str(phase["capture"], f"{label} capture name")
            )
        )
        adversarial = phase.get("adversarial")
        if adversarial is not None:
            record_adversarial(
                adversarial,
                label,
                {
                    "command": request["command"],
                    "expected": "the window closes without touching the inventory",
                    "request_marker": fence,
                    "window_closed": True,
                    "inventory": counts,
                    "inventory_unchanged": True,
                },
            )
        return record

    def run_stale_phase(
        phase: dict[str, Any],
        label: str,
        menu: dict[str, Any],
    ) -> dict[str, Any]:
        action = require_str(phase["action"], f"{label} action")
        fence = require_str(phase["fence"], f"{label} request fence")
        before = probe.observe_client()
        before_menu = probe.require_menu(
            before, menu, menu_button_slot_count, container_slot_count, f"{label} live window"
        )
        before_inventory = probe.inventory_fingerprint(before)
        baseline = probe.marker_baseline()
        command = f"{command_root} {action}"
        probe.submit_chat(command)
        probe.wait_for_fresh_marker(fence, f"{label} request marker {fence!r}", baseline)
        after = probe.observe_client()
        after_menu = probe.require_menu(
            after,
            menu,
            menu_button_slot_count,
            container_slot_count,
            f"{label} live window after the stale request",
        )
        if after_menu != before_menu:
            raise RuntimeError(
                f"{label} replaced the live window: {before_menu} -> {after_menu}"
            )
        counts = inventory_counts(after)
        if probe.inventory_fingerprint(after) != before_inventory:
            raise RuntimeError(f"{label} changed the client-visible inventory")
        record = {
            "label": label,
            "action": action,
            "command": "/" + command,
            "request_marker": fence,
            "request_only": "the marker fences the guest's request; the live window is the effect",
            "window": after_menu,
            "window_unchanged": True,
            "inventory": counts,
            "inventory_unchanged": True,
            "scope": (
                "observed continuity of the live client state; wire-level refusal of a stale "
                "menu or session belongs to the native component tests"
            ),
        }
        menu_phases.append(record)
        screenshots.append(
            probe.capture_screen(require_str(phase["capture"], f"{label} capture name"))
        )
        adversarial = phase.get("adversarial")
        if adversarial is not None:
            record_adversarial(
                adversarial,
                label,
                {
                    "command": "/" + command,
                    "expected": "the live window and inventory stay exactly as they were",
                    "request_marker": fence,
                    "window": after_menu,
                    "window_unchanged": True,
                    "inventory": counts,
                    "inventory_unchanged": True,
                },
            )
        return record

    probe.call("ping", {})
    play = hooks.connect_or_confirm_play(client, transcript, server_addr, timeout_seconds)
    if not hooks.is_in_play(play):
        raise RuntimeError("the real client did not enter Play")
    joined = probe.observe_client()
    player = joined.get("player")
    if not isinstance(player, dict) or player.get("name") != player_username:
        raise RuntimeError(
            f"unexpected client identity: {player.get('name') if isinstance(player, dict) else player!r}"
        )

    seeded = probe.seed_trade_fixture(setup, emerald, apple, tool)
    tool_item_id = seeded["tool_item_id"]
    initial_tool = seeded["tool_fingerprint"]
    screenshots.append(seeded["screenshot"])

    for index, phase in enumerate(trades):
        entry = require_mapping(phase, f"trade phase {index}")
        run_menu_trade(
            entry,
            require_str(entry.get("label", f"trade[{index}]"), f"trade phase {index} label"),
            market,
            already_open=False,
        )

    run_close_slot_phase(close_slot, "close-slot")
    run_close_request_phase(close_request, "close-request")

    rejoin_expected_emerald = require_int(rejoin["emerald"], "rejoin emerald count")
    rejoin_expected_apple = require_int(rejoin["apple"], "rejoin apple count")
    before_rejoin = probe.observe_client()
    pre_rejoin_emerald = probe.item_count(before_rejoin, emerald_item_id)
    pre_rejoin_apple = probe.item_count(before_rejoin, apple_item_id)
    rejoin_info = probe.reconnect(server_addr, player_username)
    probe.await_inventory_counts(
        (emerald_item_id, rejoin_expected_emerald),
        (apple_item_id, rejoin_expected_apple),
    )
    rejoined = probe.observe_client()
    observed_rejoin_emerald = probe.item_count(rejoined, emerald_item_id)
    observed_rejoin_apple = probe.item_count(rejoined, apple_item_id)
    if (observed_rejoin_emerald, observed_rejoin_apple) != (pre_rejoin_emerald, pre_rejoin_apple):
        raise RuntimeError(
            "inventory did not survive the disconnect/rejoin: "
            f"before={pre_rejoin_emerald}/{pre_rejoin_apple} "
            f"after={observed_rejoin_emerald}/{observed_rejoin_apple}"
        )
    rejoin_tool = probe.tool_fingerprint(rejoined, tool_item_id)
    if rejoin_tool != initial_tool:
        raise RuntimeError(
            f"the component-bearing tool changed across the disconnect/rejoin: {rejoin_tool}"
        )
    screenshots.append(
        probe.capture_inventory_screenshot(require_str(rejoin["capture"], "rejoin capture name"))
    )
    rejoin_record = {
        "session_release": rejoin_info["session_release"],
        "inventory": {
            emerald_item_id: observed_rejoin_emerald,
            apple_item_id: observed_rejoin_apple,
        },
        "tool_fingerprint": rejoin_tool,
        "screenshot": screenshots[-1],
    }

    opened, next_open_request = probe.open_menu(command_root, next_market, "next-market")
    next_fingerprint = probe.require_menu(
        opened, next_market, menu_button_slot_count, container_slot_count, "next-market window"
    )
    screenshots.append(probe.capture_screen("next-menu-open"))
    menu_phases.append(
        {
            "label": "next-market",
            "request": next_open_request,
            "window": next_fingerprint,
            "window_kept_open": True,
        }
    )

    run_stale_phase(stale_close, "stale-menu-close", next_market)
    run_stale_phase(stale_session, "stale-session-menu", next_market)

    live_record = run_menu_trade(
        live_purchase,
        "live-session-purchase",
        next_market,
        already_open=True,
    )

    final_state = probe.call("state", {})
    probe.call("disconnect", {})

    scenario_report = {
        "result": "passed",
        "id": scenario_id,
        "package_id": package_id,
        "mode": mode,
        "player": player_username,
        "command_root": "/" + command_root,
        "ledger_key": ledger_key,
        "trade_marker_prefix": marker_prefix,
        "menu_marker_prefix": menu_marker_prefix,
        "setup": {
            "commands": seeded["commands"],
            "initial_inventory": seeded["inventory"],
            "tool_item_id": tool_item_id,
            "tool_fingerprint": initial_tool,
            "durability_attack": seeded["durability_attack"],
        },
        "phases": phases,
        "menu_phases": menu_phases,
        "rejoin": rejoin_record,
        "live_session_purchase": live_record,
        "adversarial_checks": adversarial_checks,
        "screenshots": screenshots,
        "mcp_observation_gaps": list(MCP_MENU_OBSERVATION_GAPS),
        "sibling_scenario": (
            "wasm-p3-inventory-storage stays the command-only /trade scenario with its own "
            "manifest; this mode reuses the same fixture without changing it"
        ),
    }
    return "passed", final_state, screenshots, scenario_report
