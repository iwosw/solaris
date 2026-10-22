"""Anti-xray gate: an enclosed ore is absent from the client's world; mining beside it reveals it.

Scenario (native client, seed-81 flat platform, privileged setup via real chat
commands): a 5x3x3 stone block holds two diamond ores, each fully enclosed.
Block updates deliberately carry real states, so the fixture is built and the
player then reconnects: the chunk is encoded by the
server only afterwards, exactly as for a player who walks up to an ore vein.
The client's own loaded world is then read through `minecraft_read_block` /
`minecraft_scan_blocks`: no diamond ore may be in it. Mining the stone beside
the first ore (survival, diamond pickaxe, ordinary client mining) must make the
client receive that ore, while the second, still enclosed, ore stays hidden
until mined into as well.

A final light pass covers the publish-wide chunk-delta/light fast path for a
stationary viewer: a lamp appears (neighbour block light >= 14) and is removed
(back to 0) with no reconnect; the viewer then teleports 160 blocks away until
the client forgets the chunk, the server swaps the lamp for another block/light
(neighbour light exactly 9) while it is away, and on return the client must
receive that block and light.
"""

from __future__ import annotations

import hashlib
import os
import subprocess
from pathlib import Path
from typing import Any

from .. import runtime
from ..profiles import ProfileContext
from . import native_client
from .chisel import _pad_commands, rewrite_config
from .gameplay import peak_rss_kib, prepare_operator_fixture, require_tools
from .probe_steps import ProbeSteps

USERNAME = "AntiXrayGate"
TIMEOUT = 600.0
SEED = 81

STONE = "minecraft:stone"
DIAMOND = "minecraft:diamond_ore"
AIR = "minecraft:air"
GLOWSTONE = "minecraft:glowstone"  # block light 15: the neighbour cell receives 14
CRYING_OBSIDIAN = "minecraft:crying_obsidian"  # block light 10: the neighbour cell receives 9
AWAY_BLOCKS = 160  # x offset of the teleport: 10 chunks, far outside view_distance = 4


REQUIRED_TOOLS = frozenset({
    "minecraft_observe",
    "minecraft_wait_for_play",
    "minecraft_wait_for_state_change",
    "minecraft_wait_for_block_state",
    "minecraft_wait_for_loaded_block",
    "minecraft_read_block",
    "minecraft_scan_blocks",
    "minecraft_send_chat",
    "minecraft_connect",
    "minecraft_disconnect",
    "minecraft_select_hotbar_item",
    "minecraft_look_at_block",
    "minecraft_break_block",
    "minecraft_navigate_to_block",
})


def layout(ox: int, oy: int, oz: int) -> dict[str, Any]:
    """Block coordinates of the fixture around the platform origin (pure helper)."""
    cx, cy, cz = ox + 8, oy + 1, oz
    return {
        "centre": (cx, cy + 1, cz),
        "fill": (cx - 2, cy, cz - 1, cx + 2, cy + 2, cz + 1),
        # Ore A lies behind the west face block, ore B behind the east one.
        "ore_a": (cx - 1, cy + 1, cz),
        "ore_b": (cx + 1, cy + 1, cz),
        "mine_a": (cx - 2, cy + 1, cz),
        "mine_b": (cx + 2, cy + 1, cz),
        "stand_a": (cx - 4, cy, cz),
        "stand_b": (cx + 4, cy, cz),
        # Light pass: a lamp on the pad four blocks from stand_b, probed in the air cell above it.
        "lamp": (cx + 4, cy, cz + 4),
        "lamp_probe": (cx + 4, cy + 1, cz + 4),
    }


