"""Chisel addon gate: plugin block-tool conversions on the native client.

Scenario (seed-81 flat platform, all privileged setup via real chat commands):
/give `minecraft:stick[minecraft:item_model='chisel:chisel']` is the exact
carrier+model pair core matches for the `chisel:chisel` custom item (see
`custom_for_stack` in crates/mc-data/src/item_components.rs: no other
component state participates), so the give path below grants the real
identity the block-tool rules consult. The gate then right-clicks
(`minecraft_use_item_on`) stone_bricks faces: TOP -> top slab +1 slab,
BOTTOM -> bottom slab +1 slab, SIDE (east) -> stairs facing east, then the
stairs (any face) -> slab. Wear and slab counts are asserted from observed
client state, never from dispatch success.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import Any

from .. import runtime
from ..profiles import ProfileContext
from . import native_client
from .gameplay import peak_rss_kib, prepare_operator_fixture, require_tools
from .probe_steps import ProbeSteps, item_count

USERNAME = "ChiselGate"
TIMEOUT = 600.0

# Privileged fixture: flat survival platform around the player spawn.
PAD_HALF = 20
# Chisel identity: vanilla carrier + model override, the exact pair core
# matches (`custom_for_stack`). Component values use the vanilla wire codec:
# item_model is a bare namespaced id string.
CHISEL_GIVE = (
    "give @s minecraft:stick[minecraft:item_model='chisel:chisel',"
    "minecraft:custom_name='\"Chisel\"'] 1")

# Face codes: minecraft_use_item_on takes client faces (down/up/north/south/
# west/east). Core ToolFace up/down match client up/down exactly; a side rule
# covers all four horizontal faces and $face resolves to the clicked face name.
TOP = "up"
BOTTOM = "down"
SIDE = "east"
ANY = "up"

BRICKS = "minecraft:stone_bricks"
SLAB = "minecraft:stone_brick_slab"
STAIRS = "minecraft:stone_brick_stairs"

REQUIRED_TOOLS = frozenset({
    "minecraft_observe",
    "minecraft_wait_for_play",
    "minecraft_wait_for_state_change",
    "minecraft_wait_for_block_state",
    "minecraft_wait_for_inventory",
    "minecraft_read_block",
    "minecraft_read_item_components",
    "minecraft_select_hotbar_item",
    "minecraft_look_at_block",
    "minecraft_use_item_on",
    "minecraft_send_chat",
    "minecraft_open_inventory",
})


def _package_source() -> Path:
    override = os.environ.get("SOLARIS_CHISEL_PACKAGE")
    if override:
        return Path(override).expanduser().resolve()
    return (runtime.REPO_ROOT.parent / "solaris-default-plugins" / "chisel").resolve()


def _copy_package(source: Path, target: Path) -> dict[str, Any]:
    """Copy the deployed chisel package (plugin.toml, plugin.wasm, client bundle)."""
    if not source.is_dir():
        raise RuntimeError(
            f"chisel package is missing: {source} "
            "(build ../solaris-default-plugins/chisel or set SOLARIS_CHISEL_PACKAGE)"
        )
    missing = [name for name in ("plugin.toml", "plugin.wasm") if not (source / name).is_file()]
    bundle = source / "client" / "chisel-content.zip"
    if not bundle.is_file():
        missing.append("client/chisel-content.zip")
    if missing:
        raise RuntimeError(f"chisel package {source} is missing: {', '.join(missing)}")
    if target.exists():
        shutil.rmtree(target)
    shutil.copytree(source, target)
    return {"package": str(source), "files": sorted(p.name for p in target.iterdir())}


def rewrite_config(source: str, *, port: int, world: str, plugins: str, seed: int) -> str:
    """Rewrite playable.toml tokens for an isolated gate run (pure helper)."""
    replacements = (
        ("port = 25565", f"port = {port}"),
        ('world_dir = ".analysis/test-world-v12"', f'world_dir = "{world}"'),
        ('directory = "plugins"', f'directory = "{plugins}"'),
        ("seed = 0", f"seed = {seed}"),
    )
    for old_tok, new_tok in replacements:
        if source.count(old_tok) != 1:
            raise RuntimeError(f"expected one chisel config token: {old_tok}")
        source = source.replace(old_tok, new_tok, 1)
    return source


def _pad_commands(ox: int, y0: int, oz: int) -> list[str]:
    """Fixture fills that level a grass pad (surface y0) around the origin."""
    x0, x1 = ox - PAD_HALF, ox + PAD_HALF
    z0, z1 = oz - PAD_HALF, oz + PAD_HALF
    return [
        f"fill {x0} {y0 - 3} {z0} {x1} {y0 - 1} {z1} minecraft:dirt",
        f"fill {x0} {y0} {z0} {x1} {y0} {z1} minecraft:grass_block",
        f"fill {x0} {y0 + 1} {z0} {x1} {y0 + 8} {z1} minecraft:air",
    ]


def chisel_damage(inventory: list[dict[str, Any]]) -> int | None:
    """Damage of the selected carrier-stick stack (the chisel model rides a stick)."""
    for stack in inventory:
        if stack.get("item_id") == "minecraft:stick" and stack.get("selected"):
            return int(stack.get("damage", 0))
    return None


def chisel_selected(state: dict[str, Any]) -> dict[str, Any] | None:
    selected = state.get("selected_item") or {}
    if selected.get("item_id") == "minecraft:stick":
        return selected
    for stack in state.get("inventory", []):
        if stack.get("item_id") == "minecraft:stick" and stack.get("selected"):
            return stack
    return None


def block_props(block: dict[str, Any]) -> dict[str, str]:
    props = block.get("properties") or {}
    return {str(k): str(v) for k, v in props.items()} if isinstance(props, dict) else {}


def slab_count(state: dict[str, Any]) -> int:
    return item_count(state, SLAB)


def block_matches(block: dict[str, Any], want_id: str, want_props: dict[str, str]) -> bool:
    """Pure expectation-table predicate: id plus every wanted property matches."""
    if block.get("block_id") != want_id:
        return False
    props = block_props(block)
    return all(props.get(name) == value for name, value in want_props.items())


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.client != "native":
        raise ValueError("chisel requires --client native")
    if ctx.extra_args or ctx.platform is not None:
        raise ValueError("chisel accepts only --client, --windowed and --timeout-seconds")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else TIMEOUT
    if not 0 < timeout <= 3600:
        raise ValueError("chisel --timeout-seconds must be in (0, 3600]")
    artifact = ctx.artifact_dir
    override = os.environ.get("SOLARIS_SERVER_BINARY")
    if not override:
        raise RuntimeError("chisel requires SOLARIS_SERVER_BINARY (a solaris-v2 built from this tree)")
    binary = Path(override).expanduser().resolve()
    if not binary.is_file():
        raise RuntimeError(f"missing executable server binary: {binary}")
    server_port = runtime.reserve_port()
    address = f"127.0.0.1:{server_port}"
    config = artifact / "chisel.toml"
    config.write_text(rewrite_config(
        (runtime.REPO_ROOT / "playable.toml").read_text(),
        port=server_port,
        world=(artifact / "world").relative_to(runtime.REPO_ROOT).as_posix(),
        plugins=str(artifact / "plugins"),
        seed=81,
    ))
    package = _copy_package(_package_source(), artifact / "plugins" / "chisel")
    prepare_operator_fixture(ctx, [USERNAME])
    evidence = ctx.client_evidence
    with binary.open("rb") as server_executable:
        server_hash = hashlib.file_digest(server_executable, "sha256").hexdigest()
    evidence.update(server_address=address, server_binary_path=str(binary),
                    server_binary_sha256=server_hash, world_dir=str(artifact / "world"),
                    seed=81, chisel_package=package)
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
        # The local opt-in a player would configure: chisel's permissions plus the
        # server's built-in native settlements view bundle, which the native client also receives.
        os.environ["SOLARIS_CLIENT_LOADER_PERMISSIONS"] = (
            "present_views,send_view_actions,load_assets,register_items")
        client_process, _client_log, mcp = native_client.start(
            ctx, native_client.resolve_binary(ctx), address, USERNAME, timeout, display=display)
        if ctx.windowed:
            mcp.p4_window = evidence.get("window_focus")
        mcp.request_timeout_seconds = min(timeout + 10, 300.0)
        with ProbeSteps(mcp, artifact / "chisel", timeout, "native-chisel") as probe:
            require_tools(mcp.list_tools(), REQUIRED_TOOLS, "chisel")
            _scenario(probe, ctx, artifact)
            if ctx.windowed:
                native_client.capture(mcp, artifact, "chisel-final")
        if ctx.windowed:
            evidence["capture"] = native_client.capture(mcp, artifact)
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


def _scenario(probe: ProbeSteps, ctx: ProfileContext, artifact: Path) -> None:
    probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
    rooted = probe.wait(lambda s: isinstance((s.get("player") or {}).get("x"), float),
                        "player_present")
    ox, oy, oz = (int((rooted["player"] or {})[axis]) for axis in ("x", "y", "z"))
    probe.report["origin_block"] = [ox, oy, oz]
    for command in _pad_commands(ox, oy, oz):
        probe.fixture(command)
    # Creative for setup (place blocks freely), survival for the chisel
    # itself: creative pays no wear and grants no conserved slab.
    probe.fixture("gamemode creative @s")
    bx, by, bz = ox + 2, oy + 1, oz
    probe.fixture(f"setblock {bx} {by} {bz} {BRICKS}")
    probe.fixture(f"setblock {bx + 1} {by} {bz} {BRICKS}")
    probe.fixture(f"setblock {bx + 2} {by} {bz} {BRICKS}")
    for label, x in (("bricks_placed_a", bx), ("bricks_placed_b", bx + 1),
                     ("bricks_placed_c", bx + 2)):
        matched = probe.call("minecraft_wait_for_block_state", {
            "x": x, "y": by, "z": bz, "block_id": BRICKS,
            "timeout_seconds": probe.remaining()})
        probe.check(label, (matched.get("block") or {}).get("block_id") == BRICKS)
    # The chisel is granted with its exact carrier+model identity: /give with
    # the item_model component patch materializes the admitted custom item
    # (custom_for_stack matches carrier+model only). Crafted-for-real in the
    # 2x2 inventory grid would also work but costs an inventory mini-game the
    # give path avoids; both end at the same observed components assertion.
    probe.fixture(CHISEL_GIVE)
    probe.wait(lambda s: chisel_selected(s) is not None, "chisel_granted")
    probe.fixture("gamemode survival @s")

    def use(x: int, face: str, want_id: str, want_props: dict[str, str],
            shot: str) -> tuple[dict[str, Any], dict[str, Any]]:
        probe.call("minecraft_look_at_block", {"x": x, "y": by, "z": bz, "face": face})
        probe.call("minecraft_select_hotbar_item", {"item_id": "minecraft:stick", "count": 1,
                                                    "timeout_seconds": probe.remaining()})
        probe.call("minecraft_use_item_on",
                   {"x": x, "y": by, "z": bz, "face": face, "hand": "main_hand"})
        args: dict[str, Any] = {"x": x, "y": by, "z": bz, "block_id": want_id,
                                "timeout_seconds": probe.remaining()}
        if want_props:
            args["properties"] = want_props
        # The tool answers {"status", "block": {...}}; callers match the block.
        changed = probe.call("minecraft_wait_for_block_state", args).get("block") or {}
        state = probe.observe()
        if ctx.windowed:
            native_client.capture(probe.client, artifact, shot)
        return changed, state

    opening = probe.observe()
    held, slabs = chisel_selected(opening), slab_count(opening)
    damage = chisel_damage(opening.get("inventory", []))
    probe.check("chisel_starts_undamaged", damage == 0)
    probe.check("slab_starts_empty", slabs == 0)
    probe.check("chisel_selected_is_stick_carrier",
                (held or {}).get("item_id") == "minecraft:stick")
    if ctx.windowed:
        probe.call("minecraft_open_inventory", {})
        native_client.capture(probe.client, artifact, "chisel-inventory")
        probe.call("minecraft_close_screen", {})

    # a) TOP face -> top slab, +1 slab granted, 1 wear.
    top, after_top = use(bx, TOP, SLAB, {"type": "top"}, "chisel-top-slab")
    probe.check("chisel_top_makes_top_slab", block_matches(top, SLAB, {"type": "top"}))
    probe.check("chisel_top_grants_one_slab", slab_count(after_top) == 1)
    probe.check("chisel_top_wears_once", chisel_damage(after_top.get("inventory", [])) == 1)

    # b) BOTTOM face -> bottom slab, +1 slab (2 total), 2 wear.
    bottom, after_bottom = use(bx + 1, BOTTOM, SLAB, {"type": "bottom"}, "chisel-bottom-slab")
    probe.check("chisel_bottom_makes_bottom_slab",
                block_matches(bottom, SLAB, {"type": "bottom"}))
    probe.check("chisel_bottom_grants_second_slab", slab_count(after_bottom) == 2)
    probe.check("chisel_bottom_wears_twice", chisel_damage(after_bottom.get("inventory", [])) == 2)

    # c) SIDE face (east) -> stairs facing east, no grant, 3 wear.
    stairs, after_side = use(bx + 2, SIDE, STAIRS, {"facing": "east"}, "chisel-side-stairs")
    probe.check("chisel_side_makes_stairs", block_matches(stairs, STAIRS, {"facing": "east"}))
    probe.check("chisel_side_grants_nothing", slab_count(after_side) == 2)
    probe.check("chisel_side_wears_thrice", chisel_damage(after_side.get("inventory", [])) == 3)

    # d) stairs (any face) -> slab, no grant, 4 wear.
    slabbed, after_stairs = use(bx + 2, ANY, SLAB, {}, "chisel-stairs-slab")
    probe.check("chisel_stairs_make_slab", slabbed.get("block_id") == SLAB)
    probe.check("chisel_stairs_grant_nothing", slab_count(after_stairs) == 2)
    probe.check("chisel_stairs_wear_four", chisel_damage(after_stairs.get("inventory", [])) == 4)

    components = probe.call("minecraft_read_item_components", {"source": "selected"})
    probe.report["chisel_components"] = components
    got = components.get("components") or {}
    probe.check("chisel_item_model", got.get("minecraft:item_model") == "chisel:chisel")
    probe.check("chisel_custom_name",
                "Chisel" in json.dumps(got.get("minecraft:custom_name", "")))
    probe.report["chisel_identity"] = {
        "selected_item_id": (probe.observe().get("selected_item") or {}).get("item_id"),
        "give": CHISEL_GIVE,
        "path": "give (not crafted: 2x2 craft would need iron_ingot over stick "
                "via container clicks; same observed components either way)",
    }
