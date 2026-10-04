#!/usr/bin/env python3
"""Run isolated, tick-delimited mc-server/player-scale-bots benchmarks.

Dashboard percentiles describe a rolling 1200-tick window, not an aggregation
of sampled percentiles. Short measurements are explicitly non-authoritative.
This backend launches prebuilt binaries only; the harness owns their build.
"""
from __future__ import annotations

import argparse
import asyncio
import datetime as dt
import hashlib
import json
import math
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import struct
import time
import urllib.error
import urllib.request
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
WINDOW_TICKS = 1200
SERVER_CPUS = "0,2"
BOT_CPUS = "4-11"


class RunFailure(RuntimeError):
    pass


def absolute_path(raw: str) -> Path:
    path = Path(raw)
    if not path.is_absolute():
        raise argparse.ArgumentTypeError("path must be absolute")
    return path


def positive(raw: str) -> int:
    number = int(raw)
    if number <= 0:
        raise argparse.ArgumentTypeError("value must be positive")
    return number


def binary_sha256(path: Path) -> str:
    with path.open("rb") as binary:
        return hashlib.file_digest(binary, "sha256").hexdigest()


def stamp() -> dict[str, Any]:
    return {"unix_ns": time.time_ns(), "monotonic_ns": time.monotonic_ns()}


def dump(path: Path, value: Any) -> None:
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def reserve_ports() -> tuple[int, int]:
    # Keep both reservations live until the distinct ports have been selected.
    with socket.socket() as server, socket.socket() as dashboard:
        server.bind(("127.0.0.1", 0))
        dashboard.bind(("127.0.0.1", 0))
        return server.getsockname()[1], dashboard.getsockname()[1]


def proc_sample(pid: int) -> dict[str, Any]:
    fields = (Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1]).split()
    ticks_per_second = os.sysconf("SC_CLK_TCK")
    status = {}
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        key, _, value = line.partition(":")
        if key in {"VmRSS", "VmHWM"}:
            status[key] = int(value.split()[0]) * 1024
    return {
        "pid": pid,
        "cpu_seconds": (int(fields[11]) + int(fields[12])) / ticks_per_second,
        "user_cpu_seconds": int(fields[11]) / ticks_per_second,
        "system_cpu_seconds": int(fields[12]) / ticks_per_second,
        "rss_bytes": status.get("VmRSS", 0),
        "peak_rss_bytes": status.get("VmHWM", 0),
        "threads": int(fields[17]),
    }


def host_cpu_sample() -> dict[str, dict[str, float]]:
    result = {}
    ticks_per_second = os.sysconf("SC_CLK_TCK")
    for line in Path("/proc/stat").read_text().splitlines():
        fields = line.split()
        if fields and fields[0] in {"cpu2", "cpu3"}:
            # Guest time is already included in user/nice; do not count it twice.
            ticks = [int(value) for value in fields[1:9]]
            result[fields[0]] = {
                "total_seconds": sum(ticks) / ticks_per_second,
                "busy_seconds": (sum(ticks) - ticks[3] - ticks[4]) / ticks_per_second,
            }
    return result


def foreign_cpu_load(start: dict[str, Any], end: dict[str, Any],
                     server_cpus: str) -> dict[str, Any]:
    elapsed = (end["monotonic_ns"] - start["monotonic_ns"]) / 1e9
    server_seconds = end["server"]["cpu_seconds"] - start["server"]["cpu_seconds"]
    result = {}
    for cpu in ("cpu2", "cpu3"):
        busy = end["host_cpus"][cpu]["busy_seconds"] - start["host_cpus"][cpu]["busy_seconds"]
        own = server_seconds if cpu == "cpu2" and server_cpus == "2" else (
            0.0 if cpu[3:] not in server_cpus.split(",") else None)
        residual = busy - own if own is not None else None
        result[cpu] = {
            "host_busy_seconds": busy,
            "host_busy_percent_one_core": 100 * busy / elapsed,
            "server_cpu_seconds": own,
            "foreign_cpu_seconds_raw": residual,
            "foreign_cpu_percent_one_core": 100 * max(0.0, residual) / elapsed
                if residual is not None else None,
        }
    return {
        "elapsed_seconds": elapsed, "cpus": result,
        "scope": "Host /proc/stat busy time (including steal, excluding idle/iowait); "
                 "CPU2 subtracts server process time only when pinned solely to CPU2. "
                 "CPU3 has no harness process. Jiffy quantization can make raw residual "
                 "slightly negative; percent is clamped at zero. Multi-core server "
                 "time cannot be attributed to an individual CPU.",
    }


def memory_headroom() -> dict[str, Any]:
    available = None
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemAvailable:"):
            available = int(line.split()[1]) * 1024
    result = {"host_available_bytes": available, "cgroup_available_bytes": None}
    # Resolve the process's cgroup instead of assuming it lives at mount root.
    for line in Path("/proc/self/cgroup").read_text().splitlines():
        if line.startswith("0::"):
            root = Path("/sys/fs/cgroup") / line[3:].lstrip("/")
            try:
                limit = (root / "memory.max").read_text().strip()
                if limit != "max":
                    result["cgroup_available_bytes"] = max(
                        0, int(limit) - int((root / "memory.current").read_text())
                    )
            except OSError:
                pass
    return result


def http_json(url: str, timeout: float) -> dict[str, Any]:
    # Loopback must not be routed through an operator's HTTP proxy.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(url, timeout=timeout) as response:
        value = json.load(response)
    if not isinstance(value, dict):
        raise RunFailure(f"non-object dashboard response at {url}")
    return value


def backlog(profile: dict[str, Any]) -> dict[str, Any]:
    resources = profile.get("resources", {})
    network = resources.get("network_and_entities", {})
    prepared = network.get("prepared_cache", {})
    sessions = network.get("sessions", {})
    return {
        "backlog_scope": "generation/load preparation backlog (not an exact generator queue)",
        "prepared_in_flight": prepared.get("in_flight"),
        "pending_subscribers": prepared.get("pending_subscribers"),
        "outbound_queued_or_reserved_slots": sessions.get("outbound_queued_or_reserved_slots"),
        "workers": resources.get("workers"),
        "generation_queue_depth": None,
        "generation_queue_note": "No exact generation queue-depth field in native resource schema; in-flight preparation/subscriber counts are not generation queue depth.",
    }


