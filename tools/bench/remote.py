"""A remote host reached over one multiplexed ssh connection (commands, rsync, port forwards)."""
from __future__ import annotations

from pathlib import Path
import re
import shutil
import shlex
import statistics
import subprocess
import tempfile
import time
from typing import Any

from tools.bench import common
from tools.bench.common import BenchError


class Remote:
    """One ssh ControlMaster per run: a single authentication, forwards live as long as it does."""

    def __init__(self, host: str, ssh_options: list[str], forwards: list[tuple[int, int]]):
        self.host = host
        self.ssh_options = ssh_options
        self.forwards = forwards  # (local port, remote loopback port)
        self.scratch = tempfile.mkdtemp(prefix="bench-ssh-")
        self.control = f"{self.scratch}/ctl"
        self.opened = False

    def _ssh(self, *extra: str) -> list[str]:
        return ["ssh", "-S", self.control, "-o", "BatchMode=yes", *self.ssh_options, *extra]

    def open(self) -> None:
        command = ["ssh", "-M", "-S", self.control, "-fN", "-o", "BatchMode=yes", "-o", "ExitOnForwardFailure=yes",
                   "-o", "ServerAliveInterval=15", *self.ssh_options]
        for local, remote in self.forwards:
            command += ["-L", f"{local}:127.0.0.1:{remote}"]
        done = subprocess.run([*command, self.host], capture_output=True, text=True)
        if done.returncode:
            raise BenchError(f"cannot open ssh to {self.host}: {done.stderr.strip()}")
        self.opened = True

    def close(self) -> None:
        if self.opened:
            subprocess.run(self._ssh("-O", "exit", self.host), capture_output=True)
            self.opened = False
        shutil.rmtree(self.scratch, ignore_errors=True)

    def run(self, command: str | list[str], *, check: bool = True, input: str | None = None,
            timeout: float | None = None) -> subprocess.CompletedProcess[str]:
        text = command if isinstance(command, str) else shlex.join(command)
        done = subprocess.run([*self._ssh(self.host), text], capture_output=True, text=True, input=input, timeout=timeout)
        if check and done.returncode:
            raise BenchError(f"remote command failed ({done.returncode}): {text[:200]}\n{done.stderr.strip()[-600:]}")
        return done

    def put_text(self, path: str, text: str) -> None:
        self.run(f"cat > {shlex.quote(path)}", input=text)

    def rsync(self, source: Path | str, destination: str, *, delete: bool = False) -> None:
        ssh = shlex.join(self._ssh())
        command = ["rsync", "-a", "-e", ssh, *(["--delete"] if delete else []), f"{source}", f"{self.host}:{destination}"]
        done = subprocess.run(command, capture_output=True, text=True)
        if done.returncode:
            raise BenchError(f"rsync to {self.host}:{destination} failed: {done.stderr.strip()[-600:]}")

    def rsync_from(self, source: str, destination: Path) -> None:
        ssh = shlex.join(self._ssh())
        command = ["rsync", "-a", "--no-owner", "--no-group", "--delete", "-e", ssh, f"{self.host}:{source}", f"{destination}/"]
        done = subprocess.run(command, capture_output=True, text=True)
        if done.returncode:
            raise BenchError(f"rsync from {self.host}:{source} failed: {done.stderr.strip()[-600:]}")

    # --- facts ---------------------------------------------------------------

    def is_root(self) -> bool:
        return self.run("id -u").stdout.strip() == "0"

    def cpu_pool(self) -> list[int]:
        # Respect any affinity the ssh session itself already carries.
        allowed = self.run("taskset -pc $$").stdout.rsplit(":", 1)[-1]
        return common.parse_cpu_list(allowed)

    @staticmethod
    def sibling_of(cpu: int) -> list[int]:
        return [cpu]  # remote SMT topology is not probed; a VPS exposes vCPUs

    def read_text(self, path: str) -> str | None:
        done = self.run(f"cat {shlex.quote(path)}", check=False)
        return done.stdout if done.returncode == 0 else None

    def rmtree(self, path: str) -> None:
        if "solaris-bench-" not in path:
            raise BenchError(f"refusing to delete {path}: not under a solaris-bench-* directory")
        self.run(["rm", "-rf", "--", path])

    def has_time(self) -> bool:
        return self.run("test -x /usr/bin/time", check=False).returncode == 0

    def mkdir(self, path: str) -> None:
        self.run(["mkdir", "-p", "--", path])

    def pid_status(self, pid: int) -> dict[str, int]:
        """VmHWM/VmRSS (bytes) and CPU seconds of a remote process."""
        text = self.run(f"cat /proc/{pid}/status; echo ---; cat /proc/{pid}/stat").stdout
        status, _, stat = text.partition("---\n")
        values = {m[1]: int(m[2]) * 1024 for m in re.finditer(r"^(VmHWM|VmRSS):\s+(\d+) kB", status, re.M)}
        fields = stat.rsplit(")", 1)[1].split()
        ticks = int(self.run("getconf CLK_TCK").stdout)
        values["cpu_seconds"] = (int(fields[11]) + int(fields[12])) / ticks
        return values

    def clock_offset_ns(self, samples: int = 9) -> dict[str, Any]:
        """Remote wall clock minus local wall clock (median of midpoint estimates) and the round trip."""
        offsets, trips = [], []
        for _ in range(samples):
            before = time.time_ns()
            remote = int(self.run("date +%s%N").stdout)
            after = time.time_ns()
            offsets.append(remote - (before + after) // 2)
            trips.append(after - before)
        return {"offset_ns": int(statistics.median(offsets)), "round_trip_ms_median": statistics.median(trips) / 1e6,
                "spread_ms": (max(offsets) - min(offsets)) / 1e6}

    def hardware(self) -> dict[str, Any]:
        script = ("lscpu; echo '###'; grep -E 'MemTotal|MemAvailable|SwapTotal' /proc/meminfo; echo '###'; uname -r; "
                  "echo '###'; . /etc/os-release; echo $PRETTY_NAME; echo '###'; cat /proc/loadavg; echo '###'; "
                  "cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null; echo '###'; "
                  "ps -eo pcpu,comm --sort=-pcpu | head -6; echo '###'; "
                  "systemctl is-active solaris 2>/dev/null; echo '###'; getconf GNU_LIBC_VERSION; echo '###'; hostname")
        lscpu, mem, kernel, os_name, load, governor, busiest, solaris_unit, glibc, hostname = (
            part.strip() for part in self.run(script).stdout.split("###"))
        fields = {k.strip(): v.strip() for k, _, v in (line.partition(":") for line in lscpu.splitlines())}
        kib = {m[1]: int(m[2]) for m in re.finditer(r"^(\w+):\s+(\d+) kB", mem, re.M)}
        return {"cpu_model": fields.get("Model name"), "sockets": fields.get("Socket(s)"),
                "cores_per_socket": fields.get("Core(s) per socket"), "threads_per_core": fields.get("Thread(s) per core"),
                "logical_cpus": fields.get("CPU(s)"), "cpu_max_mhz": fields.get("CPU max MHz"), "lscpu": lscpu,
                "mem_total_gib": round(kib.get("MemTotal", 0) / 2**20, 2),
                "mem_available_gib": round(kib.get("MemAvailable", 0) / 2**20, 2),
                "swap_total_gib": round(kib.get("SwapTotal", 0) / 2**20, 2),
                "kernel": kernel, "machine": fields.get("Architecture"), "os": os_name, "cpu_governor": governor or None,
                "loadavg_at_start": load, "busiest_processes_at_start": busiest.splitlines(),
                "solaris_unit_state_at_start": solaris_unit, "glibc": glibc, "hostname": hostname,
                "cpu_pool_used_by_bench": None}


class LocalHost:
    """The same small surface as Remote, for commands that run on this machine."""
    name = "local"

    def __init__(self, pool: list[int]):
        self.pool = pool

    @staticmethod
    def sibling_of(cpu: int) -> list[int]:
        return common.siblings(cpu)

    def cpu_pool(self) -> list[int]:
        return self.pool

    def is_root(self) -> bool:
        return common.os.geteuid() == 0

    def run(self, command: list[str], *, check: bool = True, input: str | None = None,
            timeout: float | None = None) -> subprocess.CompletedProcess[str]:
        done = subprocess.run(command, capture_output=True, text=True, input=input, timeout=timeout)
        if check and done.returncode:
            raise BenchError(f"command failed ({done.returncode}): {shlex.join(command)[:200]}\n{done.stderr.strip()[-600:]}")
        return done

    def put_text(self, path: str, text: str) -> None:
        Path(path).parent.mkdir(parents=True, exist_ok=True)
        Path(path).write_text(text)

    def read_text(self, path: str) -> str | None:
        file = Path(path)
        return file.read_text() if file.is_file() else None

    def rmtree(self, path: str) -> None:
        shutil.rmtree(path, ignore_errors=True)

    def has_time(self) -> bool:
        return Path("/usr/bin/time").exists()

    def mkdir(self, path: str) -> None:
        Path(path).mkdir(parents=True, exist_ok=True)
