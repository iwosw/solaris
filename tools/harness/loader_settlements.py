"""Minimal graphical native settlement-command smoke against an explicit v2 deployment.

Uses only the native `/settlement` command surface (create/list/info); no
settlements package is deployed.
"""
from __future__ import annotations

from pathlib import Path
from typing import Any

from .backends.java_p5 import _command
from .backends.probe_steps import ProbeSteps
from .loader import capture_ui


def run(client: Any, runtime: Any, artifact_dir: Path, server_log: Path) -> dict[str, Any]:
    steps = ProbeSteps(client, artifact_dir / "settlements-smoke", 120.0, "loader-v2-settlements-smoke")
    with steps:
        try:
            steps.wait(lambda state: state.get("in_play") and state.get("player", {}).get("on_ground"), "grounded")
            _command(steps, "settlement create p5qa", "Founded p5qa ")
            steps.report["create_capture"] = capture_ui(client, artifact_dir, "p5-settlement-created")
            _command(steps, "settlement list", "p5qa")
            steps.report["list_capture"] = capture_ui(client, artifact_dir, "p5-settlement-list")
            _command(steps, "settlement info p5qa", "p5qa | ")
            steps.report["info_capture"] = capture_ui(client, artifact_dir, "p5-settlement-info")
            # Bounded adversarial check: an identical read must get a fresh reply,
            # not satisfy itself from the prior command's retained chat line.
            _command(steps, "settlement info p5qa", "p5qa | ")
            steps.report["visual_review"] = "required: inspect create/list/info captures and visible chat"
        except Exception:
            try:
                steps.report["failure_capture"] = capture_ui(client, artifact_dir, "p5-settlement-failure")
            except Exception as error:
                steps.report["failure_capture_error"] = str(error)
            raise
    return steps.report
