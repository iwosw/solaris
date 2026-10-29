"""Run one coreperf profile under a core/memory limit and normalise its receipt."""
from __future__ import annotations

from dataclasses import dataclass, field
import json
import signal
from pathlib import Path
import subprocess
import sys
import time
from typing import Any

from tools.bench import common

MIB = 2**20
PROFILE_CLIENTS = {"idle": 0, "explore-20": 20, "native-100": 100, "villager-heavy": 100}


@dataclass
class Context:
    """Everything that is the same for every profile run of one bench command."""
    out: Path
    server_bin: Path
    bots_bin: Path
    build_receipt: Path
    vanilla_data_dir: Path
    fixture_cache: Path
    pool: list[int]
    warmup_seconds: int
    measure_seconds: int
    readiness_seconds: int
    prepare_memory: str
    slice_name: str | None
    mem_total_mib: int
    prepared: set[str] = field(default_factory=set)
    remote: "RemoteTarget | None" = None


@dataclass
class RemoteTarget:
    """Where the server runs for `vps`: the remote CPU pool/RAM and how profiles.py reaches it."""
    host: str
    ssh_options: list[str]
    pool: list[int]
    mem_total_mib: int
    bot_cpus: str
    data_dir: str
    game_port: int
    dash_port: int
    tunnel_game: bool
    bot_host: str | None
    parent_dir: str

    def profile_flags(self, memory: str | None) -> list[str]:
        flags = ["--remote-host", self.host, "--remote-dir", self.parent_dir, "--remote-data-dir", self.data_dir,
                 "--remote-game-port", str(self.game_port), "--remote-dash-port", str(self.dash_port)]
        for option in self.ssh_options:
            flags.append(f"--remote-ssh-option={option}")  # `=` form: options start with a dash
        if memory:
            flags += ["--remote-memory-max", memory]
        if self.tunnel_game:
            flags.append("--remote-tunnel-game")
        if self.bot_host:
            flags += ["--remote-bot-host", self.bot_host]
        return flags


