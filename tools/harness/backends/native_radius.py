"""G5 radius-split full-view gate: stationary native viewer, view 8 / sim 4.

Server residency is the union of the client-view square (view radius 8, +1
margin), the active simulation square (sim radius 4), tickets and dirty pins
(`TickState::desired_chunks`). The server sends
`LoginPlay{view_distance: 8, simulation_distance: 4}` (edge `JoinBurst` now
carries both radii independently) and streams the full 17x17 square around
the stationary player. The shared `_prepare` origin already levels a pad
under the player, so no broad floor fill is needed: every square member is
read at the pad level the player stands on.

The gate asserts all 289 view-square chunks through real client observable
chunk data — one `minecraft_read_block` per chunk at that chunk's own
(cx*16+8, cz*16+8) cell. The native client's world only answers for chunks
it actually holds: `read_state` fails the MCP call with
"client chunk is not loaded at x, y, z" (lifecycle.rs) for unheld chunks,
and `ProbeSteps.call` re-raises that failure, so a missing chunk fails the
gate instead of recording a null. A wire-trace cross-check asserts no
`ForgetLevelChunk` fired for a square member since login.

Client render-distance evidence comes from the native client's
`minecraft_observe.client_settings.render_distance` projection of the live UI
owner. The gate requires the observed value to be 8; a compiled default is
not a substitute for observing the running client. The fixed wire
`ClientInformation` view distance remains 4 and is recorded separately.
"""
from __future__ import annotations

from typing import Any

from .native_v2 import _echo
from .probe_steps import ProbeSteps

SCENARIO_ID = "native-v2-radius-full-view"
VIEW_DISTANCE = 8
SIM_DISTANCE = 4


def _square(cx: int, cz: int, radius: int) -> list[tuple[int, int]]:
    return [(cx + dx, cz + dz) for dz in range(-radius, radius + 1) for dx in range(-radius, radius + 1)]


def _read_chunk_cell(steps: ProbeSteps, cx: int, cz: int, y: int) -> dict[str, Any]:
    """Read the centre cell of exactly this chunk; unheld chunks raise."""
    cell = steps.call("minecraft_read_block", {"x": cx * 16 + 8, "y": y, "z": cz * 16 + 8})
    if not isinstance(cell.get("block_id"), str) or not cell["block_id"]:
        raise RuntimeError(f"client block identity is missing for chunk ({cx}, {cz}): {cell!r}")
    return cell


def run(steps: ProbeSteps, origin: tuple[int, int, int]) -> None:
    ox, oy, oz = origin
    observed = steps.observe()
    settings = observed.get("client_settings", {})
    render_distance = settings.get("render_distance")
    steps.check("client_render_distance_is_8", type(render_distance) is int and render_distance == VIEW_DISTANCE)
    steps.report["radii"] = {"view_distance": VIEW_DISTANCE, "simulation_distance": SIM_DISTANCE,
                             "client_render_distance": render_distance,
                             "client_render_distance_evidence": "minecraft_observe.client_settings.render_distance"}
    steps.report["wire_client_information"] = {
        "view_distance": 4, "source": "solaris-client/src/protocol/network.rs:client_information",
        "note": "fixed wire bytes; server fanout sizes the square from its own view_distance=8",
    }
    cx, cz = ox // 16, oz // 16
    want = _square(cx, cz, VIEW_DISTANCE)
    assert len(want) == (2 * VIEW_DISTANCE + 1) ** 2 == 289
    assert len(set(want)) == len(want), "square cells are unique x/z pairs"
    # Settle: every square member client-readable at its own cell.
    for _ in range(30):
        missing: list[tuple[int, int]] = []
        for index, pos in enumerate(want):
            # Native MCP admits 64 requests/s with a 128-request burst. Refill
            # through live ticks instead of overrunning it with 289 cell reads.
            if index % 32 == 0:
                steps.call("minecraft_wait_ticks", {"ticks": 20})
            try:
                _read_chunk_cell(steps, pos[0], pos[1], oy)
            except Exception as error:
                if "client chunk is not loaded" not in str(error) and "client block is not loaded" not in str(error):
                    raise
                missing.append(pos)
        if not missing:
            break
        steps.call("minecraft_wait_ticks", {"ticks": 20})
    else:
        steps.check("full_17x17_settled", False)
    cells: dict[tuple[int, int], Any] = {}
    unreadable: list[tuple[int, int]] = []
    for index, pos in enumerate(want):
        if index % 32 == 0:
            steps.call("minecraft_wait_ticks", {"ticks": 20})
        try:
            cells[pos] = _read_chunk_cell(steps, pos[0], pos[1], oy)
        except Exception as error:
            cells[pos] = {"error": str(error)}
            unreadable.append(pos)
    steps.report["view_square"] = {"center_chunk": [cx, cz], "radius": VIEW_DISTANCE,
                                   "chunks": len(want), "unique_cells": len(set(want)),
                                   "unreadable": sorted(unreadable),
                                   "sample": cells[want[0]]}
    steps.check("full_17x17_client_readable", not unreadable)
    # Wire cross-check: no forget for a square member since login.
    forgets = steps.client.light_wire.query(
        lambda wire: sorted(tuple(event["chunk"]) for event in wire.events
                             if event["kind"] == "forget" and tuple(event["chunk"]) in set(want)))
    steps.report["wire_forgets_in_square"] = forgets
    steps.check("no_forget_in_view_square", not forgets)
    _echo(steps, "radius_full_view_receipt")
