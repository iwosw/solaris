"""Player-quirk parity orchestrator: attribute-swap + diagonal-speed.

For each scenario: start VanillaOracle (void world), run quirk-probe against
it, stop; start solaris-v2 (isolated temp world dir/config, stdin console,
wait for the ready log line), run the same setup + quirk-probe, stop; compare
the two JSON observations with the scenario tolerance and write receipt.json.

CLI: python3 -m tools.harness.quirk_parity.player --out DIR [--scenario ID]
     [--solaris-bin PATH]
"""
from __future__ import annotations

import argparse
import json
import os
import queue
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO_ROOT))
MEM_RUN = REPO_ROOT / ".analysis" / "tools" / "mem-run.sh"
DEFAULT_JAR = REPO_ROOT / ".analysis" / "server.jar"
SCENARIO_DIR = Path(__file__).with_name("player_scenarios")

from tools.harness.quirk_parity.vanilla import VanillaOracle  # noqa: E402


def load_scenarios(only: str | None) -> list[dict]:
    out = []
    for path in sorted(SCENARIO_DIR.glob("*.json")):
        spec = json.loads(path.read_text())
        if only and spec["id"] != only:
            continue
        out.append(spec)
    return out


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def run_probe_handshake(probe_bin: Path, addr: str, name: str, scenario: str, out: Path,
                        on_ready=None) -> dict:
    """Spawn the probe with piped stdin/stdout; on its {"event":"ready"} line,
    run on_ready() (console setup, now that chunks around the player are
    loaded), then write "go" to its stdin and wait for the final JSON."""
    import threading as _threading
    argv = [str(probe_bin), "--addr", addr, "--name", name,
            "--scenario", scenario, "--out", str(out)]
    print(f"quirk-probe {scenario} -> {addr} ({name})", flush=True)
    proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, bufsize=1)
    stdout_lines: list[str] = []
    ready_seen = _threading.Event()

    def _pump():
        assert proc.stdout is not None
        for line in proc.stdout:
            stdout_lines.append(line)
            try:
                if json.loads(line).get("event") == "ready":
                    ready_seen.set()
            except Exception:
                pass

    pump = _threading.Thread(target=_pump, daemon=True)
    pump.start()
    # Wait for ready (probe joined + 20 ClientTickEnd pumped).
    if not ready_seen.wait(timeout=300):
        proc.kill()
        raise RuntimeError(f"quirk-probe {scenario}: no ready handshake in 300s")
    notes = on_ready() if on_ready else {}
    assert proc.stdin is not None
    proc.stdin.write("go\n")
    proc.stdin.flush()
    try:
        proc.wait(timeout=900)
    except subprocess.TimeoutExpired:
        proc.kill()
        raise RuntimeError(f"quirk-probe {scenario}: no exit in 900s after go")
    pump.join(timeout=10)
    stderr = proc.stderr.read() if proc.stderr else ""
    if proc.returncode != 0:
        raise RuntimeError(
            f"quirk-probe {scenario} failed rc={proc.returncode}\n"
            f"STDOUT:\n{''.join(stdout_lines)[-3000:]}\nSTDERR:\n{stderr[-3000:]}")
    lines = "".join(stdout_lines).strip().splitlines()
    if not lines:
        raise RuntimeError(
            f"quirk-probe {scenario} printed no JSON\nSTDERR:\n{stderr[-3000:]}")
    return {"obs": json.loads(lines[-1]), "setup_notes": notes}


def run_probe(probe_bin: Path, addr: str, name: str, scenario: str, out: Path) -> dict:
    # Direct exec, no mem-run: the probe is a trivial socket client (tiny RSS),
    # and mem-run slots are starved by other agents' heavy cargo jobs — queueing
    # here burned the whole run timeout without ever exec'ing the probe.
    argv = [str(probe_bin), "--addr", addr, "--name", name,
            "--scenario", scenario, "--out", str(out)]
    print(f"quirk-probe {scenario} -> {addr} ({name})", flush=True)
    proc = subprocess.run(argv, capture_output=True, text=True, timeout=900)
    if proc.returncode != 0:
        raise RuntimeError(
            f"quirk-probe {scenario} failed rc={proc.returncode}\n"
            f"STDOUT:\n{proc.stdout[-3000:]}\nSTDERR:\n{proc.stderr[-3000:]}")
    lines = proc.stdout.strip().splitlines()
    if not lines:
        raise RuntimeError(
            f"quirk-probe {scenario} printed no JSON\nSTDERR:\n{proc.stderr[-3000:]}")
    return json.loads(lines[-1])

