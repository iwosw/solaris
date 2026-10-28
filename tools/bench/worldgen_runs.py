"""Chunk-generation throughput: `solaris-v2 world pregenerate` and an optional vanilla server."""
from __future__ import annotations

import json
import os
from pathlib import Path
import queue
import re
import shutil
import socket
import statistics
import subprocess
import threading
import time
from typing import Any
import zipfile

from tools.bench import common

PROGRESS = re.compile(r"^pregenerate: (\d+)/(\d+) chunks .*?, (\d+) generated, (\d+) already stored, ([\d.]+)s$")
COMPLETE = re.compile(
    r"^pregenerate complete: chunks \[(-?\d+),(-?\d+)\]\.\.\[(-?\d+),(-?\d+)\] around block \((-?\d+),(-?\d+)\), "
    r"(\d+) chunks, (\d+) generated, (\d+) already stored, ([\d.]+)s$")
MAX_FORCELOAD_CHUNKS = 256  # vanilla refuses larger single forceload areas
MAX_VANILLA_CHUNKS = 30000  # one `bench:check` function run must stay under the command chain limit


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def time_wrapper(time_file: Path) -> list[str]:
    return ["/usr/bin/time", "-v", "-o", str(time_file)] if Path("/usr/bin/time").exists() else []


def parse_time_file(path: Path) -> dict[str, Any]:
    return parse_time_text(path.read_text() if path.is_file() else None)


def parse_time_text(text: str | None) -> dict[str, Any]:
    """Peak RSS and CPU seconds of the child that GNU `time -v` wrapped (None when time is unavailable)."""
    if text is None:
        return {"peak_rss_mib": None, "cpu_seconds": None}

    def number(label: str) -> float | None:
        match = re.search(rf"{re.escape(label)}: ([\d.]+)", text)
        return float(match[1]) if match else None
    user, system, rss_kib = number("User time (seconds)"), number("System time (seconds)"), number("Maximum resident set size (kbytes)")
    return {"peak_rss_mib": rss_kib / 1024 if rss_kib is not None else None,
            "cpu_seconds": user + system if user is not None and system is not None else None}


def parse_pregenerate(output: str) -> dict[str, Any]:
    """Generation-only and whole-command rates from the progress/complete lines."""
    first = None
    complete = None
    for line in output.splitlines():
        if (match := PROGRESS.match(line)) and first is None:
            first = float(match[5])  # seconds spent opening the world before generation starts
        if match := COMPLETE.match(line):
            complete = match
    if not complete:
        raise common.BenchError("pregenerate printed no `pregenerate complete` line")
    chunks, generated, stored, elapsed = int(complete[7]), int(complete[8]), int(complete[9]), float(complete[10])
    generation = max(elapsed - (first or 0.0), 1e-9)
    return {"chunk_min": [int(complete[1]), int(complete[2])], "chunk_max": [int(complete[3]), int(complete[4])],
            "chunks": chunks, "generated": generated, "already_stored": stored,
            "setup_seconds": first, "generation_seconds": generation, "elapsed_seconds": elapsed,
            "chunks_per_s": generated / generation, "chunks_per_s_incl_setup": generated / elapsed if elapsed else None}


def solaris_pregenerate(*, host: Any, out: Path, workdir: str, server_bin: str, vanilla_data_dir: str, seed: int,
                        count: int, mode: str, pool: list[int], memory: str | None, slice_name: str | None,
                        radius: int, center: str, keep_world: bool) -> dict[str, Any]:
    """One fresh-world pregenerate of a fixed square with the server pinned to `count` CPUs.

    `host` is LocalHost or Remote; `workdir`, `server_bin` and `vanilla_data_dir` are paths on that host.
    """
    split = common.split_cpus(pool, count, mode, host.sibling_of)
    out.mkdir(parents=True)
    world, config, time_file = f"{workdir}/world", f"{workdir}/server.toml", f"{workdir}/time.txt"
    host.mkdir(workdir)
    host.put_text(config, f'''[server]
name = "bench-worldgen"
[data]
world_dir = "{world}"
seed = {seed}
vanilla_data_dir = "{vanilla_data_dir}"
''')
    timed = ["/usr/bin/time", "-v", "-o", time_file] if host.has_time() else []
    command = [*(common.scope_prefix(memory, slice_name, host.is_root()) + ["--"] if memory else []),
               "taskset", "-c", split["server"], *timed,
               server_bin, "--config", config, "world", "pregenerate", "--radius", str(radius), f"--center={center}"]
    (out / "command.json").write_text(json.dumps(command))
    started = time.monotonic()
    completed = host.run(command, check=False)
    wall = time.monotonic() - started
    (out / "stdout.log").write_text(completed.stdout)
    (out / "stderr.log").write_text(completed.stderr)
    record: dict[str, Any] = {"engine": "solaris", "cores": count, "cpu_mode": mode, "cpus": split["server"],
                              "physical_cores_touched": split["physical_cores_touched"], "memory_max": memory,
                              "seed": seed, "radius_blocks": radius, "center_block": center, "wall_seconds": wall,
                              **parse_time_text(host.read_text(time_file) if timed else None)}
    if completed.returncode:
        record.update(status="failed", error=f"exit {completed.returncode}: {completed.stderr.strip()[-400:]}")
    else:
        record.update(status="ok", **parse_pregenerate(completed.stdout))
        if record["generated"] != record["chunks"]:
            record.update(status="failed", error=f"expected {record['chunks']} generated chunks, got {record['generated']}")
    if not keep_world:
        host.rmtree(world)
    return record


