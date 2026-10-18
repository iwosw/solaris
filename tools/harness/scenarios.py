#!/usr/bin/env python3
"""Discoverable catalog of *existing* executable scenarios.

The catalog is data, not a second runner: every entry names an existing harness
profile plus the exact selector (or the profile's own scenario id), so
``explain`` can answer "what would run and why" without side effects and
``run scenario <id>`` dispatches through the one shared run path in
``__main__.cmd_run`` (same artifact directory, bounded scope, receipt writer).

Assertions stay in the executable tests. An entry records only where to find
them, what they prove, and what they do not prove.

Rules for entries:

* no ``eval``, shell fragments, arbitrary plugin commands or a private DSL;
* no success condition duplicated from the test into this file;
* a native entry lists the exact test paths it requires, so a selector that
  silently matches nothing (or fewer tests than expected) fails closed.
"""

from __future__ import annotations

import os
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

from .runtime import REPO_ROOT

# Evidence kinds. A graphical or client entry never falls back to TCP evidence.
EVIDENCE_KINDS = (
    "pure",
    "native",
    "persistence",
    "component",
    "tcp",
    "graphical",
    "performance",
)

# Prerequisite tokens resolved by `missing_prerequisites`.
PREREQUISITES = (
    "sibling:solaris-default-plugins",
    "sibling:solaris-loader",
    "client-token",
    "xvfb",
)


@dataclass(frozen=True)
class Scenario:
    """One existing check, its exact selector, and the limits of its evidence."""

    id: str
    summary: str
    owner: str
    evidence: str
    profile: str
    selector: tuple[str, ...] = ()
    tests: tuple[str, ...] = ()
    requires: tuple[str, ...] = ()
    scenario_id: str | None = None
    manifest: str | None = None
    seed: str | None = None
    fixture: str | None = None
    sources: tuple[str, ...] = ()
    proves: str = ""
    limits: str = ""

    def __post_init__(self) -> None:
        if self.evidence not in EVIDENCE_KINDS:
            raise ValueError(f"{self.id}: unknown evidence kind {self.evidence!r}")
        for token in self.requires:
            if token not in PREREQUISITES:
                raise ValueError(f"{self.id}: unknown prerequisite {token!r}")
        if self.evidence == "native" and self.tests and not self.selector:
            raise ValueError(f"{self.id}: a native entry needs an exact selector")
        if self.manifest is not None and self.scenario_id is None:
            raise ValueError(f"{self.id}: a manifest needs the scenario_id it belongs to")


def _entry(**kwargs: object) -> Scenario:
    return Scenario(**kwargs)  # type: ignore[arg-type]


