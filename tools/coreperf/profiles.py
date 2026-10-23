"""Reproducible release workload measurements, not capacity/parity acceptance.

Reuses the canonical public-command fixture, process lifecycle and exact tick
stream. Preparation and login are outside the warmup/measurement windows.
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
from pathlib import Path
import platform
import signal
import statistics
import subprocess
import sys
import time
from typing import Any

from tools.coreperf import compare
from tools.harness.backends import catalog_load as load

PROFILES = {"idle": (0, 0, 0, 8), "explore-20": (20, 0, 0, 8),
            "native-100": (100, 100000, 0, 6), "villager-heavy": (100, 100000, 20000, 6)}


def fixture(mobs: int, villagers: int) -> dict[str, Any]:
    """PGOBolt's shared public-command pens; no private world seeding."""
    value = compare.fixture(mobs, 0)
    value["regions"].pop()
    regions: list[dict[str, Any]] = []
    for region in value["regions"]:
        if (regions and not regions[-1].get("paired")
                and regions[-1]["anchor"][1] == region["anchor"][1]):
            previous = regions[-1]
            previous["anchor"][0] = (previous["anchor"][0] + region["anchor"][0]) // 2
            previous["commands"].extend(region["commands"])
            previous["paired"] = True
        else:
            regions.append(region)
    value["regions"] = regions
    for group, offset in enumerate(range(0, villagers, 1024)):
        x, z = -800 - (group % 5) * 96, (group // 5) * 96
        commands = [
            f"fill {x-8} 62 {z-8} {x+72} 62 {z+72} minecraft:dirt",
            f"fill {x-8} 63 {z-8} {x+72} 63 {z+72} minecraft:grass_block",
            f"fill {x-9} 64 {z-9} {x+73} 70 {z-9} minecraft:stone",
            f"fill {x-9} 64 {z+73} {x+73} 70 {z+73} minecraft:stone",
            f"fill {x-9} 64 {z-8} {x-9} 70 {z+72} minecraft:stone",
            f"fill {x+73} 64 {z-8} {x+73} 70 {z+72} minecraft:stone",
            f"fill {x-9} 71 {z-9} {x+73} 71 {z+73} minecraft:stone",
        ]
        for dx in (0, 40):
            for dz in (0, 40):
                commands.append(f"fill {x+dx-8} 64 {z+dz-8} {x+dx+32} 70 {z+dz+32} minecraft:air")
        for index in range(min(1024, villagers-offset)):
            px, pz = x+(index % 32)*2, z+(index//32)*2
            commands.extend([
                f"setblock {px} 64 {pz} minecraft:composter",
                f"setblock {px} 64 {pz+1} minecraft:red_bed[facing=east,part=foot]",
                f"setblock {px+1} 64 {pz+1} minecraft:red_bed[facing=east,part=head]",
                f"summon minecraft:villager {px+1.5} 64 {pz+.5} {{CustomName:'{compare.COHORT_NAME}',PersistenceRequired:1b}}",
            ])
        value["regions"].append({"name": f"villagers-{group}", "anchor": [x+32, z+32],
                                 "view_distance": 8, "commands": commands})
    value["expected"]["minecraft:villager"] = villagers
    # Keep this identity byte-for-byte compatible with the shared PGO cache.
    value["variant"] = "pgo-internal-100k-mobs-20k-villagers-100-wire-players"
    value["official_performance_evidence"] = False
    if not mobs and not villagers:
        value["regions"] = [{"name": "spawn", "anchor": [32, 32], "view_distance": 8,
            "commands": ["gamerule doMobSpawning false", "gamerule doWeatherCycle false",
                         "gamerule randomTickSpeed 3", "gamerule doDaylightCycle true",
                         "weather clear", "time set 2000", "difficulty easy"]}]
        value["variant"] = "profiles-natural-terrain-no-summoned-cohort"
    return value


def schedule(args: argparse.Namespace) -> dict[str, Any]:
    active = args.profile != "idle"
    return {"windows": [{"name": name, "duration_ms": round(seconds * 1000),
                         "movement_hz": 20 if active else 0,
                         "edit_hz": 2 if args.mobs else 0,
                         "chat_hz": 1 if args.mobs else 0}
                        for name, seconds in (("warmup", args.warmup_seconds),
                                              ("measurement", args.measure_seconds))]}


def host_receipt(out: Path) -> dict[str, Any]:
    def capture(command: list[str]) -> str:
        return subprocess.check_output(command, text=True).strip()
    return {"cpu": json.loads(capture(["lscpu", "--json"])),
            "memory": Path("/proc/meminfo").read_text(), "os": platform.freedesktop_os_release(),
            "kernel": platform.uname()._asdict(),
            "disks": json.loads(capture(["lsblk", "--json", "-o", "NAME,MODEL,SIZE,ROTA,FSTYPE,MOUNTPOINTS"])),
            "artifact_filesystem": capture(["findmnt", "-T", str(out), "-o", "SOURCE,FSTYPE,OPTIONS"]),
            "cpu_affinity": sorted(os.sched_getaffinity(0)),
            "loadavg": Path("/proc/loadavg").read_text().strip(),
            "cgroup": Path("/proc/self/cgroup").read_text()}


def summarize(samples: list[dict[str, Any]], processes: list[dict[str, Any]],
              start: int, end: int) -> dict[str, Any]:
    ticks = [s for s in samples if start <= s["wall_start_unix_ns"] < end]
    rows = [r for r in processes if start <= r["captured_unix_ns"] < end]
    if not ticks or not rows:
        raise ValueError("measurement lacks exact ticks or process samples")
    seconds = (end - start) / 1e9
    tick = load.tick_metrics(ticks)
    return {"duration_seconds": seconds, "ticks": len(ticks), "tps": len(ticks) / seconds,
            "mspt": {key: tick[key + "_us"] / 1000 for key in ("p50", "p95", "p99", "max")},
            "rss": {name: {"peak_lifetime_bytes": max(r["processes"][name]["peak_rss_bytes"] for r in rows),
                           "peak_sampled_bytes": max(r["processes"][name]["rss_bytes"] for r in rows),
                           "steady_median_bytes": statistics.median(r["processes"][name]["rss_bytes"] for r in rows)}
                    for name in rows[0]["processes"]},
            "chunk_generation_per_second": None,
            "chunk_generation_scope": "not instrumented; resident chunk deltas include reloads/unloads",
            "population_observed": {"first": rows[0]["stats"]["entities"], "last": rows[-1]["stats"]["entities"]},
            "resident_chunks_observed": {"first": rows[0]["stats"]["chunks"], "last": rows[-1]["stats"]["chunks"]}}


def validate_metrics(metric: dict[str, Any], initial: dict[str, Any], cursor: int) -> int:
    if (metric.get("schema_version") != 1 or metric.get("overrun") or
            metric.get("instance_started_unix_ns") != initial.get("instance_started_unix_ns") or
            metric.get("source_skipped_ticks_total") != initial.get("source_skipped_ticks_total")):
        raise ValueError("exact tick stream overwritten, reset or lost source samples")
    for row in metric["samples"]:
        if row["tick"] != cursor + 1:
            raise ValueError("noncontiguous exact tick stream")
        if (not isinstance(row.get("wall_start_unix_ns"), int) or row["wall_start_unix_ns"] <= 0
                or not isinstance(row.get("wall_us"), int) or row["wall_us"] < 0):
            raise ValueError("exact tick lacks a valid wall start/duration")
        cursor = row["tick"]
    return cursor


async def measure(args: argparse.Namespace, directory: Path, world: Path,
                  recipe: dict[str, Any], prepared: dict[str, Any]) -> dict[str, Any]:
    directory.mkdir()
    config, url, game = load.config(args, directory, world, args.clients, operators=True)
    # Idle measures simulation, not the default pause-when-empty policy.
    config.write_text(config.read_text().replace("[server]\n", "[server]\npause_when_empty_seconds = -1\n", 1))
    session = compare.CoreSession(args, directory, config)
    plan = schedule(args)
    load.scale.dump(directory / "schedule.json", plan)
    result: dict[str, Any] = {"status": "failed", "schedule": plan, "config": str(config),
                              "timing_scope": "exact active-iteration wall_us, excluding pacing sleep"}
    samples: list[dict[str, Any]] = []
    process_rows: list[dict[str, Any]] = []
    try:
        await session.start_server()
        await load.wait_dashboard(session, url, args.readiness_seconds)
        if args.clients:
            command = ["taskset", "-c", args.bot_cpus,
                       *load.bot_command(args, directory, game, args.clients, {"group_size": 1}),
                       "--open-loop", "--schedule-file", str(directory / "schedule.json")]
            if args.mobs:
                anchors = [region["anchor"] for region in recipe["regions"]]
                load.scale.dump(directory / "anchors.json", [anchors[n % len(anchors)] for n in range(args.clients)])
                command += ["--anchors", str(directory / "anchors.json")]
            else:
                command += ["--explore", "--no-edits"]
            result["bot_command"] = command
            await session.spawn("bots", command, events=True)
            result["ready"] = await session.ready(args.clients, args.readiness_seconds)
        initial = await session.http(url + "/load-metrics", 2)
        if initial.get("schema_version") != 1 or initial.get("latest_tick") is None:
            raise ValueError("exact source tick telemetry unavailable")
        cursor = initial["latest_tick"]
        epoch = time.time_ns()
        if args.clients:
            stdin = session.processes["bots"].stdin
            assert stdin
            stdin.write(b'{"event":"run"}\n')
            await stdin.drain()
            running = await load._event(session, "running", args.readiness_seconds)
            epoch = running["open_loop"]["epoch_unix_ns"]
        start = epoch + round(args.warmup_seconds * 1e9)
        end = start + round(args.measure_seconds * 1e9)
        result.update(epoch_unix_ns=epoch, start_unix_ns=start, end_unix_ns=end)
        deadline = time.monotonic() + args.warmup_seconds + args.measure_seconds + args.readiness_seconds
        previous_wall_start = 0
        with (directory / "samples.jsonl").open("w", buffering=1) as raw:
            while True:
                session.alive()
                if time.monotonic() >= deadline:
                    raise ValueError("measurement completion deadline expired")
                metric = await session.http(url + f"/load-metrics?after_tick={cursor}&wait=true&min_samples=20", 12)
                cursor = validate_metrics(metric, initial, cursor)
                for tick in metric["samples"]:
                    if tick["wall_start_unix_ns"] <= previous_wall_start:
                        raise ValueError("source wall clock regressed")
                    previous_wall_start = tick["wall_start_unix_ns"]
                stats = await session.http(url + "/stats", 2)
                row = {"captured_unix_ns": time.time_ns(), "processes": session.process_samples(),
                       "stats": stats, "bots": session.latest_bot}
                raw.write(json.dumps({"metrics": metric, **row}) + "\n")
                samples.extend(metric["samples"])
                process_rows.append(row)
                if stats["players"]["count"] != args.clients:
                    raise ValueError("live player count differs from requested profile")
                if any(p["rss_bytes"] > args.max_rss_mb * 2**20 for p in row["processes"].values()):
                    raise ValueError("process RSS exceeded configured memory guard")
                if metric["samples"] and metric["samples"][-1]["wall_start_unix_ns"] >= end:
                    break
        result["summary"] = summarize(samples, process_rows, start, end)
    finally:
        await session.stop()
        result["shutdown"] = session.shutdown
        load.scale.dump(directory / "result.json", result)
    compare.clean_shutdown(session)
    if args.clients:
        bots = json.loads((directory / "bots-report.json").read_text())
        result["bots"] = bots
        if bots.get("success") is not True or not bots.get("open_loop", {}).get("schedule_complete"):
            raise ValueError("bot schedule failed or incomplete")
        if bots.get("movements_sent", 0) == 0:
            raise ValueError("requested active profile sent no movement")
        if args.profile == "explore-20" and bots.get("chunk_boundaries_crossed", 0) == 0:
            raise ValueError("exploration did not cross a chunk boundary")
    snapshot = await compare.snapshot(args, directory, world, config)
    result["entity_counts_after"] = {kind: group["active"] for kind, group in snapshot["entities"].items()}
    after = compare.cohort(snapshot, recipe["expected"])
    before = prepared["cohort"]
    if before["identities"].keys() != after["identities"].keys():
        raise ValueError("persistent fixture cohort changed identity")
    result["cohort_counts"] = after["counts"]
    result["status"] = "smoke" if args.smoke else "measured"
    result["publication_evidence"] = not args.smoke
    load.scale.dump(directory / "result.json", result)
    return result


async def run(args: argparse.Namespace) -> int:
    args.out.mkdir(parents=True, exist_ok=False)
    result: dict[str, Any] = {"status": "failed", "profile": args.profile,
                             "scope": "stated workload measurement; not parity/capacity acceptance",
                             "command": [sys.executable, "-m", "tools.coreperf.profiles", *sys.argv[1:]],
                             "conditions": {k: str(v) if isinstance(v, Path) else v for k, v in vars(args).items()}}
    task = asyncio.current_task()
    assert task
    asyncio.get_running_loop().add_signal_handler(signal.SIGTERM, task.cancel)
    try:
        result["host"] = host_receipt(args.out)
        build = json.loads(args.build_receipt.read_text())
        hashes = {"server_sha256": load.scale.binary_sha256(args.server_bin),
                  "bots_sha256": load.scale.binary_sha256(args.bots_bin)}
        if build.get("profile") != "release" or any(build.get(k) != v for k, v in hashes.items()):
            raise ValueError("release build receipt does not match supplied binaries")
        if not build.get("commit") or not build.get("command"):
            raise ValueError("build receipt lacks commit or exact build command")
        result["build"] = build
        result["registry"] = load.tree_hash(args.vanilla_data_dir)
        recipe = fixture(args.mobs, args.villagers)
        load.scale.dump(args.out / "fixture.json", recipe)
        key = compare.fixture_cache_key(args, recipe, result["registry"]["sha256"])
        world, prepared = await compare.prepare(args, args.out / "prepare", recipe, key, hashes)
        result["fixture_cache"] = prepared["fixture_cache"]
        result["entity_counts_before"] = {kind: group["active"] for kind, group in prepared["snapshot"]["entities"].items()}
        if args.prepare_only:
            result["status"] = "prepared"
        else:
            result["measurement"] = await measure(args, args.out / "run", world, recipe, prepared)
            result["status"] = "smoke" if args.smoke else "measured"
            result["publication_evidence"] = not args.smoke
    except (Exception, asyncio.CancelledError) as error:
        result["error"] = f"{type(error).__name__}: {error}"
    finally:
        result["logs"] = [str(path) for path in sorted(args.out.rglob("*.log"))]
        result["commands"] = [json.loads(path.read_text()) for path in sorted(args.out.rglob("server-command.json"))]
        result["exits"] = {}
        for phase, filename in (("prepare", "receipt.json"), ("run", "result.json")):
            receipt_path = args.out / phase / filename
            if receipt_path.is_file():
                receipt = json.loads(receipt_path.read_text())
                if phase == "prepare" and receipt.get("fixture_cache", {}).get("status") == "hit":
                    continue  # Cached preparation commands/exits belong to the producer, not this run.
                result["exits"][phase] = receipt.get("shutdown", {})
                for row in receipt.get("regions", []):
                    result["commands"].append(row["bot_command"])
                    result["exits"][f"prepare/{row['region']}"] = row.get("shutdown", {})
                if receipt.get("bot_command"):
                    result["commands"].append(receipt["bot_command"])
        load.scale.dump(args.out / "result.json", result)
    print(json.dumps({"status": result["status"], "receipt": str(args.out / "result.json"),
                      "error": result.get("error")}), flush=True)
    return int(result["status"] not in ("measured", "prepared", "smoke"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile", choices=PROFILES)
    parser.add_argument("--server-bin", type=Path, required=True)
    parser.add_argument("--bots-bin", type=Path, required=True)
    parser.add_argument("--build-receipt", type=Path, required=True)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--fixture-cache", type=Path, default=load.ROOT / ".analysis/profile-fixtures")
    parser.add_argument("--vanilla-data-dir", type=Path, default=load.ROOT / "data/vanilla")
    parser.add_argument("--server-cpus", default="0-7")
    parser.add_argument("--bot-cpus", default="8-11")
    parser.add_argument("--warmup-seconds", type=int, default=30)
    parser.add_argument("--measure-seconds", type=int, default=120)
    parser.add_argument("--readiness-seconds", type=int, default=1800)
    parser.add_argument("--max-rss-mb", type=int, default=4096)
    parser.add_argument("--prepare-only", action="store_true",
                        help="populate/reuse the fixture cache without measuring; never a performance receipt")
    parser.add_argument("--smoke", action="store_true",
                        help="2s warmup + 5s measurement at full population; lifecycle only, never publication evidence")
    args = parser.parse_args()
    args.clients, args.mobs, args.villagers, args.view_distance = PROFILES[args.profile]
    args.seed, args.prepare_concurrency = 42, 3
    if min(args.warmup_seconds, args.measure_seconds, args.readiness_seconds, args.max_rss_mb) <= 0:
        parser.error("durations and RSS guard must be positive")
    if args.smoke:
        args.warmup_seconds, args.measure_seconds = 2, 5
    if args.out is None:
        args.out = load.ROOT / ".analysis/validation" / f"{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-profile-{args.profile}-{os.getpid()}"
    for name in ("server_bin", "bots_bin", "build_receipt", "out", "fixture_cache", "vanilla_data_dir"):
        setattr(args, name, getattr(args, name).resolve())
    return asyncio.run(run(args))


if __name__ == "__main__":
    raise SystemExit(main())
