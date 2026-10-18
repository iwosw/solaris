"""Functional native canaries beside real Rust bots; not a perf baseline."""
from __future__ import annotations

import argparse
from dataclasses import replace
import json
import math
import os
from pathlib import Path
import queue
import signal
import subprocess
import threading
import time
from typing import Any

from .. import runtime
from ..mcp import McpClient
from . import native_client
from . import catalog_support as scale
from .client_probes import _server_binary, run_core_subset
from .gameplay import require_tools


def run(ctx: Any) -> dict[str, Any]:
    parser = argparse.ArgumentParser(prog="native-scale")
    parser.add_argument("--native-count", type=int, choices=(1, 5, 10), default=1)
    parser.add_argument("--bots", type=int, choices=(0, 50, 200), default=0)
    parser.add_argument("--warmup-seconds", type=float, default=15)
    parser.add_argument("--measure-seconds", type=float, default=30)
    parser.add_argument("--activity-seconds", type=float, default=15)
    parser.add_argument("--max-rss-mib", type=int, default=4096)
    parser.add_argument("--bots-binary", type=Path, default=Path(os.environ.get(
        "SOLARIS_BOTS_BINARY", str(runtime.REPO_ROOT / "target/release/player-scale-bots"))))
    args = parser.parse_args(ctx.extra_args)
    if ctx.client != "native" or ctx.platform is not None or ctx.plugins_dir is not None:
        raise ValueError("native-scale requires --client native without Loader/plugins")
    if getattr(ctx, "windowed", False):
        raise ValueError("native-scale is headless functional diagnostic only; --windowed is not supported")
    if args.native_count == 10 and args.bots:
        raise ValueError("N=10 is native-only; mixed diagnostics cap N at 5")
    if not all(math.isfinite(n) and n > 0 for n in (args.warmup_seconds, args.measure_seconds, args.activity_seconds)):
        raise ValueError("warmup, measurement and activity durations must be finite and positive")
    if not 512 <= args.max_rss_mib <= 4096:
        raise ValueError("combined RSS guard must be 512..4096 MiB within a 5 GiB scope")
    timeout = 300 if ctx.timeout_seconds is None else ctx.timeout_seconds
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("timeout must be finite and positive")
    if not os.environ.get("SOLARIS_HARNESS_CPU_SCOPE"):
        raise RuntimeError("native-scale must run through the memory-capped harness runner")
    artifact = ctx.artifact_dir
    artifact.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + timeout
    report: dict[str, Any] = {
        "scope": "functional headless native canaries plus protocol-bot diagnostic; not graphical or exclusive perf baseline",
        "exclusive_baseline": False, "native_count": args.native_count, "bots": args.bots,
        "affinity_plan": {"native": [0, 1], "server": [4], "bots": list(range(6, 12))},
        "memory_scope_requested_bytes": 5 * 1024**3,
        "combined_rss_guard_bytes": args.max_rss_mib * 1024**2,
        "launch_reserve_bytes": 400 * 1024**2,
        "workload": {"view_distance": 4, "seed": 81, "spacing": 256, "group_size": 64,
                     "native": "idle received-state interval, then ordinary bounded forward/jump/chat activity; chat echo proves server liveness, pose remains client-reported",
                     "bots": "real Rust protocol clients; creative/teleport operator fixture; movement/break/place/chat"},
        "native_sessions": [], "samples": [], "passed": False,
    }
    ctx.client_evidence.update(scale_report=str(artifact / "native-scale.json"), diagnostic=report)
    processes: dict[str, subprocess.Popen[Any]] = {}
    logs: list[Any] = []
    clients: list[tuple[Any, Any]] = []
    lock = threading.Lock()
    stop = threading.Event()
    events: queue.Queue[tuple[str, Any]] = queue.Queue()
    sampler = reader = None
    clock = None
    phase = "startup"

    def remaining() -> float:
        left = deadline - time.monotonic()
        if left <= 0:
            raise TimeoutError("native-scale overall deadline expired")
        return min(120, left)

    def persist() -> None:
        with lock:
            scale.dump(artifact / "native-scale.json", report)

    def spawn(name: str, argv: list[str], *, pipe: bool = False) -> Any:
        log = (artifact / f"{name}.log").open("wb")
        logs.append(log)
        index = len(ctx.commands)
        ctx.commands.append(argv)
        ctx.cwds.append(str(runtime.REPO_ROOT))
        ctx.exits.append(None)
        ctx.logs.append(str(artifact / f"{name}.log"))
        proc = subprocess.Popen(argv, cwd=runtime.REPO_ROOT, start_new_session=True,
                                stdout=subprocess.PIPE if pipe else log, stderr=log)
        with lock:
            processes[name] = proc
            report.setdefault("processes", {})[name] = {"pid": proc.pid, "command_index": index}
        return proc, log

    def sample() -> dict[str, Any]:
        with lock:
            owned = list(processes.items())
            current_phase = phase
        row: dict[str, Any] = {"monotonic_ns": time.monotonic_ns(), "phase": current_phase, "processes": {}}
        for name, proc in owned:
            measured = scale.proc_sample(proc.pid)
            if proc.poll() is not None or not measured["rss_bytes"]:
                raise RuntimeError(f"{name} exited or metrics disappeared during scale diagnostic")
            measured["affinity"] = sorted(os.sched_getaffinity(proc.pid))
            expected = [4] if name == "server" else list(range(6, 12)) if name == "bots" else [0, 1]
            # taskset may not yet have applied affinity immediately after Popen.
            # The bot ready event is the exec/workload fence; memory stays guarded throughout.
            if measured["affinity"] != expected and (name != "bots" or "bots_ready" in report):
                raise RuntimeError(f"{name} affinity differs from declared plan: {measured['affinity']}")
            row["processes"][name] = measured
        row["combined_rss_bytes"] = sum(p["rss_bytes"] for p in row["processes"].values())
        row["headroom"] = scale.memory_headroom()
        with lock:
            report["samples"].append(row)
        if row["combined_rss_bytes"] > report["combined_rss_guard_bytes"]:
            raise RuntimeError("combined RSS exceeded safe diagnostic guard")
        if any(n is not None and n < 512 * 1024**2 for n in row["headroom"].values()):
            raise RuntimeError("host/cgroup memory headroom below 512 MiB")
        return row

    def check_events() -> None:
        while True:
            try:
                kind, value = events.get_nowait()
            except queue.Empty:
                return
            if kind == "error":
                raise RuntimeError(value)
            if kind == "bot":
                report["latest_bot_event"] = value
                if value.get("event") == "failure" or any(value.get(k, 0) for k in
                    ("disconnects", "action_ack_timeouts", "action_observation_timeouts")):
                    raise RuntimeError(f"protocol bot failure: {value}")
                if value.get("event") == "ready":
                    if value.get("clients") != args.bots:
                        raise RuntimeError("protocol bot ready count mismatch")
                    report["bots_ready"] = value

    def clock_samples() -> None:
        try:
            while not stop.is_set():
                clock.call_tool("minecraft_wait_ticks", {"ticks": 20})
                if stop.is_set():
                    return
                sample()
        except Exception as error:
            if not stop.is_set():
                events.put(("error", f"native-clock sampler: {error}"))
                # Fail closed immediately even if the main thread is in a startup RPC.
                with lock:
                    owned = list(processes.values())
                for proc in owned:
                    try:
                        os.killpg(proc.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass

    def bot_events(proc: Any, log: Any) -> None:
        try:
            for raw in iter(proc.stdout.readline, b""):
                log.write(raw)
                log.flush()
                value = json.loads(raw)
                if not isinstance(value, dict) or "event" not in value:
                    raise RuntimeError("invalid protocol bot event")
                events.put(("bot", value))
            if not stop.is_set():
                events.put(("error", "protocol bot stdout closed unexpectedly"))
        except Exception as error:
            events.put(("error", str(error)))

    def dashboard(label: str) -> dict[str, Any]:
        value = scale.http_json(url + "/profile", remaining())
        scale.dump(artifact / f"{label}-profile.json", value)
        if value.get("players", {}).get("count") != args.native_count + args.bots:
            raise RuntimeError(f"{label} uncached dashboard player count mismatch: {value.get('players')}")
        report[label + "_players"] = value["players"]["count"]
        return value

    def duration(seconds: float) -> tuple[dict[str, Any], dict[str, Any]]:
        start = sample()
        begun = time.monotonic()
        while time.monotonic() - begun < seconds:
            remaining()
            clients[0][1].call_tool("minecraft_wait_ticks", {"ticks": 100})
            check_events()
        return start, sample()

    def summarize(start: dict[str, Any], end: dict[str, Any]) -> dict[str, Any]:
        elapsed = (end["monotonic_ns"] - start["monotonic_ns"]) / 1e9
        result = {"elapsed_seconds": elapsed, "processes": {}}
        with lock:
            rows = [r for r in report["samples"] if start["monotonic_ns"] <= r["monotonic_ns"] <= end["monotonic_ns"]]
        for name, last in end["processes"].items():
            first = start["processes"][name]
            cores = (last["cpu_seconds"] - first["cpu_seconds"]) / elapsed
            result["processes"][name] = {"cpu_cores": cores, "cpu_percent_affinity": 100 * cores / len(last["affinity"]),
                "rss_final_bytes": last["rss_bytes"], "rss_peak_sampled_bytes": max(r["processes"][name]["rss_bytes"] for r in rows),
                "lifetime_hwm_bytes": last["peak_rss_bytes"], "affinity": last["affinity"]}
        natives = [p for n, p in result["processes"].items() if n.startswith("native")]
        result["native_total_cpu_cores"] = sum(p["cpu_cores"] for p in natives)
        result["native_total_rss_bytes"] = sum(p["rss_final_bytes"] for p in natives)
        result["combined_peak_sampled_rss_bytes"] = max(r["combined_rss_bytes"] for r in rows)
        return result

    try:
        server_bin = _server_binary(ctx)
        native_bin = native_client.resolve_binary(ctx)
        report["server_binary"] = {"path": str(server_bin), "sha256": scale.binary_sha256(server_bin)}
        if args.bots:
            if not args.bots_binary.is_file():
                raise RuntimeError(f"missing prebuilt real Rust protocol bots: {args.bots_binary}")
            report["bots_binary"] = {"path": str(args.bots_binary), "sha256": scale.binary_sha256(args.bots_binary)}
        port, dashboard_port = scale.reserve_ports()
        address, url = f"127.0.0.1:{port}", f"http://127.0.0.1:{dashboard_port}"
        config = artifact / "server.toml"
        (artifact / "plugins").mkdir()
        config.write_text(f'''[server]
name = "native-scale-diagnostic"
motd = "isolated native scale diagnostic"
max_players = {args.native_count + args.bots + 1}
view_distance = 4
simulation_distance = 4
[network]
bind_address = "127.0.0.1"
port = {port}
[auth]
online_mode = false
[data]
world_dir = {json.dumps(str(artifact / "world"))}
vanilla_data_dir = {json.dumps(os.environ["SOLARIS_CLIENT_DATA_DIR"])}
seed = 81
[dashboard]
enabled = true
bind_address = "127.0.0.1"
port = {dashboard_port}
allow_remote = false
[plugins]
directory = {json.dumps(str(artifact / "plugins"))}
''')
        server, _ = spawn("server", ["taskset", "-c", "4", str(server_bin), "--config", str(config), "--no-console"])
        runtime.wait_port(port, remaining(), server, log_path=artifact / "server.log")
        original_affinity = os.sched_getaffinity(0)
        try:
            os.sched_setaffinity(0, {0, 1})
            for index in range(args.native_count):
                check_events()
                if processes:
                    row = sample()
                    if row["combined_rss_bytes"] + report["launch_reserve_bytes"] > report["combined_rss_guard_bytes"]:
                        raise RuntimeError("safe 400 MiB native-launch reserve would exceed combined RSS guard")
                    if any(n is not None and n < report["launch_reserve_bytes"] + 512 * 1024**2
                           for n in row["headroom"].values()):
                        raise RuntimeError("insufficient host/cgroup headroom for safe native launch")
                child = replace(ctx, artifact_dir=artifact / f"native-{index}", client_evidence={})
                username = f"NScale{index:02d}"
                launched = time.monotonic_ns()
                proc, log, mcp = native_client.start(child, native_bin, address, username, remaining())
                logs.append(log)
                clients.append((child, mcp))
                with lock:
                    processes[f"native-{index}"] = proc
                    report["processes"][f"native-{index}"] = {"pid": proc.pid, "command_index": child.client_evidence["command_index"]}
                require_tools(mcp.list_tools(), {"minecraft_wait_ticks", "minecraft_wait_for_play", "minecraft_observe",
                    "minecraft_send_chat", "minecraft_wait_for_state_change", "minecraft_press_inputs"}, "native-scale")
                mcp.request_timeout_seconds = 130
                play = mcp.call_tool("minecraft_wait_for_play", {"timeout_seconds": remaining()})
                if not play.get("in_play"):
                    raise RuntimeError("native did not enter Play")
                chat = run_core_subset(mcp, child.artifact_dir, remaining())
                state = mcp.call_tool("minecraft_observe")
                metrics = scale.proc_sample(proc.pid)
                startup_seconds = (time.monotonic_ns() - launched) / 1e9
                session = {"username": username, "identity": state["player"]["entity_uuid"],
                    "instance_id": child.client_evidence["descriptor"]["instance_id"],
                    "startup_elapsed_seconds": startup_seconds, "startup_cpu_seconds": metrics["cpu_seconds"],
                    "startup_average_cpu_cores": metrics["cpu_seconds"] / startup_seconds,
                    "startup_rss_bytes": metrics["rss_bytes"], "startup_hwm_bytes": metrics["peak_rss_bytes"],
                    "chat": chat, "launch_evidence": child.client_evidence}
                report["native_sessions"].append(session)
                sample()
                if index == 0:
                    clock = McpClient(mcp.endpoint, mcp.token, 130)
                    clock.initialize()
                    sampler = threading.Thread(target=clock_samples, daemon=True)
                    sampler.start()
        finally:
            os.sched_setaffinity(0, original_affinity)
        if len({s["identity"] for s in report["native_sessions"]}) != args.native_count or len({s["instance_id"] for s in report["native_sessions"]}) != args.native_count:
            raise RuntimeError("native identities or instances are not distinct")
        if args.bots:
            row = sample()
            if row["combined_rss_bytes"] + report["launch_reserve_bytes"] > report["combined_rss_guard_bytes"]:
                raise RuntimeError("safe protocol-bot launch reserve would exceed combined RSS guard")
            if any(n is not None and n < report["launch_reserve_bytes"] + 512 * 1024**2
                   for n in row["headroom"].values()):
                raise RuntimeError("insufficient host/cgroup headroom for safe protocol-bot launch")
            bot, log = spawn("bots", ["taskset", "-c", "6-11", str(args.bots_binary), "--addr", address,
                "--clients", str(args.bots), "--view-distance", "4", "--spacing", "256", "--group-size", "64",
                "--report", str(artifact / "bots-report.json")], pipe=True)
            reader = threading.Thread(target=bot_events, args=(bot, log), daemon=True)
            reader.start()
            while "bots_ready" not in report:
                kind, value = events.get(timeout=remaining())
                if kind == "error":
                    raise RuntimeError(value)
                events.put((kind, value))
                check_events()
        dashboard("ready")
        phase = "warmup"
        warm_start, warm_end = duration(args.warmup_seconds)
        report["warmup"] = summarize(warm_start, warm_end)
        phase = "steady"
        start, end = duration(args.measure_seconds)
        report["steady"] = summarize(start, end)
        phase = "active"
        active_start = sample()
        active_begun = time.monotonic()
        activity: list[dict[str, Any]] = []
        report["activity_actions"] = activity
        while time.monotonic() - active_begun < args.activity_seconds:
            for index, ((child, mcp), session) in enumerate(zip(clients, report["native_sessions"])):
                remaining()
                before = mcp.call_tool("minecraft_observe")
                action = mcp.call_tool("minecraft_press_inputs", {
                    "keys": ["forward", "jump"], "ticks": 10,
                })
                chat = run_core_subset(mcp, child.artifact_dir / f"active-{len(activity)}", remaining())
                after = mcp.call_tool("minecraft_observe")
                if not after.get("in_play") or after["player"]["entity_uuid"] != session["identity"]:
                    raise RuntimeError("active native session/identity liveness lost")
                activity.append({"client_index": index, "bounded_input": action,
                    "before_player": before["player"], "after_player": after["player"],
                    "server_received_liveness": chat,
                    "pose_authority": "client-reported pose; movement prediction is not an independent server position oracle"})
                check_events()
            # Clock events, not elapsed wall time, drive all progress and sampling.
            clients[0][1].call_tool("minecraft_wait_ticks", {"ticks": 20})
        active_end = sample()
        report["active"] = summarize(active_start, active_end)
        report["idle_active_per_client"] = {
            name: {"idle": report["steady"]["processes"][name],
                   "active": report["active"]["processes"][name]}
            for name in report["steady"]["processes"] if name.startswith("native-")
        }
        dashboard("final")
        for (child, mcp), session in zip(clients, report["native_sessions"]):
            state = mcp.call_tool("minecraft_observe")
            if not state.get("in_play") or state["player"]["entity_uuid"] != session["identity"]:
                raise RuntimeError("native session/identity liveness lost")
            session["final_chat"] = run_core_subset(mcp, child.artifact_dir / "final", remaining())
        check_events()
        stop.set()
        if sampler:
            sampler.join(timeout=10)
            if sampler.is_alive():
                raise RuntimeError("native-clock sampler failed to stop")
        if args.bots:
            bot = processes["bots"]
            os.killpg(bot.pid, signal.SIGINT)
            bot.wait(timeout=30)
            reader.join(timeout=5)
            final = json.loads((artifact / "bots-report.json").read_text())
            report["bot_final_report"] = final
            if bot.returncode != 0 or final.get("success") is not True or any(final.get(k, 0) for k in
                ("disconnects", "action_ack_timeouts", "action_observation_timeouts")):
                raise RuntimeError("final Rust bot report has failure/disconnect/timeout")
        report["passed"] = True
        return {"diagnostic": report, "report": str(artifact / "native-scale.json")}
    except BaseException as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        stop.set()
        if sampler:
            sampler.join(timeout=5)
        for _, mcp in clients:
            try:
                mcp.call_tool("minecraft_disconnect")
            except Exception:
                pass
            try:
                mcp.close()
            except Exception:
                pass
        if clock is not None:
            try:
                clock.close()
            except Exception:
                pass
        for name, proc in reversed(list(processes.items())):
            runtime.stop_process(proc, interrupt=name in {"server", "bots"})
            ctx.exits[report["processes"][name]["command_index"]] = proc.returncode
        if reader:
            reader.join(timeout=5)
        for log in logs:
            log.close()
        persist()
