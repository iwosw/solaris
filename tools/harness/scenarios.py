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
    "warehouse-bind-manual": _entry(
        id="warehouse-bind-manual",
        summary="A manual warehouse bind resolves a surveyed container and reuses its handle",
        owner="core:script/storage/settlement",
        evidence="native",
        profile="test",
        selector=("-p", "mc-net", "--lib", "warehouse_tests::manual_warehouse_binds"),
        tests=(
            "script::storage::settlement_tests::warehouse_tests::manual_warehouse_binds_surveyed_container_and_reuses_handle",
        ),
        sources=(
            "crates/mc-net/src/script/storage/settlement/warehouse.rs",
            "crates/mc-net/src/script/storage/settlement/warehouse_tests.rs",
            "docs/architecture/pilot-warehouse-binding.md",
        ),
        proves="real native bind semantics: container resolution, one durable change, handle idempotence",
        limits="fixture-scoped; not manual-building acceptance (CP-049 debt) and not component/client evidence",
    ),
    "warehouse-bind-refusals": _entry(
        id="warehouse-bind-refusals",
        summary="Manual binding refuses stale tokens, foreign targets and an already-bound position",
        owner="core:script/storage/settlement",
        evidence="native",
        profile="test",
        selector=("-p", "mc-net", "--lib", "warehouse_tests::manual_"),
        tests=(
            "script::storage::settlement_tests::warehouse_tests::manual_warehouse_refuses_stale_token_outside_claimed_missing_and_cross_kind",
            "script::storage::settlement_tests::warehouse_tests::manual_bind_refuses_position_held_by_authored_binding",
        ),
        sources=(
            "crates/mc-net/src/script/storage/settlement/warehouse.rs",
            "crates/mc-net/src/script/storage/settlement/warehouse_tests.rs",
        ),
        proves="the refusal fences stay in their documented order and a refusal mutates nothing",
        limits="the selector also runs the positive bind test; native evidence only",
    ),
    "building-recognized-once": _entry(
        id="building-recognized-once",
        summary="A player-built volume recognizes once through live reads and reuses its certificate",
        owner="core:script/storage/settlement",
        evidence="native",
        profile="test",
        selector=("-p", "mc-net", "--lib", "certificate_tests::recognize_building"),
        tests=(
            "script::storage::settlement_tests::certificate_tests::recognize_building_validates_live_points_and_reuses_certificate",
            "script::storage::settlement_tests::certificate_tests::recognize_building_accepts_a_sub_footprint_of_a_larger_survey",
            "script::storage::settlement_tests::certificate_tests::recognize_building_refuses_invalid_live_points",
        ),
        sources=(
            "crates/mc-net/src/script/storage/settlement/certificate.rs",
            "crates/mc-net/src/script/storage/settlement/certificate_tests.rs",
        ),
        proves="normalized footprints reuse one certificate, and invalid live interaction points refuse",
        limits="native recognition only; a certified building's live capabilities are exercised by the resident work suites",
    ),
    "warehouse-reservation-restart": _entry(
        id="warehouse-reservation-restart",
        summary="Warehouse reservations hold live stock across restart and reject an over-promise",
        owner="core:script/storage/settlement",
        evidence="persistence",
        profile="test",
        selector=("-p", "mc-net", "--lib", "warehouse_reservations_hold_live_stock"),
        tests=(
            "script::storage::settlement_tests::warehouse_reservations_hold_live_stock_across_restart_and_reject_overpromise",
        ),
        sources=(
            "crates/mc-net/src/script/storage/settlement.rs",
            "crates/mc-net/src/script/storage/settlement_tests.rs",
        ),
        proves="reopen keeps the reservation and the second bind cannot over-promise stock",
        limits="in-process reopen, not a process crash and not client-visible stock",
    ),
    "warehouse-transfer-recovery": _entry(
        id="warehouse-transfer-recovery",
        summary="An owned warehouse transfer recovers exactly once across a real process crash",
        owner="core:play/inventory",
        evidence="persistence",
        profile="test",
        selector=("-p", "mc-net", "--lib", "owned_transfer_recovers_exactly_once"),
        tests=(
            "play::session::owned_inventory_endpoint_tests::owned_transfer_recovers_exactly_once_across_a_process_crash",
        ),
        sources=(
            "crates/mc-net/src/play/owned_inventory.rs",
            "crates/mc-net/src/play/session/owned_inventory_endpoint_tests.rs",
        ),
        proves="one transferred item survives a real child-process crash exactly once",
        limits="headless native recovery; no graphical passenger and no vanilla capture",
    ),
    "inventory-storage-recovery": _entry(
        id="inventory-storage-recovery",
        summary="A compound transaction recovers the linked inventory/storage result after a crash",
        owner="core:play/inventory",
        evidence="persistence",
        profile="test",
        selector=("-p", "mc-net", "--lib", "compound_transaction_recovers_inventory"),
        tests=(
            "play::script_inventory_transaction_tests::compound_transaction_recovers_inventory_after_process_crash",
        ),
        sources=(
            "crates/mc-net/src/play/owned_inventory.rs",
            "crates/mc-net/src/play/script_inventory_transaction_tests.rs",
        ),
        proves="the storage half and the inventory half recover together, exactly once",
        limits="native recovery only; no client-visible conservation",
    ),
    "warehouse-deposit-recovery": _entry(
        id="warehouse-deposit-recovery",
        summary="A server-owned warehouse deposit appends before publishing and recovers both halves",
        owner="core:play/simulation",
        evidence="persistence",
        profile="test",
        selector=("-p", "mc-net", "--lib", "server_owned_warehouse_deposit"),
        tests=(
            "play::simulation::tests::server_owned_warehouse_deposit_appends_before_publishing_and_recovers_both",
            "play::simulation::tests::server_owned_warehouse_deposit_refuses_stale_fences_without_mutating",
        ),
        sources=(
            "crates/mc-net/src/settlement.rs",
            "crates/mc-net/src/play/simulation.rs",
            "crates/mc-net/src/play/simulation/tests.rs",
        ),
        proves="durable append precedes publication, and a stale fence refuses without mutation",
        limits="headless owner path; client-visible item conservation stays with the inventory client gate",
    ),
    "settlement-component-cargo": _entry(
        id="settlement-component-cargo",
        summary="The deployed Settlements component plus host/native warehouse and construction suites",
        owner="plugins:solaris-settlements",
        evidence="component",
        profile="settlement-cargo",
        requires=("sibling:solaris-default-plugins",),
        fixture="../solaris-default-plugins/solaris-settlements/plugin.wasm (rebuilt and hash-checked inside the sibling runner log)",
        sources=(
            "../solaris-default-plugins/tools/run_integration.py",
            "crates/mc-net/src/script/storage/settlement.rs",
        ),
        proves="the rebuilt component bytes reach the native owners and the owning native suites pass",
        limits="component coverage, not CP-049 manual-building acceptance and not a client scenario",
    ),
    "inventory-client": _entry(
        id="inventory-client",
        summary="Inventory path in a real client: crafting, cursor round-trip and held-item selection",
        owner="loader:java-agent",
        evidence="graphical",
        profile="inventory",
        scenario_id="m94-03a-inventory-oak-log-to-planks",
        requires=("sibling:solaris-default-plugins", "sibling:solaris-loader", "client-token", "xvfb"),
        fixture="../solaris-default-plugins/basic-economy operator fixture",
        sources=(
            "../solaris-loader/java-agent/src/main/java/dev/solaris/agent/javaagent/M94InventoryCraftingScenario.java",
            "tools/harness/compatibility.py",
            "docs/real-client-regression/manifests/m94-regression-pack.json",
        ),
        proves="client-visible crafting consumes one oak log and creates four planks on the real screen path",
        limits="deferred_by_owner until CP-096; a GUI pass is required before any client-preservation claim",
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
