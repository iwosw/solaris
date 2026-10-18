"""Cheap prerequisite checks for the focused P5 graphical runner."""
from __future__ import annotations

import unittest
from unittest import mock

from . import scenarios
from .backends import client_probes, java_p5, native_p5, native_v2


class JavaP5Tests(unittest.TestCase):
    def test_catalog_is_graphical_and_not_native_dispatch(self) -> None:
        for name in java_p5.SCENARIO_IDS:
            self.assertIn(name, scenarios.SCENARIOS)
            self.assertEqual(scenarios.SCENARIOS[name].evidence, "graphical")
            self.assertEqual(scenarios.SCENARIOS[name].profile, "entities")
            self.assertNotIn(name, native_v2.NATIVE_SCENARIO_IDS)

    def test_wrong_client_fails_before_server_resolution(self) -> None:
        ctx = mock.Mock(scenario_id="java-v2-p5-parity", client="native")
        with mock.patch.object(client_probes, "_server_binary") as binary:
            with self.assertRaisesRegex(ValueError, "--client java"):
                client_probes.run(ctx, "entities")
            binary.assert_not_called()

    def test_wrong_profile_fails_before_server_resolution(self) -> None:
        ctx = mock.Mock(scenario_id="java-v2-p5-towns", client="java")
        with mock.patch.object(client_probes, "_server_binary") as binary:
            with self.assertRaisesRegex(ValueError, "entities"):
                client_probes.run(ctx, "inventory")
            binary.assert_not_called()

    def test_real_screenshot_and_ordinary_action_tools_required(self) -> None:
        self.assertTrue({"minecraft_screenshot", "minecraft_press_inputs", "minecraft_click_screen_button", "minecraft_break_block"} <= java_p5.REQUIRED_TOOLS)

    def test_town_readback_rejects_a_stale_pre_command_reply(self) -> None:
        reply = "p5qa: 1 members, 1 claims."
        history = [reply]
        steps = mock.Mock(transcript=[], report={})

        def call(name: str, arguments: dict) -> dict:
            if not arguments["command"]:
                history.insert(0, arguments["message"])
            return {}

        def wait(predicate: object, label: str) -> dict:
            if label.endswith("_reply"):
                self.assertFalse(predicate({"recent_chat": list(history)}))
                history.insert(0, reply)
            state = {"recent_chat": list(history)}
            self.assertTrue(predicate(state))
            return state

        steps.call.side_effect = call
        steps.wait.side_effect = wait
        java_p5._command(steps, "town info p5qa", reply)
        self.assertEqual(len(steps.report["ordinary_commands"]), 1)

    def test_native_armor_addon_rejects_headless_observer(self) -> None:
        ctx = mock.Mock(scenario_id="native-v2-p5-humanoid-armor", client="native",
                        windowed=False)
        with mock.patch.object(client_probes, "_server_binary") as binary:
            with self.assertRaisesRegex(ValueError, "--windowed"):
                client_probes.run(ctx, "entities")
            binary.assert_not_called()

    def test_cursor_total_includes_carried_but_not_result_preview(self) -> None:
        state = {"container": {"slots": [
            {"slot": 0, "item_id": "minecraft:apple", "count": 64},
            {"slot": 1, "item_id": "minecraft:apple", "count": 1},
            {"slot": 10, "item_id": "minecraft:apple", "count": 6},
        ], "carried": {"item_id": "minecraft:apple", "count": 1}}}
        self.assertEqual(java_p5._menu_item_total(state, "minecraft:apple"), 8)
        self.assertEqual(java_p5._menu_item_total(state, "minecraft:diamond"), 0)

    def test_native_dyed_component_black_is_not_absence(self) -> None:
        steps = mock.Mock(report={})
        steps.call.return_value = {"components": {"minecraft:dyed_color": 0}}
        native_p5._component_receipt(steps, "black", 0)
        steps.call.assert_called_once_with("minecraft_read_item_components", {"source": "selected"})
        steps.check.assert_called_once_with("black_received_color", True)

    def test_native_dyed_component_must_match_received_numeric_color(self) -> None:
        for received in ({}, {"minecraft:dyed_color": 1_481_884}):
            steps = mock.Mock(report={})
            steps.call.return_value = {"components": received}
            native_p5._component_receipt(steps, "magenta", 16_711_935)
            steps.check.assert_called_once_with("magenta_received_color", False)
