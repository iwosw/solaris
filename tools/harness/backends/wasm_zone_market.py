"""Real-client zone crossing and market scenario."""

from __future__ import annotations

import math
from pathlib import Path
from typing import Any

if __package__:
    from .wasm_inventory_probe import (
        ClientProbe,
        COMMITTED_OUTCOME,
        EXIT_CYCLE_KIND,
        REFUSED_OUTCOME,
        MCP_ZONE_OBSERVATION_GAPS,
        TRADE_CYCLE_KIND,
        TRADE_MARKER_PREFIX,
        TELEPORT_EXIT_ROUTE,
        WALK_EXIT_ROUTE,
        ZONE_MARKER_PREFIX,
        TradeScenarioHooks,
        crossing_axis,
        crossing_yaw,
        face_travel,
        load_zone_market_expectation,
        point_within_box,
        require_box,
        require_float,
        require_int,
        require_mapping,
        require_position,
        require_str,
    )
else:
    from wasm_inventory_probe import (
        ClientProbe,
        COMMITTED_OUTCOME,
        EXIT_CYCLE_KIND,
        REFUSED_OUTCOME,
        MCP_ZONE_OBSERVATION_GAPS,
        TRADE_CYCLE_KIND,
        TRADE_MARKER_PREFIX,
        TELEPORT_EXIT_ROUTE,
        WALK_EXIT_ROUTE,
        ZONE_MARKER_PREFIX,
        TradeScenarioHooks,
        crossing_axis,
        crossing_yaw,
        face_travel,
        load_zone_market_expectation,
        point_within_box,
        require_box,
        require_float,
        require_int,
        require_mapping,
        require_position,
        require_str,
    )

