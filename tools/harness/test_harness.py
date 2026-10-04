#!/usr/bin/env python3
"""Fail-closed receipts and real process lifecycle checks for the harness.

Run with: python3 -m unittest tools.harness.test_harness
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import select
import signal
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from . import __main__ as cli
from . import profiles, runtime
from .backends import driver
from .backends import load_matrix, player_scale
from types import SimpleNamespace


class ClientSettingsTests(unittest.TestCase):
    def test_invalid_username_rejected(self) -> None:
        with mock.patch.dict(os.environ, {
            "SOLARIS_CLIENT_MCP_TOKEN": "t",
            "SOLARIS_CLIENT_MCP_USERNAME": "bad-name!",
        }):
            with self.assertRaises(RuntimeError):
                cli.resolve_client_settings(None)

    def test_missing_token_rejected(self) -> None:
        env = {key: value for key, value in os.environ.items()
               if key != "SOLARIS_CLIENT_MCP_TOKEN"}
        with mock.patch.dict(os.environ, env, clear=True):
            with self.assertRaises(RuntimeError):
                cli.resolve_client_settings(None)

    def test_unknown_platform_rejected(self) -> None:
        with mock.patch.dict(os.environ, {"SOLARIS_CLIENT_MCP_TOKEN": "t"}):
            with self.assertRaises(ValueError):
                cli.resolve_client_settings("quilt")

class DriverLabelTests(unittest.TestCase):
    def test_missing_manifest_defaults_to_draft(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(driver.run_quality_label(Path(tmp)), "draft")



class ReceiptContractTests(unittest.TestCase):
    def test_unknown_profile_never_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                code = cli.cmd_run(["no-such-profile"])
            self.assertEqual(code, 2)
            receipts = list(Path(tmp).glob("*/result.json"))
            self.assertEqual(len(receipts), 1)
            payload = json.loads(receipts[0].read_text())
            self.assertEqual(payload["status"], "failed")

    def test_failed_command_keeps_prior_success_and_failure_evidence(self) -> None:
        commands = [
            [sys.executable, "-c", "print('first step completed')"],
            [sys.executable, "-c", "print('second step failed'); raise SystemExit(7)"],
        ]

        def run_commands(ctx):
            for index, command in enumerate(commands):
                profiles._run_command(command, ctx, f"step-{index}.log")
            return {}

        spec = {"scope": "harness", "run": run_commands}
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                with mock.patch.dict(profiles.PROFILES, {"failing-commands": spec}):
                    code = cli.cmd_run(["failing-commands"])
            self.assertEqual(code, 1)
            receipt = next(Path(tmp).glob("*/result.json"))
            payload = json.loads(receipt.read_text())
            self.assertEqual(payload["status"], "failed")
            self.assertEqual(payload["commands"], commands)
            self.assertEqual(payload["exits"], [0, 7])
            self.assertEqual(
                [Path(path).read_text().strip() for path in payload["logs"]],
                ["first step completed", "second step failed"],
            )

    def test_missing_executable_records_attempt_without_a_fake_exit_code(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = profiles.ProfileContext(Path(tmp))
            command = [str(Path(tmp) / "missing-executable")]
            with self.assertRaises(FileNotFoundError):
                profiles._run_command(command, ctx, "missing.log")
            self.assertEqual(ctx.commands, [command])
            self.assertEqual(ctx.exits, [None])
            self.assertTrue(Path(ctx.logs[0]).is_file())

    def test_native_missing_tools_fail_receipt_before_gameplay(self) -> None:
        from .backends.gameplay import require_tools

        def missing_tools(ctx):
            require_tools(
                [{"name": "minecraft_observe"}],
                {"minecraft_observe", "minecraft_break_block", "minecraft_use_item_on"},
                "survival",
            )
            self.fail("gameplay must not execute after missing tool discovery")

        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                with mock.patch.dict(profiles.PROFILES, {
                    "survival": {"scope": "real-client", "run": missing_tools},
                }):
                    code = cli.cmd_run(["survival", "--client", "native"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
        self.assertEqual(code, 1)
        self.assertEqual(payload["status"], "failed")
        self.assertEqual(payload["client"], "native")
        self.assertIn("minecraft_break_block", payload["error"])
        self.assertIn("minecraft_use_item_on", payload["error"])

    def test_native_never_falls_back_to_java_only_profile(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                code = cli.cmd_run(["loader-live", "--client", "native"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
        self.assertEqual(code, 1)
        self.assertEqual(payload["status"], "failed")
        self.assertEqual(payload["client"], "native")
        self.assertEqual(payload["commands"], [])


class PlayerScaleReceiptTests(unittest.TestCase):
    def test_fresh_chunk_headers_ignore_timestamps_and_staged_files(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            world = Path(tmp)
            region = world / "region"
            region.mkdir()
            locations = [0] * 1024
            locations[31] = (2 << 8) | 1
            path = region / "r.-1.2.mca"
            path.write_bytes(struct.pack(">1024I", *locations) + b"\xff" * 4096)
            initial = player_scale._anvil_chunk_positions(world)
            self.assertEqual(initial, {(-1, 64)})
            locations[1023] = (3 << 8) | 1
            path.write_bytes(struct.pack(">1024I", *locations) + b"\xff" * 4096)
            (region / ".r.-1.2.mca.tmp.123").write_bytes(b"\xff" * 8192)
            stored = player_scale._anvil_chunk_positions(world)
            self.assertEqual(stored - initial, {(-1, 95)})

    def measured_rung(self, timeout_counter=None):
        counters = {"disconnects": 0, "action_ack_timeouts": 0,
                    "action_observation_timeouts": 0}
        def sample(source, monotonic, ticks):
            return {"monotonic_ns": monotonic, "unix_ns": monotonic + 1_700_000_000_000_000_000,
                    "bot_counters": dict(counters),
                    "server": {"cpu_seconds": ticks / 100, "rss_bytes": 1024,
                               "user_cpu_seconds": ticks * 0.0095,
                               "system_cpu_seconds": ticks * 0.0005,
                               "peak_rss_bytes": 1024},
                    "bots": {"cpu_seconds": ticks / 1000, "rss_bytes": 1024,
                             "user_cpu_seconds": ticks * 0.00095,
                             "system_cpu_seconds": ticks * 0.00005,
                             "peak_rss_bytes": 1024},
                    "host_cpus": {
                        "cpu2": {"busy_seconds": monotonic / 1e9 * 0.8,
                                 "total_seconds": monotonic / 1e9},
                        "cpu3": {"busy_seconds": monotonic / 1e9 * 0.1,
                                 "total_seconds": monotonic / 1e9}},
                    "dashboard": {"simulation_tick": ticks,
                        "tick": {"source_tick": source,
                                 "total": {"samples": 1200, "p95_us": 1000},
                                 "stages": {}},
                        "chunks": {"generated_total": 0, "streamed_total": 0},
                        "network": {"bytes_written": 0}}}
        start, end = sample(100, 0, 100), sample(1300, 60_000_000_000, 1300)
        if timeout_counter:
            end["bot_counters"][timeout_counter] = 1
        rung = player_scale.Rung.__new__(player_scale.Rung)
        rung.args = SimpleNamespace(measure_ticks=1200, min_tps=18)
        rung.clients, rung.server_cores = 2, 1
        rung.server_cpus = "2"
        rung.measured, rung.idle = [end], None
        rung.receipt, rung.ready, rung.profile_process = {"errors": []}, True, None
        rung.summarize(start, end)
        return rung

    def test_foreign_cpu_share_requires_single_cpu_attribution(self) -> None:
        rung = self.measured_rung()
        measured = rung.receipt["measurement"]["foreign_cpu_load"]["cpus"]
        self.assertAlmostEqual(measured["cpu2"]["foreign_cpu_percent_one_core"], 60)
        self.assertAlmostEqual(measured["cpu3"]["foreign_cpu_percent_one_core"], 10)
        start = {"monotonic_ns": 0, "server": {"cpu_seconds": 0},
                 "host_cpus": {"cpu2": {"busy_seconds": 0}, "cpu3": {"busy_seconds": 0}}}
        end = {"monotonic_ns": 1_000_000_000, "server": {"cpu_seconds": 0.51},
               "host_cpus": {"cpu2": {"busy_seconds": 0.5}, "cpu3": {"busy_seconds": 0.1}}}
        single = player_scale.foreign_cpu_load(start, end, "2")["cpus"]["cpu2"]
        self.assertLess(single["foreign_cpu_seconds_raw"], 0)
        self.assertEqual(single["foreign_cpu_percent_one_core"], 0)
        multi = player_scale.foreign_cpu_load(start, end, "0,2")["cpus"]["cpu2"]
        self.assertIsNone(multi["foreign_cpu_percent_one_core"])

    def test_action_timeouts_reject_otherwise_healthy_rung(self) -> None:
        healthy = self.measured_rung()
        self.assertEqual(healthy.receipt["status"], "passed")
        self.assertTrue(healthy.receipt["acceptance"]["baseline_holds"])
        for counter in ("action_ack_timeouts", "action_observation_timeouts"):
            with self.subTest(counter=counter):
                rung = self.measured_rung(counter)
                self.assertEqual(rung.receipt["status"], "failed")
                self.assertFalse(rung.receipt["acceptance"]["baseline_holds"])

    def test_final_action_timeout_revokes_sampled_acceptance(self) -> None:
        for counter in ("action_ack_timeouts", "action_observation_timeouts"):
            with self.subTest(counter=counter):
                rung = self.measured_rung()
                rung.validate_final_bot_report({"success": True, "disconnects": 0,
                                                counter: 1})
                self.assertEqual(rung.receipt["status"], "failed")
                self.assertFalse(rung.receipt["acceptance"]["baseline_holds"])
                self.assertEqual(rung.receipt["acceptance"][counter], 1)

    def test_smoke_backend_exit_zero_is_not_canonical_pass(self) -> None:
        original_run = profiles._run_command
        def lightweight_command(argv, ctx, log_name, **kwargs):
            if log_name == "player-scale-build.log":
                command = [sys.executable, "-c", "pass"]
            else:
                receipt = ctx.artifact_dir / "bench" / "receipt.json"
                source = (
                    "import json; from pathlib import Path; "
                    f"p=Path({str(receipt)!r}); p.parent.mkdir(); "
                    "p.write_text(json.dumps({'status':'smoke_only','baseline_holds':False})); "
                    "print(json.dumps({'event':'final','status':'smoke_only','receipt':str(p)}))"
                )
                command = [sys.executable, "-c", source]
            return original_run(command, ctx, log_name, **kwargs)
        with tempfile.TemporaryDirectory() as tmp:
            output = io.StringIO()
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                with mock.patch.object(profiles, "_run_command", lightweight_command):
                    with contextlib.redirect_stdout(output):
                        code = cli.cmd_run(["player-scale"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
        self.assertEqual(code, 0)
        self.assertEqual(payload["status"], "prepared")
        self.assertEqual(payload["details"]["backend_status"], "smoke_only")
        self.assertNotIn("[harness] PASS", output.getvalue())


class LoadMatrixCliTests(unittest.TestCase):
    def test_full_off_core_lifecycle_keeps_long_window_and_single_cpu(self) -> None:
        async def fixture(*args, **kwargs):
            return {"status": "prepared"}

        async def dry_run(rung):
            # Construct the real inherited Rung, but do not launch its processes.
            rung.receipt["status"] = "smoke_only"
            return rung.receipt

        with tempfile.TemporaryDirectory() as tmp:
            argv = ["load-matrix", "--label", "release-main",
                    "--server-bin", sys.executable, "--bots-bin", sys.executable,
                    "--fixture-bin", sys.executable, "--out", tmp,
                    "--scenario", "long-session", "--no-single-core",
                    "--smoke-cpus", "1,4"]
            with mock.patch.object(sys, "argv", argv), \
                 mock.patch.object(load_matrix, "initialize_world", fixture), \
                 mock.patch.object(load_matrix, "fixture_report", fixture), \
                 mock.patch.object(load_matrix.MatrixRung, "run", dry_run), \
                 contextlib.redirect_stdout(io.StringIO()):
                code = load_matrix.main()
            summary = json.loads(next(Path(tmp).glob("*/receipt.json")).read_text())
            receipt = json.loads(Path(summary["receipts"][0]).read_text())
        self.assertEqual(code, 0)
        self.assertFalse(receipt["canonical"])
        self.assertEqual(receipt["server_cpus"], "1")
        self.assertEqual(receipt["server_affinity_logical_cpus"], 1)
        self.assertEqual(receipt["measure_ticks_requested"], 12000)


class LoadMatrixRestartIdentityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.cow = {"uuid": "authored-cow", "kind": "minecraft:cow"}
        self.villager = {"uuid": "authored-villager", "kind": "minecraft:villager"}
        self.item = {"uuid": "authored-item", "kind": "minecraft:item"}
        self.fixture = {"entities": [self.cow, self.villager, self.item]}

    def test_dynamic_hostile_despawn_does_not_invalidate_authored_restart(self) -> None:
        hostile = {"uuid": "natural-skeleton", "kind": "minecraft:skeleton"}
        newcomer = {"uuid": "new-natural-zombie", "kind": "minecraft:zombie"}
        before = {"entities": [self.cow, self.villager, self.item, hostile]}
        after = {"entities": [{**self.cow, "position": [12, 64, 8]}, self.villager, newcomer]}
        proof = load_matrix.restart_entity_identity_proof(self.fixture, before, after)
        self.assertTrue(proof["authored"]["retained"])
        self.assertEqual(proof["authored"]["missing"], [])
        self.assertEqual(proof["dynamic"]["missing"], [("natural-skeleton", "minecraft:skeleton")])

    def test_missing_saved_authored_animal_invalidates_restart(self) -> None:
        proof = load_matrix.restart_entity_identity_proof(
            self.fixture, {"entities": [self.cow, self.villager]},
            {"entities": [self.villager]})
        self.assertFalse(proof["authored"]["retained"])
        self.assertEqual(proof["authored"]["missing"], [("authored-cow", "minecraft:cow")])

    def test_changed_authored_type_invalidates_restart(self) -> None:
        proof = load_matrix.restart_entity_identity_proof(
            self.fixture, {"entities": [self.cow, self.villager]},
            {"entities": [self.cow, {**self.villager, "kind": "minecraft:zombie_villager"}]})
        self.assertFalse(proof["authored"]["retained"])
        self.assertEqual(proof["authored"]["missing"],
                         [("authored-villager", "minecraft:villager")])

    def test_authored_entity_already_absent_before_restart_is_not_required(self) -> None:
        proof = load_matrix.restart_entity_identity_proof(
            self.fixture, {"entities": [self.cow]}, {"entities": [self.cow]})
        self.assertTrue(proof["authored"]["retained"])
        self.assertEqual(proof["authored"]["before"], [("authored-cow", "minecraft:cow")])

    def test_empty_saved_authored_cohort_cannot_prove_persistence(self) -> None:
        hostile = {"uuid": "natural-skeleton", "kind": "minecraft:skeleton"}
        proof = load_matrix.restart_entity_identity_proof(
            self.fixture, {"entities": [hostile]}, {"entities": [hostile]})
        self.assertFalse(proof["authored"]["retained"])


class SharedClientProbeTests(unittest.TestCase):
    def test_inventory_missing_click_tools_fails_before_fixture_actions(self) -> None:
        from . import compatibility

        class MissingClicks:
            def list_tools(self):
                return [{"name": name} for name in compatibility.INVENTORY_TOOLS
                        if name not in {"minecraft_click_container_slot", "minecraft_quick_move_container_slot"}]

            def call_tool(self, *args):
                raise AssertionError("a missing tool must fail before any fixture or action")

        def probe(ctx):
            return compatibility.run_inventory_probe(MissingClicks(), ctx.artifact_dir)

        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)), mock.patch.dict(profiles.PROFILES, {
                "inventory": {"scope": "real-client", "run": probe},
            }):
                code = cli.cmd_run(["inventory", "--client", "native"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
        self.assertEqual(code, 1)
        self.assertEqual(payload["status"], "failed")
        self.assertEqual(payload["client"], "native")
        self.assertIn("minecraft_click_container_slot", payload["error"])
        self.assertIn("minecraft_quick_move_container_slot", payload["error"])

    def test_entities_missing_attack_tool_fails_before_summon(self) -> None:
        from .backends import entities_probe

        class MissingAttack:
            def list_tools(self):
                return [{"name": name} for name in entities_probe.REQUIRED_TOOLS
                        if name != "minecraft_attack_entity_once"]

            def call_tool(self, *args):
                raise AssertionError("a missing attack tool must fail before summon")

        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(RuntimeError, "minecraft_attack_entity_once"):
                entities_probe.run(MissingAttack(), Path(tmp), 1.0)

    def test_unknown_container_slot_is_not_treated_as_empty(self) -> None:
        from .backends.probe_steps import container_slot

        with self.assertRaises(RuntimeError):
            container_slot({"container": {"slot_count": 9, "slots": []}}, 9)


class NativeDescriptorTests(unittest.TestCase):
    def test_descriptor_refuses_a_different_process(self) -> None:
        from .backends.native_client import _read_descriptor

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "native-test.json"
            path.write_text(json.dumps({"instance_id": "native-test", "pid": os.getpid() + 1}))
            with self.assertRaisesRegex(RuntimeError, "PID"):
                _read_descriptor(path, root, os.getpid(), {})

    def test_world_readable_token_is_not_used_for_attach(self) -> None:
        from .backends.native_client import _read_descriptor

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            token = root / "native-test.token"
            token.write_text("private-bearer-value")
            token.chmod(0o644)
            path = root / "native-test.json"
            path.write_text(json.dumps({
                "instance_id": "native-test", "pid": os.getpid(),
                "endpoint": "http://127.0.0.1:12345/mcp", "token_file": str(token),
            }))
            evidence = {}
            with self.assertRaisesRegex(RuntimeError, "0600"):
                _read_descriptor(path, root, os.getpid(), evidence)
            self.assertNotIn("private-bearer-value", json.dumps(evidence))


class SurvivalTargetTests(unittest.TestCase):
    def test_only_loaded_exposed_nonstanding_ground_is_targeted(self) -> None:
        from .backends.survival import _candidates

        blocks = []
        for x, z in ((0, 0), (1, 0), (2, 0), (0, 1)):
            blocks.append({"x": x, "y": 64, "z": z, "block_id": "minecraft:grass_block"})
            for y in (65, 66):
                blocks.append({"x": x, "y": y, "z": z, "block_id": "minecraft:air"})
        # The standing column, an occluded face and an unloaded air cell are unsafe.
        blocks.append({"x": 2, "y": 65, "z": 0, "block_id": "minecraft:stone"})
        blocks = [block for block in blocks
                  if not (block["x"] == 0 and block["z"] == 1 and block["y"] == 66)]
        targets = _candidates(blocks, {"x": 0.5, "y": 65.0, "z": 0.5})
        self.assertEqual([(block["x"], block["y"], block["z"]) for block in targets],
                         [(1, 64, 0)])


class ScenarioCatalogTests(unittest.TestCase):
    """The catalog must resolve to real checks and never report false green."""

    def _fake_scenario(self, tests, log_line):
        from . import scenarios as scenario_catalog

        def run(ctx):
            log_path = ctx.artifact_dir / "test.log"
            log_path.write_text(log_line)
            ctx.logs.append(str(log_path))
            return {}

        scenario = scenario_catalog.Scenario(
            id="fake-scenario",
            summary="fake",
            owner="test",
            evidence="native",
            profile="fake-profile",
            selector=("-p", "mc-net", "--lib", "fake"),
            tests=tests,
            proves="nothing",
            limits="test fixture",
        )
        return scenario, {"fake-profile": {"scope": "harness", "run": run}}

    def test_unknown_scenario_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                code = cli.cmd_run(["scenario", "no-such-scenario"])
            self.assertEqual(code, 2)
            receipt = next(Path(tmp).glob("*/result.json"))
            payload = json.loads(receipt.read_text())
            self.assertEqual(payload["status"], "failed")
            self.assertNotIn("named_scenario", payload["details"])

    def test_named_scenario_rejects_free_form_arguments(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                code = cli.cmd_run(["scenario", "container-transfer-recovery", "extra-arg"])
            self.assertEqual(code, 2)
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
            self.assertEqual(payload["status"], "failed")
            self.assertEqual(payload["commands"], [])

    def test_explain_writes_no_artifact_and_rejects_unknown_ids(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    self.assertEqual(cli.cmd_explain(["container-transfer-recovery"]), 0)
                    self.assertEqual(cli.cmd_explain(["no-such-scenario"]), 2)
            self.assertEqual(list(Path(tmp).iterdir()), [])

    def test_named_scenario_requires_every_expected_test_to_run(self) -> None:
        from . import scenarios as scenario_catalog

        passing = (
            "test native::fake_case ... ok\n\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2389 filtered out\n"
        )
        failing = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n"
        scenario, profile = self._fake_scenario(("native::fake_case",), passing)
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                with mock.patch.dict(profiles.PROFILES, profile), mock.patch.dict(
                    scenario_catalog.SCENARIOS, {"fake-scenario": scenario}
                ):
                    code = cli.cmd_run(["scenario", "fake-scenario"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
            self.assertEqual(code, 0)
            self.assertEqual(payload["status"], "passed")
            self.assertEqual(payload["details"]["named_scenario"]["missing_tests"], [])
            self.assertEqual(
                payload["details"]["named_scenario"]["engine_counts"],
                {"selected": 1, "executed": 1, "passed": 1, "failed": 0,
                 "ignored": 0, "filtered_out": 2389},
            )

        scenario, profile = self._fake_scenario(("native::fake_case",), failing)
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                with mock.patch.dict(profiles.PROFILES, profile), mock.patch.dict(
                    scenario_catalog.SCENARIOS, {"fake-scenario": scenario}
                ):
                    code = cli.cmd_run(["scenario", "fake-scenario"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
            self.assertEqual(code, 1)
            self.assertEqual(payload["status"], "failed")
            self.assertEqual(
                payload["details"]["named_scenario"]["missing_tests"],
                ["native::fake_case"],
            )

    def test_named_scenario_rejects_invalid_engine_counts(self) -> None:
        from . import scenarios as scenario_catalog

        passed_test = "test script::storage::settlement_tests::fake_case ... ok\n"
        summaries = {
            "executed": ("test result: ok. 3 passed; 0 failed; 1 ignored\n", 0, "passed"),
            "zero-selected": ("test result: ok. 0 passed; 0 failed; 0 ignored\n", 1, "failed"),
            "ignored-only": ("test result: ok. 0 passed; 0 failed; 2 ignored\n", 1, "failed"),
            "failed": ("test result: FAILED. 1 passed; 1 failed; 0 ignored\n", 1, "failed"),
            "missing-summary": ("", 1, "failed"),
        }
        for label, (summary, expected_exit, expected_status) in summaries.items():
            with self.subTest(label=label), tempfile.TemporaryDirectory() as tmp:
                scenario, profile = self._fake_scenario(
                    ("script::storage::settlement_tests::fake_case",), passed_test + summary
                )
                with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                    with mock.patch.dict(profiles.PROFILES, profile), mock.patch.dict(
                        scenario_catalog.SCENARIOS, {"fake-scenario": scenario}
                    ):
                        code = cli.cmd_run(["scenario", "fake-scenario"])
                payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
                self.assertEqual(code, expected_exit)
                self.assertEqual(payload["status"], expected_status)


    def test_catalog_entries_name_existing_profiles_and_sources(self) -> None:
        from . import scenarios as scenario_catalog

        for scenario in scenario_catalog.SCENARIOS.values():
            self.assertIn(scenario.profile, profiles.PROFILES, scenario.id)
            for source in scenario.sources:
                self.assertTrue((runtime.REPO_ROOT / source).exists(), f"{scenario.id}: {source}")


class ReadinessCleanupTests(unittest.TestCase):
    def test_wait_port_observes_real_server_startup(self) -> None:
        port = runtime.reserve_port()
        source = (
            "import signal,socket,sys; "
            "listener=socket.socket(); "
            "listener.bind(('127.0.0.1',int(sys.argv[1]))); "
            "listener.listen(); print('ready',flush=True); signal.pause()"
        )
        with tempfile.TemporaryDirectory() as tmp:
            log_path = Path(tmp) / "server.log"
            with log_path.open("wb") as log:
                process = subprocess.Popen(
                    [sys.executable, "-c", source, str(port)],
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
                try:
                    runtime.wait_port(port, 10.0, process, log_path=log_path)
                    self.assertTrue(runtime.port_open(port))
                    self.assertIsNone(process.poll())
                finally:
                    runtime.stop_process(process)
            self.assertFalse(runtime.port_open(port))

    def test_wait_port_reports_early_exit(self) -> None:
        port = runtime.reserve_port()
        with tempfile.TemporaryDirectory() as tmp:
            log_path = Path(tmp) / "early.log"
            with log_path.open("wb") as log:
                process = subprocess.Popen(
                    [sys.executable, "-c", "print('startup failed',flush=True); raise SystemExit(3)"],
                    stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
                )
                try:
                    with self.assertRaises(RuntimeError):
                        runtime.wait_port(port, 10.0, process, log_path=log_path)
                    self.assertEqual(process.wait(timeout=10), 3)
                finally:
                    runtime.stop_process(process)

    def test_xvfb_closed_display_announcement_stops_process_before_fallback(self) -> None:
        popen = subprocess.Popen
        processes = []
        log_handles = []

        def launch(argv, **kwargs):
            if "-displayfd" not in argv:
                raise FileNotFoundError("fallback Xvfb unavailable")
            descriptor = argv[argv.index("-displayfd") + 1]
            process = popen(
                [sys.executable, "-c",
                 "import os,signal,sys; os.close(int(sys.argv[1])); signal.pause()",
                 descriptor],
                **kwargs,
            )
            processes.append(process)
            log_handles.append(kwargs["stdout"])
            return process

        with tempfile.TemporaryDirectory() as tmp:
            try:
                with mock.patch.object(runtime.subprocess, "Popen", launch):
                    with self.assertRaises(FileNotFoundError):
                        runtime.start_xvfb(Path(tmp))
                self.assertLess(processes[0].wait(timeout=10), 0)
                self.assertTrue(log_handles[0].closed)
            finally:
                for process in processes:
                    runtime.stop_process(process)

    def test_stop_process_terminates_descendants(self) -> None:
        source = (
            "import signal,subprocess,sys; "
            "child=subprocess.Popen([sys.executable,'-c','import signal; signal.pause()']); "
            "print(child.pid,flush=True); signal.pause()"
        )
        process = subprocess.Popen(
            [sys.executable, "-c", source], stdout=subprocess.PIPE,
            text=True, start_new_session=True,
        )
        child_pid = None
        child_fd = None
        try:
            ready, _, _ = select.select([process.stdout], [], [], 10.0)
            self.assertTrue(ready, "fixture did not publish its child PID")
            child_pid = int(process.stdout.readline())
            child_fd = os.pidfd_open(child_pid)
            runtime.stop_process(process)
            self.assertLess(process.wait(timeout=10), 0)
            exited, _, _ = select.select([child_fd], [], [], 10.0)
            self.assertTrue(exited, "descendant survived process-group cleanup")
        finally:
            runtime.stop_process(process)
            if child_pid is not None:
                try:
                    os.kill(child_pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            if child_fd is not None:
                os.close(child_fd)
            process.stdout.close()



if __name__ == "__main__":
    raise SystemExit(unittest.main())
