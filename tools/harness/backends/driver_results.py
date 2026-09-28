"""Fail-closed scenario result classification."""

from __future__ import annotations

from typing import Any


def scenario_report_result(report: dict[str, Any]) -> str:
    result = report.get("result")
    if result not in {"passed", "failed", "blocked"}:
        raise RuntimeError("run_scenario did not return passed, failed, or blocked")
    return result


def combine_results(results: Any) -> str:
    saw_blocked = False
    for result in results:
        if result == "failed":
            return "failed"
        if result == "blocked":
            saw_blocked = True
        elif result != "passed":
            return "failed"
    return "blocked" if saw_blocked else "passed"