# -- vanilla side ----------------------------------------------------------

def vanilla_run(spec: dict, workdir: Path, probe_bin: Path, name: str) -> dict:
    import threading
    oracle = VanillaOracle(workdir, seed=1234, use_mem_run=False,
                           properties={"gamemode": "survival",
                                       "spawn-protection": "0",
                                       "difficulty": "normal",
                                       "op-permission-level": "4"})
    oracle.start()
    try:
        props = Path(oracle.workdir, "server.properties").read_text()
        # Match the exact line: naive split("server-port=") hits the
        # "management-server-port=0" substring first (yields bogus port 0).
        port_lines = [l.split("=", 1)[1].strip() for l in props.splitlines()
                      if l.startswith("server-port=")]
        assert len(port_lines) == 1, f"no unique server-port line: {port_lines!r}"
        port = port_lines[0]
        assert port != "0" and port.isdigit(), f"bogus vanilla port {port!r}"
        print(f"vanilla {spec['id']}: oracle on 127.0.0.1:{port}", flush=True)
        setup = spec.get("setup", {})
        # World prep pre-join (no player needed).
        for part in setup.get("platform", "").split(";"):
            if part.strip():
                oracle.run(part.strip())
        if "victim" in setup:
            oracle.run(setup["victim"])
        oracle.run("op " + name)
        for item in setup.get("items", []):
            if "{name}" not in item and not item.startswith(("give ", "clear ", "gamemode ", "tp ")):
                oracle.run(item)
        # Probe joins in the background (it waits for victim/inventory);
        # player-targeted setup runs live while it waits.
        out = workdir / f"{spec['id']}.probe.json"

        def _setup_after_ready():
            # Handshake: probe is joined + chunks loaded. World prep first
            # (no player needed), then player setup once — no retries, no
            # re-summons (each re-summon orphans the latched victim id).
            notes: dict = {}
            for part in setup.get("platform", "").split(";"):
                if part.strip():
                    notes.setdefault("platform", []).append(
                        {part.strip(): oracle.run(part.strip())})
            if "victim" in setup:
                notes["victim"] = oracle.run(setup["victim"])
            notes["op"] = oracle.run("op " + name)
            applied = []
            for item in setup.get("items", []):
                applied.append({item: oracle.run(item.replace("{name}", name))})
            notes["items"] = applied
            if "player_tp" in setup:
                notes["tp"] = oracle.run(setup["player_tp"].replace("{name}", name))
            # Barrier: the summon already returned its own console feedback
            # ("Summoned new ..." is INSIDE notes["victim"]); oracle.expect
            # would re-scan from old log positions. Verify from the captured
            # output instead of waiting on the log tail.
            victim_out = notes.get("victim", [])
            notes["victim_ok"] = any("Summoned" in l for l in victim_out)
            if not notes["victim_ok"]:
                fb = setup.get("victim_fallback_nbt")
                if fb:
                    out2 = oracle.run(fb)
                    notes["victim_fallback"] = out2
                    notes["victim_ok"] = any("Summoned" in l for l in out2)
            print(f"vanilla {spec['id']}: setup applied post-ready", flush=True)

        res = run_probe_handshake(probe_bin, f"127.0.0.1:{port}", name,
                                  spec["id"], out, on_ready=_setup_after_ready)
        obs = res["obs"]
        return {"observations": obs["observations"], "frame_counts": obs.get("frame_counts", {}),
                "setup_notes": res.get("setup_notes", {})}
    finally:
        oracle.stop()


