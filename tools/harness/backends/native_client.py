"""Native-client launch and authenticated, event-driven MCP readiness."""

from __future__ import annotations

import hashlib
import ipaddress
import json
import os
import selectors
import shutil
import stat
import struct
import subprocess
import tempfile
import time
from pathlib import Path
from typing import TYPE_CHECKING, Any, BinaryIO
from urllib.parse import urlsplit

from .. import runtime
from ..mcp import McpClient

if TYPE_CHECKING:
    from ..profiles import ProfileContext


_INOTIFY_EVENT = struct.Struct("iIII")
_DESCRIPTOR_READY = 0x8 | 0x80  # IN_CLOSE_WRITE | IN_MOVED_TO
_WATCH_FAILED = 0x400 | 0x4000 | 0x8000  # IN_DELETE_SELF | IN_Q_OVERFLOW | IN_IGNORED
_PUBLIC_DESCRIPTOR_FIELDS = (
    "instance_id",
    "pid",
    "started_unix_ms",
    "endpoint",
    "token_file",
    "protocol_versions",
    "profile",
)


def _client_root() -> Path:
    return Path(
        os.environ.get("SOLARIS_CLIENT_ROOT", runtime.REPO_ROOT.parent / "solaris-client")
    ).expanduser().resolve()


def _record_binary(ctx: ProfileContext, binary: Path) -> None:
    evidence = ctx.client_evidence
    path = str(binary)
    if evidence.get("binary_path") == path and evidence.get("binary_sha256"):
        return
    evidence["binary_path"] = path
    evidence.pop("binary_sha256", None)
    if not binary.is_file():
        raise RuntimeError(f"native client binary is missing: {binary}")
    if not os.access(binary, os.X_OK):
        raise RuntimeError(f"native client binary is not executable: {binary}")
    digest = hashlib.sha256()
    with binary.open("rb") as handle:
        while block := handle.read(1024 * 1024):
            digest.update(block)
    evidence["binary_sha256"] = digest.hexdigest()


def resolve_binary(ctx: ProfileContext) -> Path:
    """Resolve an override or build the missing default debug executable once."""
    evidence = ctx.client_evidence
    root = _client_root()
    override = os.environ.get("SOLARIS_CLIENT_BINARY")
    binary = Path(override).expanduser() if override else Path("target/debug/solaris-client")
    if not binary.is_absolute():
        binary = root / binary
    binary = binary.resolve()
    if evidence.get("binary_path") != str(binary):
        evidence.pop("binary_sha256", None)
    evidence.update({"root": str(root), "binary_path": str(binary), "binary_status": "resolving"})
    try:
        if not root.is_dir():
            raise RuntimeError(f"native client root is missing: {root}; set SOLARIS_CLIENT_ROOT")
        if not binary.exists() and not override:
            # The enclosing harness command owns its memory/CPU scope. Keep this
            # build in that scope and use debug=0 rather than a release build.
            from .. import profiles

            evidence["binary_status"] = "building"
            env = os.environ.copy()
            env.update(
                {
                    "CARGO_PROFILE_DEV_DEBUG": "0",
                    "CARGO_PROFILE_TEST_DEBUG": "0",
                    "CARGO_INCREMENTAL": "0",
                    "CARGO_TARGET_DIR": str(root / "target"),
                }
            )
            profiles._run_command(
                ["cargo", "build", "--locked", "-j", "4", "--bin", "solaris-client"],
                ctx,
                "native-client-build.log",
                cwd=root,
                env=env,
            )
        _record_binary(ctx, binary)
        evidence["binary_status"] = "ready"
        return binary
    except BaseException as exc:
        evidence.update({"binary_status": "failed", "binary_error_type": type(exc).__name__})
        raise


