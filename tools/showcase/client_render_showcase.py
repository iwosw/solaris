#!/usr/bin/env python3
"""Native-client renderer showcase: curated captures + per-preset benchmark.

One solaris release server (own world, far-terrain LOD on) and one native client per graphics preset, windowed,
pinned to one GPU through VK_ICD_FILENAMES and presented into a headless sway compositor that renders on the
same GPU (the laptop panel is wired to the NVIDIA dGPU, so the Radeon iGPU cannot present to :0).
Everything is driven through the client's MCP (`client_capture`, `minecraft_*`); fixture commands
(/tp, /time, /weather, /gamemode) only place the camera and are not gameplay.

    tools/showcase/setup_headless_compositor.sh                       # once: sway into .analysis/cage-prefix
    python3 tools/showcase/client_render_showcase.py captures         # screenshots, 8 poses x low/medium/high
    python3 tools/showcase/client_render_showcase.py bench            # 60 s frame-time + GPU pass timings
    python3 tools/showcase/client_render_showcase.py bench --gpu nvidia --render-node /dev/dri/renderD129

Frame-time p50/p99 come from the client's own per-frame trace (SOLARIS_CLIENT_FRAME_TRACE, branch
pauline/client-input); pass a client built with it via --client-bin. The client repo is only READ.
"""
from __future__ import annotations

import argparse
import json
import math
import os
import platform
import selectors
import shutil
import statistics
import subprocess
import sys
import time
import uuid
from pathlib import Path

HERE = Path(__file__).resolve()
REPO = HERE.parents[2]  # solaris worktree/checkout: tools.harness lives here
sys.path.insert(0, str(REPO))

WORK_ROOT = Path("/run/media/kaiserroman/games/solaris-work")
DEFAULT_CLIENT = WORK_ROOT / "targets/client-local-release/release/solaris-client"
DEFAULT_SERVER = WORK_ROOT / "targets/shader-show/release/solaris-v2"
CLIENT_REPO = WORK_ROOT / "solaris-client"  # read-only: resources + asset index
VANILLA = Path("/home/kaiserroman/solaris/data/vanilla")
ASSET_ROOT = Path("/home/kaiserroman/.local/share/PrismLauncher/assets")
SEED = 8674276330747006396
USER = "ShaderShow"
GPU_ICD = {"radv": "/usr/share/vulkan/icd.d/radeon_icd.json",
           "nvidia": "/usr/share/vulkan/icd.d/nvidia_icd.json"}
PRESETS = ("low", "medium", "high")


def load_poses(path: Path) -> list[dict]:
    return json.loads(path.read_text())


def _offline(name: str) -> str:
    """Offline-mode player UUID (v3 of "OfflinePlayer:<name>"), used for the fixture op entry."""
    import hashlib
    digest = bytearray(hashlib.md5(("OfflinePlayer:" + name).encode()).digest())
    digest[6] = (digest[6] & 0x0F) | 0x30
    digest[8] = (digest[8] & 0x3F) | 0x80
    return str(uuid.UUID(bytes=bytes(digest)))


def start_sway(width: int, height: int, out: Path, prefix: Path, render_node: str):
    """Headless wlroots compositor (sway) rendering on `render_node`; returns (proc, ':N' of its Xwayland).

    The Radeon iGPU cannot present to this laptop's panel (wired to the NVIDIA dGPU; mutter picks the boot-VGA
    NVIDIA as primary and RADV finds no usable surface format on it), so the client presents into a
    headless sway that runs on the same Radeon through dmabuf: GPU-accelerated, no scanout.
    """
    out.mkdir(parents=True, exist_ok=True)
    fifo = out / "display.fifo"
    if fifo.exists():
        fifo.unlink()
    os.mkfifo(fifo)
    (out / "sway.conf").write_text(
        f"output HEADLESS-1 resolution {width}x{height} position 0 0\n"
        "default_border none\nfocus_follows_mouse no\nxwayland force\n"
        "for_window [all] fullscreen enable\n"
        f"exec sh -c 'echo $DISPLAY > {fifo}'\n")
    env = {k: v for k, v in os.environ.items() if k not in ("DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY")}
    env.update(LD_LIBRARY_PATH=f"{prefix}/root/usr/lib/x86_64-linux-gnu", WLR_BACKENDS="headless",
               WLR_RENDER_DRM_DEVICE=render_node, WLR_RENDERER="gles2",
               SWAYSOCK=str(out / "sway.sock"), XDG_RUNTIME_DIR=f"/run/user/{os.getuid()}")
    proc = subprocess.Popen([f"{prefix}/root/usr/bin/sway", "--unsupported-gpu", "-c", str(out / "sway.conf")],
                            env=env, stdout=(out / "sway.log").open("wb"), stderr=subprocess.STDOUT,
                            stdin=subprocess.DEVNULL, start_new_session=True)
    with open(os.open(fifo, os.O_RDONLY | os.O_NONBLOCK), closefd=True) as r:
        sel = selectors.DefaultSelector()
        sel.register(r, selectors.EVENT_READ)
        # Event wait (60 s deadline, not polling): sway's exec writes $DISPLAY once Xwayland is up.
        display = r.readline().strip() if sel.select(60) else ""
    if not display:
        raise RuntimeError("sway did not publish an Xwayland DISPLAY; see sway.log")
    return proc, display