def run_wasm_zone_market_scenario(
    client: Any,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
    hooks: TradeScenarioHooks,
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    """Drive the market through real zone boundary crossings.

    The fixture creates one fixed box, and every phase crosses its face for real:
    ordinary forward input walks in and out, and the one crossing that leaves with
    the window still open is an operator ``/tp`` plus the client's own position
    report, because a container screen swallows ordinary movement input. Each
    crossing is asserted three ways - the client's own position past the declared
    face, the fixture's own fence marker, and the window that opens or closes as
    its effect - so a marker is never accepted as an effect and the exit-driven
    close is proven by the same window the entry opened.
    """

    expectation = load_zone_market_expectation(run_dir, scenario_id)
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    package_id = require_str(expectation["package_id"], "package id")
    mode = require_str(expectation["mode"], "fixture mode")
    if mode != "zone-market":
        raise ValueError(f"{scenario_id} must run the zone-market fixture mode, not {mode!r}")
    player_username = require_str(expectation["player_username"], "player username")
    command_root = require_str(expectation["command_root"], "command root")
    ledger_key = require_str(expectation["ledger_key"], "ledger key")
    marker_prefix = require_str(expectation["marker_prefix"], "marker prefix")
    if marker_prefix != TRADE_MARKER_PREFIX:
        raise ValueError(f"{scenario_id} trade marker prefix must be {TRADE_MARKER_PREFIX}")
    zone_marker_prefix = require_str(expectation["zone_marker_prefix"], "zone marker prefix")
    if zone_marker_prefix != ZONE_MARKER_PREFIX:
        raise ValueError(f"{scenario_id} zone marker prefix must be {ZONE_MARKER_PREFIX}")
    menu_button_slot_count = require_int(
        expectation["menu_button_slot_count"], "menu button slot count"
    )
    container_slot_count = require_int(
        expectation["menu_container_slot_count"], "menu container slot count"
    )
    zone = require_mapping(expectation["zone"], "zone definition")
    zone_id = require_str(zone["id"], "zone id")
    dimension = require_str(zone["dimension"], "zone dimension")
    box = require_box(zone, f"zone {zone_id}")
    setup_action = require_str(zone["setup_action"], f"{zone_id} setup action")
    ready_marker = require_str(zone["ready_marker"], f"{zone_id} ready marker")
    entered_marker = require_str(zone["entered_marker"], f"{zone_id} entered marker")
    exited_marker = require_str(zone["exited_marker"], f"{zone_id} exited marker")
    for label, marker in (
        ("ready", ready_marker),
        ("entered", entered_marker),
        ("exited", exited_marker),
    ):
        if not marker.startswith(ZONE_MARKER_PREFIX + " "):
            raise ValueError(
                f"{zone_id} {label} marker {marker!r} does not begin with {ZONE_MARKER_PREFIX}"
            )
    positions = require_mapping(expectation["positions"], "zone positions")
    outside = require_position(positions["outside"], "outside position")
    inside = require_position(positions["inside"], "inside position")
    teleport_command = require_str(positions["teleport_command"], "outside teleport command")
    landing_tolerance = require_float(positions["landing_tolerance"], "landing tolerance")
    axis = crossing_axis(outside, inside)
    terrain = require_mapping(expectation["terrain"], "terrain setup")
    terrain_commands = terrain.get("commands")
    if not isinstance(terrain_commands, list) or not terrain_commands:
        raise ValueError(f"{scenario_id} terrain commands must be a non-empty list")
    walkway = require_mapping(terrain.get("walkway"), "terrain walkway")
    walkway = {
        "walk_y": require_int(walkway["walk_y"], "walkway walk y"),
        "surface_y": require_int(walkway["surface_y"], "walkway surface y"),
        "z": require_int(walkway["z"], "walkway z"),
        "x_min": require_int(walkway["x_min"], "walkway x min"),
        "x_max": require_int(walkway["x_max"], "walkway x max"),
    }
    if walkway["surface_y"] != walkway["walk_y"] + 1:
        raise ValueError(f"{scenario_id} walkway surface must sit directly on its walk layer")
    if walkway["x_min"] > walkway["x_max"]:
        raise ValueError(f"{scenario_id} walkway x range is empty")
    if walkway["z"] != math.floor(outside[2]) or walkway["z"] != math.floor(inside[2]):
        raise ValueError(
            f"{scenario_id} walkway z {walkway['z']} is not the block row the declared outside "
            f"{outside} and inside {inside} stand on"
        )
    if not (walkway["x_min"] <= math.floor(outside[0]) and walkway["x_max"] >= math.floor(inside[0])):
        raise ValueError(
            f"{scenario_id} walkway x {walkway['x_min']}..{walkway['x_max']} does not span the "
            f"declared crossing from {outside} to {inside}"
        )
    crossing = require_mapping(expectation["crossing"], "crossing")
    step_ticks = require_int(crossing["step_ticks"], "crossing step ticks")
    max_steps = require_int(crossing["max_steps"], "crossing max steps")
    margin = require_float(crossing["margin"], "crossing margin")
    confirm_offset_cm = require_int(crossing["confirm_offset_cm"], "crossing confirm offset")
    setup = require_mapping(expectation["setup"], "setup")
    market = require_mapping(expectation["market"], "market window")
    cycles = expectation["cycles"]
    if not isinstance(cycles, list) or not cycles:
        raise ValueError(f"{scenario_id} cycles must be a non-empty list")
    reconnect = require_mapping(expectation["reconnect"], "reconnect phase")

    emerald = require_mapping(setup["emerald"], "setup emerald")
    apple = require_mapping(setup["apple"], "setup apple")
    tool = require_mapping(setup["tool"], "setup tool")
    emerald_item_id = require_str(emerald["item_id"], "emerald item id")
    apple_item_id = require_str(apple["item_id"], "apple item id")
    market_title = require_str(market["title"], "market title")

    # The face the walk crosses and the direction a leaving walk faces: the
    # declared outside point is the one fixed reference the thresholds use.
    # Entry opens a container immediately and consumes further movement input.
    # Crossing the real face is sufficient; extra interior travel is not required.
    entry_threshold = face_travel(outside, box, axis)
    exit_threshold = face_travel(outside, box, axis) - margin
    exit_axis = (-axis[0], -axis[1])

    probe = ClientProbe(
        client=client,
        transcript=transcript,
        hooks=hooks,
        scenario_id=scenario_id,
        run_dir=run_dir,
        screenshots_dir=screenshots_dir,
        timeout_seconds=timeout_seconds,
    )
    setup_commands: list[dict[str, Any]] = []
    cycles_records: list[dict[str, Any]] = []
    adversarial_checks: list[dict[str, Any]] = []
    screenshots: list[str] = []

    def record_adversarial(name: Any, label: str, detail: dict[str, Any]) -> None:
        adversarial_checks.append(
            {"check": require_str(name, f"{label} adversarial check"), **detail}
        )

    def window_record(observation: dict[str, Any], description: str) -> dict[str, Any]:
        return probe.require_menu(
            observation,
            market,
            menu_button_slot_count,
            container_slot_count,
            description,
        )

    def enter_zone(label: str) -> dict[str, Any]:
        """Cross the face by walking and require the window the entry opened."""

        baseline = probe.marker_baseline()
        crossed, steps = probe.walk_across(
            outside=outside,
            axis=axis,
            threshold=entry_threshold,
            entered=True,
            facing=axis,
            step_ticks=step_ticks,
            max_steps=max_steps,
            description=f"{label} entry walk",
        )
        position = probe.player_position(crossed)
        if not point_within_box(position, box):
            raise RuntimeError(
                f"{label} walked to {position}, which the declared box {box} does not contain"
            )
        probe.require_fresh_prefix_marker(
            zone_marker_prefix,
            entered_marker,
            f"{label} entry fence",
            baseline,
        )
        opened = probe.wait_for_menu(market_title, f"{label} entry window")
        window = window_record(opened, f"{label} entry window")
        return {
            "position": position,
            "movement_steps": steps,
            "marker": entered_marker,
            "window": window,
        }

    def exit_zone(label: str, route: str, window_open: bool) -> dict[str, Any]:
        """Cross the face leaving and require the window this fixture holds closed."""

        if route not in {WALK_EXIT_ROUTE, TELEPORT_EXIT_ROUTE}:
            raise ValueError(
                f"{label} exit route must be {WALK_EXIT_ROUTE} or {TELEPORT_EXIT_ROUTE}"
            )
        before = probe.observe_client()
        held = window_record(before, f"{label} window before the exit") if window_open else None
        baseline = probe.marker_baseline()
        confirmation: dict[str, Any] | None = None
        steps: int | None = None
        if route == TELEPORT_EXIT_ROUTE:
            # A container screen swallows ordinary movement input, so this one
            # crossing is the operator teleport plus the client's own report of
            # where it now stands. The close below is the fixture's exit-driven
            # close, not a screen the driver dismissed.
            probe.submit_chat(teleport_command)
            crossed = probe.await_crossing(
                outside=outside,
                axis=axis,
                threshold=exit_threshold,
                entered=False,
                description=f"{label} teleport landing",
            )
            confirmation = probe.confirm_position(
                -axis[0] * confirm_offset_cm,
                -axis[1] * confirm_offset_cm,
                f"{label} client position report after the teleport",
            )
        else:
            crossed, steps = probe.walk_across(
                outside=outside,
                axis=axis,
                threshold=exit_threshold,
                entered=False,
                facing=exit_axis,
                step_ticks=step_ticks,
                max_steps=max_steps,
                description=f"{label} exit walk",
            )
        position = probe.player_position(crossed)
        if point_within_box(position, box):
            raise RuntimeError(
                f"{label} left the client at {position}, which the declared box {box} still contains"
            )
        probe.require_fresh_prefix_marker(
            zone_marker_prefix,
            exited_marker,
            f"{label} exit fence",
            baseline,
        )
        closed = probe.wait_for_closed_screen(f"{label} exit window close")
        settled = probe.player_position(closed)
        if point_within_box(settled, box):
            raise RuntimeError(f"{label} settled back inside the declared box: {settled}")
        return {
            "route": route,
            "movement_steps": steps,
            "teleport_command": "/" + teleport_command if route == TELEPORT_EXIT_ROUTE else None,
            "position_confirmation": confirmation,
            "position": position,
            "settled_position": settled,
            "marker": exited_marker,
            "window_before_exit": held,
            "window_closed": True,
        }

    def run_zone_trade(cycle: dict[str, Any], label: str) -> dict[str, Any]:
        """One click on the window the zone entry opened, through the shared fixture."""

        slot = require_int(cycle["slot"], f"{label} slot")
        button = require_str(cycle["button"], f"{label} click button")
        outcome = require_str(cycle["outcome"], f"{label} outcome")
        if outcome not in {COMMITTED_OUTCOME, REFUSED_OUTCOME}:
            raise ValueError(f"{label} outcome must be {COMMITTED_OUTCOME} or {REFUSED_OUTCOME}")
        ledger = require_int(cycle["ledger"], f"{label} ledger value")
        expected_marker = require_str(cycle["marker"], f"{label} marker")
        if f"ledger={ledger}" not in expected_marker:
            raise ValueError(f"{label} marker {expected_marker!r} does not carry ledger={ledger}")
        expected_counts = (
            (emerald_item_id, require_int(cycle["emerald"], f"{label} emerald count")),
            (apple_item_id, require_int(cycle["apple"], f"{label} apple count")),
        )
        before = probe.observe_client()
        before_inventory = probe.inventory_fingerprint(before)
        before_tool = probe.tool_fingerprint(before, tool_item_id)
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
        probe.wait_for_closed_screen(f"{label} accepted window close")
        record = {
            "click": click,
            "outcome": outcome,
            "ledger": ledger,
            "window_closed": True,
            **evidence,
        }
        capture_name = cycle.get("inventory_capture")
        if capture_name is not None:
            screenshots.append(
                probe.capture_inventory_screenshot(
                    require_str(capture_name, f"{label} inventory capture")
                )
            )
        adversarial = cycle.get("adversarial")
        if adversarial is not None:
            record_adversarial(
                adversarial,
                label,
                {
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

    def run_movement_probe(cycle: dict[str, Any], label: str, *, entered: bool) -> dict[str, Any]:
        """Move without crossing the face and require nothing happened.

        Movement that stays on one side of the boundary is not a crossing: it must
        not publish a transition marker, must not open a window, and must not
        leave the side it started on.
        """

        ticks = require_int(cycle["ticks"], f"{label} probe ticks")
        steps = require_int(cycle["steps"], f"{label} probe steps")
        direction = axis if entered else exit_axis
        baseline = probe.marker_baseline()
        before = probe.observe_client()
        before_position = probe.player_position(before)
        if point_within_box(before_position, box) is not entered:
            raise RuntimeError(
                f"{label} probe began on the wrong side of the declared box: {before_position}"
            )
        probe.call("look", {"yaw_deg": crossing_yaw(direction), "pitch_deg": 0})
        for _ in range(steps):
            probe.call("move_forward", {"ticks": ticks})
        after = probe.observe_client()
        after_position = probe.player_position(after)
        fresh = probe.fresh_prefixed_lines(after, baseline, zone_marker_prefix)
        if fresh:
            raise RuntimeError(f"{label} probe published a transition it did not cross: {fresh}")
        if probe.screen(after).get("open") is True:
            raise RuntimeError(f"{label} probe opened a window without a crossing")
        if point_within_box(after_position, box) is not entered:
            raise RuntimeError(
                f"{label} probe left its own side of the declared box: {after_position}"
            )
        return {
            "ticks": ticks,
            "steps": steps,
            "from": before_position,
            "to": after_position,
            "side": "inside" if entered else "outside",
            "fresh_markers": fresh,
            "window_open": False,
        }

    def run_reconnect(label: str) -> dict[str, Any]:
        """Reconnect the same player and require the fixture survived it outside the box."""

        expected_emerald = require_int(reconnect["emerald"], "reconnect emerald count")
        expected_apple = require_int(reconnect["apple"], "reconnect apple count")
        before = probe.observe_client()
        pre_reconnect = {
            emerald_item_id: probe.item_count(before, emerald_item_id),
            apple_item_id: probe.item_count(before, apple_item_id),
        }
        info = probe.reconnect(server_addr, player_username)
        probe.await_inventory_counts(
            (emerald_item_id, expected_emerald),
            (apple_item_id, expected_apple),
        )
        rejoined = probe.observe_client()
        observed = {
            emerald_item_id: probe.item_count(rejoined, emerald_item_id),
            apple_item_id: probe.item_count(rejoined, apple_item_id),
        }
        if observed != pre_reconnect:
            raise RuntimeError(
                "inventory did not survive the disconnect/rejoin: "
                f"before={pre_reconnect} after={observed}"
            )
        expected = {emerald_item_id: expected_emerald, apple_item_id: expected_apple}
        if observed != expected:
            raise RuntimeError(f"rejoined inventory {observed} does not match {expected}")
        rejoined_tool = probe.tool_fingerprint(rejoined, tool_item_id)
        if rejoined_tool != initial_tool:
            raise RuntimeError(
                f"the component-bearing tool changed across the disconnect/rejoin: {rejoined_tool}"
            )
        position = probe.player_position(rejoined)
        if point_within_box(position, box):
            raise RuntimeError(
                f"the rejoined client stood inside the declared box at {position}: the next entry "
                "would not be a crossing"
            )
        screenshots.append(
            probe.capture_inventory_screenshot(
                require_str(reconnect["capture"], "reconnect capture name")
            )
        )
        return {
            "session_release": info["session_release"],
            "inventory": observed,
            "tool_fingerprint": rejoined_tool,
            "position": position,
            "window_open": False,
        }

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

    # The trade fixture seeds itself at the world spawn, before the walkway
    # exists, so the summoned damage target never stands in the crossing path.
    seeded = probe.seed_trade_fixture(setup, emerald, apple, tool)
    tool_item_id = seeded["tool_item_id"]
    initial_tool = seeded["tool_fingerprint"]
    screenshots.append(seeded["screenshot"])

    # The walkway and the fixed outside position are declared operator setup
    # (no_debug_commands false). Each builder command is accepted only on its own
    # verified-block feedback, the whole strip is then read back through the
    # client's own block scan, and the landing is accepted only once the client
    # reports it with solid ground under it.
    terrain_records: list[dict[str, Any]] = []
    for index, raw in enumerate(terrain_commands):
        entry = require_mapping(raw, f"terrain command {index}")
        terrain_records.append(
            probe.run_confirmed_setup_command(
                require_str(entry["command"], f"terrain command {index} command"),
                require_str(entry["prefix"], f"terrain command {index} prefix"),
                require_str(entry["feedback"], f"terrain command {index} feedback"),
                setup_commands,
            )
        )
    walkway = probe.require_walkway(
        top_y=walkway["walk_y"],
        surface_y=walkway["surface_y"],
        walkway_z=walkway["z"],
        x_min=walkway["x_min"],
        x_max=walkway["x_max"],
        description=f"{zone_id} walkway",
    )
    probe.run_setup_command(teleport_command, setup_commands)
    landing = probe.require_platform(
        position=outside,
        tolerance=landing_tolerance,
        description="outside landing",
    )

    # The zone is the fixture's own box, created by the fixture's own command: the
    # ready marker is its report that the owner answered the upsert Applied, and
    # the crossing below is only driven once that answer exists.
    zone_baseline = probe.marker_baseline()
    zone_command = f"{command_root} {setup_action}"
    probe.submit_chat(zone_command)
    ready = probe.require_fresh_prefix_marker(
        zone_marker_prefix,
        ready_marker,
        f"{zone_id} upsert applied",
        zone_baseline,
    )
    if probe.screen(ready).get("open") is True:
        raise RuntimeError(f"{zone_id} setup opened a window while the client was outside")
    if probe.container(ready).get("container_id") != 0:
        raise RuntimeError(f"{zone_id} setup left a container open: {probe.container(ready)}")
    outside_position = probe.player_position(ready)
    if point_within_box(outside_position, box):
        raise RuntimeError(f"{zone_id} setup began with the client inside the box: {outside_position}")
    screenshots.append(probe.capture_screen("zone-ready"))
    zone_record = {
        "id": zone_id,
        "dimension": dimension,
        "minimum": list(box[0]),
        "maximum": list(box[1]),
        "command": "/" + zone_command,
        "ready_marker": ready_marker,
        "ready_precondition": (
            "the guest publishes the ready marker only from the owner's Applied answer to the "
            "fixture's own upsert, so a refused or missing answer fails this wait instead of "
            "being reported as a created zone"
        ),
        "position": outside_position,
        "window_open": False,
    }
    record_adversarial(
        "zone_setup_applied_outside",
        "zone-setup",
        {
            "command": "/" + zone_command,
            "expected": ready_marker,
            "position": outside_position,
            "window_open": False,
        },
    )

    for index, raw in enumerate(cycles):
        cycle = require_mapping(raw, f"cycle {index}")
        label = require_str(cycle.get("label", f"cycle[{index}]"), f"cycle {index} label")
        kind = require_str(cycle["kind"], f"{label} kind")
        route = require_str(cycle["exit_route"], f"{label} exit route")
        record: dict[str, Any] = {"label": label, "kind": kind, "exit_route": route}
        if cycle.get("reconnect_before") is True:
            record["reconnect"] = run_reconnect(label)
        record["entry"] = enter_zone(label)
        screenshots.append(
            probe.capture_screen(require_str(cycle["window_capture"], f"{label} window capture"))
        )
        if kind == TRADE_CYCLE_KIND:
            record["trade"] = run_zone_trade(cycle, label)
        elif kind != EXIT_CYCLE_KIND:
            raise ValueError(f"{label} kind must be {TRADE_CYCLE_KIND} or {EXIT_CYCLE_KIND}")
        if cycle.get("interior_probe") is not None:
            if kind == EXIT_CYCLE_KIND:
                raise ValueError(
                    f"{label} cannot move inside while the window this crossing must close is open"
                )
            probe_config = require_mapping(cycle["interior_probe"], f"{label} interior probe")
            record["interior_probe"] = run_movement_probe(probe_config, label, entered=True)
            adversarial = probe_config.get("adversarial")
            if adversarial is not None:
                record_adversarial(
                    adversarial,
                    label,
                    {
                        "movement": "ordinary forward input that stays inside the box",
                        **record["interior_probe"],
                    },
                )
        record["exit"] = exit_zone(label, route, window_open=kind == EXIT_CYCLE_KIND)
        if cycle.get("exterior_probe") is not None:
            probe_config = require_mapping(cycle["exterior_probe"], f"{label} exterior probe")
            record["exterior_probe"] = run_movement_probe(probe_config, label, entered=False)
            adversarial = probe_config.get("adversarial")
            if adversarial is not None:
                record_adversarial(
                    adversarial,
                    label,
                    {
                        "movement": "ordinary forward input that stays outside the box",
                        **record["exterior_probe"],
                    },
                )
        screenshots.append(
            probe.capture_screen(require_str(cycle["closed_capture"], f"{label} closed capture"))
        )
        if kind == EXIT_CYCLE_KIND and cycle.get("adversarial") is not None:
            # A trade cycle's check belongs to its own click, which run_zone_trade
            # recorded with the settlement it observed; this one belongs to the
            # crossing that has to close the live window.
            record_adversarial(
                cycle["adversarial"],
                label,
                {
                    "kind": kind,
                    "exit_route": route,
                    **record["exit"],
                },
            )
        cycles_records.append(record)

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
        "zone_marker_prefix": zone_marker_prefix,
        "zone": zone_record,
        "crossing": {
            "outside": list(outside),
            "inside": list(inside),
            "axis": list(axis),
            "entry_threshold": entry_threshold,
            "exit_threshold": exit_threshold,
            "walk_route": (
                "ordinary forward input held for real ticks through the bridge, stopped by the "
                "client's own reported position"
            ),
            "teleport_route": (
                "operator " + teleport_command + " sent while the window was open, followed by one "
                "real client position report, because a container screen swallows ordinary "
                "movement input; the close that follows is the fixture's exit-driven close"
            ),
        },
        "operator_setup": {
            "commands": setup_commands,
            "terrain": terrain_records,
            "walkway": walkway,
            "landing": landing,
        },
        "fixture_seed": {
            "commands": seeded["commands"],
            "initial_inventory": seeded["inventory"],
            "tool_item_id": tool_item_id,
            "tool_fingerprint": initial_tool,
            "durability_attack": seeded["durability_attack"],
        },
        "cycles": cycles_records,
        "adversarial_checks": adversarial_checks,
        "screenshots": screenshots,
        "mcp_observation_gaps": list(MCP_ZONE_OBSERVATION_GAPS),
        "sibling_scenarios": (
            "wasm-p3-inventory-storage and wasm-p3-inventory-menu keep their own manifests, "
            "fixture modes and routes; this mode reuses the same fixture, market and ledger "
            "machine without changing either"
        ),
    }
    return "passed", final_state, screenshots, scenario_report
