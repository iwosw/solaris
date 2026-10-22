"""Driver for one real 26.1.2 vanilla server used as the parity oracle.

The world is a void superflat world; every scenario force-loads its box, runs
setup/phase commands through the server console, advances time with
`tick sprint N` (completion is the server's own "Sprint completed" log line)
and reads blocks/entities back from the saved Anvil files. No sleeps: every
wait is on a specific server log line.
"""
from __future__ import annotations

import queue
import shutil
import socket
import subprocess
import threading
import time
from pathlib import Path

from . import anvil

REPO_ROOT = Path(__file__).resolve().parents[3]
MEM_RUN = REPO_ROOT / ".analysis" / "tools" / "mem-run.sh"
DEFAULT_JAR = REPO_ROOT / ".analysis" / "server.jar"

_ERROR_MARKERS = (
    "Unknown or incomplete command",
    "Could not ",
    "That position is not loaded",
    "Expected ",
    "Incorrect argument",
    "No entity was found",
    "Unknown ",
)


class VanillaError(RuntimeError):
    pass


class VanillaOracle:
    def __init__(self, workdir: Path, *, seed: int, jar: Path = DEFAULT_JAR,
                 java: str = "java", timeout: float = 180.0, use_mem_run: bool = True,
                 properties: dict[str, str] | None = None):
        self.workdir = workdir
        self.seed = seed
        self.jar = jar
        self.java = java
        self.timeout = timeout
        self.use_mem_run = use_mem_run and MEM_RUN.is_file()
        self.extra_properties = properties or {}
        self.process: subprocess.Popen | None = None
        self.lines: queue.Queue[str | None] = queue.Queue()
        self.log_path = workdir / "vanilla.log"
        self._sync = 0
        self.game_ticks = 0

    # -- lifecycle ---------------------------------------------------------
    def start(self) -> None:
        if not self.jar.is_file():
            raise VanillaError(f"missing vanilla bundler jar: {self.jar}")
        if self.workdir.exists():
            shutil.rmtree(self.workdir)
        self.workdir.mkdir(parents=True)
        (self.workdir / "eula.txt").write_text("eula=true\n")
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        properties = {
            "online-mode": "false", "server-ip": "127.0.0.1", "server-port": str(port),
            "level-name": "world", "level-seed": str(self.seed),
            "level-type": "minecraft:flat",
            "generator-settings": '{"biome":"minecraft:plains","layers":[],"features":false,"lakes":false,"structure_overrides":[]}',
            "generate-structures": "false", "view-distance": "2", "simulation-distance": "2",
            "spawn-protection": "0", "pause-when-empty-seconds": "-1", "max-tick-time": "-1",
            "enable-rcon": "false", "enable-query": "false", "difficulty": "normal",
            "sync-chunk-writes": "false", "max-players": "1", "spawn-monsters": "false",
            "spawn-animals": "false", "spawn-npcs": "false", "enable-command-block": "true",
            "allow-nether": "true",
        }
        properties.update(self.extra_properties)
        (self.workdir / "server.properties").write_text(
            "".join(f"{key}={value}\n" for key, value in properties.items()))
        argv = [self.java, "-Xms256M", "-Xmx1280M", "-jar", str(self.jar), "nogui"]
        if self.use_mem_run:
            argv = [str(MEM_RUN), "-m", "2G"] + argv
        self.process = subprocess.Popen(
            argv, cwd=self.workdir, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, text=True, bufsize=1)

        def collect():
            assert self.process and self.process.stdout
            with self.log_path.open("w") as handle:
                for line in self.process.stdout:
                    handle.write(line)
                    handle.flush()
                    self.lines.put(line.rstrip())
            self.lines.put(None)

        threading.Thread(target=collect, daemon=True).start()
        # mem-run may queue the JVM behind other heavy jobs for a long time: wait on the log line.
        self.expect("Done (", timeout=3600)

    def stop(self) -> None:
        proc = self.process
        if proc is None or proc.poll() is not None:
            return
        try:
            assert proc.stdin
            proc.stdin.write("stop\n")
            proc.stdin.flush()
            proc.wait(timeout=60)
        except Exception:
            proc.kill()
            proc.wait(timeout=10)

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, *_exc):
        self.stop()

    # -- log waits ---------------------------------------------------------
    def expect(self, fragment: str, *, timeout: float | None = None,
               collect: list[str] | None = None) -> str:
        deadline = time.monotonic() + (timeout or self.timeout)
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise VanillaError(f"timed out waiting for {fragment!r}; see {self.log_path}")
            try:
                line = self.lines.get(timeout=remaining)
            except queue.Empty:
                continue
            if line is None:
                raise VanillaError(f"vanilla exited waiting for {fragment!r}; see {self.log_path}")
            if collect is not None:
                collect.append(line)
            if fragment in line:
                return line

    def send(self, command: str) -> None:
        assert self.process and self.process.stdin
        self.process.stdin.write(command + "\n")
        self.process.stdin.flush()

    # -- commands ----------------------------------------------------------
    def run(self, command: str) -> list[str]:
        """Execute one console command and return its feedback/error lines."""
        self._sync += 1
        marker = f"QP_SYNC_{self._sync}"
        collected: list[str] = []
        self.send(command)
        self.send(f"say {marker}")
        self.expect(f"[Server] {marker}", collect=collected)
        collected.pop()
        return [line.split("]: ", 1)[-1] for line in collected]

    def run_checked(self, commands: list[str], chunk: int = 200) -> list[str]:
        """Run commands pipelined (one sync per chunk); return rejection feedback lines.

        Feedback lines cannot be tied to the command that caused them, so a
        rejection row carries the chunk's first command for orientation.
        """
        problems = []
        for start in range(0, len(commands), chunk):
            batch = commands[start:start + chunk]
            self._sync += 1
            marker = f"QP_SYNC_{self._sync}"
            collected: list[str] = []
            for command in batch:
                self.send(command)
            self.send(f"say {marker}")
            self.expect(f"[Server] {marker}", timeout=max(self.timeout, 60 + len(batch)), collect=collected)
            for line in collected[:-1]:
                text = line.split("]: ", 1)[-1]
                if "Could not set the block" in text or "No entity was found" in text:
                    continue  # target already held that block: not a rejection
                if any(marker_ in text for marker_ in _ERROR_MARKERS):
                    problems.append(f"[chunk@{start}: {batch[0]} ..] {text}")
        return problems

    def sprint(self, ticks: int) -> None:
        if ticks <= 0:
            return
        # Drain stale lines so "Sprint completed" is this sprint's own report.
        self.send(f"tick sprint {ticks}")
        self.expect("Sprint completed")
        self.game_ticks += ticks

    def freeze(self) -> None:
        self.run("tick freeze")

    # -- observation -------------------------------------------------------
    def snapshot(self, boxes, dimension: str = "overworld") -> dict:
        """Snapshot of the saved world: union plus per-box `cells` when several boxes."""
        self.send("save-all flush")
        self.expect("Saved the game", timeout=120)
        base = self.workdir / "world" / "dimensions" / "minecraft" / dimension
        cells = []
        for cell in anvil.read_world(base, boxes):
            cells.append({"blocks": cell["blocks"], "containers": cell["containers"],
                          "entities": [normalize_entity(nbt) for nbt in cell["entities"]]})
        out = {"blocks": {}, "entities": [], "containers": {}}
        for cell in cells:
            out["blocks"].update(cell["blocks"])
            out["containers"].update(cell["containers"])
            out["entities"].extend(cell["entities"])
        if len(boxes) > 1:
            out["cells"] = cells
        return out


def normalize_entity(nbt: dict) -> dict:
    entry = {
        "type": nbt.get("id"),
        "pos": nbt.get("Pos"),
        "motion": nbt.get("Motion"),
    }
    if "Health" in nbt:
        entry["health"] = nbt["Health"]
    item = nbt.get("Item")
    if isinstance(item, dict):
        entry["item"] = item.get("id")
        entry["count"] = item.get("count", item.get("Count", 1))
    if "fuse" in nbt:
        entry["fuse"] = nbt["fuse"]
    if "Age" in nbt:
        entry["age"] = nbt["Age"]
    return entry