def _bounded_run(command: list[str], log: Path) -> tuple[int, dict[str, Any] | None]:
    """Run `command` (cwd = repo root), keep its output in `log`, return exit + last JSON line."""
    with log.open("w") as handle:
        child = subprocess.Popen(command, cwd=common.ROOT, stdout=handle, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = child.wait()
        except KeyboardInterrupt:
            # profiles.py turns SIGTERM into a cancel whose `finally` stops bots and, for vps, the remote
            # unit and directory; subprocess.run would kill it halfway through that cleanup instead.
            child.send_signal(signal.SIGTERM)
            child.wait()
            raise
    last = None
    for line in reversed(log.read_text().splitlines()):
        if line.startswith("{"):
            try:
                last = json.loads(line)
            except ValueError:
                continue
            break
    return code, last


def profile_command(ctx: Context, profile: str, out: Path, *, server_cpus: str, bot_cpus: str,
                    wrapper: str, max_rss_mb: int, prepare_only: bool) -> list[str]:
    command = [sys.executable, "-m", "tools.coreperf.profiles", profile,
               "--server-bin", str(ctx.server_bin), "--bots-bin", str(ctx.bots_bin),
               "--build-receipt", str(ctx.build_receipt), "--out", str(out),
               "--fixture-cache", str(ctx.fixture_cache), "--vanilla-data-dir", str(ctx.vanilla_data_dir),
               "--server-cpus", server_cpus, "--bot-cpus", bot_cpus, "--max-rss-mb", str(max_rss_mb),
               "--warmup-seconds", str(ctx.warmup_seconds), "--measure-seconds", str(ctx.measure_seconds),
               "--readiness-seconds", str(ctx.readiness_seconds)]
    if wrapper:
        command += ["--server-wrapper", wrapper]
    if prepare_only:
        command.append("--prepare-only")
    return command


def ensure_fixture(ctx: Context, profile: str) -> None:
    """Populate the pristine fixture cache once, with the whole pool and no per-run limit.

    The cache key depends only on fixture inputs (seed, mobs, view distance, recipe), never on
    CPU pinning or memory caps, so every limited measurement afterwards copies the same world.
    The harness insists on a bounded cgroup while it builds a fixture, hence the scope.
    """
    if profile in ctx.prepared:
        return
    out = ctx.out / f"prepare-{profile}"
    pool = common.cpu_list(ctx.pool)
    command = [*common.scope_prefix(ctx.prepare_memory, ctx.slice_name), "--",
               *profile_command(ctx, profile, out, server_cpus=pool, bot_cpus=pool, wrapper="",
                                max_rss_mb=int(common.parse_size(ctx.prepare_memory) / MIB), prepare_only=True)]
    code, last = _bounded_run(command, ctx.out / f"prepare-{profile}.log")
    if code or not last or last.get("status") != "prepared":
        raise common.BenchError(
            f"fixture preparation for {profile} failed (exit {code}): {last and last.get('error')}; "
            f"see {ctx.out / f'prepare-{profile}.log'}")
    ctx.prepared.add(profile)


def run_profile(ctx: Context, profile: str, count: int, memory: str | None, label: str, mode: str = "logical") -> dict[str, Any]:
    """One limited measurement: server pinned to `count` CPUs inside a MemoryMax cap (local scope or remote unit)."""
    ensure_fixture(ctx, profile)
    target = ctx.remote
    if target:
        split = common.split_cpus(target.pool, count, mode, lambda cpu: [cpu])  # remote SMT topology is not probed
        split.update(bots=target.bot_cpus, bots_share_server_cpus=False)  # bots run on this machine, not the server's
        host_mib = target.mem_total_mib
    else:
        split = common.split_cpus(ctx.pool, count, mode)
        host_mib = ctx.mem_total_mib
    max_rss_mb = int(common.parse_size(memory) / MIB) if memory else int(host_mib * 0.75)
    out = ctx.out / "runs" / label
    out.parent.mkdir(exist_ok=True)
    command = profile_command(ctx, profile, out, server_cpus=split["server"], bot_cpus=split["bots"],
                              wrapper="" if target else common.shell_prefix(memory, ctx.slice_name),
                              max_rss_mb=max_rss_mb, prepare_only=False)
    if target:
        command += target.profile_flags(memory)
    started = time.monotonic()
    code, last = _bounded_run(command, ctx.out / "runs" / f"{label}.log")
    return normalise(profile, split, memory, out, code, last, time.monotonic() - started)


def normalise(profile: str, split: dict[str, Any], memory: str | None, out: Path, code: int,
              last: dict[str, Any] | None, wall: float) -> dict[str, Any]:
    row: dict[str, Any] = {
        "profile": profile, "protocol_clients": PROFILE_CLIENTS[profile], "cores": split["cores"],
        "cpus_server": split["server"], "cpus_bots": split["bots"], "cpu_mode": split["mode"],
        "physical_cores_touched": split["physical_cores_touched"],
        "bots_share_server_cpus": split["bots_share_server_cpus"], "memory_max": memory,
        "receipt": str(out / "result.json"), "wall_seconds": round(wall, 1)}
    receipt_path = out / "result.json"
    receipt = json.loads(receipt_path.read_text()) if receipt_path.is_file() else {}
    summary = (receipt.get("measurement") or {}).get("summary")
    ok = code == 0 and receipt.get("status") in ("measured", "smoke") and summary
    row["status"] = "ok" if ok else "failed"
    if not ok:
        row["error"] = receipt.get("error") or (last or {}).get("error") or f"exit {code}, no receipt"
        return row
    seconds = summary["duration_seconds"]
    rss = summary["rss"]
    # The dashboard reports `chunks: {"loaded_total": n}`; keep plain counts in the result.
    chunks = {k: v["loaded_total"] if isinstance(v, dict) else v for k, v in summary["resident_chunks_observed"].items()}
    row.update(
        measured_seconds=seconds, ticks=summary["ticks"], tps=summary["tps"],
        mspt_ms=summary["mspt"],
        server_rss_mib={"peak": rss["server"]["peak_lifetime_bytes"] / MIB,
                        "steady_median": rss["server"]["steady_median_bytes"] / MIB},
        bots_rss_mib={"peak": rss["bots"]["peak_lifetime_bytes"] / MIB} if "bots" in rss else None,
        entities={"first": summary["population_observed"]["first"], "last": summary["population_observed"]["last"]},
        resident_chunks=chunks,
        # Not generation throughput: the dashboard only exposes resident chunks (loads minus
        # unloads, reloads included). Real generation rate is measured by the worldgen run.
        resident_chunks_per_s_proxy=(chunks["last"] - chunks["first"]) / seconds)
    return row
