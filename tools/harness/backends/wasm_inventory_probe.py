"""Real graphical P3 inventory/storage, market-menu and zone-market acceptance.

Three scenarios share this module because they drive the same fixture through the
same primitives:

- ``wasm-p3-inventory-storage`` is the command-only trade terminal: explicit
  operator setup seeds four emeralds and one diamond pickaxe, and one ordinary
  survival attack then gives that pickaxe a real durability component, so the
  unaffected stack carries a non-default component the bridge can read back.
- ``wasm-p3-inventory-menu`` drives that same fixture mode through the real
  client's menu surface: ``/trade menu`` asks the guest for the server-owned
  ``trade-market`` window, a real primary/secondary click on the window's own
  slots buys or refunds, and a click on the window's close slot or
  ``/trade menu-close`` closes it. The ``P3_MENU`` request markers are ordering
  fences only - the effect is read from the client's own screen, container slots
  and inventory - and the stale probes keep the live next-menu open while the
  guest is asked to close a foreign menu and to act on the session that the
  reconnect left behind.

- ``wasm-p3-zone-market`` drives that fixture's ``zone-market`` mode through real
  boundary crossings: the operator prepares the walking strip with the server's
  own debug corridor builder, whose four end caps form one continuous twelve-block
  stone walkway at y79 across x 5..16 (the zone box starts at y80), and the
  fixture creates the fixed ``trade-zone`` box on top of that. Ordinary walking
  then crosses the box face, and the fixture's own zone entry event opens the
  market on the entering session. The trade clicks reuse the same market and the
  same ledger machine.
  The one crossing that cannot be walked is the exit with the window still open,
  because a container screen swallows ordinary movement input: that crossing is a
  real operator ``/tp`` sent while the window is open followed by the client's own
  position report, and the close that follows is the fixture's own exit-driven
  close rather than a forced one. Every crossing is asserted from the client's own
  position against the declared box, the fixture's own fence marker, and the
  window that opens or closes as its effect.

All three fixtures require every ``/trade`` submission to publish a fresh marker, and
each phase re-reads the client-visible inventory. Chat waits use the client's
versioned state notifications and inventory waits use the bridge's
producer-driven exact-count wait, so no step sleeps or polls.

The reused request id, the remembered first session, the storage revision and
the refusal reasons belong to the guest and are not observable from a graphical
client: this driver asserts only the committed/refused outcome carried by the
marker, the exact client-visible counts, the client-visible window, and that the
unaffected tool's exposed components never drift. It deliberately claims neither
an operation-id receipt nor crash durability.
"""

from __future__ import annotations

import json
import math
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

if __package__:
    from .wasm_plugin_join_hello import HarnessHooks
else:
    # Direct scripts get this path automatically; importlib/runpy file loaders
    # do not. Both entry modes need the same sibling scenario module.
    import sys

    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from wasm_plugin_join_hello import HarnessHooks


WASM_INVENTORY_STORAGE_SCENARIOS = frozenset({"wasm-p3-inventory-storage"})
WASM_INVENTORY_MENU_SCENARIOS = frozenset({"wasm-p3-inventory-menu"})
WASM_ZONE_MARKET_SCENARIOS = frozenset({"wasm-p3-zone-market"})
TRADE_MARKER_PREFIX = "P3_TRADE"
MENU_MARKER_PREFIX = "P3_MENU"
ZONE_MARKER_PREFIX = "P3_ZONE"
TRADE_CYCLE_KIND = "trade"
EXIT_CYCLE_KIND = "exit"
WALK_EXIT_ROUTE = "walk"
TELEPORT_EXIT_ROUTE = "tp"
COMMITTED_OUTCOME = "committed"
REFUSED_OUTCOME = "refused"
# The screen the real client opens for the server-owned generic 9xN menu, so the
# window class a menu phase must see through the bridge.
MENU_SCREEN_CLASS = "net.minecraft.client.gui.screens.inventory.ContainerScreen"
# Every component a client-visible item stack exposes through the bridge's
# `observe` payload. Arbitrary data components and storage revisions are not
# readable from the real client and are reported as observation gaps instead.
FINGERPRINT_FIELDS = (
    "item_id",
    "count",
    "name",
    "damage",
    "max_damage",
    "foil",
    "enchantments",
)
MCP_OBSERVATION_GAPS = (
    "minecraft_observe exposes item_id, count, name, damage, max_damage, foil and "
    "enchantments only; arbitrary data components and plugin storage revisions are "
    "not readable from the real client, so a refused storage precondition is "
    "asserted from the guest's own marker plus an unchanged client inventory.",
    "No core operator command can create a custom-named or enchanted stack, so the "
    "unaffected tool's non-default component is durability damage produced by an "
    "ordinary survival attack through the bridge; a named or enchanted stack would "
    "need a new grant path the graphical surface does not have.",
    "The real client cannot observe the durable world journal; this run proves "
    "live commit/refusal and reconnect persistence, not operation-id receipts "
    "or crash durability.",
)
MCP_MENU_OBSERVATION_GAPS = (
    "The bridge offers primary and secondary container clicks, and one "
    "quick-move action that always sends the window's quick-move input with "
    "button 0. The contract's shift-primary menu click is therefore reachable "
    "(as a quick move) and its shift-secondary click has no bridge action at all; "
    "neither is part of this scenario's acceptance path, and any such click would "
    "be covered by native wire tests instead.",
    "No MCP action can forge a stale wire click or a stale session: the stale "
    "probes drive the guest's own request path, fence it with its marker, and "
    "observe that the live window and inventory are unchanged. Proving refusal at "
    "the wire belongs to the native component tests, not to this graphical driver.",
    "The real client cannot read the plugin's ledger record: the ledger value is "
    "asserted from the marker the guest writes after its own read-back, and the "
    "client-visible counts are asserted separately.",
)
MCP_ZONE_OBSERVATION_GAPS = (
    "This server has no /fill or /setblock operator command, so the platform the "
    "zone crossing stands on is prepared with its own /debug water-corridor "
    "builder: four fixed invocations at y=78 whose z=13 end caps form one "
    "continuous stone walkway at y79 across x 5..16 on the z=12 block row. Each "
    "builder command is accepted only on its own verified-block feedback line and "
    "the whole strip is then read back through the client's own block scan "
    "(solid at y79, air at y80), so a refused or partial build fails the run "
    "instead of moving the crossing to a height the client cannot stand at.",
    "The real client cannot read the server's zone table, the plugin's tracked "
    "session or the zone command's typed answer: a crossing is asserted from the "
    "client-observed position against the box the manifest declares, the guest's "
    "own P3_ZONE fence marker, and the effect that marker precedes - the market "
    "window opening on entry and closing on exit. The zone's Applied answer is "
    "the guest's own precondition for its ready marker, so an unapplied upsert "
    "shows up as a missing marker and fails the phase rather than as a claim.",
    "A container screen swallows ordinary movement input, so the one crossing "
    "that leaves the zone with the market still open is an operator /tp sent "
    "while the window is open: the driver does not close the window itself, and "
    "the closed screen after that crossing is the guest's own exit-driven close. "
    "Every other crossing in this scenario is ordinary walking. The run records "
    "which route each crossing used rather than presenting them as identical.",
    "Ordinary movement is client-authoritative: the bridge walks the real client "
    "and the box is the manifest's own declaration, so a crossing proves the "
    "client position moved across that face plus the guest's transition marker, "
    "not a server-side zone membership read the graphical client cannot make.",
    "The real client cannot observe the durable world journal; this run proves "
    "the live entry/exit transitions with a real purchase and refund and their "
    "persistence across a reconnect, not crash durability or operation-id "
    "receipts.",
)