SCENARIOS: dict[str, Scenario] = {
    "loader-v2-settlements-smoke": _entry(
        id="loader-v2-settlements-smoke", summary="Frozen v2 server native settlement commands through Fabric Loader",
        owner="core:harness", evidence="graphical", profile="loader-live", scenario_id="loader-v2-settlements-smoke",
        requires=("sibling:solaris-loader", "client-token", "xvfb"),
        sources=("tools/harness/loader.py", "tools/harness/loader_settlements.py"),
        fixture="explicit --server-bin and --plugins-dir; isolated Loader permission/cache; no settlements package deployed",
        proves="Loader connection, native settlement create/list/info and repeated read-only info command",
        limits="minimal native settlement command smoke, not plugin production/war coverage; visual review required",
    ),
    "native-v2-p5-humanoid-armor": _entry(
        id="native-v2-p5-humanoid-armor", summary="One native graphical observer views a headless actor's worn armor and unchanged stand",
        owner="core:harness", evidence="graphical", profile="entities", scenario_id="native-v2-p5-humanoid-armor",
        requires=("xvfb",),
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p5.py"),
        fixture="one graphical native observer and one headless actor; given magenta and crafted cyan leather; summoned mob equipment",
        proves="same-player iron/diamond/dyed armor; dyed player and stand custody; iron zombie and diamond skeleton captures",
        limits="visual review required; requires dyed_color, dye crafting and summon equipment support; no plain-leather fallback",
    ),
    "native-v2-light-seam": _entry(
        id="native-v2-light-seam", summary="Native windowed chunk-admission lighting: shadowed roof/tunnel seam whose second chunk is admitted after the first, in both orders",
        owner="core:harness", evidence="graphical", profile="entities", scenario_id="native-v2-light-seam",
        requires=("xvfb",),
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_light.py",
                 "tools/harness/backends/light_seam_model.py", "tools/harness/backends/light_wire.py"),
        fixture="floating obsidian platform with stone roof, tunnel and two torches across a chunk border plus far standing pads; creative operator; two save-all + same-world server restarts",
        proves="after a settled first-chunk admission (server residency = simulation_distance square, second chunk outside it) a one-chunk teleport admits the neighbour; geometry_ready with zero pending sections; received client light equals the server's wire light and an analytic oracle at every open cell of a 24x7x16 border region (A then B, B then A, plus a near close-up); no fully dark admitted chunk; retained screenshots",
        limits="server light is its own chunk/light packet stream, not a live light query; separate admission is proved by residency construction plus wire order, not a server admission log; operator-built terrain, not earned; visual review of screenshots still required",
    ),
    "native-v2-radius-full-view": _entry(
        id="native-v2-radius-full-view", summary="G5 radius split: stationary native viewer at view 8 / sim 4 holds the full 17x17 square",
        owner="core:edge", evidence="graphical", profile="entities", scenario_id="native-v2-radius-full-view",
        requires=("xvfb",),
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_radius.py",
                 "tools/harness/backends/light_wire.py", "tools/harness/backends/client_probes.py"),
        fixture="server view_distance=8 simulation_distance=4 (LoginPlay 8/4); live client render_distance=8 required; shared _prepare stationary pad, no broad fills; per-chunk client cell at the pad level",
        proves="live minecraft_observe.client_settings.render_distance is 8; all 289 view-square chunks client-readable via minecraft_read_block at unique per-chunk cells (unheld chunks raise, fail-closed); no ForgetLevelChunk for a square member on the wire",
        limits="stationary readability only, not gameplay activity at the square edge or visual fidelity of every distant chunk mesh",
    ),
    "java-v2-p5-parity": _entry(
        id="java-v2-p5-parity", summary="Vanilla Java P4 surfaces, persistence and post-teleport walking on v2",
        owner="core:harness", evidence="graphical", profile="entities", scenario_id="java-v2-p5-parity",
        requires=("sibling:solaris-loader", "xvfb"),
        fixture="one real Java client; declared operator fixtures and clean same-world server restarts",
        proves="ordinary mining/crafting, vanilla screens, stand nameplate, damage, Q hopper custody and teleport movement",
        limits="visual screenshot review required; no native replacement or load coverage",
    ),
    "java-v2-p5-towns": _entry(
        id="java-v2-p5-towns", summary="Sequential non-op Java town leader and guest, protected builds and restart",
        owner="core:harness", evidence="graphical", profile="entities", scenario_id="java-v2-p5-towns",
        requires=("sibling:solaris-loader", "xvfb"),
        sources=("tools/harness/backends/java_p5.py", "tools/harness/backends/client_probes.py"),
        fixture="server plugin deployment; temporary fixture op grants revoked before all tested player actions",
        proves="non-op leader creates/claims/builds, guest builds are corrected without item loss, town/claim survive restart",
        limits="one graphical process at a time; bounded town build protection, not all plugin commands",
    ),
    "java-v2-p5-cursor": _entry(
        id="java-v2-p5-cursor", summary="Bounded vanilla Java cursor split, swap, merge and carried-close custody",
        owner="core:harness", evidence="graphical", profile="entities", scenario_id="java-v2-p5-cursor",
        requires=("sibling:solaris-loader", "xvfb"),
        sources=("tools/harness/backends/java_p5.py", "tools/harness/backends/client_probes.py"),
        fixture="one real Java client with eight apples and three diamonds in an ordinary crafting menu",
        proves="primary pickup, secondary placement, different-item swap, merge, quick move and carried-close conservation",
        limits="bounded cursor/inventory click coverage; screenshots require independent visual review",
    ),
    "inventory-client": _entry(
        id="inventory-client",
        summary="Inventory path in a real client: crafting, cursor round-trip and held-item selection",
        owner="core:harness",
        evidence="tcp",
        profile="inventory",
        scenario_id="m94-03a-inventory-oak-log-to-planks",
        fixture="fresh isolated offline operator fixture; item setup declared in receipt",
        sources=(
            "tools/harness/backends/client_probes.py",
            "tools/harness/compatibility.py",
            "tools/harness/backends/probe_steps.py",
        ),
        proves="ordinary slot clicks consume one oak log and create four planks, conserve two apples across cursor pickup/return, and select the crafted stack",
        limits="focused inventory probe only; native state/protocol evidence is not rendering/GUI preservation; bare Java inventory retains its broader compatibility gates",
    ),
    "core-replay-seed-81": _entry(
        id="core-replay-seed-81",
        summary="Core replay seed-81 actions in a real client",
        owner="loader:real-client-regression",
        evidence="graphical",
        profile="replay",
        scenario_id="core-actions-seed-81",
        manifest="docs/real-client-regression/manifests/core-replay-seed-81.json",
        seed="81",
        requires=("sibling:solaris-loader", "client-token", "xvfb"),
        fixture="fresh per-run world; the manifest names the seed-81 action sequence",
        sources=(
            "docs/real-client-regression/manifests/core-replay-seed-81.json",
            "../solaris-loader/README.md",
        ),
        proves="the recorded seed-81 action sequence still runs against the current server",
        limits="deferred_by_owner until CP-096",
    ),
    "native-v2-redstone": _entry(
        id="native-v2-redstone", summary="Native lever-wire-repeater-lamp on/off and reversible second cycle",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-redstone",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/client_probes.py"),
        fixture="native v2 only; explicit isolated operator pad and unpowered circuit",
        proves="ordinary lever use yields exact received wire power, repeater power and lamp lit states in both directions",
        limits="not piston/hopper/comparator coverage; graphical only with --windowed",
    ),
    "native-v2-stations": _entry(
        id="native-v2-stations", summary="Native station menus, netherite smithing and chest/copper-chest conservation",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-stations",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/client_probes.py"),
        fixture="native v2 only; explicit operator pad, stations and exactly one of each smithing input",
        proves="received station menu shapes; exact netherite result/debits; chest and copper-chest reopen/return without duplicate",
        limits="anvil/enchanting/crafter menu admission only; no operation parity or earned input claim",
    ),
    "native-v2-brewing": _entry(
        id="native-v2-brewing", summary="Native water/wart/fuel insertion produces one received awkward potion",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-brewing",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/client_probes.py"),
        fixture="native v2 only; explicit operator stand, water bottle, nether wart and blaze powder",
        proves="ordinary menu clicks transform received water component to awkward and conserve the returned bottle/input debits",
        limits="no progress/fuel HUD predicate; graphical only with --windowed",
    ),
    "native-v2-jukebox": _entry(
        id="native-v2-jukebox", summary="Native jukebox record insertion, inventory debit and ejection",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-jukebox",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/client_probes.py"),
        fixture="native v2 only; explicit operator empty jukebox and one music disc",
        proves="ordinary use changes received has_record false/true/false, debits one disc and ejects the exact UUID-fenced disc stack",
        limits="no audible playback claim",
    ),
    "native-v2-tnt-fire-light": _entry(
        id="native-v2-tnt-fire-light", summary="Native torch light, fire ignition and TNT priming/blast",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-tnt-fire-light",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/client_probes.py"),
        fixture="native v2 only; explicit obsidian escape lane, torch/flint supply and TNT/dirt fixtures",
        proves="ordinary torch placement increases received block light; ignition produces fire and UUID-fenced TNT removes dirt",
        limits="no fire-spread/extinguishing or sky-light parity; graphical only with --windowed",
    ),
    "native-v2-dimensions": _entry(
        id="native-v2-dimensions", summary="Native command travel to Nether/End and cross-dimension death/respawn",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-dimensions",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/client_probes.py"),
        fixture="native v2 only; explicit operator creative, dimension-qualified teleport and kill",
        proves="received dimension transitions, own chat in each world and UUID-preserving live overworld respawn",
        limits="command travel only; not portal entry, earned exploration, dragon combat or rendered dimension parity",
    ),
    "native-v2-ranged": _entry(
        id="native-v2-ranged", summary="Native bow/crossbow/trident shots debit ammunition and damage one fenced target",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-ranged",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/client_probes.py"),
        fixture="native v2 only; explicit isolated range, NoAI iron golem and one weapon/ammunition per shot",
        proves="ordinary use/release yields a new projectile, exact ammunition debit and received damage to the same UUID",
        limits="no arrow pickup, Loyalty, weapon enchantment or full flight presentation claim",
    ),
    "native-v2-automation-storage": _entry(
        id="native-v2-automation-storage", summary="Native crafter pulse, shelf swap and sculk vibration cycle",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-automation-storage",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/client_probes.py"),
        fixture="native v2 only; explicit crafter/log, shelf/one diamond/one emerald, lever/sensor fixtures",
        proves="one ordinary pulse yields four planks without held-power duplicate; shelf swap/withdraw conserves both items; sensor active/cooldown/inactive follows ordinary use",
        limits="no powered shelf chains, disabled crafter slots, calibrated filtering, shrieker/warden or visual model parity",
    ),
    "native-v2-storage-adversarial": _entry(
        id="native-v2-storage-adversarial", summary="Native bounded chest cursor-close and occupied-slot swap adversarial pass",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-storage-adversarial",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/client_probes.py"),
        fixture="native v2 only; isolated chest and exactly one diamond/emerald",
        proves="close with carried item returns once, occupied-slot swap preserves both owners, reopen and withdrawal conserve both items",
        limits="no crash/reconnect or full-inventory overflow; graphical only with --windowed",
    ),
    "native-v2-p1-armor-stand": _entry(
        id="native-v2-p1-armor-stand", summary="Native armor stand placement, equip/take, name and two-punch drops",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p1-armor-stand",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p1.py"),
        fixture="native v2; isolated pad, one stand/chestplate/named tag",
        proves="ordinary placement, exact armor custody, name-tag debit, stand/equipment drops and pickup",
        limits="rendered equipment/name require retained windowed screenshot review",
    ),
    "native-v2-p1-signs": _entry(
        id="native-v2-p1-signs", summary="Native sign text/color/glow/wax, edit refusal and empty replacement editor",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p1-signs",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p1.py"),
        fixture="native v2; isolated sign and one dye/glow ink/honeycomb",
        proves="ordinary sign edit/decorations, wax refusal, break/pickup and empty editor at the same replacement position",
        limits="text/color/glow require retained windowed screenshot review",
    ),
    "native-v2-p1-piglin": _entry(
        id="native-v2-p1-piglin", summary="Native piglin gold barter and no unpaid second payout",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p1-piglin",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p1.py"),
        fixture="native v2; isolated overworld piglin and one gold ingot",
        proves="ordinary gold interaction/debit and newly received non-gold loot UUID",
        limits="admire pose requires retained windowed screenshot review; no gold-armor neutrality claim",
    ),
    "native-v2-p1-trader": _entry(
        id="native-v2-p1-trader", summary="Native wandering trader merchant admission and close/reopen",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p1-trader",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p1.py"),
        fixture="native v2; isolated NoAI wandering trader",
        proves="ordinary interaction receives merchant menu twice with no inventory credit",
        limits="nonempty offers require retained windowed screenshot review; no purchase/restock claim",
    ),
    "native-v2-p1-leaves": _entry(
        id="native-v2-p1-leaves", summary="Native log mining, leaf distance/decay/drops and persistent control",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p1-leaves",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p1.py"),
        fixture="native v2; natural canopy, two trunk logs, persistent leaf, randomTickSpeed 4096 after mining",
        proves="player log removal, received distance seven then air, leaf drops and surviving persistent control",
        limits="bounded fixture canopy; not terrain-generated tree/restart/chunk-seam coverage",
    ),
    "native-v2-p1-helmet-equip": _entry(
        id="native-v2-p1-helmet-equip", summary="Native air-use helmet equip, occupied-slot swap and exact custody",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p1-helmet-equip",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p1.py"),
        fixture="native v2; one gold and one iron helmet, empty head slot and air raycast",
        proves="ordinary right-click equips and swaps the head slot; empty repeat use creates no items",
        limits="head-slot swap only; no full inventory, other armor slots or enchants",
    ),
    "native-v2-p1-player-observer": _entry(
        id="native-v2-p1-player-observer", summary="Two windowed native clients receive and render a player holding a sword",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p1-player-observer",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p1.py"),
        fixture="native v2; one actor and one observer on the same server/Xvfb, ordinary held sword",
        proves="observer receives actor UUID as minecraft:player with held equipment; slot change clears held equipment",
        limits="one observer and one held item; no remote skins, armor layers, reconnect or movement claim",
    ),
    "native-v2-p4-nameplates": _entry(
        id="native-v2-p4-nameplates", summary="Native explicit/hidden armor-stand labels, targeted pig and server restart",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p4-nameplates",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p4.py"),
        fixture="native v2; explicit CustomNameVisible stand, default-hidden named stand and NoAI named pig",
        proves="actual crosshair targets and same stand UUIDs after owned server restart",
        limits="visible/absent label pixels require retained windowed screenshot review",
    ),
    "native-v2-p4-advancements": _entry(
        id="native-v2-p4-advancements", summary="Native story advancements, earned stone pickaxe and restart",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p4-advancements",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p4.py"),
        fixture="fresh native player; declared wood/table/cobble/stick supply, ordinary table crafting",
        proves="actual earned pickaxe, advancement screen and recipes before/after real server restart",
        limits="story labels/completion/chat and no toast flood require screenshot/transcript review",
    ),
    "native-v2-p4-damage": _entry(
        id="native-v2-p4-damage", summary="Native zombie hit and retained before/hit/after frames",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p4-damage",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p4.py"),
        fixture="native v2; nighttime zombie beside healthy survival player",
        proves="received health loss and live client/input after the hit",
        limits="hurt tilt/direction requires hit-frame visual review; health loss alone is not presentation",
    ),
    "native-v2-p4-hopper": _entry(
        id="native-v2-p4-hopper", summary="Native Q drop enters nearby hopper, exact withdrawal and no duplicate",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p4-hopper",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p4.py"),
        fixture="native v2; adjacent hopper over solid pad and exactly one diamond",
        proves="ordinary Q debit, exact hopper menu receipt rather than player re-pickup, withdrawal/reopen custody",
        limits="single hopper/drop; no chains, powered state or unloaded boundary",
    ),
    "native-v2-p4-statistics": _entry(
        id="native-v2-p4-statistics", summary="Native earned statistics, tabs, restart and repeated authoritative requests",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p4-statistics",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p4.py"),
        fixture="native v2; ordinary walking/jump and stone/dirt mining/pickup, native Statistics request UI",
        proves="received plausible movement and exact mining/pickup counters survive restart and repeated requests",
        limits="one player and bounded actions; no synthetic request or inferred AwardStats receipt",
    ),
    "native-v2-p2-slime": _entry(
        id="native-v2-p2-slime", summary="Native medium slime kill, small children and no recursive split",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p2-slime",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p1.py"),
        fixture="native v2; one Size:1 NoAI slime and ordinary diamond sword",
        proves="UUID-fenced medium death yields 2-4 distinct small live children; a small child does not split",
        limits="one isolated split family; no magma cube, restart or natural spawning claim",
    ),
    "native-v2-p2-enderman": _entry(
        id="native-v2-p2-enderman", summary="Native arrow reverses and Enderman teleports without damage",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p2-enderman",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p1.py"),
        fixture="native v2 release; dry clear teleport pad, NoAI Enderman, bow and one arrow",
        proves="ordinary shot debits arrow; new arrow reverses, same Enderman UUID teleports and retains health",
        limits="no water/stare anger/block carrying or teleport persistence coverage",
    ),
    "native-v2-p2-fishing": _entry(
        id="native-v2-p2-fishing", summary="Native open-water cast, real bite, reel, caught inventory and early-reel refusal",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p2-fishing",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p1.py"),
        fixture="native v2 release; source-water pool and one ordinary rod; unmodified hook metadata trace",
        proves="real native cast/reel after hook biting yields received inventory item; early reel does not award another",
        limits="canonical native bite projection missing; forwarded wire metadata is supplemental wait evidence",
    ),
    "native-v2-p2-creaking": _entry(
        id="native-v2-p2-creaking", summary="Native Creaking freezes under gaze, walks away and refreezes",
        owner="core:harness", evidence="tcp", profile="entities", scenario_id="native-v2-p2-creaking",
        sources=("tools/harness/backends/native_v2.py", "tools/harness/backends/native_p1.py"),
        fixture="native v2; ordinary unbound Creaking on a clear pad, survival observer",
        proves="same UUID remains still during first gaze, moves on look-away and freezes on second gaze",
        limits="no heart linkage, pumpkin, occlusion or multiple observers coverage",
    ),
}