class Env:
    """Server + per-preset client lifecycle under one work directory."""

    def __init__(self, args):
        from tools.harness import runtime
        from tools.harness.backends import native_client
        from tools.harness.backends.probe_steps import ProbeSteps
        from tools.harness.profiles import ProfileContext
        self.runtime, self.native_client = runtime, native_client
        self.ProbeSteps, self.ProfileContext = ProbeSteps, ProfileContext
        self.args = args
        self.work = Path(args.work).resolve()
        self.work.mkdir(parents=True, exist_ok=True)
        self.server = None
        self.port = None
        # Xwayland grants input focus to a new window asynchronously through the compositor; the harness
        # checks once, right after mapping. Retry the same check until the compositor has focused it.
        from tools.harness.backends import native_focus
        original = native_focus.acquire

        def acquire(display, pid, timeout):
            for _ in range(40):
                try:
                    return original(display, pid, timeout)
                except RuntimeError as error:
                    if "did not grant" not in str(error):
                        raise
                    time.sleep(0.25)
            return original(display, pid, timeout)
        native_focus.acquire = acquire

    # ---- server -------------------------------------------------------
    def start_server(self):
        a = self.args
        world = self.work / "world"
        plugins = self.work / "plugins"
        plugins.mkdir(exist_ok=True)
        self.port = self.runtime.reserve_port()
        toml = f'''[server]
name = "client-showcase"
motd = "renderer showcase"
view_distance = {a.server_view_distance}
simulation_distance = 4

[network]
bind_address = "127.0.0.1"
port = {self.port}

[auth]
online_mode = false

[plugins]
directory = "{plugins}"

[data]
world_dir = "{world}"
vanilla_data_dir = "{VANILLA}"
seed = {a.seed}

[lod]
enabled = true
max_distance = {a.lod_distance}
max_inflight_tiles = 8
workers = {a.lod_workers}
share_terrain_keys = false
'''
        (self.work / "server.toml").write_text(toml)
        (self.work / "ops.json").write_text(json.dumps([{
            "uuid": _offline(USER), "name": USER, "level": 4, "bypassesPlayerLimit": False}]))
        log = (self.work / "server.log").open("ab")
        self.server = subprocess.Popen([str(a.server_bin), "--config", str(self.work / "server.toml"), "--no-console"],
                                       cwd=self.work, stdout=log, stderr=subprocess.STDOUT,
                                       stdin=subprocess.DEVNULL, start_new_session=True)
        self.runtime.wait_port(self.port, 600.0, self.server, log_path=self.work / "server.log")

    def stop_server(self):
        if self.server is not None:
            self.runtime.stop_process(self.server, interrupt=True)
            self.server = None

    # ---- client -------------------------------------------------------
    def client_env(self, settings_path: Path):
        a = self.args
        env = {
            "SOLARIS_CLIENT_SETTINGS": str(settings_path),
            "SOLARIS_CLIENT_ROOT": str(self.work / "client-root"),
            "SOLARIS_CLIENT_DATA_DIR": str(VANILLA),
            "SOLARIS_CLIENT_RESOURCE_ROOT": str(CLIENT_REPO / ".analysis/resources"),
            "SOLARIS_CLIENT_ASSET_ROOT": str(ASSET_ROOT),
            "SOLARIS_CLIENT_ASSET_INDEX": str(CLIENT_REPO / ".analysis/evidence/C00/asset-index-current.json"),
            "SOLARIS_CLIENT_FAR_TERRAIN": str(a.far),
            "SOLARIS_CLIENT_LOADER_PERMISSIONS": "present_views,send_view_actions,load_assets,"
            "register_items,send_world_selection,present_world_previews,play_sounds",
        }
        if a.gpu != "default":
            env["VK_ICD_FILENAMES"] = GPU_ICD[a.gpu]
        else:
            os.environ.pop("VK_ICD_FILENAMES", None)
        return env

    def launch_client(self, preset: str, out: Path, extra_settings: dict | None = None):
        a = self.args
        out.mkdir(parents=True, exist_ok=True)
        (self.work / "client-root").mkdir(exist_ok=True)
        settings = out / "settings.json"
        settings.write_text(json.dumps({
            "render_distance": a.render_distance, "fov": 70., "vsync": False, "max_fps": a.max_fps,
            "fullscreen": False, "ui_scale": 0., "graphics_preset": preset, "far_terrain": a.far,
            **(extra_settings or {})}))
        os.environ.update(self.client_env(settings))
        # `play` takes --width/--height; the harness argv has no hook, so wrap the binary.
        wrapper = out / "client-wrapper.sh"
        wrapper.write_text(f'#!/bin/sh\nexec "{a.client_bin}" "$@" --width {a.width} --height {a.height}\n')
        wrapper.chmod(0o755)
        ctx = self.ProfileContext(artifact_dir=out, windowed=True, client="native")
        display = a.display
        self.sway = None
        if display == "sway":
            self.sway, display = start_sway(a.width, a.height, out, Path(a.compositor_prefix), a.render_node)
        elif not os.environ.get("XAUTHORITY"):
            auth = sorted(Path(f"/run/user/{os.getuid()}").glob(".mutter-Xwaylandauth.*"))
            if auth:
                os.environ["XAUTHORITY"] = str(auth[0])
        client, _, mcp = self.native_client.start(ctx, wrapper, f"127.0.0.1:{self.port}", USER, 240,
                                                   display=display)
        mcp.request_timeout_seconds = 240
        ctx.client_evidence["real_binary"] = str(a.client_bin)
        ctx.client_evidence["display"] = display
        return ctx, client, mcp

    def stop_client(self, ctx, client, mcp, out: Path):
        (out / "client-evidence.json").write_text(json.dumps(ctx.client_evidence, indent=2, default=str))
        try:
            if mcp is not None:
                mcp.close()
        finally:
            self.runtime.stop_process(client)
            if self.sway is not None:
                self.runtime.stop_process(self.sway)
                self.sway = None


