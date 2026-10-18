"""Real-client chunk-admission lighting gate: a shadowed seam whose second chunk is admitted later.

Fixture (explicit operator commands, never earned gameplay): see `light_seam_model`.

Server residency is the square of `simulation_distance` (4) around each player
(`TickState::desired_chunks`), so the second chunk's admission is separated from
the first by construction, not by wire delivery order. Each staged episode saves,
restarts the server on the same world (nothing resident) and logs the player in
five chunks short of the seam: only the first chunk is wanted, loaded and its
light solved. After the client settles on it, one 16-block teleport makes the
neighbour wanted: it is loaded and lit with the first chunk resident.

* `a-then-b`: first = A (west of the border), second = B;
* `b-then-a`: mirrored, first = B, second = A;
* `near`: teleport beside the seam for the retained close-up (re-reads resident chunks).

Every comparison waits (client tick events, no sleeps) for terrain
`geometry_ready`, zero pending/running sections and a quiet arena light stream,
then checks the client's received light, the server's wire light and the analytic
oracle at every non-solid cell of a 24x7x16 region straddling the border.
"""
from __future__ import annotations

from pathlib import Path
from typing import Any

from . import light_pixels, native_client
from .light_seam_model import REGION_X, REGION_Y, REGION_Z, SeamModel
from .native_v2 import _block, _echo
from .probe_steps import ProbeSteps

SCENARIO_ID = "native-v2-light-seam"
FLOOR_Y = 200
PROBES = {"roof_seam_west": (-1, 1, 5), "roof_seam_east": (0, 1, 5), "roof_edge_west": (-1, 3, 3),
          "roof_edge_east": (0, 3, 3), "torch_roof": (3, 1, 5),
          "tunnel_mouth": (-8, 1, 13), "tunnel_west_seam": (-1, 1, 13), "tunnel_east_seam": (0, 1, 13),
          "tunnel_torch": (4, 1, 13), "tunnel_high_seam": (-1, 2, 13), "open_sky_west": (-1, 6, 5),
          "open_sky_east": (0, 6, 5)}
Chunk = tuple[int, int]