@dataclass(frozen=True)
class TradeScenarioHooks(HarnessHooks):
    """The join/hello hooks plus the reconnect primitives both trade routes need."""

    leave_play: Callable[..., dict[str, Any]]
    await_session_release: Callable[..., str]
    await_interactive_play: Callable[..., dict[str, Any]]


def require_str(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{label} must be a non-empty string")
    return value


def require_int(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ValueError(f"{label} must be an integer")
    return value


def require_mapping(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be an object")
    return value


def load_inventory_storage_expectation(run_dir: Path, scenario_id: str) -> dict[str, Any]:
    """Read the copied run manifest and return this scenario's declared fixture."""

    manifest = json.loads((run_dir / "manifest.json").read_text(encoding="utf-8"))
    scenarios = [
        entry
        for entry in manifest.get("scenarios", [])
        if isinstance(entry, dict) and entry.get("id") == scenario_id
    ]
    if len(scenarios) != 1:
        raise ValueError(f"{scenario_id} must appear exactly once in the run manifest")
    scenario = scenarios[0]
    if scenario.get("no_debug_commands") is not False:
        raise ValueError(
            f"{scenario_id} seeds its fixture with explicit operator commands and must "
            "declare no_debug_commands false"
        )
    return require_mapping(
        scenario.get("wasm_inventory_storage_expectation"),
        f"{scenario_id} wasm_inventory_storage_expectation",
    )


def load_inventory_menu_expectation(run_dir: Path, scenario_id: str) -> dict[str, Any]:
    """Read the copied run manifest and return this scenario's declared fixture.

    The menu fixture seeds itself with the same explicit operator commands as the
    storage fixture, so it must declare ``no_debug_commands`` false for the same
    reason.
    """

    manifest = json.loads((run_dir / "manifest.json").read_text(encoding="utf-8"))
    scenarios = [
        entry
        for entry in manifest.get("scenarios", [])
        if isinstance(entry, dict) and entry.get("id") == scenario_id
    ]
    if len(scenarios) != 1:
        raise ValueError(f"{scenario_id} must appear exactly once in the run manifest")
    scenario = scenarios[0]
    if scenario.get("no_debug_commands") is not False:
        raise ValueError(
            f"{scenario_id} seeds its fixture with explicit operator commands and must "
            "declare no_debug_commands false"
        )
    return require_mapping(
        scenario.get("wasm_inventory_menu_expectation"),
        f"{scenario_id} wasm_inventory_menu_expectation",
    )


def load_zone_market_expectation(run_dir: Path, scenario_id: str) -> dict[str, Any]:
    """Read the copied run manifest and return this scenario's declared fixture.

    The zone fixture seeds itself with the same explicit operator commands - a
    prepared platform, one teleport to the fixed outside position and the trade
    fixture's own grants - so it must declare ``no_debug_commands`` false.
    """

    manifest = json.loads((run_dir / "manifest.json").read_text(encoding="utf-8"))
    scenarios = [
        entry
        for entry in manifest.get("scenarios", [])
        if isinstance(entry, dict) and entry.get("id") == scenario_id
    ]
    if len(scenarios) != 1:
        raise ValueError(f"{scenario_id} must appear exactly once in the run manifest")
    scenario = scenarios[0]
    if scenario.get("no_debug_commands") is not False:
        raise ValueError(
            f"{scenario_id} seeds its fixture with explicit operator commands and must "
            "declare no_debug_commands false"
        )
    return require_mapping(
        scenario.get("wasm_zone_market_expectation"),
        f"{scenario_id} wasm_zone_market_expectation",
    )


def require_float(value: Any, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{label} must be a number")
    return float(value)


def require_position(value: Any, label: str) -> tuple[float, float, float]:
    if not isinstance(value, list) or len(value) != 3:
        raise ValueError(f"{label} must be a list of three numbers")
    return (
        require_float(value[0], f"{label}[0]"),
        require_float(value[1], f"{label}[1]"),
        require_float(value[2], f"{label}[2]"),
    )


def require_box(
    zone: dict[str, Any], label: str
) -> tuple[tuple[float, float, float], tuple[float, float, float]]:
    minimum = require_position(zone.get("minimum"), f"{label} minimum")
    maximum = require_position(zone.get("maximum"), f"{label} maximum")
    if any(low > high for low, high in zip(minimum, maximum)):
        raise ValueError(f"{label} minimum {minimum} is past its maximum {maximum}")
    return minimum, maximum


def point_within_box(
    position: tuple[float, float, float],
    box: tuple[tuple[float, float, float], tuple[float, float, float]],
) -> bool:
    minimum, maximum = box
    return all(low <= value <= high for value, low, high in zip(position, minimum, maximum))


def crossing_axis(
    outside: tuple[float, float, float], inside: tuple[float, float, float]
) -> tuple[int, int]:
    """The one horizontal axis a boundary crossing travels along.

    The fixture declares two fixed points the walk crosses the box between;
    exactly one horizontal component may differ, because a diagonal walk would
    leave the distance to the crossed face ambiguous.
    """

    dx = inside[0] - outside[0]
    dz = inside[2] - outside[2]
    if (dx == 0.0) == (dz == 0.0):
        raise ValueError(
            f"the zone fixture's inside {inside} and outside {outside} must differ on "
            "exactly one horizontal axis"
        )
    return (1 if dx > 0.0 else -1, 0) if dx != 0.0 else (0, 1 if dz > 0.0 else -1)


def crossing_yaw(axis: tuple[int, int]) -> int:
    """The vanilla yaw that faces the crossing direction."""

    return round(math.degrees(math.atan2(-axis[0], axis[1])))


def crossing_travel(
    outside: tuple[float, float, float],
    axis: tuple[int, int],
    position: tuple[float, float, float],
) -> float:
    """How far the position has moved from the outside point along the axis."""

    return (position[0] - outside[0]) * axis[0] + (position[2] - outside[2]) * axis[1]


def face_travel(
    outside: tuple[float, float, float],
    box: tuple[tuple[float, float, float], tuple[float, float, float]],
    axis: tuple[int, int],
) -> float:
    """How far along the axis the box face nearest the outside point stands."""

    minimum, maximum = box
    if axis[0] != 0:
        face = minimum[0] if axis[0] > 0 else maximum[0]
        return (face - outside[0]) * axis[0]
    face = minimum[2] if axis[1] > 0 else maximum[2]
    return (face - outside[2]) * axis[1]


@dataclass
class ClientProbe:
    """One real client plus the observation primitives both P3 fixtures share.

    Chat waits use the client's versioned state notifications and inventory waits
    use the bridge's producer-driven exact-count wait, so no phase sleeps or polls
    on a fixed interval. Every assertion re-reads client-visible state instead of
    trusting the bridge's own reply.
    """

    client: Any
    transcript: list[dict[str, Any]]
    hooks: TradeScenarioHooks
    scenario_id: str
    run_dir: Path
    screenshots_dir: Path
    timeout_seconds: float

    @property
    def answer_wait_seconds(self) -> float:
        """The budget one marker, inventory sync or attack answer may take.

        Chat, inventory-sync and combat answers arrive within a tick or two; a
        bounded budget keeps a missing answer from hanging the scenario for the
        whole client timeout.
        """

        return min(self.timeout_seconds, 30.0)

    def call(
        self,
        command: str,
        payload: dict[str, Any],
        timeout: float | None = None,
    ) -> dict[str, Any]:
        return self.hooks.call(
            self.client,
            self.transcript,
            command,
            payload,
            self.timeout_seconds if timeout is None else timeout,
        )

    def recent_chat(self, observation: dict[str, Any]) -> list[str]:
        lines = observation.get("recent_chat")
        if not isinstance(lines, list) or not all(isinstance(line, str) for line in lines):
            raise RuntimeError("client observation did not contain string recent_chat entries")
        return lines

    @staticmethod
    def line_counts(lines: list[str]) -> dict[str, int]:
        counts: dict[str, int] = {}
        for line in lines:
            counts[line] = counts.get(line, 0) + 1
        return counts

    def fresh_chat_lines(
        self,
        observation: dict[str, Any],
        baseline: dict[str, int],
    ) -> list[str]:
        counts = self.line_counts(self.recent_chat(observation))
        return [
            line for line, count in counts.items()
            for _ in range(count - baseline.get(line, 0))
        ]

    def marker_baseline(self) -> dict[str, int]:
        """The chat already visible, so a later marker must be new."""

        return self.line_counts(self.recent_chat(self.observe_client()))

    def observe_client(self) -> dict[str, Any]:
        observation = self.call("observe", {})
        if not self.hooks.is_in_play(observation):
            raise RuntimeError(f"client left Play state during {self.scenario_id}")
        return observation

    def await_state(self, observation: dict[str, Any], budget: float) -> None:
        state_version = observation.get("state_version")
        if isinstance(state_version, bool) or not isinstance(state_version, int):
            raise RuntimeError("client observation did not contain integer state_version")
        # The deadline-driven callers can pass a budget below the client's
        # accepted 0.1 s floor; clamp instead of turning an ordinary timeout
        # into a rejected RPC argument.
        event_timeout = max(0.1, budget)
        self.call(
            "wait_state_change",
            {"observed_version": state_version, "timeout_seconds": event_timeout},
            event_timeout + 2.0,
        )

    def submit_chat(self, message: str) -> None:
        self.call("send_chat", {"message": message, "command": True})

    def wait_for_fresh_chat(
        self,
        matcher: Callable[[list[str]], bool],
        description: str,
        baseline: dict[str, int],
    ) -> dict[str, Any]:
        deadline = time.monotonic() + self.answer_wait_seconds
        while True:
            observation = self.observe_client()
            if matcher(self.fresh_chat_lines(observation, baseline)):
                return observation
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(
                    f"{description} was not observed after the command; fresh chat: "
                    f"{self.fresh_chat_lines(observation, baseline)}"
                )
            self.await_state(observation, remaining)

    def wait_for_fresh_marker(
        self,
        marker: str,
        description: str,
        baseline: dict[str, int],
    ) -> dict[str, Any]:
        """Wait for one fresh chat line that begins with ``marker``.

        A marker is an ordering fence for the request that produced it, never an
        acknowledged effect: every caller reads the effect from client state.
        """

        return self.wait_for_fresh_chat(
            lambda fresh: any(line.startswith(marker) for line in fresh),
            description,
            baseline,
        )

    def fresh_prefixed_lines(
        self,
        observation: dict[str, Any],
        baseline: dict[str, int],
        prefix: str,
    ) -> list[str]:
        return [
            line
            for line in self.fresh_chat_lines(observation, baseline)
            if line.startswith(prefix + " ")
        ]

    def require_fresh_prefix_marker(
        self,
        prefix: str,
        expected: str,
        description: str,
        baseline: dict[str, int],
    ) -> dict[str, Any]:
        """Wait for one fence marker and require it is the only fresh one.

        Every marker here fences the request that produced it instead of
        reporting an effect, so an extra fresh line of the same family is an
        event the crossing did not ask for - a duplicated entry or a spurious
        exit - and fails the phase instead of being ignored. The expected line is
        matched by prefix, so a builder that reports what it verified after the
        part a caller fixes stays one line of that family.
        """

        self.wait_for_fresh_marker(expected, description, baseline)
        settled = self.observe_client()
        fresh = self.fresh_prefixed_lines(settled, baseline, prefix)
        if len(fresh) != 1 or not fresh[0].startswith(expected):
            raise RuntimeError(
                f"{description} expected exactly one fresh {prefix} marker beginning with "
                f"{expected!r}, observed {fresh}"
            )
        return settled

    def player_position(self, observation: dict[str, Any]) -> tuple[float, float, float]:
        player = observation.get("player")
        if not isinstance(player, dict):
            raise RuntimeError("client observation did not contain a player object")
        position = (player.get("x"), player.get("y"), player.get("z"))
        if not all(
            isinstance(value, (int, float)) and not isinstance(value, bool) for value in position
        ):
            raise RuntimeError(f"client observation exposed no numeric player position: {position}")
        return (float(position[0]), float(position[1]), float(position[2]))

    def wait_for_position(
        self,
        expected: tuple[float, float, float],
        tolerance: float,
        description: str,
    ) -> dict[str, Any]:
        """Wait for the client to report the position an operator command asked for."""

        deadline = time.monotonic() + self.answer_wait_seconds
        while True:
            observation = self.observe_client()
            position = self.player_position(observation)
            if all(abs(value - want) <= tolerance for value, want in zip(position, expected)):
                return observation
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(
                    f"{description} did not reach {expected} within {tolerance}: {position}"
                )
            self.await_state(observation, remaining)

    def await_crossing(
        self,
        *,
        outside: tuple[float, float, float],
        axis: tuple[int, int],
        threshold: float,
        entered: bool,
        description: str,
    ) -> dict[str, Any]:
        """Wait until the client's own position is past the declared face.

        What is waited on is the position the client itself reports, so a crossing
        is a boundary the server's zone owner can see rather than a claim the
        driver makes on the client's behalf.
        """

        deadline = time.monotonic() + self.answer_wait_seconds
        while True:
            observation = self.observe_client()
            position = self.player_position(observation)
            travel = crossing_travel(outside, axis, position)
            reached = travel >= threshold if entered else travel <= threshold
            if reached:
                return observation
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(
                    f"{description} did not cross the declared face: position {position}, "
                    f"travel {travel:.3f}, threshold {threshold:.3f}"
                )
            self.await_state(observation, remaining)

    def walk_across(
        self,
        *,
        outside: tuple[float, float, float],
        axis: tuple[int, int],
        threshold: float,
        entered: bool,
        facing: tuple[int, int],
        step_ticks: int,
        max_steps: int,
        description: str,
    ) -> tuple[dict[str, Any], int]:
        """Walk the real client across the declared face, one movement step at a time.

        ``axis`` is the measurement axis the threshold is expressed on, while
        ``facing`` is the direction the walk holds forward input in - leaving the
        box walks the opposite way from entering it. Each step holds the client's
        own forward input for real ticks, and the stop condition is re-read from
        the client's own position, so a short step walks again instead of being
        assumed. No step sleeps or polls on a fixed interval.
        """

        self.call("look", {"yaw_deg": crossing_yaw(facing), "pitch_deg": 0})
        observation = self.observe_client()
        position = self.player_position(observation)
        travel = crossing_travel(outside, axis, position)
        for step in range(max_steps + 1):
            reached = travel >= threshold if entered else travel <= threshold
            if reached:
                return observation, step
            if step == max_steps:
                break
            self.call("move_forward", {"ticks": step_ticks})
            observation = self.observe_client()
            position = self.player_position(observation)
            travel = crossing_travel(outside, axis, position)
        raise TimeoutError(
            f"{description} did not cross the declared face in {max_steps} movement steps: "
            f"last position {position}, travel {travel:.3f}, threshold {threshold:.3f}"
        )

    def confirm_position(self, dx_cm: int, dz_cm: int, description: str) -> dict[str, Any]:
        """Send one real client position packet after an operator teleport.

        An operator ``/tp`` commits the server-side pose but does not feed the
        zone observer, which consumes accepted client movement: the position
        packet this action sends is the client reporting where it now stands, and
        it is what turns the teleport into an observed boundary crossing.
        """

        response = self.call("move_by", {"dx_cm": dx_cm, "dz_cm": dz_cm})
        return {
            "dx_cm": dx_cm,
            "dz_cm": dz_cm,
            "bridge_response": response,
            "description": description,
        }

    def require_platform(
        self,
        *,
        position: tuple[float, float, float],
        tolerance: float,
        description: str,
    ) -> dict[str, Any]:
        """Require the teleported client to stand on the prepared platform.

        The operator setup is only real once the client sees it: the landing
        position is read back from the client, and the block the client occupies
        and the block under it are read through the bridge's own block reader at
        the client's own reported position rather than at a declared one.
        """

        landed = self.wait_for_position(position, tolerance, description)
        observed = self.player_position(landed)
        feet = {
            "x": math.floor(observed[0]),
            "y": round(observed[1]),
            "z": math.floor(observed[2]),
        }
        ground = {"x": feet["x"], "y": feet["y"] - 1, "z": feet["z"]}
        blocks: dict[str, Any] = {}
        for name, probe in (("feet", feet), ("ground", ground)):
            self.call(
                "wait_loaded_block",
                {**probe, "timeout_seconds": self.answer_wait_seconds},
                self.answer_wait_seconds + 2.0,
            )
            blocks[name] = self.call("read_block", probe)
        if blocks["ground"].get("is_air") is not False:
            raise RuntimeError(f"{description} found no block under the client: {blocks['ground']}")
        if blocks["feet"].get("is_air") is not True:
            raise RuntimeError(f"{description} left the client inside a block: {blocks['feet']}")
        return {"position": observed, "feet_probe": feet, "ground_probe": ground, "blocks": blocks}

    def inventory_entries(self, observation: dict[str, Any]) -> list[dict[str, Any]]:
        entries = observation.get("inventory")
        if not isinstance(entries, list) or not all(isinstance(entry, dict) for entry in entries):
            raise RuntimeError("client observation did not contain an inventory list")
        return entries

    def inventory_fingerprint(self, observation: dict[str, Any]) -> list[dict[str, Any]]:
        fingerprint: list[dict[str, Any]] = []
        for entry in self.inventory_entries(observation):
            slot = entry.get("slot")
            if isinstance(slot, bool) or not isinstance(slot, int):
                raise RuntimeError("client inventory entry did not expose an integer slot")
            stack = {field: entry.get(field) for field in FINGERPRINT_FIELDS}
            stack["slot"] = slot
            fingerprint.append(stack)
        fingerprint.sort(key=lambda stack: stack["slot"])
        return fingerprint

    def item_count(self, observation: dict[str, Any], item_id: str) -> int:
        total = 0
        for entry in self.inventory_entries(observation):
            if entry.get("item_id") != item_id:
                continue
            count = entry.get("count")
            if isinstance(count, bool) or not isinstance(count, int):
                raise RuntimeError(f"client inventory entry for {item_id} had a non-integer count")
            total += count
        return total

    def tool_fingerprint(self, observation: dict[str, Any], item_id: str) -> dict[str, Any]:
        matching = [
            entry for entry in self.inventory_entries(observation) if entry.get("item_id") == item_id
        ]
        if len(matching) != 1:
            raise RuntimeError(
                f"expected exactly one client-visible {item_id} stack, observed {len(matching)}"
            )
        entry = matching[0]
        fingerprint = {field: entry.get(field) for field in FINGERPRINT_FIELDS}
        count = fingerprint["count"]
        if isinstance(count, bool) or not isinstance(count, int):
            raise RuntimeError(f"client-visible {item_id} stack had a non-integer count")
        damage = fingerprint["damage"]
        if isinstance(damage, bool) or not isinstance(damage, int):
            raise RuntimeError(
                f"client-visible {item_id} stack exposed no integer damage component"
            )
        max_damage = fingerprint["max_damage"]
        if isinstance(max_damage, bool) or not isinstance(max_damage, int) or max_damage <= 0:
            raise RuntimeError(
                f"client-visible {item_id} stack exposed no positive max_damage component"
            )
        return fingerprint

    def await_inventory_counts(self, *counts: tuple[str, int]) -> None:
        for item_id, count in counts:
            self.call(
                "wait_inventory",
                {
                    "item_id": item_id,
                    "count": count,
                    "timeout_seconds": self.answer_wait_seconds,
                },
                self.answer_wait_seconds + 2.0,
            )

    def require_trade_settlement(
        self,
        *,
        label: str,
        expected_marker: str,
        outcome: str,
        expected_counts: tuple[tuple[str, int], ...],
        baseline: dict[str, int],
        before_inventory: list[dict[str, Any]],
        before_tool: dict[str, Any],
        initial_tool: dict[str, Any],
    ) -> tuple[dict[str, Any], dict[str, Any]]:
        """Require one submission's marker, counts and untouched tool.

        The marker is the guest's own report of what its read-back agreed with, so
        the committed/refused outcome is only accepted here together with the
        client-visible counts it implies and a byte-identical unaffected tool.
        """

        self.wait_for_fresh_marker(
            expected_marker,
            f"{label} marker {expected_marker!r}",
            baseline,
        )
        self.await_inventory_counts(*expected_counts)
        settled = self.observe_client()
        fresh_markers = [
            line
            for line in self.fresh_chat_lines(settled, baseline)
            if line.startswith(TRADE_MARKER_PREFIX + " ")
        ]
        if fresh_markers != [expected_marker]:
            raise RuntimeError(
                f"{label} expected exactly one fresh trade marker {expected_marker!r}, "
                f"observed {fresh_markers}"
            )
        observed_counts = {
            item_id: self.item_count(settled, item_id) for item_id, _ in expected_counts
        }
        expected = dict(expected_counts)
        if observed_counts != expected:
            raise RuntimeError(
                f"{label} inventory mismatch: expected {expected}, observed {observed_counts}"
            )
        tool_item_id = before_tool.get("item_id")
        if not isinstance(tool_item_id, str) or not tool_item_id:
            raise RuntimeError(f"{label} tool baseline named no item: {before_tool!r}")
        after_tool = self.tool_fingerprint(settled, tool_item_id)
        if after_tool != before_tool or after_tool != initial_tool:
            raise RuntimeError(
                f"{label} changed the unaffected component-bearing tool: {after_tool}"
            )
        inventory_unchanged = self.inventory_fingerprint(settled) == before_inventory
        if outcome == REFUSED_OUTCOME and not inventory_unchanged:
            raise RuntimeError(f"{label} refused but still changed the client-visible inventory")
        if outcome == COMMITTED_OUTCOME and inventory_unchanged:
            raise RuntimeError(f"{label} committed without changing the client-visible inventory")
        return settled, {
            "marker": expected_marker,
            "fresh_markers": fresh_markers,
            "inventory": observed_counts,
            "inventory_unchanged": inventory_unchanged,
            "tool_fingerprint": after_tool,
            "tool_fingerprint_unchanged": after_tool == before_tool,
        }

    def screen(self, observation: dict[str, Any]) -> dict[str, Any]:
        screen = observation.get("screen")
        if not isinstance(screen, dict):
            raise RuntimeError("client observation did not contain a screen object")
        return screen

    def container(self, observation: dict[str, Any]) -> dict[str, Any]:
        container = observation.get("container")
        if not isinstance(container, dict):
            raise RuntimeError("client observation did not contain a container object")
        return container

    def container_slots(self, observation: dict[str, Any]) -> list[dict[str, Any]]:
        slots = self.container(observation).get("slots")
        if not isinstance(slots, list) or not all(isinstance(entry, dict) for entry in slots):
            raise RuntimeError("client observation did not contain a container slot list")
        return slots

    def menu_buttons(
        self,
        observation: dict[str, Any],
        menu_button_slot_count: int,
    ) -> list[dict[str, Any]]:
        """The window's own slots, i.e. the container slots before the appended
        player inventory. They are fixed buttons the server owns, so their
        contents never move while the window is open."""

        buttons: list[dict[str, Any]] = []
        for entry in self.container_slots(observation):
            slot = entry.get("slot")
            if isinstance(slot, bool) or not isinstance(slot, int):
                raise RuntimeError("client container slot entry did not expose an integer slot")
            if slot >= menu_button_slot_count:
                continue
            stack = {field: entry.get(field) for field in FINGERPRINT_FIELDS}
            stack["slot"] = slot
            buttons.append(stack)
        buttons.sort(key=lambda stack: stack["slot"])
        return buttons

    def wait_for_menu(self, title: str, description: str) -> dict[str, Any]:
        """Wait for the client to show a plain container window with this title."""

        deadline = time.monotonic() + self.answer_wait_seconds
        while True:
            observation = self.observe_client()
            screen = self.screen(observation)
            if screen.get("open") is True and screen.get("title") == title:
                return observation
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(
                    f"{description} did not open on the real client: screen is {screen!r}"
                )
            self.await_state(observation, remaining)

    def wait_for_closed_screen(self, description: str) -> dict[str, Any]:
        """Wait for the client to hold no server-owned window at all.

        The vanilla client answers a server container close by dropping to its own
        inventory menu, so the container id must be back to zero as well.
        """

        deadline = time.monotonic() + self.answer_wait_seconds
        while True:
            observation = self.observe_client()
            screen = self.screen(observation)
            if screen.get("open") is not True:
                container_id = self.container(observation).get("container_id")
                if container_id != 0:
                    raise RuntimeError(
                        f"{description} left container {container_id!r} without a screen"
                    )
                return observation
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(f"{description} did not close on the real client: {screen!r}")
            self.await_state(observation, remaining)

    def require_menu(
        self,
        observation: dict[str, Any],
        menu: dict[str, Any],
        menu_button_slot_count: int,
        container_slot_count: int,
        label: str,
    ) -> dict[str, Any]:
        """Require the client to show one declared window, button for button."""

        screen = self.screen(observation)
        title = require_str(menu["title"], f"{label} title")
        if screen.get("open") is not True:
            raise RuntimeError(f"{label} has no screen open: {screen!r}")
        if screen.get("class") != MENU_SCREEN_CLASS:
            raise RuntimeError(
                f"{label} opened {screen.get('class')!r} instead of a plain container screen"
            )
        if screen.get("title") != title:
            raise RuntimeError(
                f"{label} window title is {screen.get('title')!r}, expected {title!r}"
            )
        container = self.container(observation)
        container_id = container.get("container_id")
        if isinstance(container_id, bool) or not isinstance(container_id, int) or container_id == 0:
            raise RuntimeError(f"{label} exposed no server-owned container id: {container!r}")
        if container.get("slot_count") != container_slot_count:
            raise RuntimeError(
                f"{label} window declares {container.get('slot_count')!r} slots, expected "
                f"{container_slot_count}: the declared buttons plus the appended inventory"
            )
        declared = menu.get("slots")
        if not isinstance(declared, list) or not declared:
            raise ValueError(f"{label} declares no menu slots")
        buttons = {stack["slot"]: stack for stack in self.menu_buttons(observation, menu_button_slot_count)}
        declared_slots = sorted(require_int(slot["slot"], f"{label} slot index") for slot in declared)
        if sorted(buttons) != declared_slots:
            raise RuntimeError(
                f"{label} shows buttons {sorted(buttons)} on the client, expected {declared_slots}"
            )
        for slot in declared:
            index = require_int(slot["slot"], f"{label} slot index")
            stack = buttons[index]
            for field, key in (("item_id", "item_id"), ("count", "count"), ("name", "label")):
                # The window's label reaches the client as the stack's own name.
                if stack.get(field) != slot.get(key):
                    raise RuntimeError(
                        f"{label} button {index} {field} is {stack.get(field)!r}, expected "
                        f"{slot.get(key)!r}"
                    )
        return {
            "container_id": container_id,
            "screen_class": screen.get("class"),
            "title": screen.get("title"),
            "slot_count": container.get("slot_count"),
            "buttons": [buttons[index] for index in declared_slots],
        }

    def open_menu(
        self,
        command_root: str,
        menu: dict[str, Any],
        description: str,
    ) -> tuple[dict[str, Any], dict[str, Any]]:
        """Request one menu and wait for its window to appear.

        The returned record states what the request was: the marker the guest
        published is a fence for the request, and the window that follows is the
        effect this method waits for.
        """

        action = require_str(menu["open_action"], f"{description} open action")
        fence = require_str(menu["open_fence"], f"{description} open fence")
        command = f"{command_root} {action}"
        baseline = self.marker_baseline()
        self.submit_chat(command)
        self.wait_for_fresh_marker(fence, f"{description} request marker {fence!r}", baseline)
        observation = self.wait_for_menu(
            require_str(menu["title"], f"{description} title"),
            f"{description} window",
        )
        return observation, {
            "action": action,
            "command": "/" + command,
            "request_marker": fence,
            "request_only": "the marker fences the request; the window is the effect",
        }

    def request_close(
        self,
        command_root: str,
        action: str,
        fence: str,
        description: str,
    ) -> tuple[dict[str, Any], dict[str, Any]]:
        """Send one close request and wait for the client to hold no window."""

        command = f"{command_root} {action}"
        baseline = self.marker_baseline()
        self.submit_chat(command)
        self.wait_for_fresh_marker(fence, f"{description} request marker {fence!r}", baseline)
        closed = self.wait_for_closed_screen(f"{description} window close")
        return closed, {
            "action": action,
            "command": "/" + command,
            "request_marker": fence,
            "request_only": "the marker fences the request; the closed window is the effect",
        }

    def click_container_slot(
        self,
        slot: int,
        button: str,
        description: str,
    ) -> dict[str, Any]:
        """Every fixture click closes its window and must be confirmed."""

        response = self.call("click_container_slot", {"slot": slot, "button": button})
        if response.get("confirmed") is not True:
            raise RuntimeError(f"{description} was not confirmed: {response!r}")
        return {
            "slot": slot,
            "button": button,
            "confirmed": True,
            "bridge_response": response,
        }

    def capture_screen(self, name: str) -> str:
        return self.hooks.capture_screenshot(
            self.client,
            self.transcript,
            self.run_dir,
            self.screenshots_dir,
            f"{self.scenario_id}-{name}",
            self.timeout_seconds,
        )

    def capture_inventory_screenshot(self, name: str) -> str:
        self.call("open_inventory", {})
        path = self.capture_screen(name)
        self.call("close_screen", {})
        state = self.call("state", {})
        # Closing the vanilla inventory is immediate; a bounded wait keeps a
        # stuck screen from hanging the whole scenario for the client timeout.
        self.hooks.await_interactive_play(
            self.client,
            self.transcript,
            state,
            min(self.timeout_seconds, 15.0),
        )
        return path

    def reconnect(self, server_addr: str, player_username: str) -> dict[str, Any]:
        """Disconnect, wait for the server to release the session, and reconnect."""

        self.call("disconnect", {})
        self.hooks.leave_play(self.client, self.transcript, self.timeout_seconds)
        session_release = self.hooks.await_session_release(self.run_dir, self.timeout_seconds)
        self.call("connect", {"server_addr": server_addr})
        play = self.call("wait_play", {"timeout_seconds": self.timeout_seconds})
        play = self.hooks.await_interactive_play(
            self.client,
            self.transcript,
            play,
            self.timeout_seconds,
        )
        if not self.hooks.is_in_play(play):
            raise RuntimeError("the real client did not reach Play state after reconnect")
        observation = self.observe_client()
        player = observation.get("player")
        if not isinstance(player, dict) or player.get("name") != player_username:
            raise RuntimeError(
                "the rejoined client identity changed: "
                f"{player.get('name') if isinstance(player, dict) else player!r}"
            )
        return {"session_release": session_release, "observation": observation}

    def run_setup_command(self, command: str, setup_commands: list[dict[str, Any]]) -> None:
        self.submit_chat(command)
        setup_commands.append({"command": "/" + command})

    def run_confirmed_setup_command(
        self,
        command: str,
        prefix: str,
        feedback: str,
        setup_commands: list[dict[str, Any]],
    ) -> dict[str, Any]:
        """Run one operator setup command and require its own feedback line.

        The server's debug builders answer with what they verified, so the
        confirmation is the server's own read-back of the blocks it wrote rather
        than an assumption that the command applied. Exactly one fresh line of
        that family is required, so a second report cannot be read as this
        command's.
        """

        setup_commands.append({"command": "/" + command})
        baseline = self.marker_baseline()
        self.submit_chat(command)
        settled = self.require_fresh_prefix_marker(
            prefix,
            feedback,
            f"operator setup {command!r} feedback",
            baseline,
        )
        return {
            "command": "/" + command,
            "feedback": feedback,
            "observed": [
                line
                for line in self.fresh_prefixed_lines(settled, baseline, prefix)
            ],
        }

    def require_walkway(
        self,
        *,
        top_y: int,
        surface_y: int,
        walkway_z: int,
        x_min: int,
        x_max: int,
        description: str,
    ) -> dict[str, Any]:
        """Require the client to see the prepared walkway, block for block.

        The operator setup is only real once the client sees it: the whole
        crossing strip is read through the bridge's own block scan, and every
        column is required to be solid where the walk happens and air where the
        player stands. A missing or refused setup command therefore fails here
        instead of showing up as a fall in the middle of a crossing.
        """

        self.call(
            "wait_loaded_block",
            {"x": x_min, "y": top_y, "z": walkway_z, "timeout_seconds": self.answer_wait_seconds},
            self.answer_wait_seconds + 2.0,
        )
        scan = self.call(
            "scan_blocks",
            {
                "min_x": x_min,
                "min_y": top_y,
                "min_z": walkway_z,
                "max_x": x_max,
                "max_y": surface_y,
                "max_z": walkway_z,
                "max_blocks": 512,
            },
        )
        blocks = scan.get("blocks")
        if not isinstance(blocks, list) or not blocks:
            raise RuntimeError(f"{description} block scan returned no blocks: {scan!r}")
        columns: dict[int, dict[str, Any]] = {}
        for block in blocks:
            if not isinstance(block, dict):
                raise RuntimeError(f"{description} block scan returned a non-object entry")
            x = require_int(block.get("x"), f"{description} block x")
            y = require_int(block.get("y"), f"{description} block y")
            columns.setdefault(x, {})[str(y)] = block
        expected_x = list(range(x_min, x_max + 1))
        if sorted(columns) != expected_x:
            raise RuntimeError(
                f"{description} block scan covered {sorted(columns)}, expected {expected_x}"
            )
        for x in expected_x:
            column = columns[x]
            solid = column.get(str(top_y))
            surface = column.get(str(surface_y))
            if solid is None or surface is None:
                raise RuntimeError(f"{description} column {x} is missing a layer: {column!r}")
            if solid.get("is_air") is not False:
                raise RuntimeError(f"{description} column {x} has no block to walk on: {solid!r}")
            if surface.get("is_air") is not True:
                raise RuntimeError(f"{description} column {x} stands the player inside a block: {surface!r}")
        return {
            "x": expected_x,
            "walk_y": top_y,
            "surface_y": surface_y,
            "z": walkway_z,
            "blocks": [
                {
                    "x": x,
                    "walk_block": columns[x][str(top_y)].get("block_id"),
                    "surface_block": columns[x][str(surface_y)].get("block_id"),
                }
                for x in expected_x
            ],
        }

    def seed_trade_fixture(
        self,
        setup: dict[str, Any],
        emerald: dict[str, Any],
        apple: dict[str, Any],
        tool: dict[str, Any],
    ) -> dict[str, Any]:
        """Seed the fixture with explicit operator commands and one real attack.

        The commands are declared debug setup (``no_debug_commands`` false) and are
        never claimed as no-debug survival. The attack exists so the unaffected
        stack carries a non-default, client-readable component before any trade
        runs.
        """

        emerald_item_id = require_str(emerald["item_id"], "emerald item id")
        apple_item_id = require_str(apple["item_id"], "apple item id")
        tool_item_id = require_str(tool["item_id"], "tool item id")
        damage_entity_type = require_str(setup["damage_entity_type"], "damage entity type")
        setup_commands: list[dict[str, Any]] = []
        self.run_setup_command(
            require_str(setup["gamemode_command"], "gamemode command"), setup_commands
        )
        self.run_setup_command(require_str(emerald["command"], "emerald give command"), setup_commands)
        self.run_setup_command(require_str(tool["command"], "tool give command"), setup_commands)
        self.await_inventory_counts(
            (emerald_item_id, require_int(emerald["count"], "seeded emerald count")),
            (apple_item_id, require_int(apple["initial_count"], "initial apple count")),
        )
        self.call(
            "wait_inventory",
            {"item_id": tool_item_id, "count": 1, "timeout_seconds": self.answer_wait_seconds},
            self.answer_wait_seconds + 2.0,
        )
        seeded = self.observe_client()
        if self.item_count(seeded, tool_item_id) != 1:
            raise RuntimeError(f"seed did not leave exactly one {tool_item_id} in the inventory")

        placement = seeded["player"]
        summon_command = require_str(
            setup["summon_command_template"], "summon command template"
        ).format(
            x=f"{float(placement['x']) + 1.0:.1f}",
            y=f"{float(placement['y']):.1f}",
            z=f"{float(placement['z']):.1f}",
        )
        self.run_setup_command(summon_command, setup_commands)
        self.call(
            "select_hotbar_item",
            {"item_id": tool_item_id, "count": 1, "timeout_seconds": self.answer_wait_seconds},
        )
        deadline = time.monotonic() + self.answer_wait_seconds
        while True:
            observation = self.observe_client()
            visible = self.call("list_entities", {"radius": 24.0, "limit": 64})
            entities = visible.get("entities")
            if not isinstance(entities, list):
                raise RuntimeError("list_entities did not return an entity list")
            targets = [
                entity for entity in entities
                if entity["entity_type"] == damage_entity_type
            ]
            if targets:
                target = min(targets, key=lambda entity: entity["distance"])
                break
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(f"no client-visible {damage_entity_type} arrived after summon")
            self.await_state(observation, remaining)
        attack = self.call(
            "attack_entity_once",
            {
                "entity_id": require_int(target.get("entity_id"), "damage entity id"),
                "entity_uuid": require_str(target.get("entity_uuid"), "damage entity uuid"),
                "entity_type": damage_entity_type,
                "timeout_seconds": self.answer_wait_seconds,
            },
            self.answer_wait_seconds + 2.0,
        )
        if attack.get("confirmed") is not True:
            raise RuntimeError(f"the real survival attack was not confirmed by the server: {attack}")
        if attack.get("removed") is True:
            # A killed entity can drop a collectible item, which would perturb the
            # unchanged-inventory assertions this fixture depends on.
            raise RuntimeError(
                "the durability attack removed the summoned entity instead of only "
                f"damaging it: {attack}"
            )

        deadline = time.monotonic() + self.answer_wait_seconds
        damaged: dict[str, Any] | None = None
        while damaged is None:
            observation = self.observe_client()
            seeded_tool = self.tool_fingerprint(observation, tool_item_id)
            if seeded_tool["damage"] > 0:
                damaged = observation
                break
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(
                    "the survival attack left no durability component on "
                    f"{tool_item_id}: {seeded_tool}"
                )
            self.await_state(observation, remaining)
        initial_tool = self.tool_fingerprint(damaged, tool_item_id)
        screenshot = self.capture_inventory_screenshot(
            require_str(setup["inventory_screenshot"], "seed screenshot name")
        )
        return {
            "commands": setup_commands,
            "screenshot": screenshot,
            "inventory": {
                emerald_item_id: self.item_count(damaged, emerald_item_id),
                apple_item_id: self.item_count(damaged, apple_item_id),
            },
            "tool_item_id": tool_item_id,
            "tool_fingerprint": initial_tool,
            "durability_attack": {
                "entity_type": damage_entity_type,
                "entity_id": target.get("entity_id"),
                "confirmed": attack.get("confirmed"),
                "removed": attack.get("removed"),
                "health_before": attack.get("health_before"),
                "health_after": attack.get("health_after"),
            },
        }