class Session:
    """Helpers around one connected client."""

    def __init__(self, env: Env, preset: str, out: Path, ctx, mcp):
        self.env, self.preset, self.out, self.ctx, self.mcp = env, preset, out, ctx, mcp
        self.p = env.ProbeSteps(mcp, out, 6 * 3600, f"showcase-{preset}")
        self.p.__enter__()
        self._seq = 0

    def call(self, name, args=None):
        return self.p.call(name, args)

    def setup(self, hide_hud: bool = False):
        self.call("minecraft_wait_for_play", {"timeout_seconds": 240})
        for c in ("/gamerule advance_time false", "/gamerule advance_weather false",
                  "/gamerule random_tick_speed 0", "/gamerule spawn_mobs false", "/difficulty peaceful",
                  "/gamemode creative"):
            self.p.fixture(c)
        if hide_hud:
            self.hide_hud()

    def hide_hud(self):
        """Options > Interface > Hide HUD through the real screens (the key is session-only, not a setting file key)."""
        scr = "net.minecraft.client.gui.screens."
        self.call("minecraft_open_pause_menu")
        self.call("minecraft_click_screen_button", {"expected_screen_class": scr + "PauseScreen",
                  "expected_title": "Game Menu", "button_label": "Options"})
        opts = {"expected_screen_class": scr + "options.OptionsScreen", "expected_title": "Options"}
        for label in ("Interface", "Hide HUD: Off", "Done"):
            self.call("minecraft_click_screen_button", {**opts, "button_label": label})
        self.call("minecraft_close_screen")

    def place(self, pose: dict):
        p = self.p
        p.fixture(f"/time set {pose.get('time', 6000)}")
        p.fixture("/weather " + pose.get("weather", "clear"))
        p.fixture(f"/gamemode {pose.get('mode', 'creative')}")
        x, y, z = pose["tp"]
        p.fixture(f"/tp @s {x} {y} {z}")
        # A long /tp leaves Play until the destination chunks arrive: wait for it (declared travel boundary).
        p.wait(lambda s: abs(s["player"]["x"] - x) < 0.5 and abs(s["player"]["z"] - z) < 0.5, "tp",
               wait_for_play=True)
        self.aim(pose["yaw"], pose["pitch"])

    def aim(self, yaw, pitch):
        for _ in range(8):
            self.call("minecraft_look", {"yaw_deg": yaw, "pitch_deg": pitch})
            self.call("minecraft_wait_ticks", {"ticks": 6})
            pl = self.p.observe(wait_for_play=True)["player"]
            if abs((pl["yaw"] - yaw + 180) % 360 - 180) < 1.0 and abs(pl["pitch"] - pitch) < 1.0:
                return
        raise RuntimeError("could not aim")

    def settle(self, deadline_s: float):
        """Wait for near geometry + quiet far-terrain streaming (event waits, bounded)."""
        end = time.monotonic() + deadline_s
        last = None
        while True:
            self._seq += 1
            c = self.call("client_capture", {"name": f"settle-{time.time_ns()}"})
            s = self.p.observe()
            far = s.get("far_terrain", {}) or {}
            gen = far.get("generation", {}) or {}
            key = (far.get("tiles_received"), gen.get("tiles_gpu"), gen.get("tiles_cpu"))
            quiet = (far.get("inflight", 0) == 0 and gen.get("pending", 0) == 0 and key == last)
            if c["terrain_extraction"].get("geometry_ready") and (self.env.args.far == 0 or quiet):
                return far
            last = key
            if time.monotonic() >= end:
                return far
            self.call("minecraft_wait_ticks", {"ticks": 40})

    def capture(self, name: str):
        return self.env.native_client.capture(self.mcp, self.out, name)

    def close(self):
        self.p.__exit__(None, None, None)