def _descriptor_names(notify_fd: int) -> list[str]:
    """Read complete-file announcements, never a partially written descriptor."""
    names: list[str] = []
    while True:
        try:
            events = os.read(notify_fd, 65536)
        except BlockingIOError:
            return names
        if not events:
            raise RuntimeError("native descriptor inotify stream closed")
        offset = 0
        while offset < len(events):
            _, mask, _, name_length = _INOTIFY_EVENT.unpack_from(events, offset)
            offset += _INOTIFY_EVENT.size
            name = os.fsdecode(events[offset : offset + name_length].split(b"\0", 1)[0])
            offset += name_length
            if mask & _WATCH_FAILED:
                raise RuntimeError("native descriptor inotify watch became unavailable")
            if mask & _DESCRIPTOR_READY and name.startswith("native-") and name.endswith(".json"):
                names.append(name)


def _read_descriptor(
    path: Path, runtime_dir: Path, pid: int, evidence: dict[str, Any]
) -> tuple[dict[str, Any], str]:
    descriptor = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(descriptor, dict):
        raise RuntimeError("native descriptor must be a JSON object")
    evidence["descriptor"] = {
        name: descriptor[name] for name in _PUBLIC_DESCRIPTOR_FIELDS if name in descriptor
    }
    if "pid" in descriptor and (type(descriptor["pid"]) is not int or descriptor["pid"] != pid):
        raise RuntimeError("native descriptor PID does not match the launched process")
    instance = descriptor.get("instance_id")
    if not isinstance(instance, str) or instance != path.stem:
        raise RuntimeError("native descriptor instance_id does not match its filename")
    endpoint = descriptor.get("endpoint")
    if not isinstance(endpoint, str):
        raise RuntimeError("native descriptor omitted the public MCP endpoint")
    address = urlsplit(endpoint)
    try:
        loopback = ipaddress.ip_address(address.hostname or "").is_loopback
        port = address.port
    except ValueError:
        raise RuntimeError("native descriptor MCP endpoint is invalid") from None
    if (
        address.scheme != "http"
        or not loopback
        or not port
        or address.path != "/mcp"
        or address.username is not None
        or address.password is not None
        or address.query
        or address.fragment
    ):
        raise RuntimeError("native descriptor requires an uncredentialed loopback HTTP /mcp endpoint")
    expected_token = runtime_dir / f"{instance}.token"
    if descriptor.get("token_file") != str(expected_token):
        raise RuntimeError("native descriptor token_file is not its isolated runtime sibling")
    # O_NOFOLLOW and fstat check the exact opened credential, not a raced path.
    fd = os.open(expected_token, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    with os.fdopen(fd, "rb") as handle:
        metadata = os.fstat(handle.fileno())
        if (
            not stat.S_ISREG(metadata.st_mode)
            or stat.S_IMODE(metadata.st_mode) != 0o600
            or metadata.st_uid != os.getuid()
            or metadata.st_size > 128
        ):
            raise RuntimeError("native MCP token must be an owner-owned 0600 regular file of at most 128 bytes")
        token = handle.read(129).decode("utf-8")
    if not token or len(token.encode("utf-8")) > 128 or any(char.isspace() for char in token):
        raise RuntimeError("native MCP token must be a nonempty raw bearer token")
    return descriptor, token


def start(
    ctx: ProfileContext,
    binary: Path,
    server_addr: str | None,
    username: str | None,
    timeout: float,
    *,
    display: str | None = None,
) -> tuple[subprocess.Popen[Any], BinaryIO, McpClient]:
    """Launch the selected native mode and await its live MCP handshake.

    The caller owns the returned process, log and MCP session, and any Xvfb
    it explicitly creates for windowed mode. Failure never falls back to
    another client mode; readiness uses events, not sleeps or polling.
    """
    evidence = ctx.client_evidence
    evidence.update({"launch_status": "starting", "headless": not ctx.windowed,
                     "validation_mode": "graphical" if ctx.windowed else "functional",
                     "initialized": False})
    process: subprocess.Popen[Any] | None = None
    log_handle: BinaryIO | None = None
    client: McpClient | None = None
    selector: selectors.BaseSelector | None = None
    notify_fd = pid_fd = -1
    command_index: int | None = None
    stage = "prerequisites"
    token = ""
    nonce = ""
    try:
        if timeout <= 0:
            raise ValueError("native client startup timeout must be positive")
        if username is not None and not runtime.USERNAME_RE.fullmatch(username):
            raise ValueError("native client username must contain 1-16 ASCII letters, digits or underscores")
        root = _client_root()
        evidence["root"] = str(root)
        if not root.is_dir():
            raise RuntimeError(f"native client root is missing: {root}; set SOLARIS_CLIENT_ROOT")
        binary = binary.expanduser().resolve()
        _record_binary(ctx, binary)
        env = os.environ.copy()
        if ctx.windowed:
            if not display:
                raise ValueError("windowed native launch requires an explicit display")
            env["DISPLAY"] = display
            env.pop("WAYLAND_DISPLAY", None)
            resource_names = ("SOLARIS_CLIENT_DATA_DIR", "SOLARIS_CLIENT_RESOURCE_ROOT",
                              "SOLARIS_CLIENT_ASSET_ROOT", "SOLARIS_CLIENT_ASSET_INDEX")
        else:
            resource_names = ("SOLARIS_CLIENT_DATA_DIR",)
        evidence["resource_paths"] = {
            name: env[name] for name in resource_names if env.get(name)
        }
        for name in resource_names:
            if not env.get(name):
                raise RuntimeError(f"native client requires {name} for verified local content")
        for name, value in evidence["resource_paths"].items():
            path = Path(value)
            if not path.is_absolute():
                path = root / path
            available = path.is_file() if name == "SOLARIS_CLIENT_ASSET_INDEX" else path.is_dir()
            if not available:
                raise RuntimeError(f"native client content prerequisite {name} is missing: {path}")
        ctx.artifact_dir.mkdir(parents=True, exist_ok=True)
        runs_dir = root / ".analysis" / "harness-runs"
        runs_dir.mkdir(parents=True, exist_ok=True)
        run_dir = Path(tempfile.mkdtemp(prefix="native-", dir=runs_dir)).resolve()
        runtime_dir = run_dir / "runtime"
        runtime_dir.mkdir(mode=0o700)
        artifact_dir = run_dir / "artifacts"
        artifact_dir.mkdir(mode=0o700)
        log_path = ctx.artifact_dir / "native-client.log"
        evidence.update(
            {
                "runtime_dir": str(runtime_dir),
                "artifact_dir": str(artifact_dir),
                "log": str(log_path),
            }
        )
        # Title-mode launch (server_addr/username both None): omit --server /
        # --username so the client starts on the Title screen with prefill.
        # Mixed (one set, one None) is a caller bug — fail loudly.
        if (server_addr is None) != (username is None):
            raise ValueError("title-mode launch requires both server_addr and username unset")
        argv = [
            str(binary),
            "play",
            "--profile", "offline-test",
            *([] if server_addr is None else ["--server", server_addr]),
            *([] if username is None else ["--username", username]),
            "--automation",
            "--mcp-http", "127.0.0.1:0",
            "--windowed" if ctx.windowed else "--headless",
            "--runtime-dir", str(runtime_dir),
            "--artifact-dir", str(artifact_dir),
        ]
        # Subscribe before Popen so even immediate descriptor publication cannot
        # race the watch. A pidfd is attached as soon as the child exists.
        selector, notify_fd, _ = runtime._require_event_fds(None, runtime_dir)
        command_index = len(ctx.commands)
        ctx.commands.append(argv)
        ctx.cwds.append(str(root))
        ctx.exits.append(None)
        try:
            ctx.logs.append(log_path.relative_to(runtime.REPO_ROOT).as_posix())
        except ValueError:
            ctx.logs.append(str(log_path))
        evidence["command_index"] = command_index
        log_handle = log_path.open("wb")
        stage = "launch"
        deadline = time.monotonic() + timeout
        process = subprocess.Popen(
            argv,
            cwd=root,
            env=env,
            stdout=log_handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        evidence["pid"] = process.pid
        pid_fd = runtime._pidfd_open(process.pid)
        if pid_fd < 0:
            raise RuntimeError("native client readiness requires Linux pidfd")
        selector.register(pid_fd, selectors.EVENT_READ)
        stage = "descriptor"
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError("native client did not publish its descriptor before the startup deadline")
            events = selector.select(remaining)
            if any(key.fd == pid_fd for key, _ in events):
                process.wait()
                raise RuntimeError(f"native client exited {process.returncode} before MCP readiness")
            names = _descriptor_names(notify_fd) if any(key.fd == notify_fd for key, _ in events) else []
            if not names:
                continue
            if len(set(names)) != 1:
                raise RuntimeError("native client published multiple descriptors in its isolated runtime directory")
            descriptor_path = runtime_dir / names[0]
            evidence["descriptor_path"] = str(descriptor_path)
            descriptor, token = _read_descriptor(descriptor_path, runtime_dir, process.pid, evidence)
            nonce = descriptor.get("nonce", "")
            break
        stage = "MCP handshake"
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise RuntimeError("native client descriptor appeared after the startup deadline")
        client = McpClient(descriptor["endpoint"], token, remaining)
        initialized = client.initialize()
        if time.monotonic() >= deadline:
            raise RuntimeError("native MCP handshake exceeded the startup deadline")
        if ctx.windowed:
            from . import native_focus

            stage = "X11 input focus"
            evidence["window_focus"] = native_focus.acquire(
                display, process.pid, max(0.0, deadline - time.monotonic())
            )
        evidence.update(
            {
                "launch_status": "ready",
                "initialized": True,
                "protocol_version": initialized["protocolVersion"],
            }
        )
        return process, log_handle, client
    except BaseException as exc:
        evidence.update(
            {"launch_status": "failed", "failure_stage": stage, "error_type": type(exc).__name__}
        )
        try:
            if client is not None:
                try:
                    client.close()
                except Exception:
                    pass
        finally:
            try:
                runtime.stop_process(process, interrupt=isinstance(exc, KeyboardInterrupt))
            finally:
                if process is not None and command_index is not None:
                    ctx.exits[command_index] = process.returncode
                    evidence["exit_code"] = process.returncode
                if log_handle is not None:
                    log_handle.close()
        if isinstance(exc, Exception):
            message = str(exc)
            for secret in (token, nonce):
                if isinstance(secret, str) and secret:
                    message = message.replace(secret, "[REDACTED]")
            raise RuntimeError(f"native client startup failed during {stage}: {message}") from None
        raise
    finally:
        if selector is not None:
            selector.close()
        if notify_fd >= 0:
            os.close(notify_fd)
        if pid_fd >= 0:
            os.close(pid_fd)


def capture(client: McpClient, artifact_dir: Path, name: str = "native-final") -> dict[str, Any]:
    """Retain a completed actual-window GPU target, not a state-only screenshot."""
    metadata = client.call_tool("client_capture", {"name": name})
    required = ("frame_id", "state_revision", "scene_revision", "resource_epoch")
    if (metadata.get("gpu_completion") is not True
            or metadata.get("render_target") != "actual_window_scene_target"
            or metadata.get("offscreen_only") is not False
            or metadata.get("present_requested") is not True
            or any(not metadata.get(key) for key in required)):
        raise RuntimeError(f"native capture lacks actual-window GPU completion evidence: {metadata}")
    source = Path(metadata["path"])
    screenshots = artifact_dir / "screenshots"
    screenshots.mkdir(exist_ok=True)
    target = screenshots / f"{name}.png"
    shutil.copyfile(source, target)
    with target.open("rb") as handle:
        if handle.read(8) != b"\x89PNG\r\n\x1a\n":
            raise RuntimeError("native capture did not retain a PNG")
    metadata["retained_path"] = str(target)
    (screenshots / f"{name}.json").write_text(json.dumps(metadata, indent=2) + "\n")
    return metadata
