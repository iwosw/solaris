"""Pure regression coverage for catalog outcome and received-work contracts."""
import asyncio
import io
import json
import tempfile
from contextlib import redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch
import unittest

import uuid
from .backends import catalog_load as load
from . import __main__ as cli


def native_population(count):
    anchors = [{"name": f"city-{n}", "anchor": [n * 100, 64, 0], "radius": 32}
               for n in range(4)]
    cohort = [{"uuid": str(uuid.UUID(int=n + 1)), "settlement": anchors[n % 4]["name"]}
              for n in range(count)]
    return {"count": count, "cohort": cohort, "settlement_anchors": anchors,
            "spawn_position": [0, 70, 0]}


def is_bind_write(event):
    return event[0] == "write" and event[1].startswith("settlement bind ")


class NativeConsoleServer:
    """Fake native console: stdin writes queue replies, drain appends them to a real log.

    Replies surface only when the feeder drains, so every log line is a reaction to a
    completed feeder write. A batch started before its gate (every Founded for the
    cohort batch, every Bound for the final batch) is recorded as a violation.
    """

    def __init__(self, directory, total, fail_with=None):
        self.log = directory / "server.log"
        self.log.write_text("")
        self.total, self.fail_with = total, fail_with
        self.outbox = []
        self.batches, self.violations = [], []
        self.events = []  # ("write", command) / ("log", line) / ("clock", url), in order

    def logged(self, prefix):
        return sum(1 for kind, line in self.events if kind == "log" and line.startswith(prefix))

    def write(self, data):
        batch = data.decode().splitlines()
        gates = {1: ("Founded ", 4), 2: ("Bound ", self.total)}
        if len(self.batches) in gates:
            prefix, count = gates[len(self.batches)]
            if self.logged(prefix) < count:
                self.violations.append(f"batch {len(self.batches)} sent before {count} {prefix}")
        self.batches.append(batch)
        for line in batch:
            self.events.append(("write", line))
            words = line.split()
            if words[:2] == ["settlement", "create"]:
                self.outbox.append(self.fail_with or f"Founded {words[2]} (revision 1).")
            elif words[:2] == ["settlement", "bind"]:
                self.outbox.append(f"Bound {words[3]} to {words[2]}.")
            elif words[:2] == ["settlement", "info"]:
                self.outbox.append(f"{words[2]} | owner=Server members=0")

    async def drain(self):
        lines, self.outbox = self.outbox, []
        self.events.extend(("log", line) for line in lines)
        with self.log.open("a") as stream:
            stream.write("".join(line + "\n" for line in lines))


