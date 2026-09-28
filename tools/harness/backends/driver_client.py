"""Loopback bridge transport, client state waits, and screenshot observations."""

from __future__ import annotations

import binascii
import ctypes
import json
import os
import select
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib import error, parse, request

INOTIFY_MASK = 0x00000002 | 0x00000004 | 0x00000008 | 0x00000080 | 0x00000100


def wait_for_path_condition(path: Path, timeout_seconds: float, condition: Any) -> Any:
    parent = path.parent
    parent.mkdir(parents=True, exist_ok=True)
    libc = ctypes.CDLL(None, use_errno=True)
    inotify_init1 = libc.inotify_init1
    inotify_init1.argtypes = [ctypes.c_int]
    inotify_init1.restype = ctypes.c_int
    inotify_add_watch = libc.inotify_add_watch
    inotify_add_watch.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
    inotify_add_watch.restype = ctypes.c_int

    descriptor = inotify_init1(os.O_CLOEXEC)
    if descriptor < 0:
        error_number = ctypes.get_errno()
        raise OSError(error_number, os.strerror(error_number))
    try:
        watch = inotify_add_watch(
            descriptor,
            os.fsencode(parent),
            INOTIFY_MASK,
        )
        if watch < 0:
            error_number = ctypes.get_errno()
            raise OSError(error_number, os.strerror(error_number), parent)

        deadline = time.monotonic() + timeout_seconds
        while True:
            matched, value = condition()
            if matched:
                return value
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(f"filesystem event timeout for {path}")
            readable, _, _ = select.select([descriptor], [], [], remaining)
            if not readable:
                raise TimeoutError(f"filesystem event timeout for {path}")
            os.read(descriptor, 64 * 1024)
    finally:
        os.close(descriptor)