def adapter_summary(meta: dict) -> dict:
    a = (meta.get("previous_render_metrics") or {}).get("adapter") or {}
    return {k: a.get(k) for k in ("name", "backend", "device_type", "driver", "driver_info",
                                   "timestamp_query_enabled", "multi_draw_indirect_enabled",
                                   "hiz_occlusion_enabled")}


# ---------------------------------------------------------------- captures

def run_captures(env: Env, presets, shots_dir: Path):
    """Every pose per preset, HUD hidden, spectator camera; screenshots land in work/captures-<preset>/screenshots."""
    args = env.args
    poses = load_poses(Path(args.poses))
    rows = []
    for preset in presets:
        out = env.work / f"captures-{preset}"
        ctx, client, mcp = env.launch_client(preset, out)
        s = None
        try:
            s = Session(env, preset, out, ctx, mcp)
            s.setup(hide_hud=True)
            for pose in poses:
                s.place(pose)
                s.call("minecraft_wait_ticks", {"ticks": 255})  # chat feedback fades out
                s.settle(args.settle)
                label = f"{preset}-{pose['label']}"
                best = None
                for k in range(args.repeats):
                    best = s.capture(f"{label}-r{k}")
                dest = shots_dir / preset / f"{pose['label']}.png"
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(best["retained_path"], dest)
                rows.append({"preset": preset, "pose": pose["label"], "screenshot": str(dest),
                             "gpu_total_ms": (best.get("gpu_timestamps") or {}).get("total_ms"),
                             "far": best.get("far_terrain"), "adapter": adapter_summary(best),
                             "width": best.get("width"), "height": best.get("height")})
                (env.work / "captures-index.json").write_text(json.dumps(rows, indent=2))
                print(label, rows[-1]["gpu_total_ms"], flush=True)
        finally:
            if s:
                s.close()
            env.stop_client(ctx, client, mcp, out)
    return rows


# ---------------------------------------------------------------- bench

def pct(xs, q):
    xs = sorted(xs)
    if not xs:
        return None
    return xs[min(len(xs) - 1, max(0, math.ceil(q * len(xs)) - 1))]  # nearest rank


