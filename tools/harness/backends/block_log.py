"""Block-log gate: grief, `/co rollback`, restart, `/co restore` on the native client.

Scenario (native client, seed-81 flat pad, privileged setup via real chat
commands, operator `BlockLogGate`): two chests are filled through ordinary menu
quick-moves. After a gap the same player griefs: breaks a stone block, places
dirt, quick-moves the diamonds out of one chest and breaks the other (full)
chest. `/co lookup` over exactly the grief window must list four changes and
`/co rollback` must revert them in one tick: the client's own world shows the
stone and chest back, the dirt gone, and both chests' menus show their exact
contents. The server is then restarted on the same world; the history and its
rolled-back marks must survive and `/co restore` must re-apply the grief.
"""

from __future__ import annotations

import hashlib
import math
import os
import subprocess
import time
from pathlib import Path
from typing import Any

from .. import runtime
from ..profiles import ProfileContext
from . import native_client
from .chisel import _pad_commands, rewrite_config
from .gameplay import peak_rss_kib, prepare_operator_fixture, require_tools
from .probe_steps import ProbeSteps

USERNAME = "BlockLogGate"
TIMEOUT = 900.0
SEED = 81
# Grief starts this long after the setup's own (also recorded) chest deposits,
# so a `t:` window can select the grief alone.
GAP_TICKS = 120

AIR = "minecraft:air"
STONE = "minecraft:stone"
DIRT = "minecraft:dirt"
CHEST = "minecraft:chest"
DIAMOND = "minecraft:diamond"
GOLD = "minecraft:gold_ingot"
CHEST_MENU = "net.minecraft.world.inventory.ChestMenu"

REQUIRED_TOOLS = frozenset({
    "minecraft_observe",
    "minecraft_wait_for_play",
    "minecraft_wait_for_state_change",
    "minecraft_wait_for_block_state",
    "minecraft_read_block",
    "minecraft_send_chat",
    "minecraft_select_hotbar_item",
    "minecraft_set_hotbar_slot",
    "minecraft_look_at_block",
    "minecraft_use_item_on",
    "minecraft_break_block",
    "minecraft_quick_move_container_slot",
    "minecraft_close_screen",
    "minecraft_wait_ticks",
    "minecraft_connect",
    "minecraft_disconnect",
})


def layout(ox: int, oy: int, oz: int) -> dict[str, tuple[int, int, int]]:
    """Fixture cells around the player's block (pure helper); all within 4.5 reach."""
    by = oy + 1
    return {
        "stone": (ox + 2, by, oz),
        "dirt": (ox + 3, by, oz),
        "dirt_floor": (ox + 3, oy, oz),
        "chest": (ox + 2, by, oz + 2),
        "full_chest": (ox - 2, by, oz + 2),
    }


def grief_window_seconds(grief_started: float, now: float) -> int:
    """`t:` seconds covering the grief but not the setup GAP before it (pure helper)."""
    return int(now - grief_started) + 2


def chat_has(state: dict[str, Any], text: str) -> bool:
    return any(text in str(line) for line in state.get("recent_chat", []))


def rolled_back_lines(state: dict[str, Any]) -> int:
    """Recent chat lines that list a rolled-back record (pure helper)."""
    return sum("(rolled back)" in str(line) for line in state.get("recent_chat", []))


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.client != "native":
        raise ValueError("block-log requires --client native")
    if ctx.extra_args or ctx.platform is not None:
        raise ValueError("block-log accepts only --client, --windowed and --timeout-seconds")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else TIMEOUT
    if not 0 < timeout <= 3600:
        raise ValueError("block-log --timeout-seconds must be in (0, 3600]")
    artifact = ctx.artifact_dir
    override = os.environ.get("SOLARIS_SERVER_BINARY")
    if not override:
        raise RuntimeError("block-log requires SOLARIS_SERVER_BINARY (a solaris-v2 built from this tree)")
    binary = Path(override).expanduser().resolve()
    if not binary.is_file():
        raise RuntimeError(f"missing executable server binary: {binary}")
    server_port = runtime.reserve_port()
    address = f"127.0.0.1:{server_port}"
    config = artifact / "block-log.toml"
    (artifact / "plugins").mkdir(exist_ok=True)
    # playable.toml leaves [block_log] at its defaults: enabled, 30 days.
    config.write_text(rewrite_config(
        (runtime.REPO_ROOT / "playable.toml").read_text(),
        port=server_port,
        world=(artifact / "world").relative_to(runtime.REPO_ROOT).as_posix(),
        plugins=(artifact / "plugins").as_posix(),
        seed=SEED,
    ))
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
    env = os.environ.copy()
    env["SOLARIS_HARNESS_LOG_STDOUT"] = "1"

    def start_server() -> subprocess.Popen[bytes]:
        process = subprocess.Popen([str(binary), "--config", str(config), "--no-console"],
                                   cwd=runtime.REPO_ROOT, stdout=server_log,
                                   stderr=subprocess.STDOUT, start_new_session=True, env=env)
        runtime.wait_port(server_port, timeout, process, log_path=server_log_path)
        return process

    try:
        server_log = server_log_path.open("wb")
        ctx.logs.append(str(server_log_path))
        server = start_server()
        display = None
        if ctx.windowed:
            xvfb, display, _ = runtime.start_xvfb(artifact)
            ctx.logs.append(str(artifact / "xvfb.log"))
        client_process, _client_log, mcp = native_client.start(
            ctx, native_client.resolve_binary(ctx), address, USERNAME, timeout, display=display)
        if ctx.windowed:
            mcp.p4_window = evidence.get("window_focus")
        mcp.request_timeout_seconds = min(timeout + 10, 300.0)

        def restart() -> None:
            nonlocal server
            runtime.stop_process(server, interrupt=True)
            server = start_server()

        with ProbeSteps(mcp, artifact / "block-log", timeout, "native-block-log") as probe:
            require_tools(mcp.list_tools(), REQUIRED_TOOLS, "block-log")
            _scenario(probe, ctx, artifact, address, restart)
            if ctx.windowed:
                native_client.capture(mcp, artifact, "block-log-final")
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