def _anvil_chunk_positions(world: Path) -> set[tuple[int, int]]:
    positions: set[tuple[int, int]] = set()
    for path in (world / "region").glob("r.*.*.mca"):
        _, region_x, region_z, _ = path.name.split(".")
        with path.open("rb") as region:
            header = region.read(4096)
        if len(header) != 4096:
            raise ValueError(f"truncated Anvil location header: {path}")
        for slot, (location,) in enumerate(struct.iter_unpack(">I", header)):
            if location >> 8 and location & 255:
                positions.add((int(region_x) * 32 + slot % 32,
                               int(region_z) * 32 + slot // 32))
    return positions


class Rung:
    def __init__(self, args: argparse.Namespace, clients: int, directory: Path,
                 deadline: float, interrupted: asyncio.Event) -> None:
        self.args, self.clients, self.directory = args, clients, directory
        self.initial_stored_chunks: set[tuple[int, int]] | None = None
        self.server_cpus = "2" if args.single_core else SERVER_CPUS
        self.server_cores = 1 if args.single_core else 2
        self.deadline, self.interrupted = deadline, interrupted
        self.events: asyncio.Queue[tuple[str, Any]] = asyncio.Queue()
        self.processes: dict[str, asyncio.subprocess.Process] = {}
        self.tasks: list[asyncio.Task[Any]] = []
        self.logs: list[Any] = []
        self.bot: dict[str, Any] = {}
        self.bot_at: float | None = None
        self.ready = False
        self.stopping = False
        self.stats: dict[str, Any] = {}
        self.previous: dict[str, Any] | None = None
        self.measured: list[dict[str, Any]] = []
        self.idle: dict[str, Any] | None = None
        self.profile_process: asyncio.subprocess.Process | None = None
        self.profile_start: dict[str, Any] | None = None
        self.receipt: dict[str, Any] = {
            "schema": 1, "clients": clients, "status": "running", "phase": "startup", "started": stamp(),
            "directory": str(directory), "server_cpus": self.server_cpus, "bot_cpus": BOT_CPUS,
            "server_affinity_logical_cpus": self.server_cores,
            "chunk_worker_threads_override": 1 if args.single_core else None,
            "binaries": {
                "server": {"path": str(args.server_bin), "sha256": binary_sha256(args.server_bin)},
                "bots": {"path": str(args.bots_bin), "sha256": binary_sha256(args.bots_bin)},
            },
            "view_distance": args.view_distance, "seed": args.seed, "worldgen_mode": "vanilla_like",
            "worldgen_revision": None,
            "proof_scope": "Generator-revision-specific pinned-core baseline; not VPS capacity. Any generator cutover requires a separately identified scale run.",
            "vps_capacity_claim": False,
            "spacing": args.spacing, "group_size": args.group_size,
            "steady_state": args.steady_state,
            "route_scope": "Safe landing and walking stay in each original anchor chunk; "
                "the full VD+two preparation/light borders are checked against committed source headers."
                if args.steady_state else "Unrestricted accumulated walking route",
            "warmup_ticks": args.warmup_ticks, "measure_ticks_requested": args.measure_ticks,
            "memory_limits_mib": {"server_rss": args.server_max_rss_mb,
                "bots_rss": args.bots_max_rss_mb, "combined_rss": args.max_rss_mb,
                "minimum_headroom": args.min_headroom_mb},
            "profiling_requested": args.profile,
            "profiling_note": "Optional profiler adds overhead. Profiled runs are diagnostic, never an unprofiled baseline capacity claim.",
            "dashboard": {"bind": "127.0.0.1", "stats_path": "/stats", "profile_path": "/profile",
                          "authentication": "none; existing loopback-only dashboard contract"},
            "samples": str(directory / "samples.jsonl"), "errors": [], "shutdown": {},
        }
        self.anchor_path: Path | None = None
        if args.fixture_summary:
            self.receipt["terrain_fixture"] = json.loads(args.fixture_summary.read_text())
            if self.receipt["terrain_fixture"]["seed"] != args.seed:
                raise RunFailure("offline terrain fixture seed differs from requested runtime seed")
            terrain = self.receipt["terrain_fixture"]
            wet = {anchor[0] for anchor in terrain["wet_anchors"]}
            origin_x, origin_z = terrain["origin"]
            width, spacing = terrain["grid_width"], terrain["spacing"]
            anchors = [[origin_x + index % width * spacing,
                        origin_z + index // width * spacing]
                       for index in range(terrain["anchors"]) if index not in wet]
            skipped = sorted(set(getattr(args, "skip_anchors", ())))
            if skipped:
                anchors = [anchor for position, anchor in enumerate(anchors) if position not in skipped]
                self.receipt["skipped_dry_anchor_positions"] = skipped
            groups = (clients + args.group_size - 1) // args.group_size
            if groups > len(anchors):
                raise RunFailure("requested groups exceed the fixture's certified dry anchors")
            self.anchor_path = directory / "anchors.json"
            dump(self.anchor_path, anchors)
            self.receipt["anchor_fixture"] = {
                "path": str(self.anchor_path), "available_dry_anchors": len(anchors),
                "selected_anchor_count": groups, "selection": "first dry anchors in original grid order",
                "original_1000_player_goal_certified": False,
            }
        self.sample_file = (directory / "samples.jsonl").open("w", buffering=1)
        dump(directory / "receipt.json", self.receipt)

    def record(self, event: dict[str, Any]) -> None:
        self.sample_file.write(json.dumps({**stamp(), **event}, sort_keys=True) + "\n")

    def remaining(self) -> float:
        left = self.deadline - time.monotonic()
        if left <= 0:
            raise RunFailure("overall deadline exceeded (not extended)")
        if self.interrupted.is_set():
            raise RunFailure("orchestrator interrupted")
        return left

    async def spawn(self, name: str, command: list[str], *, stdin: bool = False,
                    events: bool = False) -> asyncio.subprocess.Process:
        self.remaining()
        log = (self.directory / f"{name}.log").open("wb")
        self.logs.append(log)
        process = await asyncio.create_subprocess_exec(
            *command, cwd=ROOT, start_new_session=True,
            stdin=asyncio.subprocess.PIPE if stdin else asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE if events else log, stderr=log,
        )
        self.processes[name] = process
        self.receipt[f"{name}_command"] = command
        self.receipt[f"{name}_pid"] = process.pid
        dump(self.directory / "receipt.json", self.receipt)
        self.tasks.append(asyncio.create_task(self.watch_exit(name, process)))
        if events:
            self.tasks.append(asyncio.create_task(self.read_bots(process, log)))
        return process

    async def watch_exit(self, name: str, process: asyncio.subprocess.Process) -> None:
        code = await process.wait()
        await self.events.put(("exit", {"process": name, "returncode": code}))

    async def read_bots(self, process: asyncio.subprocess.Process, log: Any) -> None:
        assert process.stdout is not None
        try:
            while raw := await process.stdout.readline():
                log.write(raw)
                log.flush()
                event = json.loads(raw)
                if not isinstance(event, dict) or "event" not in event:
                    raise ValueError("missing event object")
                await self.events.put(("bot", event))
        except (ValueError, UnicodeError, OSError) as error:
            await self.events.put(("error", f"invalid bot stdout stream: {error}"))

    async def dashboard_sampler(self, url: str) -> None:
        # Timer callbacks drive explicit sampling events; readiness itself is
        # event/deadline driven, never a short-timeout polling loop.
        loop = asyncio.get_running_loop()
        next_at = loop.time()
        timer: asyncio.TimerHandle | None = None
        try:
            while not self.stopping:
                wake = loop.create_future()
                timer = loop.call_at(next_at, wake.set_result, None)
                await wake
                timer = None
                left = self.deadline - time.monotonic()
                if left <= 0:
                    return
                await self.events.put(("metrics_tick", None))
                try:
                    stats = await asyncio.to_thread(http_json, url + "/stats", min(2.0, left))
                    await self.events.put(("stats", stats))
                except (OSError, ValueError, urllib.error.URLError) as error:
                    await self.events.put(("dashboard_error", str(error)))
                # Skip missed cadence slots instead of issuing catch-up bursts.
                next_at += max(1, int(loop.time() - next_at) + 1)
        except asyncio.CancelledError:
            return
        finally:
            if timer is not None:
                timer.cancel()

    async def native_profile(self, name: str) -> dict[str, Any]:
        timeout = min(10.0, self.remaining())
        profile = await asyncio.to_thread(http_json, self.dashboard_url + "/profile", timeout)
        dump(self.directory / f"{name}.json", profile)
        self.record({"event": "native_profile", "name": name, "backlog": backlog(profile),
                     "simulation_tick": profile.get("simulation_tick")})
        self.receipt[name] = str(self.directory / f"{name}.json")
        return profile

    async def start_profiler(self, phase: str) -> None:
        if self.profile_process is not None:
            return
        mode = self.args.profile
        if mode is None:
            return
        if shutil.which(mode) is None:
            raise RunFailure(f"requested profiler not installed: {mode}")
        extension = "perf.data" if mode == "perf" else "samply.json.gz"
        output = (self.directory / extension).resolve()
        pid = str(self.processes["server"].pid)
        command = (["perf", "record", "--all-user", "-F", "199", "--call-graph", "fp",
                    "--pid", pid, "-o", str(output)] if mode == "perf" else
                   ["samply", "record", "--pid", pid, "--save-only", "--output", str(output)])
        self.profile_process = await self.spawn("profiler", command)
        self.profile_start = {**stamp(), "simulation_tick": self.stats.get("simulation_tick"),
                              "phase": phase}
        self.receipt["profiler"] = {"mode": mode, "output": str(output), "start": self.profile_start}

    def sample_processes(self, stats: dict[str, Any], *, dashboard_fresh: bool = True) -> dict[str, Any]:
        row = {**stamp(), "event": "sample", "dashboard": stats,
               "dashboard_sample_fresh": dashboard_fresh,
               "bot_counters": self.bot.copy(), "bot_sample_age_seconds":
               time.monotonic() - self.bot_at if self.bot_at is not None else None}
        row["host_cpus"] = host_cpu_sample()
        for name in ("server", "bots"):
            process = self.processes.get(name)
            if process is None:
                continue
            try:
                measured = proc_sample(process.pid)
            except (OSError, ValueError):
                if process.returncode is not None:
                    raise RunFailure(f"{name} exited with {process.returncode}")
                raise RunFailure(f"could not capture /proc metrics for {name}")
            measured["cpu_percent_one_core"] = None
            measured["cpu_percent_affinity_basis"] = None
            cores = self.server_cores if name == "server" else 8
            measured["cpu_basis_cores"] = cores
            if self.previous and name in self.previous:
                elapsed = (row["monotonic_ns"] - self.previous["monotonic_ns"]) / 1e9
                used = measured["cpu_seconds"] - self.previous[name]["cpu_seconds"]
                measured["cpu_percent_one_core"] = 100 * used / elapsed
                measured["cpu_percent_affinity_basis"] = 100 * used / elapsed / cores
                measured["core_equivalents"] = used / elapsed
            row[name] = measured
        if self.previous and "server" in self.previous and "server" in row:
            row["foreign_cpu_load"] = foreign_cpu_load(self.previous, row, self.server_cpus)
            self.receipt["foreign_cpu_load_latest"] = row["foreign_cpu_load"]
        self.previous = row
        headroom = memory_headroom()
        row["memory_headroom"] = headroom
        self.record(row)
        self.receipt["latest_sample"] = row
        self.receipt["latest_bots"] = self.bot
        dump(self.directory / "receipt.json", self.receipt)
        combined = sum(row.get(name, {}).get("rss_bytes", 0) for name in ("server", "bots"))
        if combined > self.args.max_rss_mb * 1024 * 1024:
            raise RunFailure(f"combined server/bots RSS exceeds {self.args.max_rss_mb} MiB")
        for name, ceiling in (("server", self.args.server_max_rss_mb),
                              ("bots", self.args.bots_max_rss_mb)):
            if row.get(name, {}).get("rss_bytes", 0) > ceiling * 1024 * 1024:
                raise RunFailure(f"{name} RSS exceeds {ceiling} MiB")
        if any(value is not None and value < self.args.min_headroom_mb * 1024 * 1024
               for value in headroom.values()):
            raise RunFailure(f"host/cgroup memory headroom below {self.args.min_headroom_mb} MiB")
        return row

    async def next_stats(self, *, ready_by: float | None = None) -> dict[str, Any]:
        while True:
            timeout = self.remaining()
            if ready_by is not None:
                timeout = min(timeout, ready_by - time.monotonic())
                if timeout <= 0:
                    raise RunFailure("readiness deadline exceeded")
            try:
                kind, value = await asyncio.wait_for(self.events.get(), timeout)
            except asyncio.TimeoutError:
                raise RunFailure("readiness deadline exceeded" if ready_by is not None
                                 else "overall deadline exceeded (not extended)")
            if kind == "metrics_tick":
                self.sample_processes(self.stats, dashboard_fresh=False)
                continue
            if kind == "exit":
                raise RunFailure(f"{value['process']} exited unexpectedly: {value['returncode']}")
            if kind == "error":
                raise RunFailure(value)
            if kind == "bot":
                self.record({"event": "bot_event", "payload": value})
                self.bot = value
                self.bot_at = time.monotonic()
                self.receipt["latest_bots"] = value
                dump(self.directory / "receipt.json", self.receipt)
                if value["event"] == "failure":
                    raise RunFailure(f"bot failure: {value.get('error', value)}")
                if value.get("disconnects", 0):
                    raise RunFailure(f"bots reported {value['disconnects']} disconnects")
                if value["event"] == "ready":
                    if value.get("clients") != self.clients:
                        raise RunFailure("bot readiness client count mismatch")
                    self.ready = True
                    self.receipt["ready"] = {**stamp(), "payload": value}
                elif self.ready and value["event"] == "sample" and value.get("ready_clients") != self.clients:
                    raise RunFailure("bot ready client count fell below requested clients")
            elif kind == "dashboard_error":
                self.record({"event": "dashboard_error", "error": value})
                if self.stats:
                    raise RunFailure(f"dashboard metrics unavailable after startup: {value}")
            elif kind == "stats":
                if not isinstance(value.get("simulation_tick"), int):
                    raise RunFailure("dashboard lacks simulation_tick; rebuild server with tick schema")
                if self.stats and value["simulation_tick"] < self.stats["simulation_tick"]:
                    raise RunFailure("dashboard simulation_tick regressed")
                revision = value.get("world", {}).get("worldgen_revision")
                if not isinstance(revision, int):
                    raise RunFailure("dashboard lacks compiled worldgen_revision")
                self.receipt["worldgen_revision"] = revision
                self.stats = value
                row = self.sample_processes(value)
                if self.ready and value["simulation_tick"] > self.receipt.get("warmup_start_tick", value["simulation_tick"]):
                    if value.get("players", {}).get("count") != self.clients:
                        raise RunFailure("dashboard active players differ from ready bot count")
                    if value.get("autoscale", {}).get("enabled") or value.get("autoscale", {}).get("view_distance") != self.args.view_distance:
                        raise RunFailure("fixed view-distance/autoscale contract changed")
                return row

    async def wait_tick(self, target: int) -> dict[str, Any]:
        while True:
            row = await self.next_stats()
            if row["dashboard"]["simulation_tick"] >= target:
                return row

    def summarize(self, start: dict[str, Any], end: dict[str, Any]) -> None:
        elapsed = (end["monotonic_ns"] - start["monotonic_ns"]) / 1e9
        tick = end["dashboard"]["tick"]
        source = tick.get("source_tick")
        samples = tick.get("total", {}).get("samples", 0)
        start_tick = start["dashboard"]["tick"]["source_tick"]
        authoritative = (self.args.measure_ticks >= WINDOW_TICKS and samples == WINDOW_TICKS
                         and isinstance(source, int) and source - samples + 1 > start_tick)
        windows = {row["dashboard"]["tick"].get("source_tick"): row["dashboard"]["tick"]
                   for row in self.measured if row["dashboard"].get("tick", {}).get("total", {}).get("samples", 0)}
        self.receipt["measurement"] = {
            "start_tick": start_tick, "start_simulation_tick": start["dashboard"]["simulation_tick"],
            "end_simulation_tick": end["dashboard"]["simulation_tick"],
            "start_unix_ns": start["unix_ns"], "end_unix_ns": end["unix_ns"],
            "start_monotonic_ns": start["monotonic_ns"], "end_monotonic_ns": end["monotonic_ns"],
            "elapsed_seconds": elapsed,
            "observed_ticks": end["dashboard"]["simulation_tick"] - start["dashboard"]["simulation_tick"],
            "actual_tps": (end["dashboard"]["simulation_tick"] - start["dashboard"]["simulation_tick"]) / elapsed,
            "percentiles": {"source": "dashboard rolling window", "capacity_ticks": WINDOW_TICKS,
                "source_tick": source, "samples": samples,
                "first_tick": source - samples + 1 if isinstance(source, int) else None,
                "last_tick": source, "authoritative_post_warmup_window": authoritative,
                "exact_whole_measurement_aggregate": False,
                "note": "Final rolling window only. Sample windows overlap; their percentiles are never averaged or merged. Short runs are smoke-only and can include warmup.",
                "total": tick.get("total"), "stages": tick.get("stages"),
                "sampled_distinct_windows": len(windows)},
            "chunks_start": start["dashboard"].get("chunks"),
            "chunks_end": end["dashboard"].get("chunks"),
            "bots_start": start["bot_counters"], "bots_end": end["bot_counters"],
        }
        generated_start = start["dashboard"]["chunks"]["generated_total"]
        generated_end = end["dashboard"]["chunks"]["generated_total"]
        self.receipt["measurement"]["generation"] = {
            "generated_chunks": generated_end - generated_start,
            "elapsed_seconds": elapsed,
            "generated_chunks_per_second": (generated_end - generated_start) / elapsed,
        }
        for name, cores in (("server", self.server_cores), ("bots", 8)):
            first, last = start[name], end[name]
            peak = max(row[name]["peak_rss_bytes"] for row in [start, *self.measured])
            idle_rss = self.idle["server"]["rss_bytes"] if self.idle and name == "server" else None
            self.receipt["measurement"][name] = {
                "rss_current_bytes": last["rss_bytes"], "rss_peak_bytes": peak,
                "rss_peak_scope": "process lifetime /proc VmHWM (includes startup/warmup)",
                "rss_current_bytes_per_player": last["rss_bytes"] / self.clients,
                "rss_peak_bytes_per_player": peak / self.clients,
                "idle_rss_bytes": idle_rss,
                "idle_delta_bytes_per_player": (last["rss_bytes"] - idle_rss) / self.clients if idle_rss is not None else None,
                "cpu_seconds": last["cpu_seconds"] - first["cpu_seconds"],
                "user_cpu_seconds": last["user_cpu_seconds"] - first["user_cpu_seconds"],
                "system_cpu_seconds": last["system_cpu_seconds"] - first["system_cpu_seconds"],
                "cpu_percent_one_core": 100 * (last["cpu_seconds"] - first["cpu_seconds"]) / elapsed,
                "core_equivalents": (last["cpu_seconds"] - first["cpu_seconds"]) / elapsed,
                "cpu_percent_affinity_basis": 100 * (last["cpu_seconds"] - first["cpu_seconds"]) / elapsed / cores,
                "cpu_basis_cores": cores,
            }
        self.receipt["measurement"]["foreign_cpu_load"] = foreign_cpu_load(
            start, end, self.server_cpus)
        counter_names = (
            "packets_received", "decoded_bytes_received", "chunks_received",
            "movements_sent", "breaks_sent", "places_sent", "breaks_acked",
            "places_acked", "break_updates_observed", "place_updates_observed",
            "block_acks", "block_updates", "keepalives_replied", "chunk_batches_acked",
            "teleports_confirmed", "chats_sent", "sprint_toggles", "disconnects",
            "action_ack_timeouts", "action_observation_timeouts",
            "chunk_boundaries_crossed", "walking_distance_blocks",
        )
        self.receipt["measurement"]["bot_counters_delta"] = {
            key: end["bot_counters"][key] - start["bot_counters"][key]
            for key in counter_names
            if isinstance(start["bot_counters"].get(key), (int, float)) and isinstance(end["bot_counters"].get(key), (int, float))
        }
        self.receipt["measurement"]["bot_latency_scope"] = (
            "Cumulative process-lifetime action-ack histogram/percentiles in bots_end and final report; "
            "not an exact measurement-only latency percentile."
        )
        start_bytes = start["dashboard"].get("network", {}).get("bytes_written")
        end_bytes = end["dashboard"].get("network", {}).get("bytes_written")
        byte_source = "server actual outbound bytes_written (all protocol framing/compression)"
        if not isinstance(start_bytes, int) or not isinstance(end_bytes, int):
            start_bytes = start["bot_counters"].get("decoded_bytes_received")
            end_bytes = end["bot_counters"].get("decoded_bytes_received")
            byte_source = "bot decoded_bytes_received (decoded payload, NOT wire bytes)"
        self.receipt["measurement"]["traffic"] = {
            "source": byte_source, "elapsed_seconds": elapsed,
            "bytes_delta": end_bytes - start_bytes if isinstance(start_bytes, int) and isinstance(end_bytes, int) else None,
            "bytes_per_player_per_second": (end_bytes - start_bytes) / elapsed / self.clients
                if isinstance(start_bytes, int) and isinstance(end_bytes, int) else None,
            "bot_counter_note": "Cumulative bot snapshots sampled asynchronously; raw timestamped events and their sample age are in JSONL.",
        }
        p95 = tick.get("total", {}).get("p95_us")
        self.receipt["acceptance"] = {
            "p95_budget_us": 50_000, "p95_us": p95, "readiness": self.ready,
            "disconnects": end["bot_counters"].get("disconnects", 0),
            "percentile_authoritative": authoritative,
            "actual_tps": self.receipt["measurement"]["actual_tps"],
            "minimum_tps": self.args.min_tps,
            "baseline_holds": False,
            "action_ack_timeouts": end["bot_counters"].get("action_ack_timeouts", 0),
            "action_observation_timeouts": end["bot_counters"].get("action_observation_timeouts", 0),
        }
        if (self.receipt["acceptance"]["action_ack_timeouts"]
                or self.receipt["acceptance"]["action_observation_timeouts"]):
            self.receipt["status"] = "failed"
            self.receipt["stop_reason"] = "bot action acknowledgement or observation timed out"
        elif not authoritative:
            self.receipt["status"] = "smoke_only"
            self.receipt["stop_reason"] = "non-authoritative rolling percentile window; ladder not continued"
        elif self.receipt["measurement"]["actual_tps"] < self.args.min_tps:
            self.receipt["status"] = "failed"
            self.receipt["stop_reason"] = "actual simulation TPS below minimum despite tick-duration percentiles"
        elif isinstance(p95, int) and p95 <= 50_000:
            self.receipt["status"] = "passed"
            self.receipt["acceptance"]["baseline_holds"] = self.profile_process is None
        else:
            self.receipt["status"] = "failed"
            self.receipt["stop_reason"] = "tick p95 exceeds 50ms or unavailable"

    def validate_final_bot_report(self, final: Any) -> None:
        self.receipt["bot_final_report"] = final
        failed = (not isinstance(final, dict) or not final.get("success")
                  or final.get("disconnects", 0) or final.get("action_ack_timeouts", 0)
                  or final.get("action_observation_timeouts", 0))
        if "acceptance" in self.receipt and isinstance(final, dict):
            self.receipt["acceptance"]["action_ack_timeouts"] = final.get("action_ack_timeouts", 0)
            self.receipt["acceptance"]["action_observation_timeouts"] = final.get("action_observation_timeouts", 0)
        if failed:
            self.receipt["errors"].append({"type": "bot_final_report", "message": "bot final report declares failure or action timeout"})
            self.receipt["status"] = "failed"
            if "acceptance" in self.receipt:
                self.receipt["acceptance"]["baseline_holds"] = False

    async def stop_process(self, name: str, *, console: bool = False) -> None:
        process = self.processes.get(name)
        if process is None:
            return
        forced = False
        if process.returncode is None:
            try:
                if console and process.stdin is not None:
                    process.stdin.write(b"stop\n")
                    await process.stdin.drain()
                    process.stdin.close()
                else:
                    os.killpg(process.pid, signal.SIGINT)
            except (ProcessLookupError, BrokenPipeError, ConnectionResetError):
                pass
            try:
                grace = max(0.001, min(12.0, self.args.shutdown_deadline - time.monotonic()))
                await asyncio.wait_for(process.wait(), grace)
            except asyncio.TimeoutError:
                forced = True
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                await process.wait()
        # Reap/terminate any descendants left in the dedicated process group.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        self.receipt["shutdown"][name] = {"returncode": process.returncode, "forced": forced}

    async def run(self) -> dict[str, Any]:
        try:
            server_port, dashboard_port = reserve_ports()
            self.dashboard_url = f"http://127.0.0.1:{dashboard_port}"
            config = self.directory / "config.toml"
            world = self.directory / "world"
            if self.args.pregenerated_world:
                shutil.copytree(self.args.pregenerated_world, world)
                self.receipt["world_preparation"] = {
                    "mode": "independent_copy_of_pregenerated_world",
                    "source": str(self.args.pregenerated_world),
                }
            else:
                world.mkdir()
                self.receipt["world_preparation"] = {"mode": "fresh_world"}
            self.initial_stored_chunks = _anvil_chunk_positions(world)
            if self.args.steady_state:
                anchors = json.loads(self.anchor_path.read_text())
                member_width = math.isqrt(self.args.group_size - 1) + 1
                radius = self.args.view_distance + 2
                missing = set()
                centres = set()
                for index in range(self.clients):
                    base_x, base_z = anchors[index // self.args.group_size]
                    member = index % self.args.group_size
                    centre = ((base_x + member % member_width * 4) // 16,
                              (base_z + member // member_width * 4) // 16)
                    centres.add(centre)
                    for cz in range(centre[1] - radius, centre[1] + radius + 1):
                        for cx in range(centre[0] - radius, centre[0] + radius + 1):
                            if (cx, cz) not in self.initial_stored_chunks:
                                missing.add((cx, cz))
                self.receipt["steady_state_coverage"] = {
                    "anchor_chunks": sorted(centres), "preparation_and_light_border_chunks": 2,
                    "missing_chunks": len(missing),
                }
                if missing:
                    raise RunFailure(f"steady-state footprint lacks {len(missing)} committed chunks")
            (self.directory / "plugins").mkdir()
            content = f'''[server]
name = "player-scale-{self.clients}"
motd = "isolated player-scale benchmark"
max_players = {max(20, self.clients)}
view_distance = {self.args.view_distance}
simulation_distance = {self.args.view_distance}
[network]
bind_address = "127.0.0.1"
port = {server_port}
[auth]
online_mode = false
[admin]
allow_local_dev_operators = true
[data]
world_dir = {json.dumps(str(world))}
seed = {self.args.seed}
worldgen_mode = "vanilla_like"
[autoscale]
enabled = false
[dashboard]
enabled = true
bind_address = "127.0.0.1"
port = {dashboard_port}
allow_remote = false
[plugins]
directory = {json.dumps(str(self.directory / "plugins"))}
'''
            if self.args.single_core:
                content += "[chunk_pipeline]\nworker_threads = 1\n"
            if self.args.vanilla_data_dir:
                content = content.replace(f"seed = {self.args.seed}\n", f"vanilla_data_dir = {json.dumps(str(self.args.vanilla_data_dir))}\nseed = {self.args.seed}\n")
            config.write_text(content)
            self.receipt["config"] = str(config)
            self.receipt["world"] = str(world)
            await self.spawn("server", ["taskset", "-c", self.server_cpus,
                str(self.args.server_bin), "--config", str(config), "--no-console"], stdin=True)
            await self.start_profiler("startup-readiness-warmup-measurement")
            self.tasks.append(asyncio.create_task(self.dashboard_sampler(self.dashboard_url)))
            ready_by = min(self.deadline, time.monotonic() + self.args.readiness_seconds)
            self.idle = await self.next_stats(ready_by=ready_by)
            self.receipt["worldgen_revision"] = self.idle["dashboard"]["world"]["worldgen_revision"]
            dump(self.directory / "idle.json", self.idle)
            await self.native_profile("idle-profile")
            bot_command = ["taskset", "-c", BOT_CPUS, str(self.args.bots_bin),
                "--addr", f"127.0.0.1:{server_port}", "--clients", str(self.clients),
                "--view-distance", str(self.args.view_distance), "--spacing", str(self.args.spacing),
                "--group-size", str(self.args.group_size), "--report", str(self.directory / "bots-report.json")]
            if self.anchor_path:
                bot_command += ["--anchors", str(self.anchor_path)]
            if self.args.steady_state:
                bot_command += ["--anchor-chunk-walk"]
            if getattr(self.args, "action_log", False):
                bot_command += ["--action-log"]
            await self.spawn("bots", bot_command, events=True)
            self.receipt["phase"] = "readiness"
            while not self.ready:
                await self.next_stats(ready_by=ready_by)
            # /stats has a one-second cache: an event captured just before the
            # ready event must not falsely look like a player-count regression.
            ready_profile = await self.native_profile("ready-profile")
            if ready_profile.get("players", {}).get("count") != self.clients:
                raise RunFailure("native readiness capture active players differs from ready bot count")
            ready_tick = ready_profile["simulation_tick"]
            generation_elapsed = (time.monotonic_ns() - self.idle["monotonic_ns"]) / 1e9
            generated = ready_profile["chunks"]["generated_total"] - self.idle["dashboard"]["chunks"]["generated_total"]
            self.receipt["readiness_generation"] = {
                "generated_chunks": generated, "elapsed_seconds": generation_elapsed,
                "generated_chunks_per_second": generated / generation_elapsed,
                "backlog": backlog(ready_profile),
            }
            self.receipt["warmup_start_tick"] = ready_tick
            self.receipt["phase"] = "warmup"
            warmed = await self.wait_tick(ready_tick + self.args.warmup_ticks)
            # Align to a *new* percentile publication after warmup. At the default
            # 1200 ticks, the final window is wholly beyond this source boundary.
            old_source = warmed["dashboard"].get("tick", {}).get("source_tick")
            if not isinstance(old_source, int):
                raise RunFailure("dashboard tick.source_tick missing")
            while True:
                start = await self.next_stats()
                source = start["dashboard"]["tick"]["source_tick"]
                if source > old_source and source >= ready_tick + self.args.warmup_ticks:
                    break
            self.receipt["warmup_end_tick"] = source
            self.receipt["phase"] = "measurement"
            await self.native_profile("measurement-start-profile")
            target = source + self.args.measure_ticks
            while True:
                end = await self.next_stats()
                self.measured.append(end)
                if (end["dashboard"]["simulation_tick"] >= target
                        and end["dashboard"]["tick"].get("source_tick", 0) >= target):
                    break
            self.summarize(start, end)
            final_profile = await self.native_profile("measurement-end-profile")
            self.receipt["measurement"]["backlog"] = backlog(final_profile)
        except (Exception, asyncio.CancelledError) as error:
            self.receipt["status"] = "failed"
            self.receipt["stop_reason"] = str(error) or type(error).__name__
            self.receipt["errors"].append({"type": type(error).__name__, "message": str(error)})
            # Failure captures are best-effort and remain within the original
            # budget. Avoid a costly traversal during a memory safety stop.
            if self.stats and self.deadline - time.monotonic() > 2 and "RSS" not in str(error) and "memory headroom" not in str(error):
                try:
                    profile = await asyncio.to_thread(http_json, self.dashboard_url + "/profile", 2.0)
                    dump(self.directory / "failure-profile.json", profile)
                    self.receipt["failure-profile"] = str(self.directory / "failure-profile.json")
                    self.receipt["failure_backlog"] = backlog(profile)
                except Exception as capture_error:
                    self.receipt["errors"].append({"type": "failure-profile", "message": str(capture_error)})
        finally:
            self.stopping = True
            # Stop instrumentation before stopping the sampled processes.
            for name in ("profiler", "bots", "server"):
                try:
                    await self.stop_process(name, console=name == "server")
                except Exception as error:
                    self.receipt["errors"].append({"type": "shutdown", "process": name, "message": str(error)})
                    self.receipt["status"] = "failed"
                    process = self.processes.get(name)
                    if process is not None:
                        try:
                            os.killpg(process.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        await process.wait()
                        self.receipt["shutdown"][name] = {"returncode": process.returncode, "forced": True}
            for task in self.tasks:
                if not task.done():
                    task.cancel()
            await asyncio.gather(*self.tasks, return_exceptions=True)
            report = self.directory / "bots-report.json"
            if "bots" in self.processes:
                try:
                    self.validate_final_bot_report(json.loads(report.read_text()))
                except (OSError, ValueError) as error:
                    self.receipt["errors"].append({"type": "bot_final_report", "message": str(error)})
                    self.receipt["status"] = "failed"
            if self.profile_process is not None:
                profile = self.receipt["profiler"]
                profile["end"] = stamp()
                profile["returncode"] = self.profile_process.returncode
                path = Path(profile["output"])
                profile["bytes"] = path.stat().st_size if path.is_file() else 0
                if not profile["bytes"] or self.receipt["shutdown"].get("profiler", {}).get("forced", True):
                    self.receipt["errors"].append({"type": "profiler", "message": "profile output missing/empty or forced shutdown"})
                    self.receipt["status"] = "failed"
            for name, row in self.receipt["shutdown"].items():
                if row["returncode"] != 0:
                    self.receipt["status"] = "failed"
                    self.receipt["errors"].append({"type": "shutdown", "process": name, "message": "nonzero child return code", "detail": row})
            if any(row["forced"] for row in self.receipt["shutdown"].values()):
                self.receipt["status"] = "failed"
                self.receipt["errors"].append({"type": "shutdown", "message": "forced child termination"})
            if self.initial_stored_chunks is not None:
                try:
                    stored = _anvil_chunk_positions(world)
                    fresh = sorted(stored - self.initial_stored_chunks)
                    diff = {
                        "schema": 1, "world": str(world),
                        "initial_stored_chunks": len(self.initial_stored_chunks),
                        "final_stored_chunks": len(stored),
                        "fresh_generated_chunks": len(fresh),
                        "scope": "Unique newly committed Anvil chunks over the entire rung, including centre and neighbour generation; excludes staged/unflushed chunks, so forced termination yields a lower bound. Not an exact measurement-window count or generator-call count.",
                        "positions": fresh,
                    }
                    diff_path = self.directory / "generated-chunk-header-diff.json"
                    dump(diff_path, diff)
                    self.receipt["fresh_generated_chunks"] = len(fresh)
                    self.receipt["fresh_generated_chunks_scope"] = diff["scope"]
                    self.receipt["generated_chunk_header_diff"] = str(diff_path)
                    if self.args.steady_state and fresh:
                        self.receipt["errors"].append({
                            "type": "steady_state_generation",
                            "message": f"steady-state rung committed {len(fresh)} new chunks",
                        })
                        self.receipt["status"] = "failed"
                except (OSError, ValueError, struct.error) as error:
                    self.receipt["errors"].append({"type": "generated_chunk_header_diff", "message": str(error)})
                    self.receipt["status"] = "failed"
            if self.profile_process is not None and self.args.profile_first_failing:
                profile = self.receipt["profiler"]
                profile["retained"] = self.receipt["status"] == "failed"
                if not profile["retained"]:
                    Path(profile["output"]).unlink(missing_ok=True)
                    profile["note"] = "Successful candidate profile discarded; only first failing rung profile retained."
            if self.receipt["status"] != "passed" and "acceptance" in self.receipt:
                self.receipt["acceptance"]["baseline_holds"] = False
            self.receipt["phase"] = "finished"
            self.receipt["finished"] = stamp()
            self.sample_file.close()
            for log in self.logs:
                log.close()
            dump(self.directory / "receipt.json", self.receipt)
        return self.receipt


async def run(args: argparse.Namespace) -> int:
    args.out.mkdir(parents=True, exist_ok=True)
    run_dir = args.out / (dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%S") + "-" + secrets.token_hex(4))
    run_dir.mkdir()
    interrupted = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, interrupted.set)
    args.shutdown_deadline = time.monotonic() + args.deadline_seconds
    cleanup_reserve = min(36.0, args.deadline_seconds / 10)
    deadline = args.shutdown_deadline - cleanup_reserve
    ladder = [50, 100, 250, 500, 800] if args.fixture_summary else [50, 100, 250, 500, 1000]
    summary: dict[str, Any] = {"schema": 1, "started": stamp(), "run_dir": str(run_dir),
        "deadline_seconds": args.deadline_seconds, "ladder": [args.clients] if args.clients else ladder,
        "receipts": [], "status": "failed"}
    summary["cleanup_reserve_seconds"] = cleanup_reserve
    summary["profiling_requested"] = args.profile
    summary["baseline_holds"] = False
    summary["proof_scope"] = "Generator-revision-specific pinned-core baseline, not VPS capacity"
    summary["server_cpus"] = "2" if args.single_core else SERVER_CPUS
    summary["chunk_worker_threads_override"] = 1 if args.single_core else None
    summary["seed"] = args.seed
    summary["fixture_summary"] = str(args.fixture_summary) if args.fixture_summary else None
    summary["worldgen_revisions"] = []
    summary["passed_unprofiled_clients"] = []
    dump(run_dir / "receipt.json", summary)
    try:
        for clients in summary["ladder"]:
            directory = run_dir / f"clients-{clients}"
            directory.mkdir()
            summary["active_rung"] = str(directory / "receipt.json")
            dump(run_dir / "receipt.json", summary)
            receipt = await Rung(args, clients, directory, deadline, interrupted).run()
            summary["receipts"].append(str(directory / "receipt.json"))
            summary["status"] = receipt["status"]
            revision = receipt.get("worldgen_revision")
            if revision is not None and revision not in summary["worldgen_revisions"]:
                summary["worldgen_revisions"].append(revision)
            if receipt.get("acceptance", {}).get("baseline_holds"):
                summary["passed_unprofiled_clients"].append(clients)
                summary["baseline_holds"] = True
            print(json.dumps({"event": "rung", "clients": clients, "status": receipt["status"],
                              "receipt": str(directory / "receipt.json")}), flush=True)
            if receipt["status"] != "passed":
                summary["stop_reason"] = receipt.get("stop_reason", receipt.get("errors"))
                break
    except Exception as error:
        summary["status"] = "failed"
        summary["stop_reason"] = str(error)
    finally:
        summary["finished"] = stamp()
        dump(run_dir / "receipt.json", summary)
        print(json.dumps({"event": "final", "status": summary["status"],
                          "receipt": str(run_dir / "receipt.json")}), flush=True)
    return 0 if summary["status"] in {"passed", "smoke_only"} else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clients", type=positive, help="single rung; default ladder: 50,100,250,500,1000")
    parser.add_argument("--view-distance", type=positive, default=6)
    parser.add_argument("--single-core", action="store_true",
                        help="pin server to CPU2 and set chunk_pipeline.worker_threads=1; bots stay on CPUs4-11")
    parser.add_argument("--warmup-ticks", type=positive, default=200)
    parser.add_argument("--measure-ticks", type=positive, default=1200)
    parser.add_argument("--spacing", type=positive, default=256)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--fixture-summary", type=absolute_path,
                        help="offline surface-coverage receipt for this seed; embedded in every rung receipt")
    parser.add_argument("--pregenerated-world", type=absolute_path,
                        help="copy this existing world into each rung before startup; source is never modified")
    parser.add_argument("--steady-state", action="store_true",
                        help="bound walking to source-covered anchor chunks (requires fixture and pregenerated world)")
    parser.add_argument("--group-size", type=positive, default=1)
    parser.add_argument("--skip-anchors", type=lambda raw: [int(part) for part in raw.split(",") if part],
                        default=[], help="comma-separated positions in the fixture's dry-anchor list to exclude "
                        "(anchor chunks the bots report as lacking a safe landing); recorded in the receipt")
    parser.add_argument("--action-log", action="store_true",
                        help="pass --action-log to the bots: bounded per-action block-edit diagnostics in the final report")
    parser.add_argument("--out", type=absolute_path, default=ROOT / ".analysis/codex-logs/player-scale")
    parser.add_argument("--server-bin", type=absolute_path, default=ROOT / "target/release/mc-server")
    parser.add_argument("--bots-bin", type=absolute_path, default=ROOT / "target/release/player-scale-bots")
    parser.add_argument("--vanilla-data-dir", type=absolute_path)
    parser.add_argument("--deadline-seconds", type=positive, default=1800,
                        help="finite overall budget including reserved child cleanup; never extended")
    parser.add_argument("--readiness-seconds", type=positive, default=300)
    parser.add_argument("--max-rss-mb", type=positive, default=9216, help="combined server+bots RSS ceiling (MiB)")
    parser.add_argument("--server-max-rss-mb", type=positive, default=8192)
    parser.add_argument("--bots-max-rss-mb", type=positive, default=1024)
    parser.add_argument("--min-headroom-mb", type=positive, default=512, help="minimum host/cgroup memory headroom (MiB)")
    parser.add_argument("--min-tps", type=positive, default=19,
                        help="actual tick progress/sec minimum (default 95%% of nominal 20 TPS)")
    parser.add_argument("--profile", choices=("perf", "samply"))
    parser.add_argument("--profile-first-failing", action="store_true",
                        help="capture each candidate from startup, retain only the first failing rung's profile")
    args = parser.parse_args()
    if args.steady_state and not (args.fixture_summary and args.pregenerated_world):
        parser.error("--steady-state requires --fixture-summary and --pregenerated-world")
    if args.profile_first_failing and not args.profile:
        parser.error("--profile-first-failing requires --profile perf|samply")
    return asyncio.run(run(args))


if __name__ == "__main__":
    raise SystemExit(main())