def setup_console(oracle: VanillaOracle, spec: dict, name: str, vanilla: bool) -> dict:
    """Run scenario setup commands through the server console; return notes."""
    notes: dict = {}
    setup = spec["setup"]
    # Wait for the player to be joinable: poll `list` until server is up
    # (Done already seen by start()).
    cmds: list[str] = []
    plat = setup.get("platform", "")
    for part in plat.split(";"):
        if part.strip():
            cmds.append(part.strip())
    if "victim" in setup:
        out = oracle.run(setup["victim"].replace("{name}", name))
        if any("Unknown" in l or "Could not" in l or "Incorrect" in l for l in out):
            fb = setup.get("victim_fallback_nbt")
            if fb:
                out2 = oracle.run(fb.replace("{name}", name))
                notes["victim_fallback"] = out2
            else:
                notes["victim_error"] = out
    for item in setup.get("items", []):
        cmd = item.replace("{name}", name)
        out = oracle.run(cmd)
        notes.setdefault("items", []).append({cmd: out})
    # op + tp last (player must exist; retry tp a few times via `list` check).
    oracle.run(f"op {name}")
    if "player_tp" in setup:
        for _ in range(5):
            out = oracle.run(setup["player_tp"].replace("{name}", name))
            if not any("No entity" in l or "Unknown" in l for l in out):
                break
            time.sleep(1)
        notes["tp"] = out
    for cmd in cmds:
        out = oracle.run(cmd)
        if out:
            notes.setdefault("platform", []).append({cmd: out})
    oracle.run("save-all")
    return notes


# -- solaris side ----------------------------------------------------------

class SolarisServer:
    """Launch solaris-v2 with an isolated world dir + stdin console.

    Launch is detached via setsid so the server survives the spawning
    tool-call scope (background children of a tool call are reaped when the
    call returns; setsid + a wrapper script keeps it alive). Stdin console
    stays attached through a FIFO so setup commands can be sent.
    """

    def __init__(self, workdir: Path, binary: Path, port: int):
        self.workdir = workdir
        self.binary = binary
        self.port = port
        self.process: subprocess.Popen | None = None
        self.lines: queue.Queue[str | None] = queue.Queue()
        self.log_path = workdir / "solaris.log"
        self._sync = 0
        self._stopped = False
        self._fifo: object = None
        self._line_count = 0

    def start(self) -> None:
        if self.workdir.exists():
            shutil.rmtree(self.workdir)
        self.workdir.mkdir(parents=True)
        cfg = (f"[server]\nname = \"quirk\"\nmotd = \"quirk\"\n"
               f"max_players = 4\nview_distance = 4\nsimulation_distance = 2\n"
               f"pause_when_empty_seconds = 0\n"
               f"[network]\nbind_address = \"127.0.0.1\"\nport = {self.port}\n"
               f"[auth]\nonline_mode = false\n"
               f"[anti_cheat]\npacket_rate_per_second = 0\npacket_burst = 0\n"
               f"max_airtime_ticks = 0\n"
               f"[data]\nworld_dir = \"world\"\nseed = 42\n"
               f"vanilla_data_dir = \"/home/kaiserroman/solaris/data/vanilla\"\n")
        (self.workdir / "quirk.toml").write_text(cfg)
        # Detached launch: background children of a tool call are reaped when
        # the call returns, so plain-Popen servers die between tool calls
        # (observed: listening server gone 60s later, connection refused).
        # setsid detaches it; stdin console goes through a FIFO we hold open.
        # Direct exec, no mem-run (slots starved by other agents' cargo jobs).
        fifo = self.workdir / "stdin.fifo"
        run_sh = self.workdir / "run.sh"
        run_sh.write_text(
            "#!/bin/bash\n"
            f"cd {self.workdir}\n"
            f"exec {self.binary} --config quirk.toml < stdin.fifo > solaris.log 2>&1\n"
        )
        run_sh.chmod(0o755)
        if fifo.exists():
            fifo.unlink()
        os.mkfifo(fifo)
        # Holding fd must be opened AFTER the reader side exists: open RDWR
        # (never blocks) right before launching the reader.
        self.process = subprocess.Popen(
            ["setsid", str(run_sh)], cwd=self.workdir,
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, start_new_session=True)
        self._own_process = False
        self._fifo = open(fifo, "w", buffering=1)

        def collect():
            # solaris.log is written by the detached server; tail it into the
            # line queue (poll-free: blocking readline on a growing file).
            assert self.log_path is not None
            while True:
                try:
                    with open(self.log_path) as handle:
                        break
                except FileNotFoundError:
                    time.sleep(0.2)
            with open(self.log_path) as handle:
                while True:
                    line = handle.readline()
                    if line:
                        self.lines.put(line.rstrip())
                    else:
                        time.sleep(0.2)
                        if self._stopped:
                            self.lines.put(None)
                            return

        self._stopped = False
        threading.Thread(target=collect, daemon=True).start()
        self.expect("Solaris is listening", timeout=180)

    def stop(self) -> None:
        self._stopped = True
        try:
            self.send("stop")
        except Exception:
            pass
        # Give the detached server a moment to checkpoint + exit, then close
        # the FIFO (EOF on stdin) and wait for the log tail to finish.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            try:
                with open(self.log_path) as handle:
                    tail = handle.read()[-2000:]
                if "stopping" in tail or "shutdown" in tail or "stopped" in tail:
                    break
            except FileNotFoundError:
                pass
            time.sleep(0.5)
        try:
            if self._fifo is not None:
                self._fifo.close()
        except Exception:
            pass
        proc = self.process
        if proc is not None:
            try:
                proc.wait(timeout=5)
            except Exception:
                pass

    def expect(self, fragment: str, timeout: float = 60.0) -> str:
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError(
                    f"timed out waiting for {fragment!r}; see {self.log_path}")
            try:
                line = self.lines.get(timeout=remaining)
            except queue.Empty:
                continue
            if line is None:
                raise RuntimeError(
                    f"solaris exited waiting for {fragment!r}; see {self.log_path}")
            if fragment in line:
                return line

    def send(self, command: str) -> None:
        assert self._fifo is not None, "server FIFO not open"
        self._fifo.write(command + "\n")
        self._fifo.flush()
    def run(self, command: str) -> list[str]:
        """Console command with tick-output sync.

        The old `say QP_SYNC_n` chat-echo delimiter is unreliable: `say` (and
        most command feedback) goes to the CONSOLE connection outbox, which
        `console::output` only prints for Chat/StyledChat/Disconnect/Title —
        Translated feedback (teleport/give/summon success keys) never prints,
        so the marker never arrives and the wait times out even though the
        command EXECUTED. Instead, sync on tick advancement: record the
        server's current tick from the log clock, submit the command, and
        wait until the log shows a later state (any new INFO line past the
        command tick). Resilient: never raises on a quiet server; returns
        whatever output lines appeared.
        """
        self._sync += 1
        try:
            self.send(command)
        except (BrokenPipeError, ValueError):
            return [f"<server gone before {command!r}>"]
        # Event-driven wait: drain new log lines until the server logs any
        # activity after our command (tick checkpoint / command output), or a
        # bounded 30s cap. Commands execute on the next tick (~50ms), so any
        # fresh line proves admission.
        collected: list[str] = []
        deadline = time.monotonic() + 30
        baseline = self._line_count
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            try:
                line = self.lines.get(timeout=min(remaining, 5.0))
            except queue.Empty:
                continue
            if line is None:
                collected.append("<server exited mid-command>")
                break
            collected.append(line)
            self._line_count += 1
            # A fresh line after submission proves the tick pump admitted the
            # command (execution is same-tick). One extra line of margin for
            # the command's own output, then return.
            if self._line_count > baseline + 1:
                break
        return collected[-8:]