class CatalogLoadTests(unittest.TestCase):
    def test_outcome_precedence(self):
        self.assertEqual(load.classify(capability="UNSUPPORTED_MECHANIC", infra=["missing"]), "UNSUPPORTED_MECHANIC")
        self.assertEqual(load.classify(infra=["tick gap"], correctness=["wrong"], performance=["slow"]), "INFRA_BLOCKED")
        self.assertEqual(load.classify(setup=["not ready"], correctness=["wrong"]), "SETUP_FAILED")
        self.assertEqual(load.classify(correctness=["missing credit"], performance=["slow"]), "CORRECTNESS_FAIL")
        self.assertEqual(load.classify(performance=["slow"]), "PERF_FAIL")
        self.assertEqual(load.classify(), "PASS")

    def test_percentile_uses_full_wall_not_tick_subset(self):
        samples = [{"wall_us": n, "total_us": 1} for n in range(1, 101)]
        measured = load.tick_metrics(samples)
        self.assertEqual(measured["p50_us"], 50)
        self.assertEqual(measured["p95_us"], 95)
        self.assertEqual(measured["p99_us"], 99)
        self.assertEqual(measured["max_us"], 100)
        self.assertEqual(load.tick_metrics([])["p99_us"], None)

    def test_packet_writes_do_not_prove_block_edits(self):
        report = {"clients": 10, "ready_clients": 10, "chunks_received": 100,
                  "breaks_sent": 100, "places_sent": 100}
        self.assertFalse(load.work_proof("join-ready-assisted", report, 10)["passed"])
        report.update(break_updates_observed=1, place_updates_observed=1)
        self.assertTrue(load.work_proof("join-ready-assisted", report, 10)["passed"])

    def test_every_join_requires_server_confirmed_first_action(self):
        report = {"clients": 10, "ready_clients": 10, "chunks_received": 100,
                  "chat_echo_clients": 9}
        self.assertFalse(load.work_proof("JOIN-01", report, 10)["passed"])
        report["chat_echo_clients"] = 10
        self.assertTrue(load.work_proof("JOIN-01", report, 10)["passed"])

    def test_console_session_keeps_logs_and_exposes_native_stdin(self):
        async def run(directory, console):
            process = SimpleNamespace(wait=AsyncMock(return_value=0))
            session = load.Session(SimpleNamespace(), directory)
            with patch.object(asyncio, "create_subprocess_exec", AsyncMock(return_value=process)) as spawn:
                await session.spawn("server", ["native-server"], console=console)
                options = spawn.call_args.kwargs
                session.stopping = True
                await asyncio.gather(*session.readers)
            for stream in session.logs:
                stream.close()
            return options
        with tempfile.TemporaryDirectory() as temporary:
            for console in (False, True):
                options = asyncio.run(run(Path(temporary), console))
                self.assertEqual(options["stdin"], asyncio.subprocess.PIPE if console
                                 else asyncio.subprocess.DEVNULL)
                self.assertNotEqual(options["stdout"], asyncio.subprocess.PIPE)


    def test_native_shutdown_grace_is_opt_in_and_server_only(self):
        async def run(timeout):
            session = load.Session(SimpleNamespace(), Path("/unused"))
            for name, pid in (("bots", 1), ("server", 2)):
                process = SimpleNamespace(pid=pid, returncode=None)
                async def finish(process=process):
                    process.returncode = 0
                    return 0
                process.wait = AsyncMock(side_effect=finish)
                session.processes[name] = process
            with patch.object(load.os, "killpg"), \
                 patch.object(asyncio, "wait_for", wraps=asyncio.wait_for) as waits:
                await session.stop(**({} if timeout is None else {"server_timeout": timeout}))
            self.assertEqual([call.args[1] for call in waits.call_args_list],
                             [10, 10 if timeout is None else timeout])
            self.assertTrue(all(not child["forced"] and child["returncode"] == 0
                                for child in session.shutdown.values()))
        for timeout in (None, 1800):
            with self.subTest(timeout=timeout):
                asyncio.run(run(timeout))

    def test_native_rss_events_persist_live_sample_and_process_exit(self):
        from tools.coreperf import constantinople
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / "run"
            directory.mkdir()
            process = SimpleNamespace(pid=123, returncode=None)
            session = SimpleNamespace(directory=directory, processes={"server": process},
                process_samples=lambda: {"server": {"pid": 123, "rss_bytes": 456,
                                                    "peak_rss_bytes": 789}})
            constantinople.native_rss_event(session, "bound", bound_count=100)
            process.returncode = 0
            constantinople.native_rss_event(session, "process_exit")
            rows = [json.loads(line) for line in
                    (directory / "rss-events.jsonl").read_text().splitlines()]
            self.assertEqual(rows[0]["phase"], "run")
            self.assertEqual(rows[0]["event"], "bound")
            self.assertEqual(rows[0]["bound_count"], 100)
            self.assertEqual(rows[0]["rss_bytes"], 456)
            self.assertEqual(rows[0]["peak_rss_bytes"], 789)
            self.assertIsNone(rows[1]["rss_bytes"])
            self.assertEqual(rows[1]["returncode"], 0)
            self.assertLessEqual(rows[0]["monotonic_ns"], rows[1]["monotonic_ns"])

    def test_native_clock_readiness_waits_for_real_sample(self):
        from tools.coreperf import constantinople
        session = SimpleNamespace(alive=lambda: None, http=AsyncMock(side_effect=[
            {"latest_tick": None, "wait_timed_out": True},
            {"latest_tick": 3603030217, "wait_timed_out": False},
        ]))
        result = asyncio.run(constantinople.native_game_clock(session, "http://native", 1800))
        self.assertEqual(result["latest_tick"], 3603030217)
        self.assertEqual(session.http.await_count, 2)
        for call in session.http.await_args_list:
            self.assertEqual(call.args[0], "http://native/load-metrics?wait=true&min_samples=1")
            self.assertGreater(call.args[1], 0)
            self.assertLessEqual(call.args[1], 1800)
        session.http = AsyncMock(return_value={"latest_tick": None, "wait_timed_out": False})
        with self.assertRaisesRegex(RuntimeError, "native game clock unavailable"):
            asyncio.run(constantinople.native_game_clock(session, "http://native", 1800))
        session.http = AsyncMock()
        with self.assertRaisesRegex(TimeoutError, "native game clock unavailable"):
            asyncio.run(constantinople.native_game_clock(session, "http://native", 0))
        session.http.assert_not_awaited()

    def run_native_setup(self, ticks=(100, 140), **options):
        """Drive native_console_setup against a real server.log and fake console stdin."""
        from tools.coreperf import constantinople
        population = native_population(40)
        ticks = iter(ticks)
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / "run"
            directory.mkdir()
            server = NativeConsoleServer(directory, len(population["cohort"]), **options)
            process = SimpleNamespace(pid=123, returncode=None, stdin=SimpleNamespace(
                write=server.write, drain=server.drain))
            session = SimpleNamespace(alive=lambda: None, directory=directory,
                processes={"server": process},
                process_samples=lambda: {"server": {"pid": 123, "rss_bytes": 1,
                                                    "peak_rss_bytes": 1}})

            async def clock(clock_session, url, timeout):
                server.events.append(("clock", url))
                return {"latest_tick": next(ticks), "instance_started_unix_ns": 123}
            with patch.object(constantinople, "native_game_clock", clock):
                result = asyncio.run(constantinople.native_console_setup(
                    session, population, 30, "http://native"))
        return result, server, population

    def test_native_console_setup_runs_create_bind_and_info_phases_with_bind_metrics(self):
        from tools.coreperf import constantinople
        result, server, population = self.run_native_setup()
        self.assertEqual(server.violations, [])
        # Initial create/spawn commands, one batch of every bind, final spawn/info.
        self.assertEqual([len(batch) for batch in server.batches], [8, 40, 5])
        self.assertEqual([line for batch in server.batches for line in batch],
                         constantinople.native_settlement_commands(population))
        self.assertEqual(result["founded"], sorted(row["name"] for row in
                                                   population["settlement_anchors"]))
        self.assertEqual(result["bound_villagers"], 40)
        self.assertEqual((result["start_tick"], result["end_tick"], result["bind_ticks"]),
                         (100, 140, 40))
        self.assertEqual(result["binds_per_tick"], 1.0)
        self.assertGreater(result["bind_seconds"], 0)
        self.assertGreater(result["binds_per_second"], 0)
        for removed in ("console_window", "queue_refusals", "max_outstanding_binds"):
            self.assertNotIn(removed, result)
        # The clock brackets only the bind batch: after Founded, before the first bind;
        # after the last Bound, before the final spawn/info batch.
        events = server.events
        clocks = [at for at, (kind, _) in enumerate(events) if kind == "clock"]
        self.assertEqual(len(clocks), 2)
        founded = [at for at, (kind, line) in enumerate(events)
                   if kind == "log" and line.startswith("Founded ")]
        bound = [at for at, (kind, line) in enumerate(events)
                 if kind == "log" and line.startswith("Bound ")]
        first_bind = next(at for at, event in enumerate(events) if is_bind_write(event))
        final_spawn = events.index(("write", "setworldspawn 0 70 0"))
        self.assertLess(founded[-1], clocks[0])
        self.assertLess(clocks[0], first_bind)
        self.assertLess(bound[-1], clocks[1])
        self.assertLess(clocks[1], final_spawn)

    def test_native_console_setup_rejects_bind_clock_that_did_not_advance(self):
        with self.assertRaisesRegex(RuntimeError, "did not advance"):
            self.run_native_setup(ticks=(100, 100))

    def test_native_console_setup_command_error_stays_fatal(self):
        with self.assertRaisesRegex(RuntimeError, "Command failed: unknown settlement city-9"):
            self.run_native_setup(fail_with="Command failed: unknown settlement city-9")

    def test_constantinople_native_setup_deadline_is_separate_from_tick_fence(self):
        from tools.coreperf import constantinople
        base = ["constantinople", "native-smoke", "--world", "w", "--server-bin", "s",
                "--bots-bin", "b", "--vanilla-data-dir", "v", "--out", "o", "--seed", "1"]
        for extra, setup, reopen in (([], 3600, False),
                                     (["--setup-timeout-seconds", "7200"], 7200, False),
                                     (["--reopen-only"], 3600, True)):
            with self.subTest(extra=extra), patch("sys.argv", base + extra), \
                 patch.object(constantinople, "native_smoke", AsyncMock()) as smoke:
                constantinople.main()
            args = smoke.call_args.args[0]
            self.assertEqual((args.timeout_seconds, args.setup_timeout_seconds), (1800, setup))
            self.assertEqual(args.reopen_only, reopen)

    def test_native_cohort_snapshot_requires_exact_uuid_and_settlement(self):
        from tools.coreperf import constantinople
        population = native_population(4)
        snapshot = {
            "globals": {
                f"settlement/{index + 1}": {"payload": {"value": {
                    "id": index + 1, "name": anchor["name"]}}}
                for index, anchor in enumerate(population["settlement_anchors"])},
            "entity_records": [
                {"uuid": member["uuid"], "active": True, "payload": {
                    "id": "minecraft:villager",
                    "solaris:VillagerRuntime": {"settlement": index + 1}}}
                for index, member in enumerate(population["cohort"])],
            "source_world": {"sha256": "fixture-digest"},
        }

        def observe(value):
            with patch.object(load, "native_snapshot", AsyncMock(return_value=value)):
                return asyncio.run(constantinople.native_cohort_snapshot(
                    SimpleNamespace(world=Path("/fixture")), Path("/receipt/cohort"),
                    Path("/config"), population))

        result = observe(snapshot)
        self.assertEqual(result["verified_bound_uuids"], 4)
        self.assertEqual(result["source_world_sha256"], "fixture-digest")
        for fault in ("replaced_uuid", "wrong_settlement", "duplicate_uuid"):
            changed = json.loads(json.dumps(snapshot))
            row = changed["entity_records"][0]
            if fault == "replaced_uuid":
                row["uuid"] = str(uuid.UUID(int=999))
            elif fault == "wrong_settlement":
                row["payload"]["solaris:VillagerRuntime"]["settlement"] = 2
            else:
                changed["entity_records"].append(row)
            with self.subTest(fault=fault), self.assertRaisesRegex(
                    RuntimeError, "duplicate native UUID|native bound UUID membership mismatch"):
                observe(changed)

    def test_constantinople_native_mode_uses_existing_fixture_profile(self):
        from . import profiles
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / "constantinople.log").write_text('{"receipt":"/fixture/receipt.json"}\n')
            for native in (False, True):
                extra = ["--", *(["--native"] if native else []), "--world", "/fixture"]
                context = SimpleNamespace(extra_args=extra, artifact_dir=directory)
                with patch.object(profiles, "_run_command", return_value={}) as command, \
                     patch.object(profiles, "_backend_env", return_value={}):
                    result = profiles.run_constantinople(context)
                self.assertEqual(command.call_args.args[0][3],
                                 "native-smoke" if native else "smoke")
                self.assertNotIn("--native", command.call_args.args[0])
                self.assertFalse(result["performance_claim"])

    def test_hash_is_order_independent_and_content_bound(self):
        self.assertEqual(load.digest({"a": 1, "b": 2}), load.digest({"b": 2, "a": 1}))
        self.assertNotEqual(load.digest({"a": 1}), load.digest({"a": 2}))

    def test_sustained_tps_does_not_hide_a_slow_subwindow(self):
        samples = [{"wall_start_unix_ns": second * 1_000_000_000 + tick * 10_000_000}
                   for second in range(10) for tick in range(10 if second < 5 else 30)]
        self.assertEqual(len(samples) / 10, 20)
        self.assertEqual(load.sustained_tps(samples, 0, 10_000_000_000, 5), 10)
        self.assertIsNone(load.sustained_tps(samples, 0, 2_000_000_000, 5))

    def test_recovery_demand_and_undrained_chats_fail(self):
        report = {"clients": 1, "chunks_received": 25, "chat_echo_clients": 1,
                  "chats_sent": 2, "chat_echoes_received": 1,
                  "open_loop": {"schedule_complete": True, "windows": [
                      {"recovery": True, "chat": {"offered": 1, "admitted": 0}}]}}
        proof = load.work_proof("JOIN-01", report, 1)
        self.assertFalse(proof["passed"])
        self.assertIn("recovery window offered/admitted gameplay demand", proof["failures"])
        self.assertIn("admitted ordinary chats did not drain to received echoes", proof["failures"])

    def test_canonical_cli_forwards_backend_arguments_with_or_without_separator(self):
        for separator in ([], ["--"]):
            with self.subTest(separator=separator), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                out = root / "classified"
                with patch.object(cli, "ARTIFACT_ROOT", root / "canonical"), redirect_stdout(io.StringIO()):
                    result = cli.cmd_run(["catalog-load", *separator,
                                          "--smoke", "--scenario", "plugin-heavy", "--out", str(out)])
                self.assertEqual(result, 0)
                receipt = json.loads((out / "plugin-heavy/result.json").read_text())
                self.assertEqual(receipt["outcome"], "INFRA_BLOCKED")
                canonical = json.loads(next((root / "canonical").glob("*/result.json")).read_text())
                self.assertEqual(canonical["status"], "prepared")


    def test_controls_use_real_recipes_and_mix_capabilities_do_not_alias(self):
        mapping = json.loads(load.MAPPING.read_text())
        entries = {row["id"]: row for row in mapping["scenarios"]}
        for name in load.catalog_controls.CONTROL_IDS:
            recipe = load.catalog_controls.recipe(name, 10, True)
            self.assertEqual(recipe["id"], name)
            self.assertTrue(recipe["preparation_regions"])
            self.assertIsNone(entries[name]["capability"]["outcome"])
        for name in ("MIX-01", "MIX-02", "MIX-04", "MIX-08", "MIX-18", "MIX-21",
                     "mixed-survival-long"):
            with self.subTest(scenario=name):
                row = entries[name]
                self.assertIsNone(row["capability"]["outcome"])
                self.assertTrue(row["work_proof"]["required"])
                self.assertEqual(row["oracle"]["required"], row["invariants"])
                self.assertTrue(row["catalog_closure"])

    def test_mixed_timed_actions_keep_fixed_phase_offsets(self):
        recipe = {"mechanic_spec": {"schema_version": 1, "actions": [],
            "timed_actions": [{"id": "final", "phase": "burst", "offset_ms": 8000,
                               "bot": 0, "kind": "command", "command": "setblock 64 64 0 diamond_block"}]}}
        spec = load.mechanic_spec(recipe)
        self.assertNotIn("timed_actions", spec)
        self.assertEqual(spec["actions"][0]["window"], "burst")
        self.assertEqual(spec["actions"][0]["at_ms"], 8000)
        self.assertEqual(spec["actions"][0]["id"], "final")
        self.assertIn("timed_actions", recipe["mechanic_spec"])

    def test_wire_only_crafting_menu_is_not_a_persisted_container(self):
        recipe = {"mechanic_spec": {"schema_version": 1,
            "blocks": [{"name": "table", "position": [32, 64, -92]}],
            "containers": [
                {"name": "chest_a", "position": [32, 64, -96], "bot": 0, "slots": 27},
                {"name": "chest_b", "position": [64, 64, -96], "bot": 1, "slots": 27,
                 "wire_only": False},
                {"name": "craft", "position": [32, 64, -92], "bot": 0, "slots": 10,
                 "wire_only": True}]}}
        before = json.dumps(recipe, sort_keys=True)
        live = load.mechanic_spec(recipe)
        native = load.mechanic_spec(recipe, native=True)
        self.assertEqual([probe["name"] for probe in live["containers"]], ["chest_a", "chest_b", "craft"])
        self.assertEqual([probe["name"] for probe in native["containers"]], ["chest_a", "chest_b"])
        self.assertEqual(live["blocks"], native["blocks"])
        self.assertEqual(native["blocks"], recipe["mechanic_spec"]["blocks"])
        self.assertTrue(all("wire_only" not in probe for spec in (live, native)
                            for probe in spec["containers"]))
        self.assertEqual(json.dumps(recipe, sort_keys=True), before)

    def test_burst_cohort_uses_fresh_unassisted_process_and_finite_first_action(self):
        async def exercise(directory):
            args = SimpleNamespace(bots_bin=Path("/bots"), view_distance=4,
                                   readiness_seconds=30, burst_seconds=10)
            stdin = SimpleNamespace(write=lambda _: None, drain=AsyncMock())
            session = SimpleNamespace(directory=directory, spawn=AsyncMock(),
                ready=AsyncMock(return_value={"ready_unix_ns": 120}),
                processes={"bots": SimpleNamespace(stdin=stdin)})
            with patch.object(load, "_event", AsyncMock(return_value={
                    "event": "schedule_complete", "open_loop": {"schedule_complete": True}})):
                row = await load.release_cohort(args, session, 25565, 2, 1)
            command = session.spawn.call_args.args[1]
            self.assertIn("--unassisted-login", command)
            self.assertEqual(command[command.index("--clients") + 1], "2")
            self.assertEqual(command[command.index("--name-prefix") + 1], "cohort")
            self.assertGreaterEqual(row["released_unix_ns"], row["scheduled_unix_ns"])
            self.assertEqual(row["requested"], 2)
            schedule = json.loads((directory / "schedule.json").read_text())
            self.assertEqual(schedule["windows"][0]["chat_hz"], 1)
            self.assertEqual(schedule["windows"][1]["chat_hz"], 0)
            session.ready.assert_awaited_once_with(2, 6)
        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(exercise(Path(directory)))

    def test_thousand_client_command_preserves_denominator_and_bounded_group(self):
        args = SimpleNamespace(bots_bin=Path("/bots"), view_distance=6, readiness_seconds=30)
        command = load.bot_command(args, Path("/receipt"), 25565, 1000, {"group_size": 1000})
        self.assertEqual(command[command.index("--clients") + 1], "1000")
        self.assertEqual(command[command.index("--group-size") + 1], "64")

    def test_wire_trace_is_opt_in_and_limits_selection_to_actual_clients(self):
        args = SimpleNamespace(bots_bin=Path("/bots"), view_distance=4,
                               readiness_seconds=30, wire_trace_bots=[3, 6, 7, 9])
        measured = load.bot_command(args, Path("/receipt"), 25565, 10)
        self.assertEqual(measured[measured.index("--wire-trace-bots") + 1], "3,6,7,9")
        self.assertEqual(measured[measured.index("--wire-trace-file") + 1], "/receipt/bots-wire.jsonl")
        preparation = load.bot_command(args, Path("/prep"), 25565, 1)
        self.assertNotIn("--wire-trace-file", preparation)
        args.wire_trace_bots = []
        self.assertNotIn("--wire-trace-file", load.bot_command(args, Path("/receipt"), 25565, 10))

    def test_prepare_concurrency_is_opt_in(self):
        args = SimpleNamespace(bots_bin=Path("/bots"), view_distance=6, readiness_seconds=30)
        self.assertNotIn("--prepare-concurrency", load.bot_command(args, Path("/r"), 25565, 10))
        args.prepare_concurrency = 32
        command = load.bot_command(args, Path("/r"), 25565, 10)
        self.assertEqual(command[command.index("--prepare-concurrency") + 1], "32")


