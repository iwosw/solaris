"""`python3 -m tools.bench {weak-hw|scaling|worldgen|vps}`: one command per published number.

A thin matrix runner over tools.coreperf.profiles (server/bot/fixture lifecycle, TPS/MSPT/RSS
definitions: docs/performance/PROFILES.md) and `solaris-v2 world pregenerate`. It builds nothing:
it measures supplied release binaries with fixed seeds and writes `result.json` + `result.md`
(hardware and exact cpusets included). Exit code is non-zero if any run failed.
See docs/performance/BENCHMARKS.md.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import shlex
import shutil
import signal
import sys
import tarfile
from typing import Any

from tools.bench import common, profile_runs, worldgen_runs
from tools.bench.common import BenchError, fmt, table
from tools.bench.remote import LocalHost, Remote

SMOKE_NOTE = "SMOKE RUN: short windows prove the pipeline only. Not publishable numbers."
DEFAULT_BUILD = ["cargo", "build", "--release", "--bin", "solaris-v2", "--bin", "player-scale-bots"]
ALL_PROFILES = sorted(profile_runs.PROFILE_CLIENTS)


def csv_ints(text: str) -> list[int]:
    return [int(part) for part in text.split(",") if part]


def csv_modes(text: str) -> list[str]:
    modes = [part for part in text.split(",") if part]
    if not modes or any(m not in ("logical", "physical") for m in modes):
        raise argparse.ArgumentTypeError("cpu modes are logical and/or physical, e.g. logical,physical")
    return modes


def add_common(parser: argparse.ArgumentParser, *, bots: bool) -> None:
    parser.add_argument("--server-bin", type=Path, help="release solaris-v2 (default: $CARGO_TARGET_DIR/release, ./target/release)")
    if bots:
        parser.add_argument("--bots-bin", type=Path, help="release player-scale-bots (same default lookup)")
    parser.add_argument("--out", type=Path, help="new result directory (default .analysis/bench/<UTC>-<command>-<pid>)")
    parser.add_argument("--vanilla-data-dir", type=Path,
                        help="imported content cache (default $SOLARIS_VANILLA_DATA_DIR, ./data/vanilla, ~/.local/share/solaris/content/26.1.2)")
    parser.add_argument("--seed", type=int, default=42, help="world seed for worldgen runs (profiles always use 42)")
    parser.add_argument("--cpu-pool", help="local CPU ids the run may use, e.g. 0-11 (default: this process's affinity)")
    parser.add_argument("--slice", dest="slice_name", help="systemd slice for the limited scopes (shared hosts: solaris-heavy.slice)")
    parser.add_argument("--commit", help="source identity recorded instead of `git rev-parse HEAD`")
    parser.add_argument("--smoke", action="store_true",
                        help="short pipeline check: 5s warmup + 25s window, tiny worldgen square, cheap defaults; never publishable")
    parser.add_argument("--warmup-seconds", type=int)
    parser.add_argument("--measure-seconds", type=int)
    parser.add_argument("--readiness-seconds", type=int, help="bot login / server stop deadline (default 1800; 300 under --smoke)")
    parser.add_argument("--fixture-cache", type=Path, help="pristine fixture cache (default .analysis/profile-fixtures)")
    parser.add_argument("--prepare-memory", default="8G", help="MemoryMax while building a profile fixture (default 8G)")


def parser() -> argparse.ArgumentParser:
    top = argparse.ArgumentParser(prog="python3 -m tools.bench", description=__doc__.splitlines()[0])
    sub = top.add_subparsers(dest="command", required=True)

    weak = sub.add_parser("weak-hw", help="server pinned to N logical CPUs + memory cap (default 2 CPUs / 4G)")
    add_common(weak, bots=True)
    weak.add_argument("--cores", type=int, default=2, help="logical CPUs (or physical cores with --cpu-mode physical)")
    weak.add_argument("--cpu-mode", default="logical", choices=("logical", "physical"))
    weak.add_argument("--memory", default="4G", help="MemoryMax of the server scope; `none` disables")
    weak.add_argument("--profiles", help=f"comma list of {','.join(ALL_PROFILES)} (default idle,explore-20,native-100)")
    weak.add_argument("--worldgen-radius", type=int, help="block radius of the pregenerate run under the same limits; 0 skips (default 256)")

    vps = sub.add_parser("vps", help="weak-hw against a remote host: server there, bots here")
    add_common(vps, bots=True)
    vps.add_argument("--host", required=True, help="ssh destination, e.g. root@203.0.113.7")
    vps.add_argument("--ssh-options", default="", help='extra ssh options as one string, e.g. "-p 2222 -i ~/.ssh/key"')
    vps.add_argument("--remote-data-dir", required=True, help="imported content cache on the host (dir with version.json)")
    vps.add_argument("--remote-dir", default="/var/tmp", help="parent of the per-run solaris-bench-* directory")
    vps.add_argument("--game-port", type=int, default=25590)
    vps.add_argument("--dash-port", type=int, default=25591)
    vps.add_argument("--tunnel-game", action="store_true", help="carry bot traffic through ssh when the game port is not reachable (costs sshd CPU on the host)")
    vps.add_argument("--bot-host", help="address bots dial (default: host part of --host)")
    vps.add_argument("--bot-cpus", help="local CPUs for the bots (default: the whole local pool)")
    vps.add_argument("--cores", type=int, help="remote logical CPUs for the server (default: all of them)")
    vps.add_argument("--cpu-mode", default="logical", choices=("logical", "physical"))
    vps.add_argument("--memory", default="none", help="MemoryMax of the server unit (default none: the VPS itself is the limit)")
    vps.add_argument("--profiles", help="default idle,explore-20")
    vps.add_argument("--worldgen-radius", type=int, help="default 256; 0 skips")
    vps.add_argument("--server-tarball", type=Path, help="release solaris-<target>.tar.gz; its `solaris` is uploaded instead of --server-bin")

    scaling = sub.add_parser("scaling", help="one workload at 1,2,4,6,8,12 logical CPUs, plus an optional physical-core series")
    add_common(scaling, bots=True)
    scaling.add_argument("--cores", type=csv_ints, default=[1, 2, 4, 6, 8, 12], help="comma list (default 1,2,4,6,8,12)")
    scaling.add_argument("--cpu-mode", type=csv_modes, default=["logical"],
                         help="logical, physical or logical,physical; physical = one CPU per physical core, counts above the core count are skipped")
    scaling.add_argument("--profile", help="default native-100 (explore-20 under --smoke)")
    scaling.add_argument("--memory", default="none", help="MemoryMax of the server scope (default none)")

    gen = sub.add_parser("worldgen", help="chunks/s of a fixed pregenerate square, optional vanilla head-to-head")
    add_common(gen, bots=False)
    gen.add_argument("--cores", type=csv_ints, default=[2], help="comma list of CPU counts (default 2)")
    gen.add_argument("--cpu-mode", type=csv_modes, default=["logical"], help="logical, physical or logical,physical")
    gen.add_argument("--memory", default="none", help="MemoryMax of the server scope (default none)")
    gen.add_argument("--radius", type=int, help="block radius of the square, 256 = 33x33 chunks (default 256; 64 under --smoke)")
    gen.add_argument("--center", default="0,0", help="block center x,z (default 0,0)")
    gen.add_argument("--repeat", type=int, default=1, help="fresh-world repeats per CPU count; the median is reported")
    gen.add_argument("--vanilla-jar", type=Path, help="Mojang bundler server.jar for the head-to-head (default $SOLARIS_VANILLA_JAR, ./.analysis/server.jar); skipped when absent")
    gen.add_argument("--java", default="java")
    gen.add_argument("--vanilla-timeout", type=float, default=3600)
    gen.add_argument("--keep-worlds", action="store_true")
    return top


# --- shared setup -----------------------------------------------------------

def find_binary(explicit: Path | None, name: str) -> Path:
    candidates = [explicit] if explicit else []
    if not explicit:
        if target := os.environ.get("CARGO_TARGET_DIR"):
            candidates.append(Path(target) / "release" / name)
        candidates.append(common.ROOT / "target/release" / name)
    for candidate in candidates:
        if candidate.is_file():
            return candidate.resolve()
    raise BenchError(f"{name} not found (tried {[str(c) for c in candidates]}); build it with "
                     f"`{' '.join(DEFAULT_BUILD)}` and pass --{'server' if name == 'solaris-v2' else 'bots'}-bin")


def extract_server(tarball: Path, directory: Path) -> Path:
    with tarfile.open(tarball) as archive:
        member = archive.getmember("solaris")
        if not member.isfile():
            raise BenchError("`solaris` in the tarball is not a regular file")
        target = directory / "solaris-v2"
        with archive.extractfile(member) as source, target.open("wb") as handle:
            shutil.copyfileobj(source, handle)
    target.chmod(0o755)
    return target


def find_data_dir(explicit: Path | None) -> Path:
    candidates = [explicit] if explicit else [
        Path(os.environ["SOLARIS_VANILLA_DATA_DIR"]) if "SOLARIS_VANILLA_DATA_DIR" in os.environ else None,
        common.ROOT / "data/vanilla", Path.home() / ".local/share/solaris/content/26.1.2"]
    for candidate in candidates:
        if candidate and (candidate / "version.json").is_file():
            return candidate.resolve()
    raise BenchError("imported vanilla content not found; run `solaris-v2 content import --version 26.1.2 --download` "
                     "and pass --vanilla-data-dir <content cache>/26.1.2 (the dir holding version.json)")


def resolve_durations(args: argparse.Namespace) -> None:
    warmup, measure = (5, 25) if args.smoke else (30, 120)
    args.warmup_seconds = args.warmup_seconds or warmup
    args.measure_seconds = args.measure_seconds or measure
    # Bounds login/fixture waits and the final save; a smoke must fail fast instead of waiting the profile default.
    args.readiness_seconds = args.readiness_seconds or (300 if args.smoke else 1800)
    # A canonical window is the documented 30s + 120s or longer; anything shorter is never publishable.
    args.publishable = not args.smoke and args.warmup_seconds >= 30 and args.measure_seconds >= 120


def memory_option(text: str | None) -> str | None:
    return None if text in (None, "", "none") else text


def setup(args: argparse.Namespace, *, bots: bool, server_bin: Path | None = None) -> tuple[Path, list[int], dict[str, Any], dict[str, Any], Path]:
    """Output dir, local CPU pool, local hardware, source record, content dir; preflight first."""
    problems = common.preflight(need_memory_scope=bool(memory_option(getattr(args, "memory", None))) or bots)
    if problems:
        raise BenchError("preflight failed:\n  - " + "\n  - ".join(problems))
    pool = common.available_pool(args.cpu_pool)
    out = (args.out or common.ROOT / ".analysis/bench" / f"{common.utc_stamp()}-{args.command}-{os.getpid()}").resolve()
    out.mkdir(parents=True)
    server = server_bin or find_binary(args.server_bin, "solaris-v2")
    bots_bin = find_binary(args.bots_bin, "player-scale-bots") if bots else None
    source = common.source_record(server, bots_bin, args.commit)
    return out, pool, common.hardware(pool), source, find_data_dir(args.vanilla_data_dir)


def make_context(args: argparse.Namespace, out: Path, pool: list[int], source: dict[str, Any], data_dir: Path,
                 hw: dict[str, Any]) -> profile_runs.Context:
    receipt = common.write_build_receipt(out / "build-receipt.json", source, DEFAULT_BUILD)
    return profile_runs.Context(
        out=out, server_bin=Path(source["server_bin"]), bots_bin=Path(source["bots_bin"]), build_receipt=receipt,
        vanilla_data_dir=data_dir, fixture_cache=(args.fixture_cache or common.ROOT / ".analysis/profile-fixtures").resolve(),
        pool=pool, warmup_seconds=args.warmup_seconds, measure_seconds=args.measure_seconds,
        readiness_seconds=args.readiness_seconds,
        prepare_memory=args.prepare_memory, slice_name=args.slice_name, mem_total_mib=int(hw["mem_total_gib"] * 1024))


def header(args: argparse.Namespace, hw: dict[str, Any], source: dict[str, Any], title: str, hw_title: str = "Host") -> list[str]:
    lines = [f"# {title}", ""]
    if args.smoke or not args.publishable:
        lines += [f"> **{SMOKE_NOTE}**", ""]
    lines += [f"## {hw_title}", "", common.hardware_markdown(hw), "",
              "Topology (`lscpu -e`: logical CPU, physical core, socket):", "", "```", hw.get("topology") or "unavailable", "```", "",
              "## Source", "",
              f"- commit `{source['commit']}`" + (" (tracked changes present)" if source["dirty_tracked_files"] else ""),
              f"- server `{source['server_sha256'][:16]}…`" + (f", bots `{source['bots_sha256'][:16]}…`" if source.get("bots_sha256") else ""),
              ""]
    return lines


def document(args: argparse.Namespace, hw: dict[str, Any], source: dict[str, Any], started: str, **extra: Any) -> dict[str, Any]:
    return {"schema": 1, "command": args.command, "argv": sys.argv[1:], "started_utc": started,
            "smoke": bool(args.smoke), "publication_evidence": bool(args.publishable),
            "hardware": hw, "source": source, **extra}


def guard(call, *call_args, **call_kwargs) -> dict[str, Any]:
    """A BenchError inside one run fails that run, not the whole command."""
    try:
        return call(*call_args, **call_kwargs)
    except BenchError as error:
        return {"status": "failed", "error": str(error)}


def cpu_label(row: dict[str, Any]) -> str:
    return f"{row['cores']} {row.get('cpu_mode', 'logical')}"


# --- tables -----------------------------------------------------------------

PROFILE_HEADERS = ["profile", "CPUs (cpuset; physical cores)", "TPS", "MSPT p50 / p99 / max (ms)", "RSS steady / peak (MiB)",
                   "resident chunks/s (proxy)", "error"]


def profile_row(run: dict[str, Any]) -> list[Any]:
    cpus = f"{cpu_label(run)} (`{run.get('cpus_server', '?')}`; {run.get('physical_cores_touched', '?')})"
    if run["status"] != "ok":
        return [run["profile"], cpus, "FAILED", "", "", "", run.get("error", "").replace("\n", " ")[:140]]
    mspt, rss = run["mspt_ms"], run["server_rss_mib"]
    return [run["profile"], cpus, fmt(run["tps"], 2), f"{mspt['p50']:.1f} / {mspt['p99']:.1f} / {mspt['max']:.1f}",
            f"{rss['steady_median']:.0f} / {rss['peak']:.0f}", fmt(run["resident_chunks_per_s_proxy"], 2), ""]


def worldgen_line(w: dict[str, Any]) -> str:
    if w["status"] != "ok":
        return f"- worldgen pregenerate FAILED: {w.get('error')}"
    cpu = f", {w['cpu_seconds'] * 1000 / w['generated']:.1f} CPU-ms/chunk" if w.get("cpu_seconds") else ""
    rss = f", peak RSS {w['peak_rss_mib']:.0f} MiB" if w.get("peak_rss_mib") else ""
    return (f"- **{w['chunks_per_s']:.1f} chunks/s** (generation only; {w['chunks_per_s_incl_setup']:.1f} incl. world open), "
            f"{w['generated']} chunks, square {w['chunk_min']}..{w['chunk_max']}, seed {w['seed']}{cpu}{rss}")


def limits_lines(split: dict[str, Any], memory: str | None, remote: str | None = None) -> list[str]:
    where = f" on `{remote}`" if remote else ""
    return [f"- server{where}: {split['cores']} {split['mode']} CPU(s), cpuset `{split['server']}` "
            f"({split['physical_cores_touched']} physical core(s) touched), MemoryMax `{memory or 'none'}`",
            f"- protocol bots: CPUs `{split['bots']}`" + (" — **shared with the server** (no spare CPUs)" if split["bots_share_server_cpus"] else "")]


# --- weak-hw / vps ----------------------------------------------------------

def remote_target(args: argparse.Namespace, bot_pool: list[int], remote: Remote, rhw: dict[str, Any]) -> profile_runs.RemoteTarget:
    return profile_runs.RemoteTarget(
        host=args.host, ssh_options=shlex.split(args.ssh_options), pool=remote.cpu_pool(),
        mem_total_mib=int(rhw["mem_total_gib"] * 1024), bot_cpus=args.bot_cpus or common.cpu_list(bot_pool),
        data_dir=args.remote_data_dir, game_port=args.game_port, dash_port=args.dash_port, tunnel_game=args.tunnel_game,
        bot_host=args.bot_host, parent_dir=args.remote_dir.rstrip("/"))


def remote_preflight(remote: Remote, args: argparse.Namespace, server_bin: Path) -> list[str]:
    problems = []
    for tool in ("rsync", "taskset", "systemd-run", "systemctl", "tail", "timeout", "bash"):
        if remote.run(f"command -v {tool}", check=False).returncode:
            problems.append(f"remote host lacks `{tool}`")
    if remote.run(["test", "-f", f"{args.remote_data_dir}/version.json"], check=False).returncode:
        problems.append(f"remote content cache {args.remote_data_dir} has no version.json")
    if remote.run(["test", "-w", args.remote_dir], check=False).returncode:
        problems.append(f"remote directory {args.remote_dir} is not writable")
    for port in (args.game_port, args.dash_port):
        if f":{port} " in remote.run("ss -ltn", check=False).stdout:
            problems.append(f"remote TCP port {port} is already in use (pick another with --game-port/--dash-port)")
    need = common.glibc_required(server_bin)
    have = remote.run("getconf GNU_LIBC_VERSION").stdout.split()[-1]
    if need and tuple(map(int, have.split("."))) < need:
        problems.append(f"binary needs glibc {'.'.join(map(str, need))}, remote has {have}: build in an older environment")
    return problems


def cmd_weak_hw(args: argparse.Namespace) -> int:
    resolve_durations(args)
    vps = args.command == "vps"
    server_override = None
    if vps and args.server_tarball:
        args.out = args.out.resolve() if args.out else None
        scratch = (args.out or common.ROOT / ".analysis/bench").resolve()
        scratch.mkdir(parents=True, exist_ok=True)
        server_override = extract_server(args.server_tarball, scratch)
    out, pool, local_hw, source, data_dir = setup(args, bots=True, server_bin=server_override)
    memory = memory_option(args.memory)
    profiles = (args.profiles or ("idle,explore-20" if vps or args.smoke else "idle,explore-20,native-100")).split(",")
    unknown = [p for p in profiles if p not in profile_runs.PROFILE_CLIENTS]
    if unknown:
        raise BenchError(f"unknown profile(s) {unknown}; choose from {ALL_PROFILES}")
    radius = args.worldgen_radius if args.worldgen_radius is not None else (64 if args.smoke else 256)
    ctx = make_context(args, out, pool, source, data_dir, local_hw)
    started = common.utc_stamp()
    remote = None
    hw = local_hw
    if vps:
        remote = Remote(args.host, shlex.split(args.ssh_options), [])
        remote.open()
        hw = remote.hardware()
        problems = remote_preflight(remote, args, Path(source["server_bin"]))
        if problems:
            remote.close()
            raise BenchError("remote preflight failed:\n  - " + "\n  - ".join(problems))
        ctx.remote = remote_target(args, pool, remote, hw)
        server_pool, sibling_of = ctx.remote.pool, lambda cpu: [cpu]
        hw["cpu_pool_used_by_bench"] = common.cpu_list(server_pool)
    else:
        server_pool, sibling_of = pool, common.siblings
    try:
        cores = args.cores or len(server_pool)
        split = common.split_cpus(server_pool, cores, args.cpu_mode, sibling_of)
        if vps:
            split.update(bots=ctx.remote.bot_cpus, bots_share_server_cpus=False)  # bots run on this machine
        runs = [guard(profile_runs.run_profile, ctx, p, cores, memory, f"{p}-{cores}{args.cpu_mode[0]}", args.cpu_mode) for p in profiles]
        for run, profile in zip(runs, profiles):
            run.setdefault("profile", profile)
            run.setdefault("cores", cores)
            run.setdefault("cpu_mode", args.cpu_mode)
        worldgen = None
        if radius:
            worldgen = run_worldgen_once(args, out, ctx, remote, server_pool, cores, memory, radius, vps)
    finally:
        if remote:
            remote.close()
    ok = all(r["status"] == "ok" for r in runs) and (worldgen is None or worldgen["status"] == "ok")
    doc = document(args, hw, source, started, ok=ok, limits={**split, "memory_max": memory, "remote_host": args.host if vps else None},
                   windows={"warmup_seconds": args.warmup_seconds, "measure_seconds": args.measure_seconds},
                   workload={"profiles": profiles, "profile_seed": 42, "worldgen_seed": args.seed, "worldgen_radius_blocks": radius},
                   runs=runs, worldgen=worldgen, **({"bots_host_hardware": local_hw} if vps else {}))
    md = header(args, hw, source, f"{args.command}: {cores} {args.cpu_mode} CPU(s), {memory or 'no'} memory cap",
                "Server host (remote)" if vps else "Host")
    md += ["## Limits", "", *limits_lines(split, memory, args.host if vps else None), ""]
    if vps:
        md += [f"Bots run on this machine (CPUs `{ctx.remote.bot_cpus}`) and dial "
               f"`{'127.0.0.1 via ssh tunnel' if args.tunnel_game else (args.bot_host or args.host.rpartition('@')[2])}:{args.game_port}`; "
               f"server clock offset and round trip are in each run's `remote.json`.", ""]
    md += [f"## Workloads (seed 42, warmup {args.warmup_seconds}s + window {args.measure_seconds}s)", "",
           table(PROFILE_HEADERS, [profile_row(r) for r in runs]), "",
           "Resident chunks/s is a proxy (resident-chunk delta, reloads included), not generation throughput.", ""]
    if worldgen is not None:
        md += ["## Chunk generation under the same limits", "", worldgen_line(worldgen), ""]
    common.finish(out, doc, "\n".join(md))
    return 0 if ok else 1


def run_worldgen_once(args, out: Path, ctx, remote: Remote | None, pool: list[int], cores: int, memory: str | None,
                      radius: int, vps: bool) -> dict[str, Any]:
    if not vps:
        return guard(worldgen_runs.solaris_pregenerate, host=LocalHost(pool), out=out / "worldgen", workdir=str(out / "worldgen-work"),
                     server_bin=str(ctx.server_bin), vanilla_data_dir=str(ctx.vanilla_data_dir), seed=args.seed, count=cores,
                     mode=args.cpu_mode, pool=pool, memory=memory, slice_name=args.slice_name, radius=radius, center="0,0",
                     keep_world=False)
    rdir = f"{args.remote_dir.rstrip('/')}/solaris-bench-wg-{os.getpid()}"
    try:
        remote.mkdir(rdir)
        remote.rsync(ctx.server_bin, f"{rdir}/solaris-v2")
        return guard(worldgen_runs.solaris_pregenerate, host=remote, out=out / "worldgen", workdir=f"{rdir}/work",
                     server_bin=f"{rdir}/solaris-v2", vanilla_data_dir=args.remote_data_dir, seed=args.seed, count=cores,
                     mode=args.cpu_mode, pool=pool, memory=memory, slice_name=None, radius=radius, center="0,0", keep_world=False)
    finally:
        remote.rmtree(rdir)


# --- scaling ----------------------------------------------------------------

def cmd_scaling(args: argparse.Namespace) -> int:
    resolve_durations(args)
    out, pool, hw, source, data_dir = setup(args, bots=True)
    profile = args.profile or ("explore-20" if args.smoke else "native-100")
    if args.smoke and args.cores == [1, 2, 4, 6, 8, 12]:
        args.cores = [1, 2]
    memory = memory_option(args.memory)
    ctx = make_context(args, out, pool, source, data_dir, hw)
    started = common.utc_stamp()
    series: dict[str, list[dict[str, Any]]] = {}
    for mode in args.cpu_mode:
        limit = common.physical_cores(pool) if mode == "physical" else len(pool)
        runs = []
        for count in args.cores:
            if count > limit:
                runs.append({"profile": profile, "cores": count, "cpu_mode": mode, "status": "skipped",
                             "error": f"only {limit} {mode} CPUs available in the pool"})
                continue
            run = guard(profile_runs.run_profile, ctx, profile, count, memory, f"{profile}-{count}{mode[0]}", mode)
            run.setdefault("profile", profile)
            run.setdefault("cores", count)
            run.setdefault("cpu_mode", mode)
            runs.append(run)
        base = next((r for r in runs if r["status"] == "ok"), None)
        for run in runs:
            if run["status"] == "ok" and base:
                run["speedup_vs_base"] = base["mspt_ms"]["p50"] / run["mspt_ms"]["p50"]
                run["parallel_efficiency"] = run["speedup_vs_base"] / (run["cores"] / base["cores"])
        series[mode] = runs
    ok = all(r["status"] in ("ok", "skipped") for runs in series.values() for r in runs)
    doc = document(args, hw, source, started, ok=ok, windows={"warmup_seconds": args.warmup_seconds, "measure_seconds": args.measure_seconds},
                   workload={"profile": profile, "profile_seed": 42, "memory_max": memory, "counts": args.cores, "cpu_modes": args.cpu_mode,
                             "speedup_definition": "MSPT p50 at the lowest count divided by MSPT p50 at N; efficiency = speedup / (N / lowest N)"},
                   series=series)
    md = header(args, hw, source, f"scaling: {profile} at {','.join(map(str, args.cores))} CPUs")
    md += [f"Server memory cap: `{memory or 'none'}`. Warmup {args.warmup_seconds}s + window {args.measure_seconds}s, seed 42.", ""]
    for mode, runs in series.items():
        rows = []
        for r in runs:
            cpus = f"`{r.get('cpus_server', '')}` ({r.get('physical_cores_touched', '?')} phys)"
            if r["status"] != "ok":
                rows.append([r["cores"], cpus if r.get("cpus_server") else "", r["status"].upper(), "", "", "", "", r.get("error", "").replace("\n", " ")[:100]])
                continue
            m = r["mspt_ms"]
            rows.append([r["cores"], cpus, fmt(r["tps"], 2), f"{m['p50']:.1f} / {m['p99']:.1f} / {m['max']:.1f}",
                         fmt(r["server_rss_mib"]["steady_median"], 0), f"{r['speedup_vs_base']:.2f}x",
                         f"{r['parallel_efficiency'] * 100:.0f}%", ""])
        md += [f"## {mode.capitalize()} series ({'first N logical CPUs, SMT siblings included' if mode == 'logical' else 'one CPU per physical core'})", "",
               table([f"{mode} CPUs", "cpuset (physical cores)", "TPS", "MSPT p50 / p99 / max (ms)", "RSS steady (MiB)", "speedup", "efficiency", "note"], rows), ""]
    md += ["Speedup = MSPT p50 at the lowest count / MSPT p50 at N (TPS saturates at 20 once a run keeps up, so it cannot show scaling). "
           "Efficiency = speedup / (N / lowest N). Logical counts are NOT physical cores: on SMT siblings 0-1, 2-3, ... "
           "`0-5` is 3 physical cores.", ""]
    if any(r.get("bots_share_server_cpus") for runs in series.values() for r in runs):
        md += ["Runs that use the whole CPU pool share CPUs with the protocol bots.", ""]
    common.finish(out, doc, "\n".join(md))
    return 0 if ok else 1


# --- worldgen ---------------------------------------------------------------

def find_vanilla_jar(explicit: Path | None) -> Path | None:
    if explicit and not explicit.is_file():
        raise BenchError(f"--vanilla-jar {explicit} does not exist")
    candidates = [explicit] if explicit else [Path(os.environ["SOLARIS_VANILLA_JAR"]) if "SOLARIS_VANILLA_JAR" in os.environ else None,
                                              common.ROOT / ".analysis/server.jar"]
    return next((c.resolve() for c in candidates if c and c.is_file()), None)


def cmd_worldgen(args: argparse.Namespace) -> int:
    resolve_durations(args)
    args.publishable = not args.smoke
    out, pool, hw, source, data_dir = setup(args, bots=False)
    radius = args.radius or (64 if args.smoke else 256)
    jar = find_vanilla_jar(args.vanilla_jar)
    if jar and shutil.which(args.java) is None and not Path(args.java).is_file():
        raise BenchError(f"vanilla jar found ({jar}) but java is missing: {args.java}")
    memory = memory_option(args.memory)
    started = common.utc_stamp()
    host = LocalHost(pool)
    rows: list[dict[str, Any]] = []
    for mode in args.cpu_mode:
        limit = common.physical_cores(pool) if mode == "physical" else len(pool)
        for count in args.cores:
            row: dict[str, Any] = {"cores": count, "cpu_mode": mode}
            rows.append(row)
            if count > limit:
                row.update(solaris=None, solaris_runs=[{"status": "skipped", "error": f"only {limit} {mode} CPUs available"}], skipped=True)
                continue
            tag = f"{count}{mode[0]}"
            runs = [guard(worldgen_runs.solaris_pregenerate, host=host, out=out / "runs" / f"solaris-{tag}-{i}", workdir=str(out / "work" / f"solaris-{tag}-{i}"),
                          server_bin=source["server_bin"], vanilla_data_dir=str(data_dir), seed=args.seed, count=count, mode=mode, pool=pool,
                          memory=memory, slice_name=args.slice_name, radius=radius, center=args.center, keep_world=args.keep_worlds)
                    for i in range(args.repeat)]
            for r in runs:
                r.setdefault("cores", count)
            row["solaris_runs"], row["solaris"] = runs, worldgen_runs.median_by_engine(runs)
            if jar and row["solaris"]:
                vanilla = [guard(worldgen_runs.vanilla_generate, out=out / "runs" / f"vanilla-{tag}-{i}", jar=jar, java=args.java, seed=args.seed,
                                 cores=count, mode=mode, pool=pool, memory=memory, slice_name=args.slice_name,
                                 chunk_min=row["solaris"]["chunk_min"], chunk_max=row["solaris"]["chunk_max"],
                                 timeout=args.vanilla_timeout, keep_world=args.keep_worlds) for i in range(args.repeat)]
                row["vanilla_runs"], row["vanilla"] = vanilla, worldgen_runs.median_by_engine(vanilla)
    for mode in args.cpu_mode:
        mode_rows = [r for r in rows if r["cpu_mode"] == mode]
        base = next((r["solaris"] for r in mode_rows if r["solaris"]), None)
        for row in mode_rows:
            if row["solaris"] and base:
                row["solaris"]["speedup_vs_base"] = row["solaris"]["chunks_per_s"] / base["chunks_per_s"]
    live = [r for r in rows if not r.get("skipped")]
    ok = all(r["solaris"] and (not jar or r.get("vanilla")) for r in live)
    doc = document(args, hw, source, started, ok=ok, workload={
        "seed": args.seed, "radius_blocks": radius, "center_block": args.center, "repeat": args.repeat, "memory_max": memory,
        "vanilla_jar": str(jar) if jar else None, "counts": args.cores, "cpu_modes": args.cpu_mode,
        "solaris_metric": "generated chunks / seconds from the first generation progress line to completion (world open excluded)",
        "vanilla_metric": "requested chunks / seconds from first forceload command to `save-all flush` done (JVM startup excluded)"},
        results=rows)
    md_rows = []
    for row in rows:
        s, v = row["solaris"], row.get("vanilla")
        if row.get("skipped"):
            md_rows.append([f"{row['cores']} {row['cpu_mode']}", "skipped", "", "", ""] + (["", "", ""] if jar else []))
            continue
        cpu = f"{s['cpu_seconds'] * 1000 / s['generated']:.1f}" if s and s.get("cpu_seconds") else "n/a"
        cpus = f"`{s['cpus']}` ({s['physical_cores_touched']} phys)" if s else ""
        md_rows.append([f"{row['cores']} {row['cpu_mode']}", cpus, fmt(s and s["chunks_per_s"]), f"{s['speedup_vs_base']:.2f}x" if s else "", cpu,
                        fmt(s and s.get("peak_rss_mib"), 0)] +
                       ([fmt(v and v["chunks_per_s"]), f"{s['chunks_per_s'] / v['chunks_per_s']:.1f}x" if s and v else "",
                         fmt(v and v.get("peak_rss_mib"), 0)] if jar else []))
    heads = ["CPUs", "cpuset (physical cores)", "Solaris chunks/s", "speedup", "CPU-ms/chunk", "peak RSS (MiB)"] + (
        ["vanilla chunks/s", "Solaris / vanilla", "vanilla peak RSS (MiB)"] if jar else [])
    md = header(args, hw, source, f"worldgen: {radius}-block radius around {args.center}")
    first = next((r["solaris"] for r in rows if r["solaris"]), None)
    md += [f"Seed {args.seed}, fresh world per run, {args.repeat} repeat(s) (median), memory cap `{memory or 'none'}`."
           + (f" Square: chunks {first['chunk_min']}..{first['chunk_max']} ({first['chunks']} chunks)." if first else ""), "",
           table(heads, md_rows), ""]
    for row in rows:
        for run in row["solaris_runs"] + row.get("vanilla_runs", []):
            if run["status"] == "failed":
                md.append(f"- FAILED {run.get('engine', 'solaris')} @ {row['cores']} {row['cpu_mode']}: {run.get('error')}")
    if not jar:
        md.append("Vanilla head-to-head skipped: no vanilla server jar (pass --vanilla-jar or set SOLARIS_VANILLA_JAR).")
    else:
        md.append("Vanilla forceloads the same chunk square and also generates a halo of neighbours it needs, "
                  "so its per-requested-chunk rate is conservative.")
    common.finish(out, doc, "\n".join(md))
    return 0 if ok else 1


def _interrupt(*_: Any) -> None:
    raise KeyboardInterrupt


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    args.publishable = False
    # Ctrl-C and SIGTERM (also when started as a background job, where SIGINT is ignored by default)
    # unwind through the `finally` blocks that stop remote units and delete remote directories.
    for number in (signal.SIGINT, signal.SIGTERM):
        signal.signal(number, _interrupt)
    try:
        if args.command in ("weak-hw", "vps"):
            return cmd_weak_hw(args)
        return {"scaling": cmd_scaling, "worldgen": cmd_worldgen}[args.command](args)
    except BenchError as error:
        print(f"bench: {error}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        print("bench: interrupted; child runs were told to stop and clean up", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
