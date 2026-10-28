"""Shared plumbing for tools.bench: host record, CPU/memory limits, result files."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import subprocess
import sys
import time
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
SIZE_UNITS = {"K": 2**10, "M": 2**20, "G": 2**30, "T": 2**40}


class BenchError(RuntimeError):
    """A clear, actionable failure (missing prerequisite, bad option)."""


def utc_stamp() -> str:
    return time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())


def parse_size(text: str) -> int:
    """`4G`, `512M`, `1T` or plain bytes; the same suffixes systemd accepts."""
    match = re.fullmatch(r"(\d+)([KMGT]?)", text.strip().upper())
    if not match:
        raise BenchError(f"size must look like 4G, 512M or a byte count, got {text!r}")
    return int(match[1]) * SIZE_UNITS.get(match[2], 1)


def sha256(path: Path) -> str:
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def run_text(command: list[str]) -> str | None:
    try:
        return subprocess.check_output(command, text=True, stderr=subprocess.DEVNULL).strip()
    except (OSError, subprocess.CalledProcessError):
        return None


# --- CPU topology -----------------------------------------------------------

def parse_cpu_list(text: str) -> list[int]:
    """Kernel cpulist syntax (`0-3,8,10-11`) to a sorted id list."""
    ids: set[int] = set()
    for part in text.strip().split(","):
        if not part:
            continue
        low, _, high = part.partition("-")
        ids.update(range(int(low), int(high or low) + 1))
    return sorted(ids)


def cpu_list(ids: list[int]) -> str:
    return ",".join(map(str, ids))


def siblings(cpu: int) -> list[int]:
    path = Path(f"/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list")
    try:
        return parse_cpu_list(path.read_text())
    except OSError:
        return [cpu]


def cpu_order(pool: list[int], sibling_of=siblings) -> list[int]:
    """Physical order: one thread per physical core first, SMT siblings last."""
    first, rest = [], []
    for cpu in pool:
        group = [c for c in sibling_of(cpu) if c in pool]
        (first if cpu == min(group) else rest).append(cpu)
    return first + rest


def physical_cores(cpus: list[int], sibling_of=siblings) -> int:
    return len({frozenset(sibling_of(cpu)) for cpu in cpus})


def split_cpus(pool: list[int], count: int, mode: str = "logical", sibling_of=siblings) -> dict[str, Any]:
    """Server cpuset for `count` CPUs; protocol bots get the rest, or share them when none remain.

    `logical`: the first `count` logical CPUs of the pool in id order, SMT siblings included
    (a "2 vCPU" VPS shape: on 0,1 = SMT siblings that is ONE physical core).
    `physical`: one CPU per physical core (0,2,4,... where siblings are 0-1,2-3,...); more than the
    physical core count is an error. The exact cpuset and the number of physical cores it
    touches are recorded with every result.
    """
    if count < 1:
        raise BenchError("cpu count must be >= 1")
    if mode not in ("logical", "physical"):
        raise BenchError(f"cpu mode must be logical or physical, got {mode}")
    ordered = sorted(pool) if mode == "logical" else cpu_order(pool, sibling_of)
    limit = len(ordered) if mode == "logical" else physical_cores(pool, sibling_of)
    if count > limit:
        raise BenchError(
            f"{count} {mode} CPUs requested but only {limit} available to this process ({cpu_list(pool)}); "
            "run in a wider affinity (mem-run.sh -x) or lower the count")
    server = sorted(ordered[:count])
    rest = [c for c in sorted(pool) if c not in server]
    # Bots prefer physical cores the server does not touch at all (no SMT contention).
    own = {s for cpu in server for s in sibling_of(cpu)}
    bots = [c for c in rest if c not in own] or rest
    return {"cores": count, "mode": mode, "server": cpu_list(server), "physical_cores_touched": physical_cores(server, sibling_of),
            "bots": cpu_list(bots or server), "bots_share_server_cpus": not bots}


def available_pool(override: str | None) -> list[int]:
    affinity = sorted(os.sched_getaffinity(0))
    if override is None:
        return affinity
    wanted = parse_cpu_list(override)
    missing = [c for c in wanted if c not in affinity]
    if missing:
        raise BenchError(f"--cpu-pool contains CPUs outside this process's affinity: {missing}")
    return wanted


# --- limits -----------------------------------------------------------------

def scope_prefix(memory_max: str | None, slice_name: str | None = None, root: bool | None = None) -> list[str]:
    """`systemd-run` scope that execs the command in place (same PID) under MemoryMax.

    Swap is disabled for the scope so the cap is a real RAM cap, as on a VPS.
    A non-root caller uses the user manager; root uses the system manager
    (`root` overrides the local euid when the command runs on another host).
    """
    prefix = ["systemd-run", "--scope", "-q"]
    if not (os.geteuid() == 0 if root is None else root):
        prefix.insert(1, "--user")
    if slice_name:
        prefix.append(f"--slice={slice_name}")
    if memory_max:
        parse_size(memory_max)
        prefix += ["-p", f"MemoryMax={memory_max}", "-p", "MemorySwapMax=0"]
    return prefix


def shell_prefix(memory_max: str | None, slice_name: str | None) -> str:
    """The same prefix as one shell-quoted string (for `profiles --server-wrapper`)."""
    if not memory_max:
        return ""
    return shlex.join([*scope_prefix(memory_max, slice_name), "--"])


def preflight(*, need_memory_scope: bool, need_java: str | None = None) -> list[str]:
    """Return the list of problems; empty means the host can run the pipeline."""
    problems = []
    if sys.version_info < (3, 11):
        problems.append(f"python >= 3.11 required, found {platform.python_version()}")
    for tool in ("taskset", "lscpu", "lsblk", "findmnt"):
        if shutil.which(tool) is None:
            problems.append(f"missing command: {tool} (util-linux)")
    if not Path("/sys/fs/cgroup/cgroup.controllers").exists():
        problems.append("cgroup v2 not mounted at /sys/fs/cgroup")
    if need_memory_scope:
        if shutil.which("systemd-run") is None:
            problems.append("missing command: systemd-run (needed for the MemoryMax cgroup)")
        else:
            probe = subprocess.run([*scope_prefix("64M"), "--", "true"], capture_output=True, text=True)
            if probe.returncode:
                who = "root" if os.geteuid() == 0 else "a systemd user session (loginctl enable-linger / run as root)"
                problems.append(f"systemd-run scope with MemoryMax failed (needs {who}): {probe.stderr.strip()}")
    if need_java and shutil.which(need_java) is None and not Path(need_java).is_file():
        problems.append(f"java not found: {need_java}")
    return problems


# --- host / source record ---------------------------------------------------

def hardware(pool: list[int]) -> dict[str, Any]:
    """Hardware, kernel and OS record stored in every result."""
    lscpu = run_text(["lscpu", "--json"])
    fields = {row["field"].rstrip(":"): row["data"] for row in json.loads(lscpu)["lscpu"]} if lscpu else {}
    meminfo = Path("/proc/meminfo").read_text()
    kib = {m[1]: int(m[2]) for m in re.finditer(r"^(\w+):\s+(\d+) kB", meminfo, re.M)}
    governor = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
    try:
        os_release = platform.freedesktop_os_release()
    except OSError:
        os_release = {}
    return {
        "cpu_model": fields.get("Model name"), "sockets": fields.get("Socket(s)"),
        "cores_per_socket": fields.get("Core(s) per socket"),
        "threads_per_core": fields.get("Thread(s) per core"), "logical_cpus": fields.get("CPU(s)"),
        "cpu_max_mhz": fields.get("CPU max MHz"),
        "lscpu": run_text(["lscpu"]),
        "mem_total_gib": round(kib.get("MemTotal", 0) / 2**20, 2),
        "mem_available_gib": round(kib.get("MemAvailable", 0) / 2**20, 2),
        "swap_total_gib": round(kib.get("SwapTotal", 0) / 2**20, 2),
        "kernel": platform.release(), "machine": platform.machine(),
        "os": os_release.get("PRETTY_NAME"),
        "cpu_governor": governor.read_text().strip() if governor.exists() else None,
        "cpu_pool_used_by_bench": cpu_list(pool),
        "loadavg_at_start": Path("/proc/loadavg").read_text().strip(),
        "hostname": platform.node(),
        # CPU -> physical core -> socket map (`lscpu -e`): "0-5" can be 3 physical cores, not 6.
        "topology": run_text(["lscpu", "-e=CPU,CORE,SOCKET"]),
    }


def source_record(server_bin: Path, bots_bin: Path | None, commit_override: str | None) -> dict[str, Any]:
    commit = commit_override or run_text(["git", "-C", str(ROOT), "rev-parse", "HEAD"])
    dirty = run_text(["git", "-C", str(ROOT), "status", "--porcelain", "--untracked-files=no"])
    record: dict[str, Any] = {"commit": commit, "dirty_tracked_files": bool(dirty) if dirty is not None else None,
                              "server_bin": str(server_bin), "server_sha256": sha256(server_bin)}
    if bots_bin:
        record.update(bots_bin=str(bots_bin), bots_sha256=sha256(bots_bin))
    return record


def write_build_receipt(path: Path, source: dict[str, Any], build_command: list[str]) -> Path:
    """The receipt `tools.coreperf.profiles` demands: release profile, hashes, commit, command."""
    receipt = {"profile": "release", "commit": source["commit"], "command": build_command,
               "rustc": run_text(["rustc", "-Vv"]), "server_sha256": source["server_sha256"],
               "bots_sha256": source.get("bots_sha256"),
               "note": "written by tools.bench from the supplied binaries; not a build log"}
    path.write_text(json.dumps(receipt, indent=2) + "\n")
    return path


# --- output -----------------------------------------------------------------

def fmt(value: Any, digits: int = 1) -> str:
    if value is None:
        return "n/a"
    if isinstance(value, float):
        return f"{value:.{digits}f}"
    return str(value)


def table(headers: list[str], rows: list[list[Any]]) -> str:
    lines = ["| " + " | ".join(headers) + " |", "|" + "|".join("---" for _ in headers) + "|"]
    lines += ["| " + " | ".join(str(c) for c in row) + " |" for row in rows]
    return "\n".join(lines)


def hardware_markdown(hw: dict[str, Any]) -> str:
    return "\n".join([
        f"- CPU: {hw['cpu_model']} ({hw['sockets']} socket, {hw['cores_per_socket']} cores/socket, "
        f"{hw['threads_per_core']} threads/core, {hw['logical_cpus']} logical)",
        f"- RAM: {hw['mem_total_gib']} GiB total, swap {hw['swap_total_gib']} GiB",
        f"- Kernel / OS: {hw['kernel']} / {hw['os']}",
        f"- CPU governor: {hw['cpu_governor']}; load average at start: {hw['loadavg_at_start']}",
        f"- CPUs this run could use: `{hw['cpu_pool_used_by_bench']}`",
    ])


def finish(out: Path, doc: dict[str, Any], markdown: str) -> None:
    """Write result.json + result.md and print their paths (the only stdout contract)."""
    (out / "result.json").write_text(json.dumps(doc, indent=2) + "\n")
    (out / "result.md").write_text(markdown.rstrip() + "\n")
    print(json.dumps({"command": doc["command"], "ok": doc["ok"], "smoke": doc["smoke"],
                      "result_json": str(out / "result.json"), "result_md": str(out / "result.md")}),
          flush=True)


def glibc_required(binary: Path) -> tuple[int, ...] | None:
    """Highest GLIBC_x.y symbol version the binary imports (None when objdump is unavailable)."""
    dump = run_text(["objdump", "-T", str(binary)])
    if dump is None:
        return None
    versions = [tuple(map(int, m[1].split("."))) for m in re.finditer(r"GLIBC_(\d+(?:\.\d+)+)", dump)]
    return max(versions) if versions else None
