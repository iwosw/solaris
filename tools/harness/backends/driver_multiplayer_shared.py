"""Prepare and capture the two real clients used by multiplayer scenarios."""

from __future__ import annotations

from pathlib import Path
from typing import Any

if __package__:
    from .driver_client import AgentClient, call_and_record, capture_screenshot, wait_for_existing_or_explicit_connection
else:
    from driver_client import AgentClient, call_and_record, capture_screenshot, wait_for_existing_or_explicit_connection


def prepare_two_client_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> Path:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)
    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary, transcript, server_addr, timeout_seconds, actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary, transcript, server_addr, timeout_seconds, actor="secondary",
    )
    return screenshots_dir


def capture_two_client_screenshots(
    primary: AgentClient,
    secondary: AgentClient,
    transcript: list[dict[str, Any]],
    run_dir: Path,
    screenshots_dir: Path,
    scenario_id: str,
    timeout_seconds: float,
) -> list[str]:
    return [
        capture_screenshot(
            primary, transcript, run_dir, screenshots_dir,
            f"{scenario_id}-primary", timeout_seconds, actor="primary",
        ),
        capture_screenshot(
            secondary, transcript, run_dir, screenshots_dir,
            f"{scenario_id}-secondary", timeout_seconds, actor="secondary",
        ),
    ]