class CatalogPluginEvidenceTests(unittest.TestCase):
    def evidence(self, *, replies=None, log=None, after=None, scenario="plugin-heavy",
                 unrenderable=0, truncated=0, extra_log=""):
        recipe = load.catalog_controls.recipe(scenario, 3, True)
        required = recipe["required_plugins"]
        if log is None:
            log = "INFO plugin.runtime_ready: loaded_packages=6\n"
            for owner in required:
                log += f'INFO plugin.command_result: plugin_id={owner} tick=10 request=storage-write outcome="committed"\n'
                log += (f'INFO plugin.command_result: plugin_id={owner} tick=10 request=send-message outcome="admitted"\n') * 20
            log += 'INFO plugin.command_result: plugin_id=solaris-economy tick=9 request=pay-status-1 outcome="rejected"\n'
        log += extra_log
        if replies is None:
            replies = [{"bot": action["bot"], "window": action["window"],
                        "text": action["expected_feedback"]}
                       for action in recipe["mechanic_spec"]["actions"]
                       if action.get("expected_feedback")]
        before = {"runtime_ready": True, "loaded_packages": 6, "host_operations": 1,
                  "wasm_invocations": 2, "command_results": 3}
        if after is None:
            after = {**before, "host_operations": 100, "wasm_invocations": 200, "command_results": 30}
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / "server.log").write_text(log)
            return load.plugin_proof(directory, recipe, {},
                {"mechanics": {"command_replies": replies,
                    "command_replies_unrenderable": unrenderable,
                    "command_replies_truncated": truncated}},
                {"steady": {"tick_range": [1, 20]}, "burst": {"tick_range": [21, 40]}},
                before, after)

    def test_actual_semantic_replies_and_journal_ack_are_separate(self):
        proof = self.evidence()
        self.assertEqual(proof["successful_replies"], 40)
        self.assertTrue(proof["runtime_ready"])
        self.assertTrue(proof["durable"])
        self.assertEqual(proof["town_name"], "load_matrix")
        self.assertEqual(proof["host_operations"], 99)
        self.assertEqual(proof["journal"]["solaris-towns"]["committed"], 1)
        self.assertFalse(proof["infra_errors"])
        self.assertFalse(proof["errors"])

    def test_error_feedback_and_work_counters_cannot_fake_success(self):
        proof = self.evidence(replies=[{"bot": 0, "window": "steady", "text": "Town not found."}] * 100)
        self.assertEqual(proof["successful_replies"], 0)
        self.assertFalse(proof["durable"])
        self.assertTrue(proof["errors"])
        self.assertEqual(len(proof["unmatched_replies"]), 100)

    def test_missing_and_outside_window_journal_results_fail_closed(self):
        for log in ("", 'INFO plugin.runtime_ready: loaded_packages=6\nINFO plugin.command_result: plugin_id=solaris-towns tick=999 request=send-message outcome="admitted"\n'):
            with self.subTest(log=log):
                proof = self.evidence(log=log)
                self.assertFalse(proof["journal"])
                self.assertTrue(proof["infra_errors"])

    def test_dashboard_counters_are_real_monotone_deltas(self):
        proof = self.evidence(after={"runtime_ready": True, "loaded_packages": 6,
                                    "host_operations": 0, "wasm_invocations": 1})
        self.assertNotIn("host_operations", proof)
        self.assertTrue(any("host_operations" in error for error in proof["infra_errors"]))
        self.assertTrue(any("command_results" in error for error in proof["infra_errors"]))

    def test_command_echoes_and_wrong_actor_cannot_replace_state_queries(self):
        recipe = load.catalog_controls.recipe("plugin-heavy", 2, True)
        for change in ("echo", "wrong_actor", "wrong_balance", "missing_replay"):
            replies = [{"bot": action["bot"], "window": action["window"],
                        "text": action["expected_feedback"]}
                       for action in recipe["mechanic_spec"]["actions"]
                       if action.get("expected_feedback")]
            for row in replies:
                if change == "echo":
                    row["text"] = "/money"
                elif change == "wrong_actor":
                    row["bot"] = 0
                elif change == "wrong_balance" and row["text"].startswith("Balance:"):
                    row["text"] = "Balance: 100 coins."
            if change == "missing_replay":
                replies = [row for row in replies
                           if row["text"] != "That transfer token was already committed."]
            with self.subTest(change=change):
                self.assertTrue(self.evidence(replies=replies)["errors"])

    def test_duplicate_state_reply_is_not_additional_work(self):
        recipe = load.catalog_controls.recipe("plugin-heavy", 2, True)
        replies = [{"bot": action["bot"], "window": action["window"],
                    "text": action["expected_feedback"]}
                   for action in recipe["mechanic_spec"]["actions"]
                   if action.get("expected_feedback")]
        replies.append({"bot": 0, "window": "steady", "text": "Balance: 93 coins."})
        self.assertTrue(self.evidence(replies=replies)["errors"])


    def test_translated_fixture_feedback_never_counts_as_plugin_state(self):
        proof = self.evidence(unrenderable=6)
        self.assertFalse(proof["infra_errors"])
        self.assertFalse(proof["errors"])
        self.assertEqual(proof["unrenderable_replies"], 6)
        self.assertTrue(self.evidence(replies=[], unrenderable=100)["errors"])
        self.assertTrue(self.evidence(truncated=1)["infra_errors"])

    def test_payment_absence_probe_requires_successful_exact_state_proof(self):
        proof = self.evidence()
        self.assertEqual(len(proof["payment_absence_probes"]), 1)
        self.assertFalse(proof["errors"])
        self.assertTrue(self.evidence(replies=[])["errors"])
        duplicate = 'INFO plugin.command_result: plugin_id=solaris-economy tick=9 request=pay-status-2 outcome="rejected"\n'
        self.assertTrue(self.evidence(extra_log=duplicate)["errors"])
        mutation = 'INFO plugin.command_result: plugin_id=solaris-economy tick=9 request=ledger-write outcome="rejected"\n'
        self.assertTrue(self.evidence(extra_log=mutation)["errors"])