def _scenario(probe: ProbeSteps, ctx: ProfileContext, artifact: Path, address: str,
              restart: Any) -> None:
    probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
    rooted = probe.wait(lambda s: isinstance((s.get("player") or {}).get("x"), float),
                        "player_present")
    # The block the player stands in: floor, since int() truncates negatives.
    ox, oy, oz = (math.floor((rooted["player"] or {})[axis]) for axis in ("x", "y", "z"))
    probe.report["origin_block"] = [ox, oy, oz]
    plan = layout(ox, oy, oz)
    probe.report["layout"] = {name: list(cell) for name, cell in plan.items()}

    def at(name: str) -> dict[str, int]:
        x, y, z = plan[name]
        return {"x": x, "y": y, "z": z}

    def wait_block(name: str, block_id: str, label: str) -> None:
        # The tool answers {"status", "block": {...}}; match the block itself.
        matched = probe.call("minecraft_wait_for_block_state", {
            **at(name), "block_id": block_id, "timeout_seconds": probe.remaining()})
        probe.check(label, (matched.get("block") or {}).get("block_id") == block_id)

    def shot(name: str) -> None:
        if ctx.windowed:
            native_client.capture(probe.client, artifact, name)

    def command(text: str, label: str, done: Any) -> list[str]:
        """Send one command and wait until `done(state)` sees its whole reply."""
        probe.call("minecraft_send_chat", {"message": text, "command": True})
        state = probe.wait(done, label)
        lines = [str(line) for line in state.get("recent_chat", [])[:12]]
        probe.report.setdefault("chat", {})[label] = lines
        return lines

    def open_chest(name: str, label: str) -> dict[str, Any]:
        probe.call("minecraft_set_hotbar_slot", {"slot": 8})
        probe.call("minecraft_use_item_on", {**at(name), "face": "up", "hand": "main_hand"})
        return probe.wait(lambda s: s["container"]["container_id"] != 0
                          and s["container"]["menu_class"] == CHEST_MENU, label)

    def close_chest(label: str) -> None:
        probe.call("minecraft_close_screen", {})
        probe.wait(lambda s: s["container"]["container_id"] == 0, label)

    def chest_slots(name: str) -> dict[str, list[Any]]:
        """Non-empty chest slots as JSON-ready evidence: {"slot": [item_id, count]}."""
        state = open_chest(name, f"{name}_menu_opened_{len(probe.report['assertions'])}")
        slots = {str(row["slot"]): [str(row["item_id"]), int(row["count"])]
                 for row in state["container"]["slots"]
                 if 0 <= int(row.get("slot", -1)) < 27 and int(row.get("count") or 0) > 0}
        close_chest(f"{name}_menu_closed_{len(probe.report['assertions'])}")
        return slots

    def deposit(name: str, item_id: str) -> None:
        state = open_chest(name, f"{name}_open_for_deposit")
        rows = [row for row in state["container"]["slots"]
                if int(row.get("slot", -1)) >= 27 and row.get("item_id") == item_id]
        probe.check(f"{name}_deposit_stack_in_inventory", bool(rows))
        probe.call("minecraft_quick_move_container_slot", {"slot": int(rows[0]["slot"])})
        probe.wait(lambda s: any(int(row.get("slot", -1)) < 27 and row.get("item_id") == item_id
                                 for row in s["container"]["slots"]),
                   f"{name}_deposit_landed")
        close_chest(f"{name}_closed_after_deposit")

    # Privileged fixture: pad, creative, stone + two chests; chests are filled
    # through ordinary menu quick-moves (recorded, but before the grief window).
    for fixture in _pad_commands(ox, oy, oz):
        probe.fixture(fixture)
    probe.fixture("gamemode creative @s")
    for name, block_id in (("stone", STONE), ("chest", CHEST), ("full_chest", CHEST)):
        x, y, z = plan[name]
        probe.fixture(f"setblock {x} {y} {z} {block_id}")
        wait_block(name, block_id, f"{name}_fixture_placed")
    probe.fixture(f"give @s {DIAMOND} 64")
    probe.fixture(f"give @s {GOLD} 16")
    probe.wait(lambda s: any(stack.get("item_id") == GOLD for stack in s.get("inventory", [])),
               "loot_granted")
    deposit("chest", DIAMOND)
    deposit("full_chest", GOLD)
    before_chest, before_full = chest_slots("chest"), chest_slots("full_chest")
    probe.report["chest_before"] = before_chest
    probe.report["full_chest_before"] = before_full
    probe.check("chest_holds_diamonds", before_chest == {"0": [DIAMOND, 64]})
    probe.check("full_chest_holds_gold", before_full == {"0": [GOLD, 16]})
    probe.fixture(f"give @s {DIRT} 1")
    probe.wait(lambda s: any(stack.get("item_id") == DIRT for stack in s.get("inventory", [])),
               "dirt_granted")
    probe.call("minecraft_wait_ticks", {"ticks": GAP_TICKS})
    shot("block-log-before-grief")

    # Grief by the same player, measured from here on.
    grief_started = time.time()
    probe.report["grief_started_unix"] = grief_started
    probe.call("minecraft_look_at_block", {**at("stone"), "face": "up"})
    probe.call("minecraft_break_block", {
        **at("stone"), "face": "up", "expected_drop_item_id": STONE,
        "expected_drop_count": 0, "timeout_seconds": probe.remaining()})
    wait_block("stone", AIR, "grief_broke_stone")
    probe.call("minecraft_select_hotbar_item", {"item_id": DIRT, "count": 1,
                                                "timeout_seconds": probe.remaining()})
    probe.call("minecraft_use_item_on", {**at("dirt_floor"), "face": "up", "hand": "main_hand"})
    wait_block("dirt", DIRT, "grief_placed_dirt")
    state = open_chest("chest", "grief_opened_chest")
    probe.call("minecraft_quick_move_container_slot", {"slot": 0})
    probe.wait(lambda s: not any(int(row.get("slot", -1)) < 27 and int(row.get("count") or 0) > 0
                                 for row in s["container"]["slots"]), "grief_emptied_chest")
    close_chest("grief_closed_chest")
    # A chest is 14/16 tall: a ray at the full cell's top face passes over it,
    # so aim at the side facing the player (north of the chest).
    probe.call("minecraft_look_at_block", {**at("full_chest"), "face": "north"})
    probe.call("minecraft_break_block", {
        **at("full_chest"), "face": "north", "expected_drop_item_id": CHEST,
        "expected_drop_count": 0, "timeout_seconds": probe.remaining()})
    wait_block("full_chest", AIR, "grief_broke_full_chest")
    shot("block-log-after-grief")

    window = grief_window_seconds(grief_started, time.time())
    probe.report["grief_window_seconds"] = window
    grief_lines = ("4 result(s)", f"{USERNAME} broke stone", f"{USERNAME} placed dirt",
                   f"{USERNAME} took 64 {DIAMOND.split(':')[1]}", f"{USERNAME} broke chest")
    command(f"co lookup u:{USERNAME} t:{window}s", "lookup_lists_grief",
            lambda s: all(chat_has(s, line) for line in grief_lines))
    shot("block-log-lookup")

    window = grief_window_seconds(grief_started, time.time())
    command(f"co rollback u:{USERNAME} t:{window}s", "rollback_reverted_four",
            lambda s: chat_has(s, "Rollback complete: 4 change(s) rolled back, 0 skipped"))
    wait_block("stone", STONE, "rollback_restored_stone")
    wait_block("dirt", AIR, "rollback_removed_dirt")
    wait_block("full_chest", CHEST, "rollback_restored_chest_block")
    after_chest, after_full = chest_slots("chest"), chest_slots("full_chest")
    probe.report["chest_after_rollback"] = after_chest
    probe.report["full_chest_after_rollback"] = after_full
    probe.check("rollback_restored_chest_contents_exactly", after_chest == before_chest)
    probe.check("rollback_restored_broken_chest_contents_exactly", after_full == before_full)
    shot("block-log-after-rollback")

    # Restart on the same world: history and rolled-back marks persist.
    restart()
    probe.call("minecraft_disconnect", {})
    probe.call("minecraft_observe")
    probe.call("minecraft_connect", {"server_addr": address})
    probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
    # No earlier chat line carries "(rolled back)": four of them means the
    # whole post-restart lookup arrived.
    lines = command(f"co lookup u:{USERNAME} t:1h", "restart_lookup_keeps_history",
                    lambda s: chat_has(s, "6 result(s)") and rolled_back_lines(s) == 4)
    probe.check("restart_keeps_rolled_back_marks",
                sum("(rolled back)" in line for line in lines) == 4)
    command(f"co restore u:{USERNAME} t:1h", "restore_reapplied_four",
            lambda s: chat_has(s, "Restore complete: 4 change(s) restored, 0 skipped"))
    wait_block("stone", AIR, "restore_removed_stone")
    wait_block("dirt", DIRT, "restore_placed_dirt")
    wait_block("full_chest", AIR, "restore_broke_chest")
    probe.check("restore_emptied_chest", chest_slots("chest") == {})
    shot("block-log-after-restore")
