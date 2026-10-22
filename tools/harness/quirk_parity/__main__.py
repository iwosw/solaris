"""Run the quirk/farm parity gate: scenarios on real 26.1.2 vanilla and Solaris.

    python3 -m tools.harness.quirk_parity --out DIR [--family F] [--scenario ID ...]
        [--vanilla-only | --solaris-only] [--jar PATH] [--java PATH]

Writes DIR/receipt.json (per-scenario verdicts, diffs, vanilla self-checks,
wall times) and exits non-zero on any unexplained divergence, vanilla
self-check failure, command rejection or timeout. A scenario may carry
`known_divergence` (documented, issue-filed gap): it is reported but does
not fail the gate; once the gap closes the stale marker itself fails it.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

from . import compare, fuzz
from .vanilla import DEFAULT_JAR, MEM_RUN, REPO_ROOT, VanillaError, VanillaOracle

SCENARIO_DIR = Path(__file__).with_name("scenarios")
TEST_NAME = "quirk_parity_tests::quirk_parity_solaris_replay"

# Same rules on both servers before any scenario command; scenarios opt out with
# "prelude": false when they need spawning, random ticks or weather.
PRELUDE = [
    "gamerule spawn_mobs false",
    "gamerule random_tick_speed 0",
    "gamerule advance_time false",
    "gamerule advance_weather false",
    "weather clear",
    "time set noon",
]


def load_scenarios(only: list[str], families: list[str]) -> list[dict]:
    scenarios = []
    for path in sorted(SCENARIO_DIR.glob("*.json")):
        scenario = json.loads(path.read_text())
        scenario.setdefault("id", path.stem)
        if only and scenario["id"] not in only:
            continue
        if families and scenario.get("family") not in families:
            continue
        scenario["_path"] = str(path)
        scenarios.append(scenario)
    return expand_seeds(scenarios)


def expand_seeds(scenarios: list[dict]) -> list[dict]:
    """A scenario with `seeds` runs once per seed on both servers (`_group` = base id)."""
    out = []
    for scenario in scenarios:
        seeds = scenario.get("seeds")
        if not seeds:
            out.append(scenario)
            continue
        for seed in seeds:
            variant = dict(scenario, id=f"{scenario['id']}#s{seed}", seed=seed, _group=scenario["id"])
            variant.pop("seeds")
            out.append(variant)
    return out


def resolved(scenario: dict) -> dict:
    """Scenario with the common prelude folded into setup (both servers see it)."""
    out = {k: v for k, v in scenario.items() if not k.startswith("_")}
    if scenario.get("prelude", True):
        out["setup"] = PRELUDE + list(scenario.get("setup", []))
    return out


def run_vanilla(scenario: dict, out: Path, args) -> dict:
    boxes = scenario.get("boxes") or [scenario["box"]]
    started = time.monotonic()
    result = {"id": scenario["id"], "server": "vanilla", "phases": [], "command_log": []}
    with VanillaOracle(out / "vanilla" / scenario["id"], seed=scenario.get("seed", 1),
                       jar=Path(args.jar), java=args.java) as oracle:
        oracle.run_checked(["tick freeze"])
        for x1, _, z1, x2, _, z2 in boxes:
            oracle.run_checked([f"forceload add {x1} {z1} {x2} {z2}"])
        # Chunks load on server tasks: run real ticks until every far corner is resident.
        for box in boxes:
            for _ in range(200):
                lines = oracle.run(f"execute if loaded {box[3]} {box[4]} {box[5]} run say QP_LOADED")
                if any("QP_LOADED" in line for line in lines):
                    break
                oracle.sprint(20)
            else:
                raise VanillaError("forceloaded box never became resident")
        result["command_log"] += oracle.run_checked(resolved(scenario)["setup"])
        oracle.sprint(scenario.get("settle_ticks", 1))
        result["phases"].append({"name": "setup", "tick": 0, "snapshot": oracle.snapshot(boxes)})
        trace = scenario.get("trace", False)
        for phase in scenario.get("phases", []):
            result["command_log"] += oracle.run_checked(phase.get("commands", []))
            rows = []
            ticks = phase.get("ticks", 0)
            if trace:
                for _ in range(ticks):
                    oracle.sprint(1)
                    rows.append(oracle.snapshot(boxes))
            else:
                oracle.sprint(ticks)
            entry = {"name": phase["name"], "tick": oracle.game_ticks, "snapshot": oracle.snapshot(boxes)}
            if trace:
                entry["trace"] = rows
            result["phases"].append(entry)
    result["wall_ms"] = int((time.monotonic() - started) * 1000)
    return result


def run_solaris(scenarios: list[dict], out: Path, args) -> dict[str, dict]:
    solaris_dir = out / "solaris"
    solaris_dir.mkdir(parents=True, exist_ok=True)
    paths = []
    for scenario in scenarios:
        path = solaris_dir / f"{scenario['id']}.json"
        path.write_text(json.dumps(resolved(scenario), indent=1))
        paths.append(str(path))
    receipt = solaris_dir / "receipt.json"
    env = dict(os.environ, SOLARIS_QUIRK_SCENARIOS=":".join(paths), SOLARIS_QUIRK_RECEIPT=str(receipt))
    command = [str(MEM_RUN), "-m", "4G", "cargo", "test", "-p", "mc-core", "--lib", TEST_NAME,
               "--", "--ignored", "--exact", "--nocapture"]
    if not MEM_RUN.is_file():
        command = command[1 + 2:]
    log = solaris_dir / "solaris.log"
    with log.open("w") as handle:
        proc = subprocess.run(command, cwd=REPO_ROOT, env=env, stdout=handle,
                              stderr=subprocess.STDOUT, check=False)
    if proc.returncode or not receipt.is_file():
        raise RuntimeError(f"Solaris replay failed (exit {proc.returncode}); see {log}")
    return {row["id"]: row for row in json.loads(receipt.read_text())}


# Solaris success/no-op feedback that carries no rejection (vanilla's "Could not set the block"
# no-op is likewise ignored by the vanilla driver).
BENIGN_SOLARIS = ("Changed ", "No blocks were changed", "Successfully filled", "Summoned ")


def solaris_rejections(run: dict) -> list[str]:
    return [row for row in run["command_log"]
            if not row.split(" => ", 1)[-1].startswith(BENIGN_SOLARIS)]


def verdict(scenario: dict, vanilla: dict | None, solaris: dict | None) -> dict:
    entry: dict = {"id": scenario["id"], "family": scenario.get("family"), "status": "pass"}
    reasons = []
    if vanilla is not None:
        entry["vanilla_wall_ms"] = vanilla.get("wall_ms")
        if vanilla["command_log"]:
            reasons.append("vanilla rejected setup commands (scenario bug)")
            entry["vanilla_command_errors"] = vanilla["command_log"]
        failures = compare.check_vanilla(scenario, vanilla)
        if failures:
            reasons.append("vanilla does not show the quirk this scenario encodes")
            entry["vanilla_selfcheck_failures"] = failures
    if solaris is not None:
        entry["solaris_wall_ms"] = solaris.get("wall_ms")
        if solaris_rejections(solaris) and not scenario.get("allow_solaris_command_errors"):
            reasons.append("Solaris rejected commands vanilla accepted")
            entry["solaris_command_errors"] = solaris_rejections(solaris)[:20]
    if vanilla is not None and solaris is not None and scenario.get("fuzz"):
        return fuzz_verdict(scenario, vanilla, solaris, entry, reasons)
    if vanilla is not None and solaris is not None:
        diff = compare.compare_scenario(scenario, vanilla, solaris)
        if not diff["match"]:
            entry["diff"] = diff["phases"]
            known = scenario.get("known_divergence")
            if known:
                entry["status"] = "known-divergence"
                entry["known_divergence"] = known
            else:
                reasons.append("Solaris diverges from vanilla")
        elif scenario.get("known_divergence"):
            reasons.append("known_divergence marker is stale: Solaris now matches vanilla")
    if reasons:
        entry["status"] = "fail"
        entry["reasons"] = reasons
    return entry


def rate_group_verdict(group: str, variants: list[dict], vanilla: dict, solaris: dict) -> dict:
    scenario = variants[0]
    entry: dict = {"id": group, "family": scenario.get("family"), "status": "pass",
                   "seeds": [v["seed"] for v in variants]}
    reasons = []
    vruns = [vanilla[v["id"]] for v in variants if v["id"] in vanilla]
    sruns = [solaris[v["id"]] for v in variants if v["id"] in solaris]
    for run in vruns:
        if run["command_log"]:
            reasons.append("vanilla rejected setup commands (scenario bug)")
            entry["vanilla_command_errors"] = run["command_log"]
            break
    for run in sruns:
        if solaris_rejections(run) and not scenario.get("allow_solaris_command_errors"):
            reasons.append("Solaris rejected commands vanilla accepted")
            entry["solaris_command_errors"] = solaris_rejections(run)[:20]
            break
    if vruns and sruns and len(vruns) == len(variants) == len(sruns):
        entry["rate"] = compare.rate_verdict(scenario, vruns, sruns)
        if not entry["rate"]["match"]:
            known = scenario.get("known_divergence")
            if known:
                entry["status"], entry["known_divergence"] = "known-divergence", known
            else:
                reasons.append(entry["rate"].get("error") or "Solaris farm rate diverges from vanilla")
        elif scenario.get("known_divergence"):
            reasons.append("known_divergence marker is stale: Solaris now matches vanilla")
    elif vruns and not sruns:
        entry["rate"] = {"vanilla_only": compare.rate_verdict(scenario, vruns, vruns)["vanilla"]}
    if reasons:
        entry["status"], entry["reasons"] = "fail", reasons
    return entry


def fuzz_verdict(scenario: dict, vanilla: dict, solaris: dict, entry: dict, reasons: list) -> dict:
    """Fuzz gate: every vanilla duplication cell must match; other cell diffs are findings."""
    diff = compare.compare_scenario(scenario, vanilla, solaris)
    dupes = fuzz.dupe_cells(scenario, vanilla)
    diverging: dict[int, list[str]] = {}
    for phase, cells in diff["phases"].items():
        for index in cells.get("cells", {}):
            diverging.setdefault(int(index), []).append(phase)
    cells_meta = {c["index"]: c for c in scenario["fuzz"]["cells"]}
    entry["fuzz"] = {
        "seed": scenario["fuzz"]["seed"], "cells": len(cells_meta),
        "vanilla_duplication_cells": {str(i): p for i, p in sorted(dupes.items())},
        "diverging_cells": {str(i): p for i, p in sorted(diverging.items())},
        "diverging_duplication_cells": sorted(set(dupes) & set(diverging)),
    }
    entry["diff_cells"] = {str(i): {"phases": diverging[i], "blocks": cells_meta[i]["blocks"],
                                    "detail": {ph: diff["phases"][ph]["cells"][str(i)] for ph in diverging[i]}}
                           for i in sorted(diverging)[:25]}
    if set(dupes) & set(diverging):
        reasons.append("Solaris diverges from vanilla on a vanilla TNT-duplication cell")
    elif diverging:
        entry["status"] = "findings"
    if reasons:
        entry["status"] = "fail"
        entry["reasons"] = reasons
    return entry


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--scenario", action="append", default=[])
    parser.add_argument("--family", action="append", default=[])
    parser.add_argument("--jar", default=str(DEFAULT_JAR))
    parser.add_argument("--java", default="java")
    parser.add_argument("--fuzz-seed", type=int, action="append", default=[])
    parser.add_argument("--fuzz-cells", type=int, default=64)
    parser.add_argument("--fuzz-style", choices=sorted(fuzz.PALETTES) + ["bud", "slime3d", "timing"], default="mixed")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--vanilla-only", action="store_true")
    mode.add_argument("--solaris-only", action="store_true")
    parser.add_argument(
        "--vanilla-results", type=Path, default=None,
        help="reuse vanilla-results.json from an earlier run for scenarios it already covers "
             "(vanilla is the fixed oracle; only valid while the scenario JSON is unchanged)")
    args = parser.parse_args(argv)
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    scenarios = load_scenarios(args.scenario, args.family) if (args.scenario or args.family or not args.fuzz_seed) else []
    scenarios += [fuzz.timing_scenario(seed, args.fuzz_cells) if args.fuzz_style == "timing"
                  else fuzz.bud_scenario(seed, args.fuzz_cells, "slime3d" if args.fuzz_style == "slime3d" else "line") if args.fuzz_style in ("bud", "slime3d")
                  else fuzz.fuzz_scenario(seed, args.fuzz_cells, args.fuzz_style) for seed in args.fuzz_seed]
    if not scenarios:
        print("no scenarios selected", file=sys.stderr)
        return 2
    receipt: dict = {"profile": "quirk-parity", "scenarios": [], "status": "fail"}
    vanilla_results: dict[str, dict] = {}
    cached_vanilla: dict[str, dict] = (
        json.loads(args.vanilla_results.read_text()) if args.vanilla_results else {})
    solaris_results: dict[str, dict] = {}
    try:
        if not args.vanilla_only:
            solaris_results = run_solaris(scenarios, out, args)
        if not args.solaris_only:
            for scenario in scenarios:
                if scenario["id"] in cached_vanilla:
                    vanilla_results[scenario["id"]] = cached_vanilla[scenario["id"]]
                    continue
                try:
                    vanilla_results[scenario["id"]] = run_vanilla(scenario, out, args)
                except VanillaError as error:
                    receipt["scenarios"].append(
                        {"id": scenario["id"], "status": "fail", "reasons": [f"vanilla: {error}"]})
        grouped: dict[str, list[dict]] = {}
        for scenario in scenarios:
            if scenario.get("_group"):
                grouped.setdefault(scenario["_group"], []).append(scenario)
                continue
            if any(row["id"] == scenario["id"] for row in receipt["scenarios"]):
                continue
            receipt["scenarios"].append(verdict(
                scenario, vanilla_results.get(scenario["id"]), solaris_results.get(scenario["id"])))
        for group, variants in grouped.items():
            receipt["scenarios"].append(rate_group_verdict(
                group, variants, vanilla_results, solaris_results))
        (out / "vanilla-results.json").write_text(json.dumps(vanilla_results, indent=1))
        failed = [row for row in receipt["scenarios"] if row["status"] == "fail"]
        receipt["status"] = "pass" if not failed else "fail"
    except Exception as error:  # fail closed, keep the receipt
        receipt["error"] = str(error)
    (out / "receipt.json").write_text(json.dumps(receipt, indent=1))
    print(json.dumps({row["id"]: row["status"] for row in receipt["scenarios"]}, indent=1))
    print("quirk-parity:", receipt["status"], receipt.get("error", ""))
    return 0 if receipt["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
