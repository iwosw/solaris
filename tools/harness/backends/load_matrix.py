#!/usr/bin/env python3
"""External-release load matrix; prebuilt binaries only, no implicit builds.

Canonical: python3 -m tools.harness.backends.load_matrix --server-bin PATH
--bots-bin PATH --fixture-bin PATH --label release-main --out PATH.
Use --no-single-core for full-duration off-window lifecycle diagnostics, or
--smoke-ticks N for shortened smokes. Neither establishes a canonical baseline.
"""
from __future__ import annotations

import argparse
import asyncio
import copy
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import shutil
import signal
import sys
import time
import tomllib
from typing import Any

from . import player_scale as scale

ROOT = scale.ROOT
MATRIX_SCENARIOS = ("dense-hostile", "spread-passive", "explore", "built-base", "fluids",
             "village", "combat", "projectiles", "farms", "plugin-heavy",
             "checkpoint-restart", "long-session")


def value_at(value: Any, path: str) -> Any:
    if not isinstance(value, dict):
        return None
    if path in value:
        return value[path]
    # Instrumented stage names themselves contain dots.
    keys = [key for key in value if path.startswith(key + ".")]
    if not keys:
        return None
    key = max(keys, key=len)
    return value_at(value[key], path[len(key) + 1:])


def cpu_list(raw: str) -> list[int]:
    result: set[int] = set()
    try:
        for group in raw.split(","):
            bounds = [int(v) for v in group.split("-")]
            if len(bounds) == 1:
                result.add(bounds[0])
            elif len(bounds) == 2 and bounds[0] <= bounds[1]:
                result.update(range(bounds[0], bounds[1] + 1))
            else:
                raise ValueError
    except ValueError as error:
        raise argparse.ArgumentTypeError("expected a CPU list such as 0,4 or 0-1,4-11") from error
    if not result or min(result) < 0 or {2, 3} & result:
        raise argparse.ArgumentTypeError("smoke CPUs must be nonnegative and exclude CPUs 2/3")
    return sorted(result)


def tree_facts(directory: Path) -> dict[str, str]:
    return {p.relative_to(directory).as_posix(): scale.binary_sha256(p)
            for p in sorted(directory.rglob("*")) if p.is_file()}