# --- vanilla head-to-head ---------------------------------------------------

def jar_data_pack_format(jar: Path) -> list[int]:
    with zipfile.ZipFile(jar) as archive:
        pack = json.loads(archive.read("version.json"))["pack_version"]
    return [pack["data_major"], pack["data_minor"]]


def forceload_commands(chunk_min: list[int], chunk_max: list[int]) -> list[str]:
    """`forceload add` rectangles of <= 16x16 chunks (vanilla's 256-chunk command limit)."""
    side = 16
    commands = []
    for z in range(chunk_min[1], chunk_max[1] + 1, side):
        for x in range(chunk_min[0], chunk_max[0] + 1, side):
            x2, z2 = min(x + side - 1, chunk_max[0]), min(z + side - 1, chunk_max[1])
            commands.append(f"forceload add {x * 16} {z * 16} {x2 * 16 + 15} {z2 * 16 + 15}")
    return commands


def write_datapack(world: Path, jar: Path, chunk_min: list[int], chunk_max: list[int]) -> int:
    """A tiny datapack: `bench:check` counts loaded chunks every 10 ticks and says BENCH_READY once all are."""
    base = world / "datapacks" / "bench"
    major, minor = jar_data_pack_format(jar)
    (base / "data/bench/function").mkdir(parents=True)
    (base / "pack.mcmeta").write_text(json.dumps({"pack": {
        "description": "bench completion observer", "min_format": [major, minor], "max_format": [major, minor]}}))
    positions = [(x * 16, z * 16) for z in range(chunk_min[1], chunk_max[1] + 1)
                 for x in range(chunk_min[0], chunk_max[0] + 1)]
    total = len(positions)
    lines = ["scoreboard players set #n bench 0"]
    lines += [f"execute if loaded {x} 0 {z} run scoreboard players add #n bench 1" for x, z in positions]
    lines += [f"execute if score #n bench matches {total} run say BENCH_READY",
              f"execute unless score #n bench matches {total} run schedule function bench:check 10t replace"]
    (base / "data/bench/function/check.mcfunction").write_text("\n".join(lines) + "\n")
    return total


class Console:
    """A server process whose stdout lines are pushed into a queue; waits block on the queue."""

    def __init__(self, argv: list[str], cwd: Path, log: Path):
        self.proc = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.STDOUT, text=True, bufsize=1,
                                     env={**os.environ, "NO_COLOR": "1"})
        self.lines: queue.Queue[tuple[float, str | None]] = queue.Queue()
        self.log = log
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self) -> None:
        assert self.proc.stdout
        with self.log.open("w") as handle:
            for line in self.proc.stdout:
                handle.write(line)
                handle.flush()
                self.lines.put((time.monotonic(), line))
        self.lines.put((time.monotonic(), None))

    def expect(self, text: str, timeout: float) -> float:
        deadline = time.monotonic() + timeout
        while True:
            try:
                stamp, line = self.lines.get(timeout=max(0.01, deadline - time.monotonic()))
            except queue.Empty:
                raise common.BenchError(f"timed out after {timeout:.0f}s waiting for {text!r}; see {self.log}") from None
            if line is None:
                raise common.BenchError(f"vanilla server exited while waiting for {text!r}; see {self.log}")
            if text in line:
                return stamp

    def send(self, command: str) -> None:
        assert self.proc.stdin
        self.proc.stdin.write(command + "\n")
        self.proc.stdin.flush()

    def stop(self) -> None:
        if self.proc.poll() is None:
            try:
                self.send("stop")
                self.proc.wait(timeout=180)
            except (BrokenPipeError, subprocess.TimeoutExpired):
                self.proc.kill()
                self.proc.wait()
        self.reader.join()


