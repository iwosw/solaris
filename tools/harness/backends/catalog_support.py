"""Process, port and receipt helpers for the catalog-load backend."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import socket
from typing import Any
import urllib.request


def binary_sha256(path: Path) -> str:
    with path.open("rb") as binary:
        return hashlib.file_digest(binary, "sha256").hexdigest()


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


def http_json(url: str, timeout: float) -> dict[str, Any]:
    # Loopback must not be routed through an operator's HTTP proxy.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(url, timeout=timeout) as response:
        value = json.load(response)
    if not isinstance(value, dict):
        raise RuntimeError(f"non-object dashboard response at {url}")
    return value


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
