"""A coreperf session whose server runs on a remote host; bots, fixtures and receipts stay local.

Plugs into `tools.coreperf.profiles.measure` (selected by `--remote-host`), so TPS / MSPT / RSS
keep the canonical definitions. The remote side is one transient systemd unit named
`solaris-bench-*` in its own `solaris-bench-*` directory; nothing else on the host is touched.
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
from pathlib import Path
import socket
import time
import tomllib
from typing import Any

from tools.bench.common import BenchError
from tools.bench.remote import Remote
from tools.coreperf import compare
from tools.harness.backends import catalog_load as load

WAIT_FOR_LINE = ('f=$1; pid=$2; p=$3; coproc T { exec tail --pid="$pid" -n +1 -F "$f"; }; '
                 'grep -m1 -q -F -- "$p" <&"${T[0]}"; rc=$?; kill "$T_PID" 2>/dev/null; exit $rc')


class RemoteSession(compare.CoreSession):
    def __init__(self, args: argparse.Namespace, directory: Path, config: Path, world: Path):
        super().__init__(args, directory, config)
        self.world = world
        parsed = tomllib.loads(config.read_text())
        self.local_dash, self.local_game = parsed["dashboard"]["port"], parsed["network"]["port"]
        forwards = [(self.local_dash, args.remote_dash_port)]
        if args.remote_tunnel_game:
            forwards.append((self.local_game, args.remote_game_port))
        self.remote = Remote(args.remote_host, args.remote_ssh_option, forwards)
        self.bot_host = args.remote_bot_host or args.remote_host.rpartition("@")[2]
        self.unit = f"solaris-bench-{os.getpid()}-{int(time.time())}"
        self.rdir = f"{args.remote_dir.rstrip('/')}/{self.unit}"
        self.pid: int | None = None
        self.root = False
        self.clock_offset_ns = 0

    # --- start ---------------------------------------------------------------

    def remote_config(self) -> str:
        """The local isolated config with remote paths/ports; every rewritten key must exist exactly once."""
        a = self.args
        data_dir = a.remote_data_dir
        if not data_dir:
            raise BenchError("--remote-data-dir is required (the imported content cache on the remote host)")
        replacements = {("data", "world_dir"): f"{self.rdir}/world", ("data", "vanilla_data_dir"): data_dir,
                        ("network", "bind_address"): "127.0.0.1" if a.remote_tunnel_game else "0.0.0.0",
                        ("network", "port"): a.remote_game_port, ("dashboard", "port"): a.remote_dash_port}
        seen, section, lines = set(), "", []
        for line in self.config.read_text().splitlines():
            stripped = line.strip()
            if stripped.startswith("["):
                section = stripped.strip("[]")
            key = stripped.split("=", 1)[0].strip()
            if (section, key) in replacements and "=" in stripped:
                value = replacements[(section, key)]
                line = f"{key} = {json.dumps(value)}"
                seen.add((section, key))
            lines.append(line)
        if seen != set(replacements):
            raise BenchError(f"isolated config template changed; could not rewrite {set(replacements) - seen}")
        return "\n".join(lines) + "\n"

    async def start_server(self) -> None:
        a, r = self.args, self.remote
        await asyncio.to_thread(r.open)
        self.root = r.is_root()
        r.run(["mkdir", "-p", self.rdir])
        await asyncio.to_thread(r.rsync, a.server_bin, f"{self.rdir}/solaris-v2")
        await asyncio.to_thread(r.rsync, f"{self.world}/", f"{self.rdir}/world/")
        await asyncio.to_thread(r.rsync, self.directory / "ops.json", f"{self.rdir}/ops.json")
        r.put_text(f"{self.rdir}/server.toml", self.remote_config())
        command = ["systemd-run", *([] if self.root else ["--user"]), f"--unit={self.unit}", "--remain-after-exit", "--quiet",
                   "-p", f"WorkingDirectory={self.rdir}", "-p", f"StandardOutput=append:{self.rdir}/server.log",
                   "-p", f"StandardError=append:{self.rdir}/server.log", "-p", "MemoryAccounting=yes"]
        if a.remote_memory_max:
            command += ["-p", f"MemoryMax={a.remote_memory_max}", "-p", "MemorySwapMax=0"]
        command += ["--", "taskset", "-c", a.server_cpus, f"{self.rdir}/solaris-v2",
                    "--config", f"{self.rdir}/server.toml", "--no-console"]
        load.scale.dump(self.directory / "server-command.json", {"host": a.remote_host, "command": command})
        r.run(command)
        self.pid = int(self.systemctl("show", "-p", "MainPID", "--value", self.unit).stdout.strip())
        if not self.pid:
            raise load.ClassifiedFailure("SETUP_FAILED", "remote server unit has no main pid")

    def systemctl(self, *args: str, check: bool = True):
        return self.remote.run(["systemctl", *([] if self.root else ["--user"]), *args], check=check)

    async def wait_ready(self, url: str, timeout: float) -> dict[str, Any]:
        """Event-driven: block on the remote log until the dashboard line, then check bot reachability and clocks."""
        assert self.pid is not None
        done = await asyncio.to_thread(
            self.remote.run, ["timeout", str(int(timeout)), "bash", "-c", WAIT_FOR_LINE, "bash",
                              f"{self.rdir}/server.log", str(self.pid), "dashboard bound and listening"],
            check=False, timeout=timeout + 60)
        if done.returncode:
            log = self.remote.run(["tail", "-n", "30", f"{self.rdir}/server.log"], check=False).stdout
            raise load.ClassifiedFailure("INFRA_BLOCKED", f"remote server not ready (exit {done.returncode}):\n{log}")
        if not self.args.remote_tunnel_game:
            try:
                socket.create_connection((self.bot_host, self.args.remote_game_port), timeout=10).close()
            except OSError as error:
                raise load.ClassifiedFailure(
                    "INFRA_BLOCKED", f"game port {self.bot_host}:{self.args.remote_game_port} unreachable from here ({error}); "
                    "open it or rerun with --remote-tunnel-game") from error
        clock = await asyncio.to_thread(self.remote.clock_offset_ns)
        self.clock_offset_ns = clock["offset_ns"]
        load.scale.dump(self.directory / "remote.json", {
            "host": self.args.remote_host, "unit": self.unit, "dir": self.rdir, "pid": self.pid,
            "clock": clock, "game_via_ssh_tunnel": self.args.remote_tunnel_game,
            "bot_addr": f"{self.bot_host}:{self.args.remote_game_port}"})
        return await asyncio.to_thread(load.scale.http_json, url + "/stats", 5)

    def bot_command(self, command: list[str]) -> list[str]:
        if self.args.remote_tunnel_game:
            return command  # 127.0.0.1:<local forward> already
        index = command.index("--addr") + 1
        return [*command[:index], f"{self.bot_host}:{self.args.remote_game_port}", *command[index + 1:]]

    # --- sampling ------------------------------------------------------------

    def process_samples(self) -> dict[str, Any]:
        assert self.pid is not None
        try:
            status = self.remote.pid_status(self.pid)
        except BenchError as error:
            raise load.ClassifiedFailure("INFRA_BLOCKED", f"remote server process metrics unavailable: {error}") from error
        server = {"pid": self.pid, "cpu_seconds": status["cpu_seconds"],
                  "rss_bytes": status.get("VmRSS", 0), "peak_rss_bytes": status.get("VmHWM", 0)}
        return {"server": server, **super().process_samples()}

    # --- stop ----------------------------------------------------------------

    async def stop(self) -> None:
        await super().stop()  # bots only: there is no local "server" process
        await asyncio.to_thread(self.stop_remote)

    def stop_remote(self) -> None:
        r = self.remote
        if not r.opened:
            return
        try:
            if self.pid is None:
                return
            self.systemctl("kill", "--signal=SIGTERM", self.unit, check=False)
            exited = r.run(["timeout", str(int(self.args.readiness_seconds)), "tail", f"--pid={self.pid}", "-f", "/dev/null"],
                           check=False, timeout=self.args.readiness_seconds + 60)
            forced = exited.returncode == 124
            if forced:
                self.systemctl("kill", "--signal=SIGKILL", self.unit, check=False)
            show = self.systemctl("show", "-p", "ExecMainStatus", "-p", "Result", self.unit, check=False).stdout
            fields = dict(line.split("=", 1) for line in show.splitlines() if "=" in line)
            self.shutdown["server"] = {"returncode": int(fields.get("ExecMainStatus", -1)), "forced": forced,
                                       "unit_result": fields.get("Result")}
            (self.directory / "server.log").write_text(r.read_text(f"{self.rdir}/server.log") or "")
            r.rsync_from(f"{self.rdir}/world/", self.world)  # so the usual saved-world cohort check runs locally
        finally:
            self.systemctl("stop", self.unit, check=False)
            self.systemctl("reset-failed", self.unit, check=False)
            r.rmtree(self.rdir)
            r.close()