class AgentClient:
    def __init__(self, bridge_url: str, secret: str) -> None:
        self.bridge_url = validate_loopback_url(bridge_url)
        self.secret = secret
        self.next_id = 1
        self.opener = request.build_opener(request.ProxyHandler({}))

    def call(self, command: str, payload: dict[str, Any], timeout_seconds: float) -> dict[str, Any]:
        request_id = self.next_id
        self.next_id += 1
        body = json.dumps(
            {
                "id": request_id,
                "secret": self.secret,
                "command": command,
                "payload": payload,
            },
            separators=(",", ":"),
        ).encode("utf-8")
        rpc_request = request.Request(
            self.bridge_url,
            data=body,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            with self.opener.open(rpc_request, timeout=timeout_seconds) as response:
                decoded = json.loads(response.read().decode("utf-8"))
        except error.HTTPError as exc:
            raise RuntimeError(structured_http_error(command, request_id, exc)) from exc
        except (error.URLError, TimeoutError, json.JSONDecodeError) as exc:
            raise RuntimeError(f"{command} failed: {exc}") from exc

        if decoded.get("id") != request_id:
            raise RuntimeError(
                f"{command} failed: response id {decoded.get('id')} did not match request id {request_id}"
            )
        if not decoded.get("ok"):
            bridge_error = decoded.get("error") or {}
            code = bridge_error.get("code", "agent-error")
            message = bridge_error.get("message", "")
            raise RuntimeError(f"{command} failed: {code}: {message}")
        payload_value = decoded.get("payload")
        if not isinstance(payload_value, dict):
            raise RuntimeError(f"{command} failed: response payload was not an object")
        return payload_value


def structured_http_error(command: str, request_id: int, exc: error.HTTPError) -> str:
    try:
        decoded = json.loads(exc.read().decode("utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return f"{command} failed: HTTP {exc.code}: {exc.reason}"

    if decoded.get("id") != request_id:
        return f"{command} failed: HTTP {exc.code}: response id {decoded.get('id')} did not match request id {request_id}"
    bridge_error = decoded.get("error") or {}
    code = bridge_error.get("code", "agent-error")
    message = bridge_error.get("message", "")
    return f"{command} failed: {code}: {message}"


def validate_loopback_url(bridge_url: str) -> str:
    parsed = parse.urlparse(bridge_url)
    if parsed.scheme != "http":
        raise ValueError("bridge URL must use http")
    if parsed.hostname not in {"127.0.0.1", "localhost", "::1"}:
        raise ValueError("bridge URL must target loopback")
    if parsed.port is None:
        raise ValueError("bridge URL must include a port")
    if not parsed.path:
        parsed = parsed._replace(path="/")
    return parse.urlunparse(parsed)


def validate_loopback_server_addr(server_addr: str) -> str:
    host, port = parse_server_addr(server_addr)
    normalized_host = host.lower()
    if normalized_host not in {"127.0.0.1", "localhost", "::1"}:
        raise ValueError("server_addr must target loopback")
    if normalized_host == "::1":
        return f"[::1]:{port}"
    return f"{normalized_host}:{port}"


def parse_server_addr(server_addr: str) -> tuple[str, int]:
    if not server_addr or any(character.isspace() for character in server_addr):
        raise ValueError("server_addr must be host:port")
    if server_addr.startswith("["):
        bracket_end = server_addr.find("]")
        if bracket_end <= 1 or bracket_end + 2 > len(server_addr):
            raise ValueError("server_addr must be host:port")
        if server_addr[bracket_end + 1] != ":":
            raise ValueError("server_addr must be host:port")
        host = server_addr[1:bracket_end]
        port_text = server_addr[bracket_end + 2 :]
    else:
        if server_addr.count(":") != 1:
            raise ValueError("server_addr must be host:port")
        host, port_text = server_addr.rsplit(":", 1)
    if not host or not port_text:
        raise ValueError("server_addr must be host:port")
    try:
        port = int(port_text, 10)
    except ValueError as exc:
        raise ValueError("server_addr port must be numeric") from exc
    if port < 1 or port > 65535:
        raise ValueError("server_addr port must be between 1 and 65535")
    return host, port


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def call_and_record(
    client: AgentClient,
    transcript: list[dict[str, Any]],
    command: str,
    payload: dict[str, Any],
    timeout_seconds: float,
    actor: str | None = None,
) -> dict[str, Any]:
    entry: dict[str, Any] = {
        "command": command,
        "payload": payload,
        "started_at": utc_now(),
    }
    if actor is not None:
        entry["client"] = actor
    transcript.append(entry)
    try:
        response = client.call(command, payload, timeout_seconds)
    except Exception as exc:
        entry["error"] = {"message": str(exc)}
        raise
    entry["response"] = response
    entry["finished_at"] = utc_now()
    return response


def wait_for_existing_or_explicit_connection(
    client: AgentClient,
    transcript: list[dict[str, Any]],
    server_addr: str,
    timeout_seconds: float,
    actor: str | None = None,
) -> dict[str, Any]:
    preconnect_timeout = min(timeout_seconds, 5.0)
    play_state = call_and_record(
        client,
        transcript,
        "wait_play",
        {"timeout_seconds": preconnect_timeout},
        preconnect_timeout + 1.0,
        actor,
    )
    if is_in_play(play_state):
        return wait_for_interactive_play(
            client,
            transcript,
            play_state,
            timeout_seconds,
            actor,
        )
    if is_connecting(play_state):
        play_state = call_and_record(
            client,
            transcript,
            "wait_play",
            {"timeout_seconds": timeout_seconds},
            timeout_seconds,
            actor,
        )
        if is_in_play(play_state):
            return wait_for_interactive_play(
                client,
                transcript,
                play_state,
                timeout_seconds,
                actor,
            )
        raise RuntimeError("client did not reach Play state from existing ConnectScreen")
    call_and_record(client, transcript, "connect", {"server_addr": server_addr}, timeout_seconds, actor)
    play_state = call_and_record(
        client,
        transcript,
        "wait_play",
        {"timeout_seconds": timeout_seconds},
        timeout_seconds,
        actor,
    )
    if not is_in_play(play_state):
        raise RuntimeError("client did not reach Play state")
    return wait_for_interactive_play(
        client,
        transcript,
        play_state,
        timeout_seconds,
        actor,
    )


def wait_for_interactive_play(
    client: AgentClient,
    transcript: list[dict[str, Any]],
    play_state: dict[str, Any],
    timeout_seconds: float,
    actor: str | None = None,
) -> dict[str, Any]:
    if is_interactive_play(play_state) or not is_in_play(play_state):
        return play_state

    deadline = time.monotonic() + timeout_seconds
    latest_state = play_state
    closed_version: int | None = None
    while time.monotonic() < deadline:
        version = state_version(latest_state)
        if is_pause_screen(latest_state) and closed_version != version:
            remaining = max(0.05, deadline - time.monotonic())
            client_thread_timeout = min(remaining, 12.0)
            call_and_record(
                client,
                transcript,
                "close_screen",
                {},
                client_thread_timeout,
                actor,
            )
            closed_version = version
            latest_state = call_and_record(
                client,
                transcript,
                "state",
                {},
                client_thread_timeout,
                actor,
            )
            if is_interactive_play(latest_state) or not is_in_play(latest_state):
                return latest_state
        latest_state = wait_for_state_event(
            client,
            transcript,
            latest_state,
            deadline,
            actor,
        )
        if is_interactive_play(latest_state) or not is_in_play(latest_state):
            return latest_state

    raise RuntimeError(
        "client reached Play state but did not become scenario-interactive"
        + f" current_screen={latest_state.get('current_screen', '')!r}"
    )


def wait_until_not_in_play(
    client: AgentClient,
    transcript: list[dict[str, Any]],
    timeout_seconds: float,
    actor: str | None = None,
) -> dict[str, Any]:
    deadline = time.monotonic() + min(timeout_seconds, 5.0)
    latest_state = call_and_record(
        client,
        transcript,
        "state",
        {},
        min(timeout_seconds, 5.0),
        actor,
    )
    while time.monotonic() < deadline:
        if not is_in_play(latest_state):
            return latest_state
        latest_state = wait_for_state_event(
            client,
            transcript,
            latest_state,
            deadline,
            actor,
        )
    raise RuntimeError("client did not leave Play state before reconnect")


def wait_for_state_event(
    client: AgentClient,
    transcript: list[dict[str, Any]],
    latest_state: dict[str, Any],
    deadline: float,
    actor: str | None,
) -> dict[str, Any]:
    remaining = deadline - time.monotonic()
    if remaining <= 0.0:
        raise RuntimeError("client state event did not arrive before timeout")
    event_timeout = min(remaining, 5.0)
    return call_and_record(
        client,
        transcript,
        "wait_state_change",
        {
            "observed_version": state_version(latest_state),
            "timeout_seconds": event_timeout,
        },
        event_timeout + 1.0,
        actor,
    )


def state_version(state: dict[str, Any]) -> int:
    value = state.get("state_version")
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise RuntimeError("client state response did not include a valid state_version")
    return value


def wait_for_server_session_release(run_dir: Path, timeout_seconds: float) -> str:
    server_log = run_dir / "server.log"
    result = f"server session release: observed log={server_log.name}"
    try:
        return wait_for_path_condition(
            server_log,
            min(timeout_seconds, 60.0),
            lambda: (server_session_release_logged(server_log), result),
        )
    except TimeoutError as error:
        raise RuntimeError(
            f"server did not log session release before reconnect: {server_log}"
        ) from error


def server_session_release_logged(server_log: Path) -> bool:
    """Observe session removal, not completion of its asynchronous player save."""
    try:
        return "play session unregistered" in server_log.read_text(
            encoding="utf-8",
            errors="replace",
        )
    except FileNotFoundError:
        return False


def capture_screenshot(
    client: AgentClient,
    transcript: list[dict[str, Any]],
    run_dir: Path,
    screenshots_dir: Path,
    name: str,
    timeout_seconds: float,
    actor: str | None = None,
) -> str:
    screenshot_path = screenshots_dir / f"{name}.png"
    call_and_record(
        client,
        transcript,
        "screenshot",
        {"path": str(screenshot_path)},
        timeout_seconds,
        actor,
    )
    png_error = wait_for_valid_png(screenshot_path, min(timeout_seconds, 5.0))
    if png_error is not None:
        raise RuntimeError(f"screenshot command wrote invalid PNG {screenshot_path}: {png_error}")
    return screenshot_path.relative_to(run_dir).as_posix()


def is_in_play(payload: dict[str, Any]) -> bool:
    return bool(payload.get("in_play"))


def is_interactive_play(payload: dict[str, Any]) -> bool:
    if not is_in_play(payload):
        return False
    current_screen = payload.get("current_screen")
    if current_screen is None:
        return True
    return current_screen == "" or str(current_screen).lower() == "none"


def is_pause_screen(payload: dict[str, Any]) -> bool:
    return payload.get("current_screen") == "net.minecraft.client.gui.screens.PauseScreen"


def is_connecting(payload: dict[str, Any]) -> bool:
    return payload.get("current_screen") == "net.minecraft.client.gui.screens.ConnectScreen"


def horizontal_distance(before: dict[str, Any], after: dict[str, Any]) -> float:
    try:
        dx = float(after.get("x", 0.0)) - float(before.get("x", 0.0))
        dz = float(after.get("z", 0.0)) - float(before.get("z", 0.0))
    except (TypeError, ValueError):
        return 0.0
    return (dx * dx + dz * dz) ** 0.5


def chunk_delta(before: dict[str, Any], after: dict[str, Any]) -> int:
    try:
        before_x = int(float(before.get("x", 0.0)) // 16)
        before_z = int(float(before.get("z", 0.0)) // 16)
        after_x = int(float(after.get("x", 0.0)) // 16)
        after_z = int(float(after.get("z", 0.0)) // 16)
    except (TypeError, ValueError):
        return 0
    return abs(after_x - before_x) + abs(after_z - before_z)


def state_observation(label: str, state: dict[str, Any]) -> str:
    return (
        f"{label}: in_play={bool(state.get('in_play'))}"
        f" dimension={state.get('dimension', '')}"
        f" position={state.get('x', 0.0)},{state.get('y', 0.0)},{state.get('z', 0.0)}"
    )


def wait_for_file(path: Path, timeout_seconds: float) -> bool:
    try:
        return wait_for_path_condition(
            path,
            timeout_seconds,
            lambda: (path.is_file(), True),
        )
    except TimeoutError:
        return False


def wait_for_valid_png(path: Path, timeout_seconds: float) -> str | None:
    last_error = ["file not created"]

    def png_ready() -> tuple[bool, None]:
        if path.is_file():
            last_error[0] = png_validation_error(path)
            return last_error[0] is None, None
        return False, None

    try:
        return wait_for_path_condition(path, timeout_seconds, png_ready)
    except TimeoutError:
        if path.is_file():
            return png_validation_error(path)
        return last_error[0]


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def png_validation_error(path: Path) -> str | None:
    try:
        data = path.read_bytes()
    except OSError as exc:
        return f"could not read file: {exc}"

    if not data.startswith(PNG_SIGNATURE):
        return "missing PNG signature"
    offset = len(PNG_SIGNATURE)
    saw_ihdr = False
    while True:
        if offset + 12 > len(data):
            return "truncated PNG chunk header"
        length = int.from_bytes(data[offset : offset + 4], "big")
        chunk_type = data[offset + 4 : offset + 8]
        chunk_start = offset + 8
        chunk_end = chunk_start + length
        crc_end = chunk_end + 4
        if crc_end > len(data):
            return f"truncated {chunk_type.decode('ascii', errors='replace')} chunk"
        expected_crc = int.from_bytes(data[chunk_end:crc_end], "big")
        actual_crc = binascii.crc32(chunk_type + data[chunk_start:chunk_end]) & 0xFFFFFFFF
        if actual_crc != expected_crc:
            return f"{chunk_type.decode('ascii', errors='replace')} CRC mismatch"
        if not saw_ihdr:
            if chunk_type != b"IHDR":
                return "first PNG chunk is not IHDR"
            if length != 13:
                return "IHDR chunk has invalid length"
            saw_ihdr = True
        if chunk_type == b"IEND":
            if length != 0:
                return "IEND chunk has invalid length"
            if crc_end != len(data):
                return "trailing bytes after IEND"
            return None
        offset = crc_end