def diamond_rows(blocks: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Rows of a block scan that are diamond ore (pure helper)."""
    return [block for block in blocks if block.get("block_id") == DIAMOND]


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.client != "native":
        raise ValueError("anti-xray requires --client native")
    if ctx.extra_args or ctx.platform is not None:
        raise ValueError("anti-xray accepts only --client, --windowed and --timeout-seconds")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else TIMEOUT
    if not 0 < timeout <= 3600:
        raise ValueError("anti-xray --timeout-seconds must be in (0, 3600]")
    artifact = ctx.artifact_dir
    override = os.environ.get("SOLARIS_SERVER_BINARY")
    if not override:
        raise RuntimeError("anti-xray requires SOLARIS_SERVER_BINARY (a solaris-v2 built from this tree)")
    binary = Path(override).expanduser().resolve()
    if not binary.is_file():
        raise RuntimeError(f"missing executable server binary: {binary}")
    server_port = runtime.reserve_port()
    address = f"127.0.0.1:{server_port}"
    config = artifact / "anti-xray.toml"
    (artifact / "plugins").mkdir(exist_ok=True)
    # playable.toml leaves [anti_xray] at its defaults: on for overworld and nether.
    # SOLARIS_ANTI_XRAY_CONTROL=1 switches it off: the gate must then FAIL at
    # hidden_ore_a_is_stone, which proves the assertion discriminates.
    text = rewrite_config(
        (runtime.REPO_ROOT / "playable.toml").read_text(),
        port=server_port,
        world=(artifact / "world").relative_to(runtime.REPO_ROOT).as_posix(),
        plugins=str(artifact / "plugins"),
        seed=SEED,
    )
    if os.environ.get("SOLARIS_ANTI_XRAY_CONTROL") == "1":
        text += "\n[anti_xray]\noverworld = false\nnether = false\n"
    config.write_text(text)
    prepare_operator_fixture(ctx, [USERNAME])
    evidence = ctx.client_evidence
    with binary.open("rb") as server_executable:
        server_hash = hashlib.file_digest(server_executable, "sha256").hexdigest()
    evidence.update(server_address=address, server_binary_path=str(binary),
                    server_binary_sha256=server_hash, world_dir=str(artifact / "world"),
                    seed=SEED)
    server = client_process = xvfb = mcp = None
    server_log = None
    server_log_path = artifact / "server.log"
    try:
        server_log = server_log_path.open("wb")
        env = os.environ.copy()
        env["SOLARIS_HARNESS_LOG_STDOUT"] = "1"
        server = subprocess.Popen([str(binary), "--config", str(config), "--no-console"],
                                  cwd=runtime.REPO_ROOT, stdout=server_log,
                                  stderr=subprocess.STDOUT, start_new_session=True, env=env)
        ctx.logs.append(str(server_log_path))
        runtime.wait_port(server_port, timeout, server, log_path=server_log_path)
        display = None
        if ctx.windowed:
            xvfb, display, _ = runtime.start_xvfb(artifact)
            ctx.logs.append(str(artifact / "xvfb.log"))
        client_process, _client_log, mcp = native_client.start(
            ctx, native_client.resolve_binary(ctx), address, USERNAME, timeout, display=display)
        if ctx.windowed:
            mcp.p4_window = evidence.get("window_focus")
        mcp.request_timeout_seconds = min(timeout + 10, 300.0)
        with ProbeSteps(mcp, artifact / "anti-xray", timeout, "native-anti-xray") as probe:
            require_tools(mcp.list_tools(), REQUIRED_TOOLS, "anti-xray")
            _scenario(probe, ctx, artifact, address)
            if ctx.windowed:
                native_client.capture(mcp, artifact, "anti-xray-final")
        if client_process is not None:
            rss = peak_rss_kib(client_process.pid)
            if rss is not None:
                evidence["peak_client_rss_kib"] = rss
        return {"scenario": probe.report}
    finally:
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        runtime.stop_process(client_process)
        runtime.stop_process(server, interrupt=True)
        runtime.stop_process(xvfb)
        if server_log is not None:
            server_log.close()


def _scenario(probe: ProbeSteps, ctx: ProfileContext, artifact: Path, address: str) -> None:
    probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
    rooted = probe.wait(lambda s: isinstance((s.get("player") or {}).get("x"), float),
                        "player_present")
    ox, oy, oz = (int((rooted["player"] or {})[axis]) for axis in ("x", "y", "z"))
    probe.report["origin_block"] = [ox, oy, oz]
    plan = layout(ox, oy, oz)
    probe.report["layout"] = {name: list(value) for name, value in plan.items()}

    def at(name: str) -> dict[str, int]:
        x, y, z = plan[name]
        return {"x": x, "y": y, "z": z}

    def read(name: str) -> dict[str, Any]:
        return probe.call("minecraft_read_block", at(name))

    def loaded(name: str) -> None:
        probe.call("minecraft_wait_for_loaded_block",
                   {**at(name), "timeout_seconds": probe.remaining()})

    def wait_block(name: str, block_id: str) -> dict[str, Any]:
        matched = probe.call("minecraft_wait_for_block_state", {
            **at(name), "block_id": block_id, "timeout_seconds": probe.remaining()})
        block = matched.get("block") or {}
        probe.check(f"{name}_becomes_{block_id.split(':')[1]}",
                    matched.get("status") == "matched" and block.get("block_id") == block_id)
        return block

    # Privileged fixture (creative): a level pad and the stone block with two
    # fully enclosed diamond ores.
    for command in _pad_commands(ox, oy, oz):
        probe.fixture(command)
    probe.fixture("gamemode creative @s")
    x0, y0, z0, x1, y1, z1 = plan["fill"]
    probe.fixture(f"fill {x0} {y0} {z0} {x1} {y1} {z1} {STONE}")
    for name in ("ore_a", "ore_b"):
        x, y, z = plan[name]
        probe.fixture(f"setblock {x} {y} {z} {DIAMOND}")
    wait_block("ore_b", DIAMOND)  # block updates carry the real state (setup only)

    # Reconnect: the server encodes these chunks for this client from scratch,
    # exactly as for a player who walks up to an ore vein.
    probe.call("minecraft_disconnect")
    probe.check("disconnect_left_play",
                probe.call("minecraft_observe").get("in_play") is False)
    probe.call("minecraft_connect", {"server_addr": address})
    rejoined = probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
    probe.check("reconnect_entered_play", rejoined.get("in_play") is True)
    sx, sy, sz = plan["stand_a"]
    probe.fixture(f"tp @s {sx} {sy} {sz}")
    loaded("ore_a")
    loaded("mine_b")

    # a) The client's loaded world holds no diamond ore.
    probe.report["client_read_ore_a"] = read("ore_a")
    probe.report["client_read_ore_b"] = read("ore_b")
    probe.check("hidden_ore_a_is_stone", read("ore_a").get("block_id") == STONE)
    probe.check("hidden_ore_b_is_stone", read("ore_b").get("block_id") == STONE)
    scan = probe.call("minecraft_scan_blocks", {
        "min_x": plan["fill"][0], "max_x": plan["fill"][3],
        "min_y": plan["fill"][1], "max_y": plan["fill"][4],
        "min_z": plan["fill"][2], "max_z": plan["fill"][5], "max_blocks": 64})
    rows = scan.get("blocks", [])
    probe.report["scan_rows"] = len(rows)
    probe.check("scan_covers_whole_block", len(rows) == 45 and scan.get("next_offset") is None)
    probe.check("client_world_has_no_diamond_ore", not diamond_rows(rows))
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "anti-xray-hidden")

    # b) Ordinary survival mining of the stone beside ore A reveals it; ore B,
    # still enclosed, stays hidden.
    probe.fixture("give @s minecraft:diamond_pickaxe 1")
    probe.call("minecraft_select_hotbar_item", {
        "item_id": "minecraft:diamond_pickaxe", "count": 1, "timeout_seconds": probe.remaining()})
    probe.fixture("gamemode survival @s")

    def mine(name: str, face: str) -> None:
        probe.call("minecraft_look_at_block", {**at(name), "face": face})
        broken = probe.call("minecraft_break_block", {
            **at(name), "face": face,
            "expected_drop_item_id": "minecraft:cobblestone", "expected_drop_count": 0,
            "timeout_seconds": probe.remaining()})
        probe.check(f"{name}_mining_started", broken.get("started") is True)
        wait_block(name, AIR)

    mine("mine_a", "west")
    revealed = wait_block("ore_a", DIAMOND)
    probe.report["revealed_ore_a"] = revealed
    probe.check("mined_beside_ore_reveals_it", revealed.get("block_id") == DIAMOND)
    probe.check("other_enclosed_ore_stays_hidden", read("ore_b").get("block_id") == STONE)
    if ctx.windowed:
        native_client.capture(probe.client, artifact, "anti-xray-revealed")

    # c) Control: ore B was a real diamond ore all along.
    bx, by, bz = plan["stand_b"]
    probe.call("minecraft_navigate_to_block", {"x": bx, "y": by, "z": bz,
                                               "timeout_seconds": probe.remaining()})
    mine("mine_b", "east")
    wait_block("ore_b", DIAMOND)

    _light_pass(probe, plan, at, loaded, wait_block)


def _light_pass(probe: ProbeSteps, plan: dict[str, Any], at: Any, loaded: Any, wait_block: Any) -> None:
    """Stationary viewer: a light source appears and vanishes without reconnect, then the chunk
    leaves the client's view, changes while away, and the returning viewer receives block + light.
    Every wait is bounded by the gate deadline; any timeout fails the gate."""
    lamp, lamp_x, lamp_z = at("lamp"), plan["lamp"][0], plan["lamp"][2]
    details: dict[str, Any] = {"lamp": list(plan["lamp"]), "probe": list(plan["lamp_probe"])}
    probe.report["light_pass"] = details

    def probe_light(accept: Any, label: str) -> int:
        while True:
            state = probe.observe()
            light = probe.call("minecraft_read_block", at("lamp_probe")).get("block_light")
            if isinstance(light, int) and accept(light):
                details[label] = light
                return light
            probe.call("minecraft_wait_for_state_change", {
                "observed_version": state["state_version"], "timeout_seconds": probe.remaining()})

    def chunk_unloaded() -> None:
        while True:
            state = probe.observe()
            try:
                probe.call("minecraft_read_block", lamp)
            except RuntimeError as error:
                if "not loaded" not in str(error):
                    raise
                return
            probe.call("minecraft_wait_for_state_change", {
                "observed_version": state["state_version"], "timeout_seconds": probe.remaining()})

    # a) The viewer stands still (creative: no fall damage in the far teleport) while the lamp
    # appears and is removed; the chunk delta/light update reaches the client each time.
    sx, sy, sz = plan["stand_b"]
    probe.fixture("gamemode creative @s")
    probe.fixture(f"tp @s {sx} {sy} {sz}")
    probe.fixture(f"forceload add {lamp_x} {lamp_z}")  # the server keeps the chunk while the viewer is away
    baseline = probe_light(lambda _: True, "baseline_block_light")
    probe.check("light_baseline_is_dark", baseline == 0)
    probe.fixture(f"setblock {lamp['x']} {lamp['y']} {lamp['z']} {GLOWSTONE}")
    wait_block("lamp", GLOWSTONE)
    lit = probe_light(lambda light: light >= 14, "lit_block_light")
    probe.check("lamp_light_reaches_neighbour", lit >= 14)
    probe.fixture(f"setblock {lamp['x']} {lamp['y']} {lamp['z']} {AIR}")
    wait_block("lamp", AIR)
    probe.check("removed_lamp_light_returns_to_zero", probe_light(lambda light: light == 0, "dark_block_light") == 0)

    # b) Leave the view window without reconnecting; the client forgets the chunk; the lamp's
    # block changes server-side meanwhile; on return the new block and its light arrive.
    probe.fixture(f"tp @s {sx + AWAY_BLOCKS} {sy + 64} {sz}")
    chunk_unloaded()
    probe.check("lamp_chunk_forgotten_by_client", True)
    probe.fixture(f"setblock {lamp['x']} {lamp['y']} {lamp['z']} {CRYING_OBSIDIAN}")
    probe.fixture(f"tp @s {sx} {sy} {sz}")
    loaded("lamp")
    wait_block("lamp", CRYING_OBSIDIAN)
    returned = probe_light(lambda light: light == 9, "returned_block_light")
    probe.check("returned_chunk_has_new_block_light", returned == 9)
    probe.fixture(f"forceload remove {lamp_x} {lamp_z}")
