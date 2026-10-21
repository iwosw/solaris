"""Army QA watchdog: fail the bind phase loudly on a wedged owner task.

The live QA failure signature: villagers visible in list_entities, but every
minecraft_send_chat fixture sinks (TransportErrorLost / owner_result_unknown)
because a prior MCP task holds the single-writer fence forever. The bind loop
then burns the whole profile budget in silence and no army-qa/ report exists.

This watchdog wraps the bind scan: each fixture send is issued through a
thread with a short join so a wedged owner task raises loudly instead of
hanging the loop, and the transcript records which bind index wedged.
"""
from __future__ import annotations

import threading
import time
from typing import Any, Callable


class WedgedOwnerError(RuntimeError):
    """A fixture send did not complete within the watchdog window."""


def send_guarded(send: Callable[[], Any], label: str, timeout: float = 45.0) -> Any:
    """Run send() guarded: raise WedgedOwnerError if it wedges past timeout.

    The owner task keeps its fence (no cancellation from here); the caller
    must fail the phase loudly with the label identifying the wedged step.
    """
    box: dict[str, Any] = {}
    done = threading.Event()

    def _run() -> None:
        try:
            box["result"] = send()
        except BaseException as error:  # noqa: BLE001 - re-raised via box
            box["error"] = error
        finally:
            done.set()

    worker = threading.Thread(target=_run, daemon=True, name=f"army-qa-watchdog-{label}")
    start = time.monotonic()
    worker.start()
    if not done.wait(timeout):
        raise WedgedOwnerError(
            f"army-qa WEDGED OWNER: fixture '{label}' sent no reply within {timeout:.0f}s "
            f"(elapsed {time.monotonic() - start:.1f}s); single-writer fence held by a stuck task")
    error = box.get("error")
    if error is not None:
        raise error
    return box.get("result")