def verify_native_evidence(log_text: str, tests: tuple[str, ...]) -> list[str]:
    """Return the expected test paths that are not reported as passed.

    ``cargo test`` prints one ``test <path> ... ok`` line per executed test; a
    selector that matches nothing prints ``running 0 tests`` and no such line,
    so a zero-selected run can never be mistaken for a pass.
    """
    missing = [name for name in tests if f"test {name} ... ok" not in log_text]
    if "test result: FAILED" in log_text:
        missing.append("<a test in this selector failed>")
    return missing


_CARGO_SUMMARY = re.compile(
    r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored"
    r"(?:; (\d+) measured)?(?:; (\d+) filtered out)?"
)


def cargo_counts(log_text: str) -> dict[str, int] | None:
    """Summarize the ``test result:`` lines of one cargo run, or None when absent.

    ``selected`` is every test the selector matched (executed + ignored) and
    ``executed`` is passed + failed; ``filtered_out`` is what cargo itself
    reports as excluded. ``None`` means the engine reported no counts, which is
    not evidence of a pass either; the caller fails closed on it.
    """
    totals = {
        "selected": 0,
        "executed": 0,
        "passed": 0,
        "failed": 0,
        "ignored": 0,
        "filtered_out": 0,
    }
    seen = False
    for match in _CARGO_SUMMARY.finditer(log_text):
        seen = True
        passed, failed, ignored = (int(match.group(index)) for index in (2, 3, 4))
        totals["passed"] += passed
        totals["failed"] += failed
        totals["ignored"] += ignored
        totals["executed"] += passed + failed
        totals["selected"] += passed + failed + ignored
        totals["filtered_out"] += int(match.group(6) or 0)
    return totals if seen else None