def solaris_run(spec: dict, workdir: Path, binary: Path, probe_bin: Path, name: str) -> dict:
    port = free_port()
    server = SolarisServer(workdir, binary, port)
    server.start()
    try:
        notes = solaris_setup(server, spec, name)
        out = workdir / f"{spec['id']}.probe.json"
        obs = run_probe(probe_bin, f"127.0.0.1:{port}", name, spec["id"], out)
        return {"observations": obs["observations"],
                "frame_counts": obs.get("frame_counts", {}),
                "setup_notes": notes}
    finally:
        server.stop()


def solaris_setup(server: SolarisServer, spec: dict, name: str) -> dict:
    """Console setup on Solaris. Player must be connected for tp/give to hit;
    retry those after the probe joins is handled by the probe itself, so here
    do world prep + static commands and best-effort player commands."""
    notes: dict = {}
    setup = spec.get("setup", {})
    for part in setup.get("platform", "").split(";"):
        if part.strip():
            notes.setdefault("platform", []).append(
                {part.strip(): server.run(part.strip())})
    if "victim" in setup:
        out = server.run(setup["victim"])
        if any("Unknown" in l or "Could not" in l or "Error" in l or "error" in l for l in out):
            fb = setup.get("victim_fallback_nbt")
            if fb:
                notes["victim_fallback"] = server.run(fb)
            else:
                notes["victim_error"] = out
        else:
            notes["victim"] = out
    # Player-targeted commands need an online player; the probe connects after
    # setup, so stash them for a second pass. First pass best-effort:
    notes["pre"] = [server.run(c.replace("{name}", name)) for c in setup.get("items", [])]
    if "player_tp" in setup:
        notes["tp_pre"] = server.run(setup["player_tp"].replace("{name}", name))
    server.run("save-all")
    return notes


