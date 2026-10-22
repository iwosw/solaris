"""Player-capacity ladder against the ordinary release v2 server and the Rust protocol bots.

One fresh generated world per rung (no prepared fixture), N open-loop protocol
bots in small groups, wall-clock steady/burst/recovery windows, exact core tick
samples from the dashboard's /load-metrics stream. Reuses the catalog-load
session, config and login-window machinery; adds per-window phase, thread-CPU,
memory and network accounting plus an optional perf capture of the server.

Run it through `.analysis/tools/mem-run.sh -p` (see docs/performance/PLAYER_CAPACITY.md).
It measures; it never classifies a capacity claim beyond the operator thresholds.
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import re
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import time
from typing import Any

from tools.harness.backends import catalog_load as load
from tools.harness.backends import catalog_support as scale

WORKLOADS = {
    # Creative builders: movement, sprint/jump legs, break/place, chat, daytime.
    "build": {
        "setup": ["time set 6000", "gamerule doDaylightCycle false"],
        "bot_args": [],
        "edit_hz": {"steady": 2, "burst": 5},
    },
    # Survival players at night: natural hostile spawns attack them, they die and respawn.
    "fight": {
        "setup": ["time set 18000", "gamerule doDaylightCycle false"],
        "bot_args": ["--survival", "--respawn"],
        "edit_hz": {"steady": 1, "burst": 2},
    },
}


def thread_cpu(pid: int) -> dict[str, float]:
    """CPU seconds per thread name (summed over same-named threads)."""
    ticks = os.sysconf("SC_CLK_TCK")
    result: dict[str, float] = {}
    for task in Path(f"/proc/{pid}/task").iterdir():
        try:
            stat = (task / "stat").read_text()
        except OSError:
            continue
        name = stat[stat.index("(") + 1:stat.rindex(")")]
        fields = stat[stat.rindex(")") + 2:].split()
        result[name] = result.get(name, 0.0) + (int(fields[11]) + int(fields[12])) / ticks
    return result


def phase_summary(samples: list[dict[str, Any]]) -> dict[str, Any]:
    names = sorted({k for s in samples for k in s.get("phase_us", {})})
    rows: dict[str, Any] = {}
    for name in names:
        values = [s["phase_us"].get(name, 0) for s in samples]
        rows[name] = {"mean_us": round(statistics.fmean(values), 1), "p95_us": load.percentile(values, .95),
                      "max_us": max(values)}
    totals = [s["total_us"] for s in samples if isinstance(s.get("total_us"), int)]
    walls = [s["wall_us"] for s in samples]
    if totals:
        timed = [sum(s.get("phase_us", {}).values()) for s in samples if isinstance(s.get("total_us"), int)]
        rows["total_us"] = {"mean_us": round(statistics.fmean(totals), 1), "p95_us": load.percentile(totals, .95)}
        rows["untimed_in_total_mean_us"] = round(statistics.fmean(totals) - statistics.fmean(timed), 1)
    rows["wall_us"] = {"mean_us": round(statistics.fmean(walls), 1), "p95_us": load.percentile(walls, .95)}
    return rows


async def run_perf(pid: int, seconds: int, out: Path, cpus: str) -> dict[str, Any]:
    data = out / "perf.data"
    command = ["taskset", "-c", cpus, "perf", "record", "-F", "499", "--call-graph", "dwarf,16384",
               "-p", str(pid), "-o", str(data), "--", "sleep", str(seconds)]
    process = await asyncio.create_subprocess_exec(
        *command, stdout=(out / "perf-record.log").open("wb"), stderr=asyncio.subprocess.STDOUT)
    code = await process.wait()
    return {"command": command, "exit": code, "data": str(data)}


def perf_reports(out: Path) -> dict[str, str]:
    data = out / "perf.data"
    reports = {}
    for name, extra in (("by-comm-sym", ["--sort", "comm,sym"]), ("by-sym", ["--sort", "sym"])):
        target = out / f"perf-{name}.txt"
        with target.open("wb") as handle:
            subprocess.run(["perf", "report", "-i", str(data), "--no-children", "--stdio", "-g", "none",
                            "--no-inline", "--percent-limit", "0.3", *extra],
                           stdout=handle, stderr=subprocess.DEVNULL, check=False)
        reports[name] = str(target)
    data.unlink(missing_ok=True)
    return reports


def rung_args(args: argparse.Namespace) -> argparse.Namespace:
    ns = argparse.Namespace(**vars(args))
    ns.wire_trace_bots = None
    return ns


async def rung(args: argparse.Namespace, clients: int, directory: Path) -> dict[str, Any]:
    directory.mkdir(parents=True)
    workload = WORKLOADS[args.workload]
    windows = [("steady", args.steady_seconds, 20, workload["edit_hz"]["steady"], 1),
               ("burst", args.burst_seconds, 20, workload["edit_hz"]["burst"], 2)]
    schedule: dict[str, Any] = {"windows": [
        {"name": name, "duration_ms": round(seconds * 1000), "movement_hz": move, "edit_hz": edit, "chat_hz": chat}
        for name, seconds, move, edit, chat in windows]}
    if args.recovery_seconds:
        schedule["windows"].append({"name": "recovery", "duration_ms": round(args.recovery_seconds * 1000),
                                    "recovery": True})
    scale.dump(directory / "schedule.json", schedule)
    scale.dump(directory / "commands.json", {"setup": workload["setup"]})
    world = directory / "world"
    config_path, url, game = load.config(args, directory, world, clients, operators=True)
    session = load.Session(args, directory)
    result: dict[str, Any] = {
        "clients": clients, "workload": args.workload, "view_distance": args.view_distance,
        "group_size": args.group_size, "schedule": schedule, "windows": {}, "infra_errors": [],
        "correctness_errors": [], "performance_errors": [], "server_cpus": args.server_cpus,
        "bot_cpus": args.bot_cpus}
    ranges: list[tuple[str, int, int]] = []
    raw_path = directory / "samples.jsonl"
    perf_task: asyncio.Task[dict[str, Any]] | None = None
    thread_marks: list[tuple[int, dict[str, float]]] = []
    try:
        server = ["taskset", "-c", args.server_cpus, str(args.server_bin), "--config", str(config_path), "--no-console"]
        result["server_command"] = server
        await session.spawn("server", server)
        await load.wait_dashboard(session, url, args.readiness_seconds)
        bots = ["taskset", "-c", args.bot_cpus, str(args.bots_bin), "--addr", f"127.0.0.1:{game}",
                "--clients", str(clients), "--view-distance", str(args.view_distance),
                "--spacing", str(args.spacing), "--group-size", str(args.group_size),
                "--report", str(directory / "bots-report.json"), "--prepare-seconds", str(args.readiness_seconds),
                "--prepare-concurrency", str(args.prepare_concurrency),
                "--commands-file", str(directory / "commands.json"),
                "--open-loop", "--schedule-file", str(directory / "schedule.json"), *workload["bot_args"]]
        result["bot_command"] = bots
        server_pid = session.processes["server"].pid
        login_start_cpu = scale.proc_sample(server_pid)
        initial = await load.login_window(session, url, bots, clients, rung_args(args), result)
        result["login_server_cpu_seconds"] = round(scale.proc_sample(server_pid)["cpu_seconds"] - login_start_cpu["cpu_seconds"], 2)
        last_tick = last_raw_tick = initial.get("latest_tick")
        instance = initial.get("instance_started_unix_ns")
        source_skips = initial.get("source_skipped_ticks_total")
        epoch_wall = time.time_ns()
        offset = 0
        for window in schedule["windows"]:
            start = epoch_wall + offset * 1_000_000
            offset += window["duration_ms"]
            ranges.append((window["name"], start, epoch_wall + offset * 1_000_000))
            result["windows"][window["name"]] = {"queue_samples": [], "process_samples": [], "stats_samples": []}
        proc = session.processes["bots"]
        assert proc.stdin
        proc.stdin.write(b'{"event":"run"}\n')
        await proc.stdin.drain()
        deadline = time.monotonic() + args.readiness_seconds
        while True:
            event = await asyncio.wait_for(session.events.get(), max(.01, deadline - time.monotonic()))
            if event.get("event") == "failure":
                session.alive()
                raise load.ClassifiedFailure("CORRECTNESS_FAIL", str(event.get("error")))
            if event.get("event") == "running":
                epoch = event.get("open_loop", {}).get("epoch_unix_ns")
                if not isinstance(epoch, int) or epoch <= 0:
                    raise load.ClassifiedFailure("INFRA_BLOCKED", "running event lacks shared wall epoch")
                shift = epoch - epoch_wall
                epoch_wall = epoch
                ranges = [(n, s + shift, e + shift) for n, s, e in ranges]
                break
        result["run_epoch_unix_ns"] = epoch_wall
        if args.perf_seconds:
            async def delayed_perf() -> dict[str, Any]:
                await asyncio.sleep(args.perf_delay_seconds)
                return await run_perf(server_pid, args.perf_seconds, directory, args.bot_cpus)
            perf_task = asyncio.create_task(delayed_perf())
            result["perf_window_offset_seconds"] = args.perf_delay_seconds
        completion_seen = False
        stream_deadline = time.monotonic() + offset / 1000 + args.readiness_seconds
        previous_wall_start: int | None = None
        with raw_path.open("w", buffering=1) as raw:
            while not completion_seen:
                if time.monotonic() >= stream_deadline:
                    raise load.ClassifiedFailure("INFRA_BLOCKED", "no completed tick starting at/after scheduled window end")
                session.alive()
                metrics = await session.http(url + f"/load-metrics?after_tick={last_tick or 0}&wait=true&min_samples=20", 12)
                captured = metrics.get("captured_unix_ns", time.time_ns())
                processes = session.process_samples()
                stats = await session.http(url + "/stats", 2)
                threads = thread_cpu(server_pid)
                raw.write(json.dumps({"captured_unix_ns": captured, "metrics": metrics, "processes": processes,
                                      "stats": stats, "threads": threads, "bots": session.latest_bot}) + "\n")
                if metrics.get("overrun"):
                    result["infra_errors"].append("raw tick ring overwrote requested samples")
                if metrics.get("instance_started_unix_ns") != instance:
                    result["infra_errors"].append("telemetry instance changed during measured stream")
                if metrics.get("source_skipped_ticks_total") != source_skips:
                    result["infra_errors"].append("core timing stream lost samples")
                for sample in metrics.get("samples", []):
                    tick = sample["tick"]
                    if last_raw_tick is not None and tick != last_raw_tick + 1:
                        result["infra_errors"].append(f"noncontiguous raw tick stream: {last_raw_tick}->{tick}")
                    last_raw_tick = last_tick = tick
                    wall_start = sample.get("wall_start_unix_ns")
                    if not isinstance(sample.get("wall_us"), int) or not isinstance(wall_start, int) or wall_start <= 0:
                        result["infra_errors"].append("raw tick lacks wall time/start")
                    elif previous_wall_start is not None and wall_start <= previous_wall_start:
                        result["infra_errors"].append("wall clock regressed")
                    previous_wall_start = wall_start
                    if isinstance(wall_start, int) and wall_start >= ranges[-1][2]:
                        completion_seen = True
                name = next((n for n, s, e in ranges if s <= captured < e), None)
                if name:
                    window = result["windows"][name]
                    window["queue_samples"].append(metrics.get("queues"))
                    window["process_samples"].append({"captured_unix_ns": captured, **processes})
                    window["stats_samples"].append({"captured_unix_ns": captured,
                        "network_bytes_written": stats.get("network", {}).get("bytes_written"),
                        "reliable_drops": stats.get("network", {}).get("reliable_drops"),
                        "entities": stats.get("entities", {}).get("total"),
                        "chunks_loaded": stats.get("chunks", {}).get("loaded_total"),
                        "players": stats.get("players", {}).get("count"),
                        "threads": threads})
                if stats.get("players", {}).get("count") != clients:
                    result["correctness_errors"].append(
                        f"roster {stats.get('players', {}).get('count')} != {clients} at {captured}")
        if perf_task is not None:
            result["perf"] = await perf_task
    except (load.ClassifiedFailure, OSError, ValueError, KeyError, asyncio.TimeoutError) as error:
        result["outcome"] = error.outcome if isinstance(error, load.ClassifiedFailure) else "INFRA_BLOCKED"
        result["reason"] = str(error) if isinstance(error, load.ClassifiedFailure) else f"{type(error).__name__}: {error}"
        result["measurement_complete"] = False
        if perf_task is not None and not perf_task.done():
            perf_task.cancel()
        if not ranges:
            return result
    finally:
        stop_started = time.monotonic()
        await session.stop(server_timeout=args.server_stop_seconds)
        result["stop_seconds"] = round(time.monotonic() - stop_started, 1)
        result["shutdown"] = session.shutdown
        shutil.rmtree(world, ignore_errors=True)
        scale.dump(directory / "result.json", result)
    failed = result.get("measurement_complete") is False
    try:
        report = json.loads((directory / "bots-report.json").read_text())
    except (OSError, ValueError):
        report = {}
    bot_epoch = report.get("open_loop", {}).get("epoch_unix_ns")
    if isinstance(bot_epoch, int):
        shift = bot_epoch - result["run_epoch_unix_ns"]
        ranges = [(n, s + shift, e + shift) for n, s, e in ranges]
    elif not failed:
        result["infra_errors"].append("bot report lacks shared wall epoch")
    by_window: dict[str, list[dict[str, Any]]] = {n: [] for n, _, _ in ranges}
    with raw_path.open() as raw:
        for line in raw:
            for sample in json.loads(line)["metrics"].get("samples", []):
                at = sample.get("wall_start_unix_ns")
                name = next((n for n, s, e in ranges if isinstance(at, int) and s <= at < e), None)
                if name:
                    by_window[name].append(sample)
    last_sample_ns = max((s["wall_start_unix_ns"] for rows in by_window.values() for s in rows), default=0)
    for name, start, end in ranges:
        window = result["windows"][name]
        window["start_unix_ns"], window["end_unix_ns"] = start, end
        samples = by_window[name]
        # A kicked/failed run stops early: measure only the part of the window the server actually ran.
        end = min(end, last_sample_ns + 50_000_000) if failed else end
        window["partial"] = end < window["end_unix_ns"]
        if not samples or start >= end:
            if not failed:
                result["infra_errors"].append(f"{name}: no tick samples")
            continue
        elapsed = (end - start) / 1e9
        window["elapsed_seconds"] = round(elapsed, 1)
        window["tick"] = load.tick_metrics(samples)
        window["tps"] = len(samples) / elapsed
        window["sustained_tps_min"] = load.sustained_tps(samples, start, end, args.sustained_seconds)
        window["phases"] = phase_summary(samples)
        if name != "recovery":
            if window["tps"] < args.min_tps or (window["sustained_tps_min"] or args.min_tps) < args.min_tps:
                result["performance_errors"].append(f"{name}: average/sustained TPS below {args.min_tps}")
            if window["tick"]["p99_us"] > args.max_tick_p99_ms * 1000:
                result["performance_errors"].append(f"{name}: tick p99 above {args.max_tick_p99_ms} ms")
            if window["tick"]["max_us"] > args.max_tick_max_ms * 1000:
                result["performance_errors"].append(f"{name}: tick max above {args.max_tick_max_ms} ms")
        procs = window.pop("process_samples")
        stats = window.pop("stats_samples")
        queues = window.pop("queue_samples")
        if len(procs) >= 2:
            first, last = procs[0], procs[-1]
            span = (last["captured_unix_ns"] - first["captured_unix_ns"]) / 1e9
            window["cpu_cores_used"] = {k: round((last[k]["cpu_seconds"] - first[k]["cpu_seconds"]) / span, 3)
                                        for k in ("server", "bots")}
            window["rss_max_mib"] = {k: round(max(p[k]["rss_bytes"] for p in procs) / 2**20, 1) for k in ("server", "bots")}
            window["rss_peak_lifetime_mib"] = {k: round(max(p[k]["peak_rss_bytes"] for p in procs) / 2**20, 1)
                                               for k in ("server", "bots")}
            window["server_threads"] = procs[-1]["server"]["threads"]
        if len(stats) >= 2:
            first, last = stats[0], stats[-1]
            span = (last["captured_unix_ns"] - first["captured_unix_ns"]) / 1e9
            written = last["network_bytes_written"] - first["network_bytes_written"]
            window["network"] = {"server_written_bytes": written, "server_written_kib_s": round(written / span / 1024, 1),
                                 "per_player_kib_s": round(written / span / 1024 / clients, 2),
                                 "reliable_drops_delta": (last["reliable_drops"] or 0) - (first["reliable_drops"] or 0)}
            window["thread_cpu_cores"] = {
                k: round((last["threads"].get(k, 0) - first["threads"].get(k, 0)) / span, 3)
                for k in sorted(last["threads"]) if last["threads"].get(k, 0) - first["threads"].get(k, 0) > 0.02 * span}
            window["entities_end"], window["chunks_loaded_end"] = last["entities"], last["chunks_loaded"]
        window["edge_send_queue_max_slots"] = max(
            (sum(q[k]["queued_or_reserved_slots"] for k in ("reliable", "world")) for q in queues if q), default=None)
        if window.get("rss_max_mib", {}).get("server", 0) > args.max_rss_mb:
            result["performance_errors"].append(f"{name}: server RSS above {args.max_rss_mb} MiB")
    result["bots"] = {k: report.get(k) for k in (
        "clients", "ready_clients", "disconnects", "decoded_bytes_received", "byte_metric", "chunks_received",
        "movements_sent", "breaks_sent", "breaks_acked", "places_acked", "health_damage_received",
        "action_ack_latency", "action_ack_timeouts", "action_observation_timeouts", "chats_sent",
        "chat_echoes_received", "elapsed_seconds", "error")}
    if report.get("disconnects"):
        result["correctness_errors"].append(f"{report['disconnects']} bot disconnects")
    for name, row in session.shutdown.items():
        if row["forced"] or row["returncode"] != 0:
            result["correctness_errors"].append(f"{name}: non-clean shutdown")
    if args.perf_seconds and (directory / "perf.data").exists():
        result["perf_reports"] = perf_reports(directory)
    server_log = (directory / "server.log").read_text(errors="replace")
    result["kicked_connections"] = server_log.count("hub queue full; connection closed")
    result["checkpoints"] = checkpoint_summary(server_log)
    if result["kicked_connections"]:
        result["correctness_errors"].append(
            f"{result['kicked_connections']} connections shed by the server (hub input queue full)")
    if not failed:
        result["outcome"] = load.classify(infra=result["infra_errors"], correctness=result["correctness_errors"],
                                          performance=result["performance_errors"])
        result["measurement_complete"] = True
    scale.dump(directory / "result.json", result)
    return result


CHECKPOINT = re.compile(r"checkpoint committed tick=(\d+) total_ms=(\d+) chunk_ms=(\d+) entity_ms=(\d+) "
                        r"db_commit_ms=(\d+) .*?rewritten_chunks=(\d+) chunk_payload_bytes=(\d+)")


def checkpoint_summary(log: str) -> dict[str, Any]:
    rows = [tuple(map(int, m.groups())) for m in CHECKPOINT.finditer(log)]
    if not rows:
        return {"count": 0}
    return {"count": len(rows), "max_total_ms": max(r[1] for r in rows),
            "mean_total_ms": round(statistics.fmean(r[1] for r in rows)),
            "max_chunk_ms": max(r[2] for r in rows), "max_db_commit_ms": max(r[4] for r in rows),
            "max_rewritten_chunks": max(r[5] for r in rows),
            "max_chunk_payload_mib": round(max(r[6] for r in rows) / 2**20, 1),
            "rows": [{"tick": r[0], "total_ms": r[1], "chunk_ms": r[2], "db_commit_ms": r[4],
                      "rewritten_chunks": r[5]} for r in rows]}
    return result


def headline(result: dict[str, Any]) -> dict[str, Any]:
    """Compact per-window figures for the receipt table."""
    rows: dict[str, Any] = {}
    login = result.get("login_burst") or {}
    if login.get("tick"):
        rows["login"] = {"seconds": round(result["login_elapsed_seconds"], 1), "tps": round(login["tps"], 2),
                         "p50_ms": login["tick"]["p50_us"] / 1000, "p95_ms": login["tick"]["p95_us"] / 1000,
                         "p99_ms": login["tick"]["p99_us"] / 1000, "max_ms": login["tick"]["max_us"] / 1000,
                         "server_cpu_cores": round(result.get("login_server_cpu_seconds", 0) / login_seconds(result), 2)}
    for name, window in result.get("windows", {}).items():
        tick = window.get("tick")
        if not tick:
            continue
        rows[name] = {"partial": window.get("partial"), "elapsed_s": window.get("elapsed_seconds"),
                      "tps": round(window["tps"], 2), "sustained_tps_min": window["sustained_tps_min"],
                      "p50_ms": tick["p50_us"] / 1000, "p95_ms": tick["p95_us"] / 1000,
                      "p99_ms": tick["p99_us"] / 1000, "max_ms": tick["max_us"] / 1000,
                      "over_50ms": tick["deadline_misses_50ms"], "samples": tick["samples"],
                      "server_cpu_cores": window.get("cpu_cores_used", {}).get("server"),
                      "bots_cpu_cores": window.get("cpu_cores_used", {}).get("bots"),
                      "server_rss_mib": window.get("rss_max_mib", {}).get("server"),
                      "kib_s_per_player": window.get("network", {}).get("per_player_kib_s"),
                      "entities": window.get("entities_end"), "chunks": window.get("chunks_loaded_end"),
                      "top_threads": dict(sorted(window.get("thread_cpu_cores", {}).items(),
                                                 key=lambda item: -item[1])[:5])}
    rows["kicked_connections"] = result.get("kicked_connections")
    rows["checkpoint"] = {k: result.get("checkpoints", {}).get(k) for k in (
        "count", "max_total_ms", "mean_total_ms", "max_rewritten_chunks", "max_chunk_payload_mib")}
    rows["stop_seconds"] = result.get("stop_seconds")
    return rows


def login_seconds(result: dict[str, Any]) -> float:
    return max(result["login_elapsed_seconds"], 1e-9)


async def main_async(args: argparse.Namespace) -> int:
    args.out.mkdir(parents=True, exist_ok=False)
    for name in ("server_bin", "bots_bin"):
        binary = getattr(args, name)
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise SystemExit(f"missing/nonexecutable {name}: {binary}")
    summary: dict[str, Any] = {
        "command": [sys.executable, "-m", "tools.coreperf.capacity_ladder", *sys.argv[1:]],
        "binaries": {n: {"path": str(getattr(args, n)), "sha256": scale.binary_sha256(getattr(args, n))}
                     for n in ("server_bin", "bots_bin")},
        "loadavg_start": Path("/proc/loadavg").read_text().strip(), "rungs": []}
    for clients in args.clients:
        result = await rung(args, clients, args.out / f"rung-{clients}")
        row = {"clients": clients, "outcome": result.get("outcome"), "reason": result.get("reason"),
               "performance_errors": result.get("performance_errors"), "headline": headline(result),
               "receipt": str(args.out / f"rung-{clients}/result.json")}
        summary["rungs"].append(row)
        print(json.dumps({"event": "rung", **row}), flush=True)
        scale.dump(args.out / "summary.json", summary)
    summary["loadavg_end"] = Path("/proc/loadavg").read_text().strip()
    scale.dump(args.out / "summary.json", summary)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--server-bin", type=Path, required=True)
    parser.add_argument("--bots-bin", type=Path, required=True)
    parser.add_argument("--vanilla-data-dir", type=Path, default=load.ROOT / "data/vanilla")
    parser.add_argument("--clients", type=int, nargs="+", default=[20, 50, 100])
    parser.add_argument("--workload", choices=sorted(WORKLOADS), default="build")
    parser.add_argument("--view-distance", type=int, default=8)
    parser.add_argument("--group-size", type=int, default=5)
    parser.add_argument("--spacing", type=int, default=256)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--steady-seconds", type=float, default=120)
    parser.add_argument("--burst-seconds", type=float, default=60)
    parser.add_argument("--recovery-seconds", type=float, default=20)
    parser.add_argument("--readiness-seconds", type=int, default=1500)
    parser.add_argument("--prepare-concurrency", type=int, default=3)
    parser.add_argument("--server-cpus", default="8-9")
    parser.add_argument("--bot-cpus", default="10-11")
    parser.add_argument("--min-tps", type=float, default=19.5)
    parser.add_argument("--sustained-seconds", type=int, default=5)
    parser.add_argument("--max-tick-p99-ms", type=float, default=50)
    parser.add_argument("--max-tick-max-ms", type=float, default=250)
    parser.add_argument("--max-rss-mb", type=float, default=3072)
    parser.add_argument("--perf-seconds", type=int, default=0,
                        help="perf-record the server this long inside the steady window (diagnostic, not a baseline)")
    parser.add_argument("--perf-delay-seconds", type=int, default=20)
    parser.add_argument("--server-stop-seconds", type=float, default=180,
                        help="SIGTERM grace before SIGKILL; the final save of a loaded world can exceed the 10 s default")
    args = parser.parse_args()
    return asyncio.run(main_async(args))


if __name__ == "__main__":
    raise SystemExit(main())