def read_frame_trace(path: Path, f0: int, f1: int):
    """Rows of the client's own per-frame TSV (SOLARIS_CLIENT_FRAME_TRACE) with f0 <= frame_id <= f1."""
    if not path.exists():
        return None
    lines = path.read_text().splitlines()
    header = lines[0].split("\t")
    rows = [dict(zip(header, line.split("\t"))) for line in lines[1:] if line]
    return [r for r in rows if f0 <= int(r["frame_id"]) <= f1]


def dist(xs):
    return {"p50": pct(xs, .5), "p95": pct(xs, .95), "p99": pct(xs, .99), "max": max(xs) if xs else None,
            "mean": round(statistics.fmean(xs), 4) if xs else None, "n": len(xs)}


def run_bench(env: Env, presets, results_path: Path):
    """Fixed view per preset: (A) frame-time window with no captures, (B) GPU pass window with 1 capture/cycle.

    Window clock = game ticks (minecraft_wait_ticks, 20 TPS), never wall-clock sleeps. Frame-time distribution
    comes from the client's own per-frame trace when the binary supports SOLARIS_CLIENT_FRAME_TRACE; otherwise only
    the exact average (frame_id delta over the tick-clocked window) is reported and p50/p99 are marked unavailable.
    """
    args = env.args
    poses = {p["label"]: p for p in load_poses(Path(args.poses))}
    pose = poses[args.bench_pose]
    results = json.loads(results_path.read_text()) if results_path.exists() else {}
    wait_calls = max(1, round(args.seconds * 20 / 240))  # 240 ticks (12 s) per call
    for preset in presets:
        out = env.work / f"bench-{preset}"
        trace = out / "frames.tsv"
        if trace.exists():
            trace.unlink()
        os.environ["SOLARIS_CLIENT_FRAME_TRACE"] = str(trace)
        ctx, client, mcp = env.launch_client(preset, out)
        s = None
        try:
            s = Session(env, preset, out, ctx, mcp)
            s.setup(hide_hud=False)
            s.place(pose)
            s.call("minecraft_wait_ticks", {"ticks": 255})
            s.settle(args.settle)
            s.call("minecraft_wait_ticks", {"ticks": 255})  # warm-up at the fixed view
            load0 = os.getloadavg()
            # (A) frame-time window: only two capture calls, at the window edges
            first = s.capture(f"bench-{preset}-a0")
            f0 = int(first["frame_id"])
            for _ in range(wait_calls):
                s.call("minecraft_wait_ticks", {"ticks": 240})
            last = s.capture(f"bench-{preset}-a1")
            f1 = int(last["frame_id"])
            rows = read_frame_trace(trace, f0 + 1, f1 - 1)
            # (B) GPU pass window
            gpu = {}
            cycles = max(1, round(args.gpu_seconds * 20 / 40))
            for k in range(cycles):
                s.call("minecraft_wait_ticks", {"ticks": 40})
                c = s.capture(f"bench-{preset}-b{k}")
                ts = c.get("gpu_timestamps") or {}
                if ts.get("status") == "measured_completed":
                    gpu[ts["frame_id"]] = (ts["total_ms"], ts["passes_ms"])
            tot = [v[0] for v in gpu.values() if v[0] is not None]
            passes = {}
            for _, pm in gpu.values():
                for k, v in (pm or {}).items():
                    if v is not None:
                        passes.setdefault(k, []).append(v)
            frames = f1 - f0
            res = {
                "adapter": adapter_summary(first), "width": first.get("width"), "height": first.get("height"),
                "present_mode": (first.get("previous_render_metrics") or {}).get("present_mode"),
                "preset": preset, "pose": pose["label"], "window_ticks": wait_calls * 240,
                "frames": frames, "avg_fps_exact": round(frames / (wait_calls * 240 / 20), 1),
                "frame_source": "client_frame_trace" if rows else "unavailable_in_this_binary",
                "gpu_total_ms": dist(tot),
                "gpu_pass_ms": {k: dist(v) for k, v in sorted(passes.items())},
                "host_load_1m_before_after": [load0[0], os.getloadavg()[0]],
                "far_terrain": first.get("far_terrain"),
            }
            if rows:
                iv = [float(r["submitted_frame_interval_ms"]) for r in rows if r["submitted_frame_interval_ms"] not in ("", "null")]
                res["frame_interval_ms"] = dist(iv)
                res["fps_at_p50"] = round(1000 / pct(iv, .5), 1)
                res["fps_1pct_low"] = round(1000 / pct(iv, .99), 1)
            results[preset] = res
            results_path.write_text(json.dumps(results, indent=2))
            print(preset, res["avg_fps_exact"], "fps", res.get("frame_interval_ms"), "gpu", res["gpu_total_ms"]["p50"],
                  flush=True)
        finally:
            os.environ.pop("SOLARIS_CLIENT_FRAME_TRACE", None)
            if s:
                s.close()
            env.stop_client(ctx, client, mcp, out)
    return results