def solaris_post_setup(server: SolarisServer, spec: dict, name: str) -> dict:
    notes: dict = {}
    setup = spec.get("setup", {})
    notes["items"] = [server.run(c.replace("{name}", name)) for c in setup.get("items", [])]
    if "player_tp" in setup:
        notes["tp"] = server.run(setup["player_tp"].replace("{name}", name))
    # Summon the victim LIVE (post-join): pre-join summons fail on Solaris
    # ("Position is not loaded" — spawn chunks not ready with no player
    # online). Use the ABSOLUTE arena coords (vanilla form); the "~" form
    # broke vanilla (no executor position pre-join). Vanilla keeps its
    # pre-join summon; this live summon only runs here.
    if "victim" in setup:
        notes["victim_live"] = server.run(setup["victim"])
    return notes


# -- comparison ------------------------------------------------------------

def compare(spec: dict, vanilla: dict, solaris: dict) -> dict:
    sid = spec["id"]
    v = vanilla["observations"]
    s = solaris["observations"]
    if sid == "attribute-swap":
        keys = ["damage_no_swap_sword", "damage_swap_wait_stick",
                "damage_same_tick_sword_then_stick", "damage_same_tick_stick_then_sword"]
        diffs = {}
        ok = True
        for k in keys:
            a, b = v.get(k), s.get(k)
            if a is None or b is None:
                diffs[k] = {"vanilla": a, "solaris": b, "note": "missing"}
                ok = False
            elif abs(a - b) > 0.01:
                diffs[k] = {"vanilla": a, "solaris": b, "delta": b - a}
                ok = False
        # Quirk check on the oracle itself: same-tick damage must equal OLD item.
        oracle_quirk = None
        if v.get("damage_no_swap_sword") is not None:
            oracle_quirk = {
                "same_tick_sword_then_stick==sword_full": (
                    v.get("damage_same_tick_sword_then_stick") is not None
                    and abs(v["damage_same_tick_sword_then_stick"] - v["damage_no_swap_sword"]) < 0.01),
            }
        return {"status": "pass" if ok else "fail", "diffs": diffs,
                "oracle_quirk": oracle_quirk,
                "vanilla": v, "solaris": s}
    if sid == "diagonal-speed":
        diffs = {}
        ok = True
        for traj in ["straight", "diagonal", "diagonal_fast10"]:
            vc = (v.get(traj) or {}).get("corrections")
            sc = (s.get(traj) or {}).get("corrections")
            if vc is None or sc is None:
                diffs[traj] = {"vanilla": vc, "solaris": sc, "note": "missing"}
                ok = False
            elif traj in ("straight", "diagonal") and vc == 0 and sc != 0:
                diffs[traj] = {"vanilla": vc, "solaris": sc,
                               "note": "Solaris corrects a trajectory vanilla accepts"}
                ok = False
            elif traj == "diagonal_fast10" and vc is not None and sc is not None and sc < vc:
                diffs[traj] = {"vanilla": vc, "solaris": sc,
                               "note": "Solaris more lenient than vanilla on fast control"}
        return {"status": "pass" if ok else "fail", "diffs": diffs,
                "vanilla": v, "solaris": s}
    return {"status": "fail", "diffs": {"unknown": spec["id"]},
            "vanilla": v, "solaris": s}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True)
    parser.add_argument("--scenario", default=None)
    parser.add_argument("--solaris-bin", default=None)
    parser.add_argument("--probe-bin", default=None)
    args = parser.parse_args(argv)

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    specs = load_scenarios(args.scenario)
    if not specs:
        print("no scenarios matched", file=sys.stderr)
        return 2

    probe_bin = Path(args.probe_bin) if args.probe_bin else (
        REPO_ROOT / "target" / "debug" / "quirk-probe")
    solaris_bin = Path(args.solaris_bin) if args.solaris_bin else (
        REPO_ROOT / "target" / "debug" / "solaris-v2")
    if not probe_bin.is_file():
        print(f"missing probe binary: {probe_bin} (build it first)", file=sys.stderr)
        return 2
    if not solaris_bin.is_file():
        print(f"missing solaris binary: {solaris_bin} (build it first)", file=sys.stderr)
        return 2

    results = []
    overall = "pass"
    logdir = out / "server-logs"
    logdir.mkdir(parents=True, exist_ok=True)
    for spec in specs:
        name = f"Quirk{spec['id'][:6]}"
        print(f"=== {spec['id']}: vanilla ===", flush=True)
        with tempfile.TemporaryDirectory(prefix=f"quirk-vanilla-{spec['id']}-") as tmp:
            vanilla = vanilla_run(spec, Path(tmp), probe_bin, name)
            try:
                shutil.copy(Path(tmp) / "vanilla.log", logdir / f"{spec['id']}.vanilla.log")
            except Exception:
                pass
        (out / f"{spec['id']}.vanilla.json").write_text(json.dumps(vanilla, indent=2))
        print(f"=== {spec['id']}: solaris ===", flush=True)
        with tempfile.TemporaryDirectory(prefix=f"quirk-solaris-{spec['id']}-") as tmp:
            # Two-phase: world prep, then join probe in background? Simpler:
            # start server, run world prep, connect a setup ping via probe is
            # already joined... Instead: start server here manually so we can
            # run player setup AFTER the probe joins. We approximate by
            # running setup twice: once pre-join (world), and the probe
            # scenario itself re-selects slots; player items/tp applied
            # pre-join may miss. Workaround: connect a throwaway probe first
            # to force the player online, run setup, disconnect, then run the
            # real probe (same name rejoins the same profile).
            import threading as _threading
            import time as _time
            port = free_port()
            server = SolarisServer(Path(tmp) / "srv", solaris_bin, port)
            server.start()
            try:
                setup = spec.get("setup", {})
                real = Path(tmp) / f"{spec['id']}.probe.json"

                def _setup_after_ready():
                    # Handshake: probe joined + 20 ClientTickEnd pumped, so the
                    # chunks around the player are loaded - but Solaris spawns the
                    # player on real worldgen far from the vanilla arena at the
                    # origin, and its console refuses edits in unloaded chunks
                    # ("Position is not loaded"). Force-load the arena, retry every
                    # arena command until the server stops reporting an unloaded
                    # position, and teleport the player onto the platform exactly
                    # like vanilla (the console `tp` panic is fixed in the core).
                    notes: dict = {}

                    def run_loaded(command: str) -> list[str]:
                        out: list[str] = []
                        for _attempt in range(8):
                            out = server.run(command)
                            if not any("not loaded" in line for line in out):
                                break
                        return out

                    notes["forceload"] = server.run("forceload add -16 -16 16 16")
                    for part in setup.get("platform", "").split(";"):
                        if part.strip():
                            notes.setdefault("platform", []).append(
                                {part.strip(): run_loaded(part.strip())})
                    notes["items"] = [server.run(c.replace("{name}", name))
                                      for c in setup.get("items", [])]
                    if "victim" in setup:
                        notes["victim"] = run_loaded(setup["victim"])
                    if "player_tp" in setup:
                        notes["player_tp"] = run_loaded(setup["player_tp"].replace("{name}", name))
                    print(f"solaris {spec['id']}: setup applied post-ready", flush=True)
                    return notes

                res = run_probe_handshake(probe_bin, f"127.0.0.1:{port}", name,
                                          spec["id"], real,
                                          on_ready=_setup_after_ready)
                obs = res["obs"]
                solaris = {"observations": obs["observations"],
                           "frame_counts": obs.get("frame_counts", {}),
                           "setup_notes": res.get("setup_notes", {})}
            finally:
                server.stop()
                try:
                    shutil.copy(Path(tmp) / "srv" / "solaris.log",
                                logdir / f"{spec['id']}.solaris.log")
                except Exception:
                    pass
        entry = {"id": spec["id"], **compare(spec, vanilla, solaris)}
        results.append(entry)
        if entry["status"] != "pass":
            overall = "fail"
        print(f"--- {spec['id']}: {entry['status']} "
              f"diffs={json.dumps(entry.get('diffs', {}))}", flush=True)

    receipt = {"status": overall, "scenarios": results}
    (out / "receipt.json").write_text(json.dumps(receipt, indent=2))
    print(f"receipt: {out / 'receipt.json'} status={overall}")
    return 0 if overall == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
