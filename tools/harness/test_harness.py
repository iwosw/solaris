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
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from . import __main__ as cli
from . import profiles, runtime
from .backends import driver


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
            self.assertIn("unknown profile", payload["error"])

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


    def test_test_profile_uses_cargo_test(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = profiles.ProfileContext(Path(tmp))
            with mock.patch.object(profiles, "_run_command", return_value={}) as run:
                profiles.run_test(ctx)
        run.assert_called_once_with(
            ["cargo", "test", "--workspace", "--all-targets"],
            ctx,
            "test.log",
        )

    def test_test_profile_forwards_focused_cargo_arguments(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = profiles.ProfileContext(
                Path(tmp), extra_args=["-p", "mc-net", "--lib", "player_attack"]
            )
            with mock.patch.object(profiles, "_run_command", return_value={}) as run:
                profiles.run_test(ctx)
        run.assert_called_once_with(
            ["cargo", "test", "-p", "mc-net", "--lib", "player_attack"],
            ctx,
            "test.log",
        )


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
            self.assertIn("unknown scenario", payload["error"])
            self.assertIn("warehouse-bind-manual", payload["error"])
            self.assertNotIn("named_scenario", payload["details"])

    def test_named_scenario_rejects_free_form_arguments(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                code = cli.cmd_run(["scenario", "warehouse-bind-manual", "extra-arg"])
            self.assertEqual(code, 2)
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
            self.assertEqual(payload["status"], "failed")
            self.assertIn("takes no extra arguments", payload["error"])
            self.assertEqual(payload["commands"], [])

    def test_explain_writes_no_artifact_and_rejects_unknown_ids(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    self.assertEqual(cli.cmd_explain(["warehouse-bind-manual"]), 0)
                    self.assertEqual(cli.cmd_explain(["no-such-scenario"]), 2)
            self.assertEqual(list(Path(tmp).iterdir()), [])
        self.assertIn("warehouse-bind-manual", output.getvalue())

    def test_named_scenario_requires_every_expected_test_to_run(self) -> None:
        from . import scenarios as scenario_catalog

        passing = (
            "test script::storage::settlement_tests::fake_case ... ok\n\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2389 filtered out\n"
        )
        failing = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n"
        scenario, profile = self._fake_scenario(("script::storage::settlement_tests::fake_case",), passing)
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

        scenario, profile = self._fake_scenario(("script::storage::settlement_tests::fake_case",), failing)
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                with mock.patch.dict(profiles.PROFILES, profile), mock.patch.dict(
                    scenario_catalog.SCENARIOS, {"fake-scenario": scenario}
                ):
                    code = cli.cmd_run(["scenario", "fake-scenario"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
            self.assertEqual(code, 1)
            self.assertEqual(payload["status"], "failed")
            self.assertIn("scenario evidence", payload["error"])
            self.assertIn("missing test: script::storage::settlement_tests::fake_case", payload["error"])
            self.assertEqual(
                payload["details"]["named_scenario"]["missing_tests"],
                ["script::storage::settlement_tests::fake_case"],
            )

    def test_zero_selected_evidence_is_not_a_pass(self) -> None:
        from . import scenarios as scenario_catalog

        self.assertEqual(
            scenario_catalog.verify_native_evidence(
                "test script::storage::settlement_tests::fake_case ... ok\n",
                ("script::storage::settlement_tests::fake_case",),
            ),
            [],
        )
        self.assertEqual(
            len(scenario_catalog.verify_native_evidence("running 0 tests\n", ("a::b",))),
            1,
        )
        self.assertIn(
            "<a test in this selector failed>",
            scenario_catalog.verify_native_evidence(
                "test a::b ... ok\ntest result: FAILED. 0 passed; 1 failed\n", ("a::b",)
            ),
        )

    def test_engine_counts_forbid_zero_selected_and_ignored_only_runs(self) -> None:
        from . import scenarios as scenario_catalog

        summary = "test result: ok. {p} passed; {f} failed; {i} ignored; 0 measured; {x} filtered out\n"
        passed = scenario_catalog.cargo_counts(summary.format(p=3, f=0, i=1, x=2389))
        self.assertEqual(
            passed,
            {"selected": 4, "executed": 3, "passed": 3, "failed": 0,
             "ignored": 1, "filtered_out": 2389},
        )
        self.assertEqual(scenario_catalog.count_failures(passed), [])

        self.assertEqual(
            scenario_catalog.count_failures(
                scenario_catalog.cargo_counts(summary.format(p=0, f=0, i=2, x=0))
            ),
            ["no test was executed; 2 selected ignored"],
        )
        self.assertEqual(
            scenario_catalog.count_failures(
                scenario_catalog.cargo_counts(summary.format(p=0, f=0, i=0, x=12))
            ),
            ["no test was selected and executed"],
        )
        self.assertEqual(
            scenario_catalog.count_failures(
                scenario_catalog.cargo_counts(summary.format(p=0, f=1, i=0, x=0))
            ),
            ["1 test(s) failed"],
        )
        self.assertEqual(
            scenario_catalog.count_failures(scenario_catalog.cargo_counts("no summary here")),
            ["the engine reported no test counts"],
        )

    def test_named_scenario_binds_its_declared_identity(self) -> None:
        from . import scenarios as scenario_catalog

        seen: dict[str, object] = {}

        def run(ctx):
            seen["scenario_id"] = ctx.scenario_id
            seen["manifest"] = ctx.manifest
            log_path = ctx.artifact_dir / "test.log"
            log_path.write_text(
                "test script::storage::settlement_tests::fake_case ... ok\n\n"
                "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out\n"
            )
            ctx.logs.append(str(log_path))
            ctx.cwds.append(str(runtime.REPO_ROOT))
            return {}

        scenario = scenario_catalog.Scenario(
            id="bound-scenario",
            summary="fake",
            owner="test",
            evidence="graphical",
            profile="bound-profile",
            scenario_id="declared-scenario-id",
            manifest="docs/real-client-regression/manifests/core-replay-seed-81.json",
            seed="81",
            fixture="fresh world",
            requires=("client-token",),
            sources=("docs/AGENT_TOOLING.md",),
            proves="binding reaches the profile",
            limits="test fixture",
        )
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                with mock.patch.dict(
                    profiles.PROFILES, {"bound-profile": {"scope": "harness", "run": run}}
                ), mock.patch.dict(scenario_catalog.SCENARIOS, {"bound-scenario": scenario}):
                    code = cli.cmd_run(["scenario", "bound-scenario"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
        self.assertEqual(code, 0)
        self.assertEqual(seen["scenario_id"], "declared-scenario-id")
        self.assertEqual(
            seen["manifest"], "docs/real-client-regression/manifests/core-replay-seed-81.json"
        )
        named = payload["details"]["named_scenario"]
        self.assertEqual(named["scenario_id"], "declared-scenario-id")
        self.assertEqual(named["fixture"], "fresh world")
        self.assertEqual(named["prerequisites"], ["client-token"])
        self.assertEqual(named["proves"], "binding reaches the profile")
        self.assertEqual(named["does_not_prove"], "test fixture")
        self.assertEqual(named["commands_cwd"], [str(runtime.REPO_ROOT)])
        self.assertIn("solaris", named["repo_revisions"])

    def test_bare_replay_keeps_environment_overrides(self) -> None:
        captured: dict[str, dict[str, str]] = {}

        def capture(argv, ctx, log_name, *, cwd=None, env=None):
            captured["env"] = dict(env or {})
            return {}

        preset = {
            "SOLARIS_REAL_CLIENT_MANIFEST": "custom.json",
            "SOLARIS_REAL_CLIENT_AGENT_SCENARIO": "custom-scenario",
        }
        with tempfile.TemporaryDirectory() as tmp:
            ctx = profiles.ProfileContext(Path(tmp))
            with mock.patch.object(profiles, "_run_client_backend", capture), mock.patch.dict(
                os.environ, preset
            ):
                profiles.run_replay(ctx)
            self.assertEqual(captured["env"]["SOLARIS_REAL_CLIENT_MANIFEST"], "custom.json")
            self.assertEqual(captured["env"]["SOLARIS_REAL_CLIENT_AGENT_SCENARIO"], "custom-scenario")

            ctx = profiles.ProfileContext(
                Path(tmp),
                scenario_id="core-actions-seed-81",
                manifest="docs/real-client-regression/manifests/core-replay-seed-81.json",
            )
            with mock.patch.object(profiles, "_run_client_backend", capture), mock.patch.dict(
                os.environ, preset
            ):
                profiles.run_replay(ctx)
            self.assertEqual(
                captured["env"]["SOLARIS_REAL_CLIENT_MANIFEST"],
                "docs/real-client-regression/manifests/core-replay-seed-81.json",
            )
            self.assertEqual(
                captured["env"]["SOLARIS_REAL_CLIENT_AGENT_SCENARIO"], "core-actions-seed-81"
            )

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

    def test_command_reports_started_and_completed_status(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = profiles.ProfileContext(Path(tmp))
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                profiles._run_command([sys.executable, "-c", ""], ctx, "stage.log")
        status = output.getvalue()
        self.assertIn("[harness] stage=stage starting pid=", status)
        self.assertIn("[harness] stage=stage exit=0", status)

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
                    with self.assertRaises(RuntimeError) as raised:
                        runtime.wait_port(port, 10.0, process, log_path=log_path)
                    self.assertIn("exited", str(raised.exception))
                    self.assertEqual(process.wait(timeout=10), 3)
                finally:
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