# ---------------------------------------------------------------- host info

def host_info(env: Env) -> dict:
    def sh(cmd):
        try:
            return subprocess.run(cmd, shell=True, capture_output=True, text=True, timeout=20).stdout.strip()
        except Exception as e:  # noqa: BLE001
            return f"unavailable: {e}"
    return {
        "date_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "kernel": platform.release(),
        "cpu": sh("grep -m1 'model name' /proc/cpuinfo | cut -d: -f2"),
        "ram": sh("free -h | awk '/Mem:/{print $2}'"),
        "vulkan": sh("vulkaninfo --summary 2>/dev/null | grep -E 'deviceName|driverInfo|driverName'"),
        "client_sha256": sh(f"sha256sum {env.args.client_bin} | cut -d' ' -f1"),
        "server_sha256": sh(f"sha256sum {env.args.server_bin} | cut -d' ' -f1"),
        "display": sh("xrandr 2>/dev/null | grep -E '\\*' | head -1"),
        "args": {k: str(v) for k, v in vars(env.args).items()},
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("mode", choices=("captures", "bench", "all"))
    ap.add_argument("--preset", nargs="+", default=list(PRESETS), choices=PRESETS)
    ap.add_argument("--work", default=str(REPO / ".analysis/client-showcase"))
    ap.add_argument("--client-bin", type=Path, default=DEFAULT_CLIENT)
    ap.add_argument("--server-bin", type=Path, default=DEFAULT_SERVER)
    ap.add_argument("--display", default="sway",
                    help="'sway' = headless sway on the chosen GPU's render node (default; the Radeon iGPU cannot "
                    "present to this laptop's panel, which is wired to the NVIDIA dGPU), or a real X display "
                    "such as :0 (use with --gpu nvidia|default)")
    ap.add_argument("--compositor-prefix", default=str(REPO / ".analysis/cage-prefix"),
                    help="dir produced by tools/showcase/setup_headless_compositor.sh")
    ap.add_argument("--render-node", default="/dev/dri/renderD128", help="DRM render node of the AMD iGPU")
    ap.add_argument("--gpu", choices=("radv", "nvidia", "default"), default="radv",
                    help="radv = AMD iGPU through VK_ICD_FILENAMES (the weak-hardware target)")
    ap.add_argument("--width", type=int, default=1920)
    ap.add_argument("--height", type=int, default=1080)
    ap.add_argument("--render-distance", type=int, default=12)
    ap.add_argument("--far", type=int, default=2048, help="client far-terrain blocks (0 = off)")
    ap.add_argument("--server-view-distance", type=int, default=12)
    ap.add_argument("--lod-distance", type=int, default=4096)
    ap.add_argument("--lod-workers", type=int, default=2)
    ap.add_argument("--seed", type=int, default=SEED)
    ap.add_argument("--max-fps", type=int, default=0, help="0 = uncapped")
    ap.add_argument("--poses", default=str(HERE.with_name("client_showcase_poses.json")))
    ap.add_argument("--bench-pose", default="vista_noon", help="pose label used for the benchmark view")
    ap.add_argument("--seconds", type=float, default=60, help="frame-time window (game-tick clock)")
    ap.add_argument("--gpu-seconds", type=float, default=40, help="GPU pass window (game-tick clock)")
    ap.add_argument("--settle", type=float, default=300)
    ap.add_argument("--repeats", type=int, default=2)
    ap.add_argument("--shots-out", default=str(WORK_ROOT / "showcase/client"))
    args = ap.parse_args()

    env = Env(args)
    (env.work / "host.json").write_text(json.dumps(host_info(env), indent=2))
    try:
        env.start_server()
        if args.mode in ("captures", "all"):
            rows = run_captures(env, args.preset, Path(args.shots_out))
        if args.mode in ("bench", "all"):
            run_bench(env, args.preset, env.work / "bench.json")
    finally:
        env.stop_server()


if __name__ == "__main__":
    main()