class CatalogEventTests(unittest.IsolatedAsyncioTestCase):

    async def test_deployment_copies_only_required_packages_without_rewriting_manifests(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            recipe = load.catalog_controls.recipe("plugin-heavy", 2, True)
            for name in recipe["required_plugins"]:
                package = source / name
                package.mkdir(parents=True)
                (package / "plugin.toml").write_text(f'id = "{name}"\n')
                (package / "plugin.wasm").write_bytes(b"fixture")
            phase = SimpleNamespace(spawn=AsyncMock(side_effect=RuntimeError("stop before startup")),
                                    stop=AsyncMock(), shutdown={})
            directory = root / "prepare"
            args = SimpleNamespace(plugins_dir=source, server_bin=Path("/server"))
            with patch.object(load, "Session", return_value=phase), \
                 patch.object(load, "config", return_value=(directory / "server.toml", "http://local", 1)):
                with self.assertRaisesRegex(RuntimeError, "stop before startup"):
                    await load.prepare_control(args, directory, recipe)
            self.assertEqual({path.name for path in (directory / "plugins").iterdir()},
                             set(recipe["required_plugins"]))
            for name in recipe["required_plugins"]:
                self.assertEqual((directory / "plugins" / name / "plugin.toml").read_bytes(),
                                 (source / name / "plugin.toml").read_bytes())

    async def test_deployment_missing_wasm_fails_before_server_start(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            (source / "solaris-towns").mkdir(parents=True)
            (source / "solaris-towns/plugin.toml").write_text('id = "solaris-towns"\n')
            with self.assertRaises(load.ClassifiedFailure) as raised:
                await load.prepare_control(SimpleNamespace(plugins_dir=source),
                    root / "prepare", load.catalog_controls.recipe("plugin-heavy", 2, True))
            self.assertEqual(raised.exception.outcome, "INFRA_BLOCKED")

    async def test_public_region_uses_explicit_supported_save_barrier(self):
        region = {"name": "shared", "view_distance": 4, "anchor": [32, 64, 32],
                  "commands": ["time set 2000"]}
        for save_command in ("save-all", "save-all flush"):
            with self.subTest(save_command=save_command), tempfile.TemporaryDirectory() as directory:
                phase = SimpleNamespace(spawn=AsyncMock(), ready=AsyncMock(return_value={"clients": 1}),
                                        stop=AsyncMock(), shutdown={}, process_samples=lambda: {})
                root = Path(directory) / "region"
                with patch.object(load, "Session", return_value=phase), \
                     patch.object(load, "bot_command", return_value=["bots"]) as bot_command, \
                     patch.object(load, "_await_fixture_checkpoint", new=AsyncMock(return_value={})):
                    await load.public_region(SimpleNamespace(readiness_seconds=1, view_distance=8),
                                             root, 25565, region, save_command=save_command)
                self.assertEqual(bot_command.call_args.args[0].view_distance, 8)
                command = phase.spawn.call_args.args[1]
                self.assertIn("--open-loop", command)
                schedule = Path(command[command.index("--schedule-file") + 1])
                self.assertEqual(schedule, root / "schedule.json")
                self.assertEqual(json.loads(schedule.read_text()),
                                 {"windows": [{"name": "steady", "duration_ms": 1000}]})
                commands = json.loads((root / "commands.json").read_text())
                self.assertEqual(commands["setup"], ["time set 2000", save_command])
                self.assertEqual(set(commands), {"setup"})
                phase.stop.assert_awaited_once()

    async def test_fixture_commits_without_one_feedback_per_setup_command(self):
        session = load.Session(None, Path("."))
        session.push_event({"event": "fixture_commands_sent", "commands_sent": 17})
        session.push_event({"event": "fixture_checkpoint_saved", "bot": 0})
        receipt = await load._await_fixture_checkpoint(session, 30)
        self.assertEqual(receipt["checkpoint_event"]["event"], "fixture_checkpoint_saved")
        self.assertEqual(receipt["command_feedback"], 0)

    async def test_feedback_count_alone_never_proves_fixture_commit(self):
        session = load.Session(None, Path("."))
        session.latest_bot = {"command_feedback_received": 1_000}
        with patch.object(session.events, "get", side_effect=asyncio.TimeoutError):
            with self.assertRaises(load.ClassifiedFailure) as raised:
                await load._await_fixture_checkpoint(session, 30)
        self.assertEqual(raised.exception.outcome, "SETUP_FAILED")

    async def test_full_event_queue_never_blocks_terminal_events(self):
        session = load.Session(None, Path("."))
        for index in range(200):
            session.push_event({"event": "sample", "sample": index})
        session.push_event({"event": "failure", "error": "observed failure"})
        self.assertEqual(session.events.qsize(), 128)
        with self.assertRaises(load.ClassifiedFailure) as raised:
            session.alive()
        self.assertEqual(raised.exception.outcome, "CORRECTNESS_FAIL")

    async def test_large_final_line_is_consumed_without_reader_backpressure(self):
        reader = asyncio.StreamReader(limit=64 * 1024 * 1024)
        payload = {"event": "final", "padding": "x" * 70_000}
        reader.feed_data(json.dumps(payload).encode() + b"\n")
        reader.feed_eof()
        session = load.Session(None, Path("."))
        await session.read_events(SimpleNamespace(stdout=reader), io.BytesIO())
        self.assertEqual(session.latest_bot, payload)
        self.assertIsNone(session.failure)

    async def test_child_death_is_not_misclassified_as_missing_telemetry(self):
        session = load.Session(None, Path("."))
        session.processes["server"] = SimpleNamespace(returncode=101)
        with patch.object(load.scale, "http_json", side_effect=OSError("connection reset")):
            with self.assertRaises(load.ClassifiedFailure) as raised:
                await session.http("http://127.0.0.1:1/load-metrics", 1)
        self.assertEqual(raised.exception.outcome, "CORRECTNESS_FAIL")

    async def test_checkpoint_ack_survives_consumed_ready_and_queue_eviction(self):
        reader = asyncio.StreamReader(limit=64 * 1024 * 1024)
        events = [{"event": "fixture_commands_sent", "commands_sent": 1},
                  {"event": "fixture_checkpoint_saved", "bot": 0}, {"event": "ready", "clients": 1}]
        reader.feed_data(b"".join(json.dumps(row).encode() + b"\n" for row in events))
        reader.feed_eof()
        session = load.Session(None, Path("."))
        await session.read_events(SimpleNamespace(stdout=reader), io.BytesIO())
        self.assertEqual((await session.ready(1, 1))["clients"], 1)
        for index in range(200):
            session.push_event({"event": "sample", "sample": index})
        receipt = await load._await_fixture_checkpoint(session, 1)
        self.assertEqual(receipt["commands_sent_event"]["commands_sent"], 1)
        self.assertEqual(receipt["checkpoint_event"]["event"], "fixture_checkpoint_saved")
        self.assertGreater(receipt["checkpoint_event"]["captured_unix_ns"], 0)

    async def test_preparation_schedule_survives_later_save_ack_and_queue_eviction(self):
        completion = {"event": "sample", "open_loop": {"schedule_complete": True}}
        session = load.Session(None, Path("."))
        session.push_event(completion)
        session.push_event({"event": "fixture_checkpoint_saved", "bot": 0})
        for index in range(200):
            session.push_event({"event": "sample", "sample": index})
        self.assertEqual(await load._await_preparation_schedule(session, 1), completion)
        self.assertIsNone(session.live_complete)

    async def test_preparation_save_ack_alone_cannot_prove_schedule_complete(self):
        session = load.Session(None, Path("."))
        session.push_event({"event": "fixture_checkpoint_saved", "bot": 0})
        with patch.object(session.events, "get", side_effect=asyncio.TimeoutError):
            with self.assertRaisesRegex(load.ClassifiedFailure, "missing completed preparation schedule"):
                await load._await_preparation_schedule(session, 1)


    async def test_native_restart_retains_known_absence_attribution(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            world = root / "world"
            world.mkdir()
            args = SimpleNamespace(seed=42, vanilla_data_dir=root, bots_bin=Path("/bots"),
                                   readiness_seconds=1)
            async def probe(*command, **_):
                report = Path(command[command.index("--report") + 1])
                report.write_text(json.dumps({"mechanics": {"entities": [], "checkpoint_tick": 10}}))
                return SimpleNamespace(returncode=0, wait=AsyncMock(return_value=0))
            initial = {"absence_probes": [{"uuid": "removed", "kind": "minecraft:armor_stand",
                "known_initial": True, "full_native_index_checked": True, "active": False}]}
            with patch.object(load.asyncio, "create_subprocess_exec", new=probe):
                result = await load.native_snapshot(args, root / "snapshot", world, root / "config",
                                                    {}, initial)
            self.assertEqual(result["absence_probes"][0]["uuid"], "removed")
            self.assertTrue(result["absence_probes"][0]["known_initial"])
            self.assertFalse(result["absence_probes"][0]["active"])

    async def test_failed_rung_keeps_classification_and_hashes_in_its_receipt(self):
        failures = [
            (load.ClassifiedFailure("SETUP_FAILED", "all-client readiness deadline expired"), "SETUP_FAILED"),
            (load.ClassifiedFailure("CORRECTNESS_FAIL", "bots exited: 1"), "CORRECTNESS_FAIL"),
            (OSError("metrics connection reset"), "INFRA_BLOCKED"),
        ]
        for error, outcome in failures:
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                world = root / "world"
                world.mkdir()
                config = root / "server.toml"
                config.write_text("seed = 42\n")
                args = SimpleNamespace(steady_seconds=1, burst_seconds=1, recovery_seconds=1,
                    server_bin=Path("/server"), readiness_seconds=1)
                stop = AsyncMock()
                with patch.object(load, "config", return_value=(config, "http://unused", 1)), \
                     patch.object(load.Session, "spawn", new=AsyncMock()), \
                     patch.object(load.Session, "stop", new=stop), \
                     patch.object(load, "wait_dashboard", new=AsyncMock(side_effect=error)):
                    result = await load.measure(args, {"id": "JOIN-01"}, root / "rung",
                                                {"world": str(world)}, 10)
                stop.assert_awaited_once()
                stored = json.loads((root / "rung/result.json").read_text())
                self.assertEqual(stored, result)
                self.assertEqual(stored["outcome"], outcome)
                self.assertFalse(stored["measurement_complete"])
                self.assertIn(str(error), stored["reason"])
                self.assertEqual(len(stored["config_sha256"]), 64)
                self.assertEqual(len(stored["command_sha256"]), 64)



if __name__ == "__main__":
    unittest.main()