def vanilla_generate(*, out: Path, jar: Path, java: str, seed: int, cores: int, mode: str, pool: list[int],
                     memory: str | None, slice_name: str | None, chunk_min: list[int], chunk_max: list[int],
                     timeout: float, keep_world: bool) -> dict[str, Any]:
    """Vanilla forceloads the same chunk rectangle; wall time runs from the first command to `save-all flush`."""
    chunks = (chunk_max[0] - chunk_min[0] + 1) * (chunk_max[1] - chunk_min[1] + 1)
    if chunks > MAX_VANILLA_CHUNKS:
        raise common.BenchError(f"vanilla head-to-head supports at most {MAX_VANILLA_CHUNKS} chunks, got {chunks}")
    split = common.split_cpus(pool, cores, mode)
    out.mkdir(parents=True)
    (out / "eula.txt").write_text("eula=true\n")
    props = {"online-mode": "false", "server-ip": "127.0.0.1", "server-port": str(free_port()), "level-name": "world",
             "level-seed": str(seed), "level-type": "minecraft:normal", "generate-structures": "true",
             "view-distance": "2", "simulation-distance": "2", "pause-when-empty-seconds": "-1",
             "max-tick-time": "-1", "max-players": "1", "sync-chunk-writes": "true",
             "enable-rcon": "false", "enable-query": "false"}
    (out / "server.properties").write_text("".join(f"{k}={v}\n" for k, v in props.items()))
    total = write_datapack(out / "world", jar, chunk_min, chunk_max)
    heap = max(512, int(common.parse_size(memory) / 2**20 * 0.6)) if memory else 4096
    time_file = out / "time.txt"
    argv = [*(common.scope_prefix(memory, slice_name) + ["--"] if memory else []),
            "taskset", "-c", split["server"], *time_wrapper(time_file),
            java, f"-Xms{min(heap, 1024)}M", f"-Xmx{heap}M", f"-XX:ActiveProcessorCount={cores}",
            "-jar", str(jar.resolve()), "nogui"]
    (out / "command.json").write_text(json.dumps(argv))
    record: dict[str, Any] = {"engine": "vanilla", "cores": cores, "cpus": split["server"], "memory_max": memory,
                              "seed": seed, "chunk_min": chunk_min, "chunk_max": chunk_max, "chunks": total,
                              "java_heap_mib": heap}
    server = Console(argv, out, out / "server.log")
    try:
        launched = time.monotonic()
        server.expect("Done (", timeout)
        record["startup_seconds"] = time.monotonic() - launched
        server.send("scoreboard objectives add bench dummy")
        start = time.monotonic()
        for command in forceload_commands(chunk_min, chunk_max):
            server.send(command)
        server.send("function bench:check")
        loaded = server.expect("BENCH_READY", timeout)
        server.send("save-all flush")
        saved = server.expect("Saved the game", timeout)
    except common.BenchError as error:
        record.update(status="failed", error=str(error))
        saved = None
    finally:
        server.stop()
    record.update(parse_time_file(time_file))
    if saved is not None:
        record.update(status="ok", load_seconds=loaded - start, save_seconds=saved - loaded,
                      elapsed_seconds=saved - start, chunks_per_s=total / (saved - start))
    if not keep_world:
        shutil.rmtree(out / "world", ignore_errors=True)
    return record


def median_by_engine(runs: list[dict[str, Any]]) -> dict[str, Any] | None:
    ok = [r for r in runs if r["status"] == "ok"]
    if not ok:
        return None
    middle = sorted(ok, key=lambda r: r["chunks_per_s"])[(len(ok) - 1) // 2]
    return {**middle, "repeats_ok": len(ok), "repeats": len(runs),
            "chunks_per_s_all": [r["chunks_per_s"] for r in runs if r["status"] == "ok"],
            "chunks_per_s_mean": statistics.fmean(r["chunks_per_s"] for r in ok)}