def count_failures(counts: dict[str, int] | None) -> list[str]:
    """Reasons the engine counts alone forbid a pass."""
    if counts is None:
        return ["the engine reported no test counts"]
    reasons: list[str] = []
    if counts["failed"]:
        reasons.append(f"{counts['failed']} test(s) failed")
    if counts["executed"] == 0:
        reasons.append(
            f"no test was executed; {counts['ignored']} selected ignored"
            if counts["ignored"]
            else "no test was selected and executed"
        )
    return reasons


def log_text(ctx: object) -> str:
    """Concatenate every log the run already recorded."""
    parts: list[str] = []
    for relative in getattr(ctx, "logs", []):
        path = Path(relative)
        if not path.is_absolute():
            path = REPO_ROOT / path
        try:
            parts.append(path.read_text(errors="replace"))
        except OSError:
            continue
    return "\n".join(parts)


def repo_revisions() -> dict[str, str]:
    """Best-effort revisions of the repos a named run may involve.

    Never fatal: a missing checkout or git reports ``unavailable`` instead of
    turning a run into a failure.
    """
    repos = {
        "solaris": REPO_ROOT,
        "solaris-default-plugins": Path(
            os.environ.get("SOLARIS_DEFAULT_PLUGINS_ROOT", REPO_ROOT.parent / "solaris-default-plugins")
        ),
        "solaris-loader": Path(os.environ.get("SOLARIS_LOADER_ROOT", REPO_ROOT.parent / "solaris-loader")),
    }
    revisions: dict[str, str] = {}
    for name, path in repos.items():
        try:
            completed = subprocess.run(
                ["git", "-C", str(path), "rev-parse", "HEAD"],
                capture_output=True,
                text=True,
                timeout=5,
                check=False,
            )
            revision = completed.stdout.strip()
            revisions[name] = revision if completed.returncode == 0 and revision else "unavailable"
        except (OSError, subprocess.SubprocessError):
            revisions[name] = "unavailable"
    return revisions


