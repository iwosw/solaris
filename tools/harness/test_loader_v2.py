"""Frozen v2 Loader launch selection without server/client builds."""
from __future__ import annotations

import json
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

from . import __main__ as cli
from . import loader, profiles, runtime


class LoaderV2Tests(unittest.TestCase):
    def test_v2_profile_never_builds_or_replaces_deployment(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            ctx = profiles.ProfileContext(root, platform="fabric",
                server_bin=root / "frozen", plugins_dir=root / "candidate",
                scenario_id="loader-v2-settlements-smoke")
            with mock.patch.object(profiles, "_run_command") as build, \
                 mock.patch.object(loader, "run", return_value={"passed": True}) as run:
                profiles.run_loader_live(ctx)
            build.assert_not_called()
            self.assertEqual(run.call_args.kwargs, {
                "server_bin": ctx.server_bin, "plugins_dir": ctx.plugins_dir})
            self.assertEqual(json.loads((root / "ops.json").read_text())[0]["name"], "GateFabric")

    def test_missing_v2_inputs_fail_before_build(self) -> None:
        ctx = profiles.ProfileContext(Path("/tmp/loader-v2"), platform="fabric")
        with mock.patch.object(profiles, "_run_command") as build:
            with self.assertRaisesRegex(ValueError, "--server-bin and --plugins-dir"):
                profiles.run_loader_live(ctx)
        build.assert_not_called()

    def test_shared_probe_uses_frozen_binary_without_building(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "solaris-v2"
            binary.write_text("#!/bin/false\n")
            binary.chmod(0o700)
            with mock.patch.dict("os.environ", {"SOLARIS_SERVER_BINARY": str(binary)}), \
                 mock.patch.object(profiles, "_run_command") as build:
                selected = profiles._server_binary(profiles.ProfileContext(Path(temporary)))
            self.assertEqual(selected, binary)
            build.assert_not_called()

    def test_shared_probe_rejects_invalid_frozen_binary_without_building(self) -> None:
        with mock.patch.dict("os.environ", {"SOLARIS_SERVER_BINARY": "/missing/solaris-v2"}), \
             mock.patch.object(profiles, "_run_command") as build:
            with self.assertRaisesRegex(RuntimeError, "missing executable server binary"):
                profiles._server_binary(profiles.ProfileContext(Path("/tmp")))
        build.assert_not_called()

    def test_v2_config_points_at_readonly_candidate_and_frozen_binary(self) -> None:
        with tempfile.TemporaryDirectory(dir=runtime.REPO_ROOT / ".analysis") as temporary:
            root = Path(temporary)
            binary = root / "frozen"
            binary.write_text("#!/bin/false\n")
            binary.chmod(0o700)
            plugins = root / "candidate"
            owner = plugins / "server-only"
            owner.mkdir(parents=True)
            (owner / "plugin.toml").write_text('id = "server-only"\n')
            artifact = root / "artifact"
            artifact.mkdir()
            deploy = mock.Mock()
            with mock.patch.object(runtime, "port_open", return_value=False), \
                 mock.patch.object(runtime, "start_xvfb", side_effect=RuntimeError("config fence")):
                with self.assertRaisesRegex(RuntimeError, "config fence"):
                    loader.run("fabric", 10, artifact, {"run": mock.Mock(), "deploy": deploy},
                        server_bin=binary, plugins_dir=plugins)
            config = tomllib.loads((artifact / "playable.toml").read_text())
            self.assertEqual(config["plugins"]["directory"], str(plugins))
            result = json.loads((artifact / "scenario.json").read_text())
            self.assertEqual(result["server_binary"], str(binary))
            self.assertEqual(result["expected_bundle_cache_files"], [])
            self.assertEqual((owner / "plugin.toml").read_text(), 'id = "server-only"\n')
            deploy.assert_not_called()

    def test_smoke_uses_native_settlement_commands_without_package(self) -> None:
        from . import loader_settlements

        with tempfile.TemporaryDirectory() as temporary:
            artifact = Path(temporary)
            (artifact / "server.log").write_text("fixture\n")
            steps = mock.MagicMock()
            steps.report = {}
            steps.__enter__.return_value = steps
            commands: list[str] = []
            def record(_steps: object, command: str, _reply: str) -> None:
                commands.append(command)
            with mock.patch.object(loader_settlements, "ProbeSteps", return_value=steps), \
                 mock.patch.object(loader_settlements, "_command", side_effect=record), \
                 mock.patch.object(loader_settlements, "capture_ui", return_value={}):
                result = loader_settlements.run(mock.Mock(), runtime, artifact, artifact / "server.log")
            self.assertEqual(commands, [
                "settlement create p5qa",
                "settlement list",
                "settlement info p5qa",
                "settlement info p5qa",
            ])
            steps.call.assert_any_call(
                "minecraft_press_inputs", {"keys": ["key.keyboard.c"], "ticks": 1}
            )
            self.assertNotIn("settlements_bundle_cache", result)

    def test_cli_forwards_typed_loader_options(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(temporary)), \
                 mock.patch.dict(profiles.PROFILES, {"loader-live": {"scope": "real-client", "run": mock.Mock(return_value={})}}):
                run = profiles.PROFILES["loader-live"]["run"]
                code = cli.cmd_run(["loader-live", "--platform", "fabric",
                    "--server-bin", "/tmp/frozen", "--plugins-dir", "/tmp/candidate"])
                ctx = run.call_args.args[0]
            self.assertEqual(code, 0)
            self.assertEqual(ctx.server_bin, Path("/tmp/frozen"))
            self.assertEqual(ctx.plugins_dir, Path("/tmp/candidate"))
            self.assertEqual(ctx.extra_args, [])


if __name__ == "__main__":
    unittest.main()