def thread_sample(pid: int) -> list[dict[str, Any]]:
    rows = []
    for task in sorted(Path(f"/proc/{pid}/task").iterdir()):
        try:
            text = (task / "stat").read_text()
            fields = text.rsplit(")", 1)[1].split()
            rows.append({"tid": int(task.name), "comm": text.split("(", 1)[1].rsplit(")", 1)[0],
                         "start_time_ticks": int(fields[19]),
                         "cpu_seconds": (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")})
        except FileNotFoundError:
            continue  # A thread may end during the snapshot; attribution stays incomplete.
    return rows


def load_configs(directory: Path, selected: list[str]) -> list[dict[str, Any]]:
    configs = {}
    for path in sorted(directory.glob("*.json")):
        config = json.loads(path.read_text())
        required = ("id", "description", "clients", "view_distance", "spacing", "group_size",
                    "warmup_ticks", "measure_ticks", "fixture")
        if not isinstance(config, dict) or any(key not in config for key in required):
            raise scale.RunFailure(f"invalid scenario config: {path}")
        name = config["id"]
        if name not in MATRIX_SCENARIOS or name in configs or config["fixture"] != name:
            raise scale.RunFailure(f"unknown/duplicate scenario or mismatched fixture in {path}")
        config.setdefault("plugin_required", name == "plugin-heavy")
        config.setdefault("restart", name == "checkpoint-restart")
        if (name == "plugin-heavy" and not config["plugin_required"]) or (name == "checkpoint-restart" and not config["restart"]):
            raise scale.RunFailure(f"{path}: required plugin/restart behavior cannot be disabled")
        for key in required[2:-1]:
            if type(config[key]) is not int or config[key] <= 0:
                raise scale.RunFailure(f"{path}: {key} must be a positive integer")
        if config["measure_ticks"] != (12000 if name == "long-session" else 1200):
            raise scale.RunFailure(f"{path}: canonical measure_ticks must be 1200 (long-session: 12000)")
        if config["warmup_ticks"] < 200 or not 1 <= config["view_distance"] <= 32:
            raise scale.RunFailure(f"{path}: warmup must be >=200 ticks; view distance 1..32")
        if config["spacing"] < 256 or config["group_size"] > 64 or config["clients"] > 4096:
            raise scale.RunFailure(f"{path}: scenario exceeds the existing bot CLI bounds")
        if "client_args" in config and (not isinstance(config["client_args"], list)
                                        or not all(isinstance(v, str) for v in config["client_args"])):
            raise scale.RunFailure(f"{path}: client_args must be a string array")
        forbidden = {"--addr", "--clients", "--view-distance", "--spacing", "--group-size",
                     "--report", "--anchors", "--commands-file"}
        if any(token.split("=", 1)[0] in forbidden for token in config.get("client_args", [])):
            raise scale.RunFailure(f"{path}: client_args cannot override the recorded lifecycle parameters")
        if "commands" in config:
            commands = config["commands"]
            if not isinstance(commands, dict) or set(commands) - {"setup", "recurring"}:
                raise scale.RunFailure(f"{path}: invalid commands object")
            groups = [commands.get("setup", [])]
            for recurring in commands.get("recurring", []):
                if not isinstance(recurring, dict) or type(recurring.get("every_ticks")) is not int or recurring["every_ticks"] <= 0:
                    raise scale.RunFailure(f"{path}: recurring commands require positive every_ticks")
                groups.append(recurring.get("commands", []))
            if any(not isinstance(group, list) or not all(isinstance(command, str) and command.strip()
                   and "\n" not in command for command in group) for group in groups):
                raise scale.RunFailure(f"{path}: commands must be nonempty single-line strings")
        config["config_path"] = str(path.resolve())
        config["config_sha256"] = scale.binary_sha256(path)
        configs[name] = config
    names = selected or list(MATRIX_SCENARIOS)
    missing = [name for name in names if name not in configs]
    if missing:
        raise scale.RunFailure(f"missing scenario configs: {', '.join(missing)}")
    return [configs[name] for name in names]


async def command_report(command: list[str], directory: Path, name: str, deadline: float,
                         interrupted: asyncio.Event) -> dict[str, Any]:
    """Bounded one-shot fixture/report tool, not a second server lifecycle."""
    row = {"command": command, "started": scale.stamp(), "log": str(directory / f"{name}.log")}
    process = None
    interrupt_task = None
    wait_task = None
    try:
        with Path(row["log"]).open("wb") as log:
            if interrupted.is_set():
                raise scale.RunFailure("orchestrator interrupted")
            process = await asyncio.create_subprocess_exec(*command, cwd=ROOT, start_new_session=True,
                                                          stdout=log, stderr=log)
            wait_task = asyncio.create_task(process.wait())
            interrupt_task = asyncio.create_task(interrupted.wait())
            done, _ = await asyncio.wait((wait_task, interrupt_task),
                                         timeout=max(0, deadline - time.monotonic()),
                                         return_when=asyncio.FIRST_COMPLETED)
            if wait_task not in done or interrupted.is_set():
                raise scale.RunFailure(f"{name}: interrupted or overall deadline exceeded")
            row["returncode"] = wait_task.result()
            if row["returncode"] != 0:
                raise scale.RunFailure(f"{name} exited {row['returncode']}; see {row['log']}")
    finally:
        if process is not None and process.returncode is None:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            await process.wait()
        for task in (wait_task, interrupt_task):
            if task is not None:
                task.cancel()
        await asyncio.gather(*(t for t in (wait_task, interrupt_task) if t is not None), return_exceptions=True)
        row["finished"] = scale.stamp()
        row["returncode"] = process.returncode if process is not None else None
        scale.dump(directory / f"{name}-command.json", row)
    return row


class MatrixRung(scale.Rung):
    """Keep Rung's readiness, tick window, safety limits, profiler and cleanup."""
    def __init__(self, args: argparse.Namespace, config: dict[str, Any], fixture: dict[str, Any],
                 directory: Path, deadline: float, interrupted: asyncio.Event) -> None:
        super().__init__(args, config["clients"], directory, deadline, interrupted)
        self.config, self.fixture = config, fixture
        if args.smoke_ticks or not args.single_core:
            self.server_cpus = str(args.smoke_cpus[0])
            self.server_cores = 1
        self.bot_cpus = ",".join(map(str, args.smoke_cpus[1:])) if args.smoke_ticks else scale.BOT_CPUS
        self.bot_cores = len(args.smoke_cpus) - 1 if args.smoke_ticks else 8
        self.receipt.update({"scenario": config, "label": args.label,
                             "fixture": fixture, "server_cpus": self.server_cpus,
                             "bot_cpus": self.bot_cpus, "canonical": not args.smoke_ticks and args.single_core,
                             "server_affinity_logical_cpus": self.server_cores,
                             "chunk_worker_threads_override": 1,
                             "runner_command": args.runner_command,
                             "proof_scope": "Single-core external release scenario; not a VPS capacity claim."})
        anchors = fixture.get("anchors")
        if anchors:
            self.anchor_path = directory / "anchors.json"
            if isinstance(anchors, str):
                anchors = json.loads(Path(anchors).read_text())
            if not isinstance(anchors, list) or len(anchors) < (self.clients + args.group_size - 1) // args.group_size:
                raise scale.RunFailure("fixture lacks enough anchors for requested client groups")
            scale.dump(self.anchor_path, anchors)
        self.checkpoint_tick: int | None = None
        self.checkpoints: list[dict[str, Any]] = []

    async def spawn(self, name: str, command: list[str], **kwargs: Any) -> asyncio.subprocess.Process:
        if name == "server":
            config_path = Path(self.receipt["config"])
            if self.args.plugin_dir and self.config.get("plugin_required"):
                target = self.directory / "plugins" / self.args.plugin_dir.name
                shutil.copytree(self.args.plugin_dir, target)
                self.receipt["plugin_package"] = {"source": str(self.args.plugin_dir),
                                                   "files_sha256": tree_facts(target)}
            if self.args.pregenerated_world:
                world = self.directory / "world"
                source = tree_facts(self.args.pregenerated_world)
                copied = tree_facts(world)
                if copied != source:
                    raise scale.RunFailure("world copy differs from exact fixture/saved-world bytes before identity rebinding")
                proof = {"source": str(self.args.pregenerated_world),
                         "source_files_sha256": source, "copied_files_sha256_before": copied,
                         "raw_copy_exact": True}
                self.receipt["world_copy_proof"] = proof
                report_path = self.directory / "world-identity.json"
                rebind_command = ["taskset", "-c", ",".join(map(str, self.args.smoke_cpus)),
                                  str(self.args.fixture_bin), "--scenario", self.config["fixture"],
                                  "--world", str(world), "--report", str(report_path),
                                  "--rebind-world-identity"]
                evidence = await command_report(rebind_command, self.directory, "world-identity",
                                                self.deadline, self.interrupted)
                identity = json.loads(report_path.read_text())
                rebound = tree_facts(world)
                changed = sorted(path for path in source.keys() | rebound.keys()
                                 if source.get(path) != rebound.get(path))
                proof.update({"copied_files_sha256_after": rebound, "changed_files": changed,
                              "exact_copy_after_rebind": rebound == source,
                              "identity_transformation": identity, "command_receipt": evidence,
                              "scope": "Exact raw directory copy followed by production metadata path-identity rebinding; only solaris/world.dat may be re-encoded. All other files remain byte-identical."})
                if (identity.get("status") != "passed"
                        or identity.get("expected_identity") != str(world)
                        or identity.get("semantic_metadata_preserved") is not True):
                    raise scale.RunFailure("copied world identity rebinding did not prove semantic metadata preservation")
                if identity.get("metadata_present") is True:
                    if (changed not in ([], ["solaris/world.dat"])
                            or identity.get("after", {}).get("world_identity") != str(world)):
                        raise scale.RunFailure("world copy changed beyond the production metadata identity transformation")
                elif identity.get("metadata_present") is not False or changed:
                    raise scale.RunFailure("fresh world metadata rebinding must not fabricate or change files")
                if self.config["id"] == "explore" and not source:
                    self.receipt["world_preparation"] = {
                        "mode": "fresh_empty_world",
                        "source": str(self.args.pregenerated_world),
                        "scope": "Certified dry anchors only; no pregenerated chunks copied.",
                    }
            if not self.args.single_core:
                config_path.write_text(config_path.read_text() + "[chunk_pipeline]\nworker_threads = 1\n")
        elif name == "bots":
            command = command.copy()
            command[2] = self.bot_cpus
            command.extend(self.config.get("client_args", []))
            if self.config.get("commands"):
                commands_path = self.directory / "commands.json"
                scale.dump(commands_path, self.config["commands"])
                command.extend(["--commands-file", str(commands_path)])
        return await super().spawn(name, command, **kwargs)

    async def start_profiler(self, phase: str) -> None:
        if phase == "measurement":
            await super().start_profiler(phase)

    async def stop_process(self, name: str, **kwargs: Any) -> None:
        await super().stop_process(name, **kwargs)
        row = self.receipt["shutdown"].get(name)
        if name != "profiler" or self.args.profile != "perf" or not row:
            return
        log = self.directory / "profiler.log"
        # perf record writes its capture, then re-raises the stop signal, so a clean stop
        # reports -SIGINT. Accept only that exact status with a reported complete capture.
        if (row["returncode"] == -signal.SIGINT and not row["forced"] and log.is_file()
                and "Captured and wrote" in log.read_text(errors="replace")):
            row.update({"raw_returncode": row["returncode"], "returncode": 0,
                        "normalized": "perf record re-raised SIGINT after reporting a complete capture"})

    def record(self, event: dict[str, Any]) -> None:
        if event.get("event") == "sample" and "bots" in event:
            event["bots"]["cpu_basis_cores"] = self.bot_cores
            cpu = event["bots"]["cpu_percent_one_core"]
            event["bots"]["cpu_percent_affinity_basis"] = cpu / self.bot_cores if cpu is not None else None
        super().record(event)

    def sample_processes(self, stats: dict[str, Any], **kwargs: Any) -> dict[str, Any]:
        row = super().sample_processes(stats, **kwargs)
        if "server" in self.processes:
            row["server_threads"] = thread_sample(self.processes["server"].pid)
            self.record({"event": "thread_snapshot", "simulation_tick": stats.get("simulation_tick"),
                         "threads": row["server_threads"]})
        return row

    async def next_stats(self, **kwargs: Any) -> dict[str, Any]:
        row = await super().next_stats(**kwargs)
        if self.config.get("restart") and self.receipt["phase"] == "measurement":
            tick = row["dashboard"]["simulation_tick"]
            if self.checkpoint_tick is None or tick - self.checkpoint_tick >= 200:
                server = self.processes["server"]
                if server.stdin is None:
                    raise scale.RunFailure("checkpoint server console unavailable")
                server.stdin.write(b"save-all\n")
                await server.stdin.drain()
                self.checkpoint_tick = tick
                self.checkpoints.append({"command": "save-all", "simulation_tick": tick, **scale.stamp()})
                self.record({"event": "checkpoint_requested", **self.checkpoints[-1]})
        return row

    async def run(self) -> dict[str, Any]:
        receipt = await super().run()
        if self.config.get("restart") and "measurement" in receipt:
            log = (self.directory / "server.log").read_text(errors="replace")
            completed = log.count("Save completed successfully.")
            receipt["live_checkpoints"] = {"requests": self.checkpoints, "completed_log_replies": completed,
                                          "proof": "ordinary save-all while bots remained connected"}
            if not self.checkpoints or completed < len(self.checkpoints):
                receipt["status"] = "failed"
                receipt["stop_reason"] = "live checkpoint completion was not acknowledged in server console output"
        scale.dump(self.directory / "receipt.json", receipt)
        return receipt

    async def native_profile(self, name: str) -> dict[str, Any]:
        profile = await super().native_profile(name)
        if name == "measurement-start-profile":
            await self.start_profiler("measurement")
        return profile

    def summarize(self, start: dict[str, Any], end: dict[str, Any]) -> None:
        super().summarize(start, end)
        measure = self.receipt["measurement"]
        measure["dashboard_start"] = start["dashboard"]
        measure["dashboard_end"] = end["dashboard"]
        measure["bots"]["cpu_basis_cores"] = self.bot_cores
        measure["bots"]["cpu_percent_affinity_basis"] = measure["bots"]["cpu_percent_one_core"] / self.bot_cores
        first = {row["tid"]: row for row in start.get("server_threads", []) if row["comm"].startswith("solaris-own-")}
        last = {row["tid"]: row for row in end.get("server_threads", []) if row["comm"].startswith("solaris-own-")}
        stable = bool(first) and first.keys() == last.keys() and all(
            first[tid]["start_time_ticks"] == last[tid]["start_time_ticks"] for tid in first)
        seconds = sum(last[tid]["cpu_seconds"] - row["cpu_seconds"] for tid, row in first.items()) if stable else None
        measure["owner_cpu"] = {"scope": "regional owner threads comm prefix solaris-own-; excludes ticker/coordinator and other process CPU",
            "complete_stable_thread_set": stable, "start": list(first.values()), "end": list(last.values()),
            "cpu_seconds": seconds, "cpu_percent_one_core": 100 * seconds / measure["elapsed_seconds"] if seconds is not None else None}
        snapshots = [{"simulation_tick": row["dashboard"]["simulation_tick"],
                      "monotonic_ns": row["monotonic_ns"], "rss_bytes": row["server"]["rss_bytes"]}
                     for row in [start, *self.measured]]
        elapsed = measure["elapsed_seconds"]
        xs = [(row["monotonic_ns"] - start["monotonic_ns"]) / 1e9 for row in [start, *self.measured]]
        ys = [row["server"]["rss_bytes"] for row in [start, *self.measured]]
        tick_xs = [row["dashboard"]["simulation_tick"] - start["dashboard"]["simulation_tick"]
                   for row in [start, *self.measured]]
        tick_span = end["dashboard"]["simulation_tick"] - start["dashboard"]["simulation_tick"]
        mean_tick = sum(tick_xs) / len(tick_xs)
        tick_variance = sum((tick - mean_tick) ** 2 for tick in tick_xs)
        mean_x, mean_y = sum(xs) / len(xs), sum(ys) / len(ys)
        variance = sum((x - mean_x) ** 2 for x in xs)
        measure["rss_growth"] = {"snapshots": snapshots,
            "endpoint_slope_bytes_per_second": (end["server"]["rss_bytes"] - start["server"]["rss_bytes"]) / elapsed,
            "least_squares_slope_bytes_per_second": sum((x - mean_x) * (y - mean_y) for x, y in zip(xs, ys)) / variance if variance else None,
            "endpoint_slope_bytes_per_tick": (end["server"]["rss_bytes"] - start["server"]["rss_bytes"]) / tick_span if tick_span else None,
            "least_squares_slope_bytes_per_tick": sum((tick - mean_tick) * (rss - mean_y) for tick, rss in zip(tick_xs, ys)) / tick_variance if tick_variance else None,
            "scope": "Measurement-window RSS snapshots; slopes are not leak attribution, allocator retention or a startup-inclusive peak."}
        if not self.receipt["canonical"] and self.receipt["status"] == "passed":
            self.receipt["status"] = "smoke_only"
            self.receipt["acceptance"]["baseline_holds"] = False


def activity_proof(receipt: dict[str, Any], config: dict[str, Any], fixture: dict[str, Any]) -> dict[str, Any]:
    measure = receipt.get("measurement", {})
    start_profile = json.loads(Path(receipt["measurement-start-profile"]).read_text())
    end_profile = json.loads(Path(receipt["measurement-end-profile"]).read_text())
    requirements = config.get("activity_requirements", fixture.get("activity_requirements", []))
    if not requirements:
        raise scale.RunFailure("scenario lacks numeric runtime activity requirements; fixture placement is not activity proof")
    proofs = []
    sources = {"profile": (start_profile, end_profile),
               "dashboard": (measure["dashboard_start"], measure["dashboard_end"]),
               "stage": tuple({row["stage"]: row for row in p["cpu"]["stages"]} for p in (start_profile, end_profile)),
               "bots": (measure["bots_start"], measure["bots_end"]),
               "saved_world": (fixture, receipt.get("saved_world", {}))}
    for requirement in requirements:
        source = requirement["source"]
        if source not in sources:
            raise scale.RunFailure(f"unknown activity source {source}")
        comparison = requirement.get("comparison", "delta")
        if comparison not in ("delta", "changed"):
            raise scale.RunFailure(f"unknown activity comparison {comparison}")
        minimum = requirement.get("minimum_delta", 1)
        if type(minimum) not in (int, float) or minimum <= 0:
            raise scale.RunFailure("activity minimum_delta must be a positive number")
        before, after = (value_at(v, requirement["path"]) for v in sources[source])
        delta = after - before if type(before) in (int, float) and type(after) in (int, float) else None
        passed = (before is not None and after is not None and before != after
                  if comparison == "changed" else delta is not None and delta >= minimum)
        proofs.append({**requirement, "start": before, "end": after, "delta": delta, "passed": passed})
    return {"passed": all(row["passed"] for row in proofs), "requirements": proofs,
            "scope": "Profile/dashboard/bot measurement-boundary deltas; saved_world spans fixture preparation through clean shutdown (includes warmup), never a measurement-only mutation count."}


def stage_ranking(receipt: dict[str, Any], end_profile: str = "measurement-end-profile") -> list[dict[str, Any]]:
    first = json.loads(Path(receipt["measurement-start-profile"]).read_text())
    last = json.loads(Path(receipt[end_profile]).read_text())
    starts = {row["stage"]: row["cpu_ns_total"] for row in first["cpu"]["stages"]}
    process_delta = last["cpu"]["process_cpu_ns_total"] - first["cpu"]["process_cpu_ns_total"]
    rows = [{"stage": row["stage"], "cpu_ns": row["cpu_ns_total"] - starts[row["stage"]],
             "process_cpu_share_percent": 100 * (row["cpu_ns_total"] - starts[row["stage"]]) / process_delta if process_delta > 0 else None}
            for row in last["cpu"]["stages"] if row["stage"] in starts]
    return sorted(rows, key=lambda row: row["cpu_ns"], reverse=True)


async def perf_ranking(receipt: dict[str, Any], directory: Path, deadline: float,
                       interrupted: asyncio.Event, *, partial: bool = False) -> dict[str, Any]:
    profiler = receipt.get("profiler")
    if not profiler or profiler["mode"] != "perf":
        return {"top5": None, "saving_share_percent": None, "reason": "perf self-symbol report not requested (stage CPU ranking remains available)"}
    command = ["perf", "report", "--stdio", "--no-children", "--percent-limit", "0", "--sort", "symbol",
               "--call-graph", "none", "--field-separator", "\t", "--show-nr-samples", "-i", profiler["output"]]
    evidence = await command_report(command, directory, "perf-report", deadline, interrupted)
    rows = []
    for line in Path(evidence["log"]).read_text().splitlines():
        fields = [v.strip() for v in line.split("\t")]
        if len(fields) >= 3 and re.fullmatch(r"\d+(?:\.\d+)?%?", fields[0]) and fields[1].isdigit():
            rows.append({"self_share_percent": float(fields[0].rstrip("%")), "samples": int(fields[1]), "symbol": fields[2]})
    if not rows or sum(row["samples"] for row in rows) <= 0:
        raise scale.RunFailure("perf report contains no usable self-share samples")
    saving = [row for row in rows if re.search(r"persist|save|flush|checkpoint", row["symbol"], re.I)]
    return {"command": evidence, "top5": sorted(rows, key=lambda row: row["self_share_percent"], reverse=True)[:5],
            "samples": sum(row["samples"] for row in rows),
            "saving_share_percent": sum(row["self_share_percent"] for row in saving), "saving_symbols": saving,
            "profiler_shutdown": receipt.get("shutdown", {}).get("profiler"),
            "scope": ("Partial failed measurement capture, from measurement profiler start through profiler stop; not a completed requested-tick window. "
                      if partial else "Measurement-phase capture. ") +
                     "Perf event-weighted self shares (cycles sampling); symbol-name matched persistence/save/flush/checkpoint, not exact CPU time or I/O wall time. Nonzero profiler shutdown remains a lifecycle failure even when the capture is readable."}


def completed_lifecycle(receipt: dict[str, Any]) -> bool:
    return ("measurement-end-profile" in receipt and not receipt.get("errors")
            and bool(receipt.get("shutdown"))
            and all(row["returncode"] == 0 and not row["forced"] for row in receipt["shutdown"].values()))


def restart_entity_identity_proof(fixture: dict[str, Any], before: dict[str, Any],
                                  after: dict[str, Any]) -> dict[str, Any]:
    authored = {(row["uuid"], row["kind"]) for row in fixture["entities"]
                if row["kind"] in ("minecraft:cow", "minecraft:sheep", "minecraft:pig",
                                   "minecraft:chicken", "minecraft:villager")}
    saved = {(row["uuid"], row["kind"]) for row in before["entities"]
             if row["kind"] != "minecraft:item"}
    resaved = {(row["uuid"], row["kind"]) for row in after["entities"]
               if row["kind"] != "minecraft:item"}
    required = saved & authored
    missing = sorted(required - resaved)
    return {
        "authored": {
            "before": sorted(required), "after": sorted(resaved & authored), "missing": missing,
            "retained": bool(required) and not missing,
            "scope": "Saved authored passive-animal/villager UUID/type pairs must survive the live restart and resave. Authored entities already absent before restart are not required; positions may change.",
        },
        "dynamic": {
            "before": sorted(saved - authored), "after": sorted(resaved - authored),
            "missing": sorted((saved - authored) - resaved),
            "scope": "Dynamic living populations have no immortality invariant. Natural hostiles may legally hard-despawn beyond 128 blocks or soft-despawn after more than 600 idle ticks beyond 32 blocks. Missing identities alone prove neither lawful despawn nor save corruption.",
        },
    }


async def enrich(receipt: dict[str, Any], config: dict[str, Any], fixture: dict[str, Any],
                 directory: Path, deadline: float, interrupted: asyncio.Event) -> None:
    if "measurement-end-profile" not in receipt:
        if "measurement-start-profile" not in receipt:
            return
        diagnostic = {
            "scope": "Partial failed measurement diagnostic only: stages end at the failure profile; perf ends at profiler stop. No full requested-tick timing, activity acceptance or capacity claim.",
            "measure_ticks_requested": receipt.get("measure_ticks_requested"),
            "start_profile": receipt["measurement-start-profile"],
            "end_profile": receipt.get("failure-profile"),
            "profiler": receipt.get("profiler"),
            "profiler_shutdown": receipt.get("shutdown", {}).get("profiler"),
            "errors": [],
        }
        receipt["partial_measurement_diagnostic"] = diagnostic
        # Keep the original failed verdict, reason and shutdown errors unchanged.
        if "failure-profile" in receipt:
            try:
                receipt["stage_cpu_ranking"] = stage_ranking(receipt, "failure-profile")
            except Exception as error:
                diagnostic["errors"].append({"type": "stage_cpu_ranking", "message": str(error)})
        try:
            receipt["perf_shares"] = await perf_ranking(receipt, directory, deadline, interrupted, partial=True)
        except Exception as error:
            diagnostic["errors"].append({"type": "perf_shares", "message": str(error)})
        return
    # Each evidence stage is independent, so a failed gate still retains hotspot evidence.
    failures: list[Exception] = []

    def record_failure(error: Exception) -> None:
        failures.append(error)
        receipt["errors"].append({"type": type(error).__name__, "message": str(error)})

    for key, build in (("stage_cpu_ranking", lambda: stage_ranking(receipt)),
                       ("perf_shares", lambda: perf_ranking(receipt, directory, deadline, interrupted)),
                       ("activity", lambda: activity_proof(receipt, config, fixture))):
        try:
            value = build()
            receipt[key] = await value if asyncio.iscoroutine(value) else value
        except Exception as error:
            record_failure(error)
    if "activity" in receipt and not receipt["activity"]["passed"]:
        record_failure(scale.RunFailure("required runtime scenario activity not observed"))
    if config.get("plugin_required"):
        try:
            profile = json.loads(Path(receipt["measurement-end-profile"]).read_text())
            if not profile.get("plugins", {}).get("loaded"):
                raise scale.RunFailure("plugin package not reported loaded by external server")
        except Exception as error:
            record_failure(error)
    if failures:
        receipt["status"] = "failed"
        receipt["stop_reason"] = "; ".join(str(error) for error in failures)


def markdown(summary: dict[str, Any], receipts: list[dict[str, Any]]) -> str:
    def number(value: Any, factor: float = 1) -> str:
        return f"{value * factor:.2f}" if type(value) in (int, float) else "unavailable"
    lines = [f"# Load matrix — {summary['label']}", "", f"Status: **{summary['status']}**. Receipt: `{summary['receipt']}`.",
             "", "Smoke is nonauthoritative. Tick percentiles are the final rolling 1200-tick window, not a merged whole-session distribution. Partial failed captures retain stage/perf diagnostics only, never full-window timing metrics.", "",
             "| Scenario | Status | p50/p95/p99 ms | Owner CPU % | Process CPU % | RSS MiB | Chunks | Gen/s | Saving self % (diagnostic) | Receipt |",
             "|---|---|---|---|---|---|---|---|---|---|"]
    for receipt in receipts:
        measurement = receipt.get("measurement", {})
        tick = measurement.get("percentiles", {}).get("total") or {}
        lines.append("| " + " | ".join([receipt["scenario"]["id"], receipt["status"],
            "/".join(number(tick.get(key), .001) for key in ("p50_us", "p95_us", "p99_us")),
            number(measurement.get("owner_cpu", {}).get("cpu_percent_one_core")),
            number(measurement.get("server", {}).get("cpu_percent_one_core")),
            number(measurement.get("server", {}).get("rss_current_bytes"), 1 / 1048576),
            "ticketed {ticketed} / prepared {prepared} / generated_total {generated_total}".format(
                **{key: measurement.get("chunks_end", {}).get(key, "unavailable")
                   for key in ("ticketed", "prepared", "generated_total")}),
            number(measurement.get("generation", {}).get("generated_chunks_per_second")),
            number(receipt.get("perf_shares", {}).get("saving_share_percent")) +
                (" (partial)" if receipt.get("partial_measurement_diagnostic") else ""),
            f"`{receipt['directory']}/receipt.json`"] ) + " |")
    for error in summary.get("errors", []):
        lines.extend(["", f"Matrix failure: {error['type']}: {error['message']}"])
    for receipt in receipts:
        measurement = receipt.get("measurement", {})
        lines.extend(["", f"## {receipt['scenario']['id']}",
                      f"Command: `{shlex.join(receipt.get('runner_command', []))}`",
                      f"Binary SHA256: `{json.dumps(receipt.get('binaries', {}), sort_keys=True)}`"])
        if receipt.get("stop_reason"):
            lines.append(f"Failure/blocker: {receipt['stop_reason']}")
        if receipt.get("partial_measurement_diagnostic"):
            lines.append(receipt["partial_measurement_diagnostic"]["scope"])
        for row in receipt.get("stage_cpu_ranking", [])[:5]:
            lines.append(f"- Stage `{row['stage']}`: {number(row['process_cpu_share_percent'])}% process CPU ({row['cpu_ns']} ns).")
        for row in receipt.get("perf_shares", {}).get("top5") or []:
            lines.append(f"- Perf self `{row['symbol']}`: {row['self_share_percent']:.2f}% ({row['samples']} samples).")
        if receipt["scenario"]["id"] == "long-session":
            lines.append(f"RSS endpoint slope: {number(measurement.get('rss_growth', {}).get('endpoint_slope_bytes_per_second'))} bytes/s; all snapshots in receipt. Not a leak claim.")
    lines.extend(["", "## Hotspot ranking", "", "Ranked diagnostic shares, with partial failed windows explicitly labelled; not comparable full-window capacity measurements. Completed-scope boundary skew and uninstrumented CPU remain explicit."])
    hotspots = sorted(((row["process_cpu_share_percent"], receipt["scenario"]["id"], row["stage"],
                        "partial failed measurement" if receipt.get("partial_measurement_diagnostic") else "completed measurement")
                       for receipt in receipts for row in receipt.get("stage_cpu_ranking", [])
                       if row["process_cpu_share_percent"] is not None), reverse=True)
    lines.extend(f"{index}. `{name}` / `{stage}`: {share:.2f}% process CPU ({scope})."
                 for index, (share, name, stage, scope) in enumerate(hotspots[:20], 1))
    lines.extend(["", "### Perf self-symbol hotspots", "",
                  "Ranked separately from instrumented stages; diagnostic sampled self shares, never children-inclusive shares."])
    symbols = sorted(((row["self_share_percent"], receipt["scenario"]["id"], row["symbol"],
                       "partial failed measurement" if receipt.get("partial_measurement_diagnostic") else "completed measurement")
                      for receipt in receipts for row in receipt.get("perf_shares", {}).get("top5") or []), reverse=True)
    lines.extend(f"{index}. `{name}` / `{symbol}`: {share:.2f}% event-weighted sampled self share ({scope})."
                 for index, (share, name, symbol, scope) in enumerate(symbols[:20], 1))
    # Disconnect diagnostics can contain binary NBT; keep Markdown readable, JSON raw.
    controls = {code: f"\\x{code:02x}" for code in range(32) if code not in (9, 10)}
    return ("\n".join(lines) + "\n").translate(controls)


async def initialize_world(args: argparse.Namespace, config: dict[str, Any], directory: Path,
                           deadline: float, interrupted: asyncio.Event) -> dict[str, Any]:
    """Let the real server create its own writable world identity, never forge it."""
    world = directory / "fixture-world"
    config_path = directory / "fixture-bootstrap.toml"
    plugins = directory / "bootstrap-plugins"
    plugins.mkdir()
    if config.get("plugin_required"):
        shutil.copytree(args.plugin_dir, plugins / args.plugin_dir.name)
    content = f'''[server]
name = "Solaris load matrix fixture bootstrap"
motd = "Offline matrix fixture preparation"
view_distance = 2
simulation_distance = 2
[network]
bind_address = "127.0.0.1"
port = 0
[data]
world_dir = {json.dumps(str(world))}
seed = {args.seed}
worldgen_mode = "vanilla_like"
[auth]
online_mode = false
[autoscale]
enabled = false
[plugins]
directory = {json.dumps(str(plugins))}
[chunk_pipeline]
worker_threads = 1
'''
    if args.vanilla_data_dir:
        content = content.replace(f"seed = {args.seed}\n",
            f"vanilla_data_dir = {json.dumps(str(args.vanilla_data_dir))}\nseed = {args.seed}\n")
    config_path.write_text(content)
    command = ["taskset", "-c", ",".join(map(str, args.smoke_cpus)), str(args.server_bin),
               "--config", str(config_path), "pregenerate", "--from", "0,0", "--to", "0,0"]
    evidence = await command_report(command, directory, "fixture-bootstrap", deadline, interrupted)
    contract = world / "solaris/world.json"
    if not contract.is_file():
        raise scale.RunFailure("real server pregeneration did not create a writable world contract")
    return {"command_receipt": evidence, "contract_sha256": scale.binary_sha256(contract),
            "scope": "Minimum real-server writable-world bootstrap before offline scenario overlay."}


async def fixture_report(args: argparse.Namespace, config: dict[str, Any], world: Path, directory: Path,
                         name: str, deadline: float, interrupted: asyncio.Event, verify: bool = False) -> dict[str, Any]:
    report = directory / f"{name}.json"
    command = ["taskset", "-c", ",".join(map(str, args.smoke_cpus)), str(args.fixture_bin),
               "--scenario", config["fixture"], "--world", str(world), "--report", str(report)]
    if verify:
        command.append("--verify-existing")
    if args.vanilla_data_dir:
        command.extend(["--vanilla-data-dir", str(args.vanilla_data_dir)])
    evidence = await command_report(command, directory, name, deadline, interrupted)
    if not world.is_dir():
        raise scale.RunFailure(f"fixture {name} did not leave an inspectable world directory")
    facts = json.loads(report.read_text())
    if not isinstance(facts, dict) or facts.get("status") not in ("passed", "prepared"):
        raise scale.RunFailure(f"fixture {name} did not prove preparation/readback: {facts}")
    if facts.get("blocked") or facts.get("blockers"):
        raise scale.RunFailure(f"fixture blockers: {facts.get('blocked') or facts.get('blockers')}")
    return {**facts, "report_path": str(report), "generator": {"path": str(args.fixture_bin),
            "sha256": scale.binary_sha256(args.fixture_bin)}, "command_receipt": evidence}


async def run(args: argparse.Namespace) -> int:
    run_dir = args.out / ("matrix-" + dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%S") + "-" + secrets.token_hex(4))
    run_dir.mkdir(parents=True)
    interrupted = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, interrupted.set)
    args.shutdown_deadline = time.monotonic() + args.deadline_seconds
    deadline = args.shutdown_deadline - min(36, args.deadline_seconds / 10)
    summary = {"schema": 1, "label": args.label, "status": "failed", "started": scale.stamp(),
               "run_dir": str(run_dir), "receipt": str(run_dir / "receipt.json"), "runner_command": args.runner_command,
               "canonical": not args.smoke_ticks and args.single_core, "receipts": [], "errors": []}
    receipts: list[dict[str, Any]] = []
    try:
        configs = load_configs(args.config_dir, args.scenario)
        for binary in (args.server_bin, args.bots_bin, args.fixture_bin):
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise scale.RunFailure(f"missing/nonexecutable prebuilt binary: {binary}")
        summary["binaries"] = {name: {"path": str(binary), "sha256": scale.binary_sha256(binary)}
                               for name, binary in (("server", args.server_bin), ("bots", args.bots_bin), ("fixture", args.fixture_bin))}
        for index, config in enumerate(configs):
            directory = run_dir / f"{index + 1:02}-{config['id']}"
            directory.mkdir()
            receipt: dict[str, Any] = {"status": "failed", "scenario": config, "directory": str(directory),
                                       "runner_command": args.runner_command, "label": args.label,
                                       "binaries": summary["binaries"], "errors": []}
            try:
                if interrupted.is_set() or time.monotonic() >= deadline:
                    raise scale.RunFailure("orchestrator interrupted or overall deadline exceeded")
                if config.get("plugin_required"):
                    if not args.plugin_dir or not (args.plugin_dir / "plugin.toml").is_file() or not (args.plugin_dir / "plugin.wasm").is_file():
                        receipt["status"] = "blocked"
                        raise scale.RunFailure("plugin-heavy requires an available real package --plugin-dir containing plugin.toml and plugin.wasm")
                    tomllib.loads((args.plugin_dir / "plugin.toml").read_text())
                bootstrap = None if config["id"] == "explore" else await initialize_world(
                    args, config, directory, deadline, interrupted)
                fixture = await fixture_report(args, config, directory / "fixture-world", directory,
                                               "fixture", deadline, interrupted)
                fixture["bootstrap"] = bootstrap
                rung_args = copy.copy(args)
                if config["id"] == "explore":
                    rung_args.fixture_summary = args.terrain_fixture_summary
                for key in ("view_distance", "spacing", "group_size", "warmup_ticks", "measure_ticks"):
                    setattr(rung_args, key, config[key])
                if args.smoke_ticks:
                    rung_args.measure_ticks = args.smoke_ticks
                    rung_args.warmup_ticks = min(config["warmup_ticks"], 20)
                rung_args.pregenerated_world = directory / "fixture-world"
                rung_dir = directory / "initial"
                rung_dir.mkdir()
                receipt = await MatrixRung(rung_args, config, fixture, rung_dir, deadline, interrupted).run()
                receipt["binaries"]["fixture"] = summary["binaries"]["fixture"]
                complete = completed_lifecycle(receipt)
                if complete:
                    try:
                        receipt["saved_world"] = await fixture_report(args, config, Path(receipt["world"]),
                            directory, "saved-verify", deadline, interrupted, True)
                    except Exception as error:
                        receipt["status"] = "failed"
                        receipt["stop_reason"] = str(error)
                        receipt["errors"].append({"type": "saved_world_verification", "message": str(error)})
                await enrich(receipt, config, fixture, directory, deadline, interrupted)
                if config.get("restart") and complete:
                    saved = Path(receipt["world"])
                    verified = receipt.get("saved_world")
                    if not verified:
                        raise scale.RunFailure("restart persistence requires successful saved-world verification")
                    rung_args.pregenerated_world = saved
                    restart_dir = directory / "restart"
                    restart_dir.mkdir()
                    restarted = await MatrixRung(rung_args, config, fixture, restart_dir, deadline, interrupted).run()
                    receipt["restart"] = {"receipt": str(restart_dir / "receipt.json"), "status": restarted["status"],
                                          "source_world": str(saved), "before": verified}
                    restart_complete = completed_lifecycle(restarted)
                    if not restart_complete:
                        await enrich(restarted, config, fixture, restart_dir, deadline, interrupted)
                        scale.dump(restart_dir / "receipt.json", restarted)
                        raise scale.RunFailure("identity-rebound saved-world restart lifecycle failed")
                    after = await fixture_report(args, config, Path(restarted["world"]), directory, "restart-verify", deadline, interrupted, True)
                    restarted["saved_world"] = after
                    idle_states = {}
                    for label, idle_dir in (("initial_idle", Path(receipt["directory"])), ("restart_idle", restart_dir)):
                        idle = json.loads((idle_dir / "idle.json").read_text())["dashboard"]
                        idle_states[label] = {"entities": idle.get("entities"), "chunks": idle.get("chunks"),
                                              "simulation_tick": idle.get("simulation_tick")}
                    receipt["restart"]["server_idle_state_before_bots"] = {**idle_states,
                        "scope": "Dashboard state before bot join; evidence only. Persistence verdict is fixture readback equality."}
                    await enrich(restarted, config, fixture, restart_dir, deadline, interrupted)
                    scale.dump(restart_dir / "receipt.json", restarted)
                    receipt["restart"]["status"] = restarted["status"]
                    if restarted["status"] == "failed":
                        receipt["status"] = "failed"
                        receipt["stop_reason"] = f"restart performance/activity gate failed: {restarted.get('stop_reason')}"
                    before_facts, after_facts = verified.get("persistence_facts"), after.get("persistence_facts")
                    if not before_facts or before_facts != after_facts:
                        raise scale.RunFailure("restart persistence facts absent or changed")
                    receipt["restart"]["after"] = after
                    identity = restart_entity_identity_proof(fixture, verified, after)
                    receipt["restart"]["living_entity_identity_proof"] = identity["authored"]
                    receipt["restart"]["dynamic_living_entity_diagnostics"] = identity["dynamic"]
                    if not identity["authored"]["retained"]:
                        raise scale.RunFailure("restart lost saved authored passive-animal/villager identities")
                    receipt["restart"]["persistence_proved"] = True
            except Exception as error:
                if receipt["status"] != "blocked":
                    receipt["status"] = "failed"
                receipt["stop_reason"] = str(error)
                receipt.setdefault("errors", []).append({"type": type(error).__name__, "message": str(error)})
            if receipt.get("status") != "passed" and "acceptance" in receipt:
                receipt["acceptance"]["baseline_holds"] = False
            scale.dump(Path(receipt["directory"]) / "receipt.json", receipt)
            receipts.append(receipt)
            summary["receipts"].append(str(Path(receipt["directory"]) / "receipt.json"))
            scale.dump(run_dir / "receipt.json", summary)
            print(json.dumps({"event": "scenario", "scenario": config["id"], "status": receipt["status"],
                              "receipt": summary["receipts"][-1]}), flush=True)
        statuses = {receipt["status"] for receipt in receipts}
        summary["status"] = "failed" if statuses - {"passed", "smoke_only"} else ("smoke_only" if "smoke_only" in statuses else "passed")
    except Exception as error:
        summary["errors"].append({"type": type(error).__name__, "message": str(error)})
    finally:
        summary["finished"] = scale.stamp()
        scale.dump(run_dir / "receipt.json", summary)
        (run_dir / "summary.md").write_text(markdown(summary, receipts))
        print(json.dumps({"event": "final", "status": summary["status"], "receipt": summary["receipt"],
                          "summary": str(run_dir / "summary.md")}), flush=True)
    return 0 if summary["status"] in ("passed", "smoke_only") else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config-dir", type=scale.absolute_path, default=ROOT / "tools/harness/load_matrix_scenarios")
    parser.add_argument("--scenario", action="append", choices=MATRIX_SCENARIOS, default=[])
    parser.add_argument("--out", type=scale.absolute_path, default=ROOT / ".analysis/codex-logs/load-matrix")
    parser.add_argument("--label", required=True, choices=("pre-L2", "release-main"))
    parser.add_argument("--server-bin", type=scale.absolute_path, required=True)
    parser.add_argument("--bots-bin", type=scale.absolute_path, required=True)
    parser.add_argument("--fixture-bin", type=scale.absolute_path, required=True)
    parser.add_argument("--vanilla-data-dir", type=scale.absolute_path)
    parser.add_argument("--terrain-fixture-summary", type=scale.absolute_path,
                        help="optional certified dry terrain anchor receipt for fresh-world exploration (no pregenerated chunks copied)")
    parser.add_argument("--plugin-dir", type=scale.absolute_path)
    parser.add_argument("--single-core", action=argparse.BooleanOptionalAction, default=True,
                        help="CPU2 canonical run; --no-single-core retains full tick windows on the first smoke CPU")
    parser.add_argument("--smoke-ticks", type=scale.positive, help="override measurement duration; nonauthoritative")
    parser.add_argument("--smoke-cpus", type=cpu_list, default=[0, 4], help="smoke server first CPU, bots remainder; excludes 2/3")
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--deadline-seconds", type=scale.positive, default=7200,
                        help="finite overall budget including reserved child cleanup; never extended")
    parser.add_argument("--readiness-seconds", type=scale.positive, default=300)
    parser.add_argument("--max-rss-mb", type=scale.positive, default=9216)
    parser.add_argument("--server-max-rss-mb", type=scale.positive, default=8192)
    parser.add_argument("--bots-max-rss-mb", type=scale.positive, default=1024)
    parser.add_argument("--min-headroom-mb", type=scale.positive, default=512)
    parser.add_argument("--min-tps", type=scale.positive, default=19)
    parser.add_argument("--profile", choices=("perf", "samply"))
    args = parser.parse_args()
    if args.seed != 42:
        parser.error("offline matrix fixtures currently require seed 42")
    if len(args.smoke_cpus) < 2:
        parser.error("--smoke-cpus requires a server CPU and at least one distinct bot CPU")
    args.profile_first_failing = False
    args.fixture_summary = None
    args.steady_state = False
    args.runner_command = [sys.executable, "-m", "tools.harness.backends.load_matrix", *sys.argv[1:]]
    return asyncio.run(run(args))


if __name__ == "__main__":
    raise SystemExit(main())