def describe(scenario: Scenario) -> str:
    """Resolved, side-effect-free description for ``explain``."""
    lines = [
        f"scenario: {scenario.id}",
        f"summary: {scenario.summary}",
        f"owner: {scenario.owner}",
        f"evidence: {scenario.evidence}",
        f"profile: {scenario.profile}",
    ]
    if scenario.selector:
        lines.append("selector: cargo test " + " ".join(scenario.selector))
    if scenario.scenario_id:
        lines.append(f"scenario id: {scenario.scenario_id} (bound for this run)")
    if scenario.manifest:
        lines.append(f"manifest: {scenario.manifest} (bound for this run)")
    if scenario.seed:
        lines.append(f"seed: {scenario.seed}")
    if scenario.fixture:
        lines.append(f"fixture: {scenario.fixture}")
    if scenario.tests:
        lines.append("required tests:")
        lines.extend(f"  {name}" for name in scenario.tests)
    if scenario.requires:
        lines.append(
            "declared prerequisites (enforced by the owning profile/backend, not here): "
            + ", ".join(scenario.requires)
        )
    if scenario.sources:
        lines.append("sources:")
        lines.extend(f"  {path}" for path in scenario.sources)
    lines.append(f"proves: {scenario.proves}")
    lines.append(f"does not prove: {scenario.limits}")
    lines.append("receipt: .analysis/validation/<stamp>-<profile>-<id>/result.json (status from the profile)")
    return "\n".join(lines)