def _geometry(origin: tuple[int, int, int]) -> SeamModel:
    border = (origin[0] // 16 + 9) * 16  # chunk A is the 16 columns west of `border`
    return SeamModel(border, FLOOR_Y, origin[2] // 16 * 16)


def _settle_capture(steps: ProbeSteps, name: str) -> dict[str, Any]:
    """Capture until the client mesher reports geometry_ready with nothing pending or running."""
    shots = steps.artifact_dir.parent / "screenshots"
    attempts: list[Path] = []
    while True:
        sequence = steps.report["captures"] = steps.report.get("captures", 0) + 1  # client artifact names are unique per run
        meta = native_client.capture(steps.client, steps.artifact_dir.parent, f"{name}-try{sequence}")
        attempts.append(shots / f"{name}-try{sequence}")
        terrain = meta["terrain_extraction"]
        if (terrain["geometry_ready"] is True and terrain["pending_sections"] == 0
                and terrain["running_jobs"] == 0):
            for stale in attempts[:-1]:
                stale.with_suffix(".png").unlink(missing_ok=True)
                stale.with_suffix(".json").unlink(missing_ok=True)
            terrain = {key: terrain[key] for key in (
                "geometry_ready", "pending_sections", "running_jobs", "unknown_light_samples",
                "source_sampling_complete", "completed_jobs", "section_descriptor_count")}
            return {"screenshot": meta["retained_path"], "attempts": len(attempts), "terrain": terrain,
                    "frame_id": meta["frame_id"], "camera_position": meta.get("camera_position")}
        steps.call("minecraft_wait_ticks", {"ticks": 10})


def _wire(steps: ProbeSteps, fn: Any) -> Any:
    return steps.client.light_wire.query(fn)


def _chunk_events(steps: ProbeSteps, chunk: Chunk, kind: str, since: int = 0) -> list[int]:
    return _wire(steps, lambda wire: [event["index"] for event in wire.events[since:]
                                       if event["kind"] == kind and tuple(event["chunk"]) == chunk])


def _client_levels(steps: ProbeSteps, model: SeamModel) -> dict[tuple[int, int, int], tuple[int, int]]:
    """Received client light per cell; slabs of 24x1x4 keep each MCP result under its 256 KiB bound."""
    levels: dict[tuple[int, int, int], tuple[int, int]] = {}
    for y in range(model.y0 + REGION_Y[0], model.y0 + REGION_Y[1] + 1):
        for z in range(model.z0 + REGION_Z[0], model.z0 + REGION_Z[1] + 1, 4):
            scan = steps.client.call_tool("minecraft_scan_blocks", {
                "min_x": model.bx + REGION_X[0], "min_y": y, "min_z": z,
                "max_x": model.bx + REGION_X[1], "max_y": y, "max_z": z + 3, "max_blocks": 96})
            levels.update({(row["x"], row["y"], row["z"]): (row["sky_light"], row["block_light"])
                           for row in scan["blocks"]})
    return levels


def _compare(steps: ProbeSteps, model: SeamModel, name: str, **context: Any) -> dict[str, Any]:
    """Settle, then compare client, server-wire and oracle light across the whole border region."""
    row: dict[str, Any] = {"name": name, **context}
    steps.report.setdefault("episodes", []).append(row)
    arena = {(model.bx // 16 + dx, model.z0 // 16) for dx in range(-4, 5)}
    for _ in range(5):
        before = _wire(steps, lambda wire: sum(1 for event in wire.events if tuple(event["chunk"]) in arena))
        row.update(_settle_capture(steps, name))
        client = _client_levels(steps, model)
        if before == _wire(steps, lambda wire: sum(1 for event in wire.events if tuple(event["chunk"]) in arena)):
            break
    else:
        steps.check(f"{name}_arena_light_stream_quiet", False)
    expected = model.expected()
    wire = _wire(steps, lambda state: {cell: state.levels(*cell) for cell in expected})
    mismatches = [{"cell": list(cell), "client": client.get(cell), "server_wire": wire[cell],
                   "oracle": list(want)} for cell, want in expected.items()
                  if client.get(cell) != want or wire[cell] != want]
    probes = {}
    for label, (dx, dy, dz) in PROBES.items():
        cell = (model.bx + dx, model.y0 + dy, model.z0 + dz)
        probes[label] = {"cell": list(cell), "client": client.get(cell), "server_wire": wire.get(cell),
                         "oracle": list(expected[cell])}
    dark = _wire(steps, lambda state: sorted(pos for pos, chunk in state.chunks.items() if chunk.fully_dark()))
    row.update(cells_compared=len(expected), mismatch_count=len(mismatches), mismatches=mismatches[:40],
               probes=probes, fully_dark_chunks=[list(pos) for pos in dark])
    steps.check(f"{name}_geometry_ready_zero_pending", row["terrain"]["geometry_ready"] is True
                and row["terrain"]["pending_sections"] == 0 and row["terrain"]["running_jobs"] == 0)
    steps.check(f"{name}_wire_decoder_clean", not steps.client.light_wire.errors)
    steps.check(f"{name}_no_fully_dark_admitted_chunk", not dark)
    steps.check(f"{name}_client_server_oracle_light_equal", not mismatches)
    return row


def _teleport(steps: ProbeSteps, x: float, y: float, z: float, yaw: int = -90, *, floored: bool = True) -> None:
    steps.fixture(f"tp @s {x} {y} {z} {yaw} 0")
    steps.wait(lambda state: abs(state["player"]["x"] - x) < 1.5 and abs(state["player"]["z"] - z) < 1.5
               and (not floored or abs(state["player"]["y"] - y) < 1.5),
               f"teleported_to_{int(x)}_{int(y)}_{int(z)}", wait_for_play=True)


def _restart(steps: ProbeSteps) -> None:
    identity = steps.observe()["player"]["entity_uuid"]
    steps.fixture("save-all")
    _echo(steps, "save_all_ordered_receipt")
    steps.call("minecraft_disconnect")
    steps.check("restart_client_left_play", steps.call("minecraft_observe")["in_play"] is False)
    proxy = steps.client.light_wire
    steps.report.setdefault("server_restarts", []).append(steps.client.p4_restart_server(steps.remaining()))
    steps.call("minecraft_connect", {"server_addr": proxy.address})
    steps.check("restart_client_entered_play",
                steps.call("minecraft_wait_for_play", {"timeout_seconds": steps.remaining()})["in_play"] is True)
    steps.check("restart_same_player_uuid", steps.observe()["player"]["entity_uuid"] == identity)


def _loaded(steps: ProbeSteps, model: SeamModel, chunk_x: int) -> None:
    """Block until the client holds the chunk with this index (a cell of the arena row)."""
    steps.call("minecraft_wait_for_loaded_block", {
        "x": chunk_x * 16 + 8, "y": model.y0 + 1, "z": model.z0 + 5, "timeout_seconds": steps.remaining()})


def _staged(steps: ProbeSteps, model: SeamModel, name: str, stage_dx: int, final_dx: int,
            first: Chunk, second: Chunk) -> None:
    """Login with only `first` wanted, settle on it, then one teleport makes `second` wanted."""
    stage_x, final_x = model.bx + stage_dx, model.bx + final_dx
    yaw = -90 if stage_dx < 0 else 90
    _teleport(steps, stage_x + .5, model.y0 + 1, model.z0 + 8.5, yaw)
    _restart(steps)
    state = steps.observe()
    steps.check(f"{name}_resumed_at_stage_pad", abs(state["player"]["x"] - (stage_x + .5)) < 3
                and abs(state["player"]["y"] - (model.y0 + 1)) < 3)
    _loaded(steps, model, first[0])
    stage = _settle_capture(steps, f"{name}-stage1")
    first_packets = _chunk_events(steps, first, "chunk")
    second_packets = _chunk_events(steps, second, "chunk")
    steps.check(f"{name}_stage1_only_first_chunk_admitted", bool(first_packets) and not second_packets)
    before = _wire(steps, lambda wire: len(wire.events))
    _teleport(steps, final_x + .5, model.y0 + 1, model.z0 + 8.5, yaw)
    _loaded(steps, model, second[0])
    second_after = _chunk_events(steps, second, "chunk", before)
    steps.check(f"{name}_second_chunk_admitted_after_first_settled",
                bool(second_after) and first_packets[0] < second_after[0])
    _compare(steps, model, name, stage1=stage, first_chunk=list(first), second_chunk=list(second),
             first_chunk_packet_indices=first_packets, second_chunk_packet_indices=second_after,
             admission_proof="server residency = square simulation_distance(4) around the player; the second "
                             "chunk was outside it (never resident since restart) until the teleport that "
                             "followed the first chunk's settled geometry_ready capture")
    # Light updates for the first chunk after the second one arrived (informational).
    steps.report["episodes"][-1]["light_updates_for_first_chunk_after_second_admission"] = len(
        _chunk_events(steps, first, "light", before))


def run(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    model = _geometry(origin)
    bx, y0, z0 = model.bx, model.y0, model.z0
    chunk_a, chunk_b = (bx // 16 - 1, z0 // 16), (bx // 16, z0 // 16)
    pads = (-72, -56, 56, 72)
    steps.report["fixture"] = {"chunk_border_x": bx, "floor_y": y0, "row_z0": z0, "chunk_a": chunk_a,
                               "chunk_b": chunk_b, "commands": model.commands(),
                               "pad_centres_dx": pads}
    steps.fixture("gamemode creative")
    # Pads/arena are placed from two vantage points whose residency square (radius 4) covers them.
    # No floor exists yet, so the creative player may fall: only x/z are fenced.
    _teleport(steps, bx - 8 + .5, y0 + 1, z0 + .5, floored=False)
    for dx in (-72, 55):
        steps.call("minecraft_wait_for_loaded_block", {
            "x": bx + dx, "y": y0, "z": z0 + 8, "timeout_seconds": steps.remaining()})
    for command in (*model.commands(), *(c for dx in (-72, -56, 56) for c in model.pad_commands(bx + dx))):
        steps.fixture(command)
    for point, block in (((bx - 1, y0, z0 + 5), "minecraft:obsidian"), ((bx - 6, y0 + 4, z0 + 5), "minecraft:stone"),
                         ((bx - 8, y0 + 1, z0 + 13), "minecraft:air"), ((bx + 6, y0 + 1, z0 + 13), "minecraft:stone"),
                         ((bx - 72, y0, z0 + 8), "minecraft:obsidian"), ((bx - 56, y0, z0 + 8), "minecraft:obsidian"),
                         ((bx + 55, y0, z0 + 8), "minecraft:obsidian"),
                         *((torch, "minecraft:torch") for torch in model.torches)):
        _block(steps, dict(zip("xyz", point)), block)
    _teleport(steps, bx + 8 + .5, y0 + 1, z0 + .5, floored=False)
    steps.call("minecraft_wait_for_loaded_block", {
        "x": bx + 79, "y": y0, "z": z0 + 8, "timeout_seconds": steps.remaining()})
    for command in model.pad_commands(bx + 72):
        steps.fixture(command)
    _block(steps, {"x": bx + 72, "y": y0, "z": z0 + 8}, "minecraft:obsidian")

    _staged(steps, model, "a-then-b", -72, -56, chunk_a, chunk_b)
    _staged(steps, model, "b-then-a", 72, 56, chunk_b, chunk_a)

    _teleport(steps, bx - 14.5, y0 + 1, z0 + .5, -75)
    steps.call("minecraft_look_at_block", {"x": bx, "y": y0 + 2, "z": z0 + 5, "face": "up"})
    near = _compare(steps, model, "near")
    # Face the flat sunlit north wall of the stone mass from 1.5 blocks: its open neighbours are all sky 15.
    _teleport(steps, bx + .5, y0 + 1, z0 + 9.5, 0)
    wall = _compare(steps, model, "wall")
    wall["pixels"] = light_pixels.dark_tiles(wall["screenshot"])
    steps.check("wall_no_sunlit_opaque_face_darker_than_threshold", wall["pixels"]["passed"])
    steps.report.update(
        limits=["server light is the server's own LevelChunkWithLight/LightUpdate stream (no live light query); "
                "oracle is vanilla propagation over the fixture cells; the near episode re-reads resident chunks"],
        final_screenshot=near["screenshot"])
