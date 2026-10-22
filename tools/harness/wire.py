"""Tracked 26.1.2 wire framing, Status and offline-join helpers.

Layouts follow ``crates/mc-protocol`` (framing/compression, Status, Login,
Configuration and the Play ``LoginPlay`` id); the packet ids are asserted by
that crate's tests. Only what harness gates need lives here, so scenarios do
not depend on ignored ``.analysis`` evidence files.
"""

from __future__ import annotations

import json
import socket
import struct
import uuid
import zlib
from typing import Any

RELEASE = "26.1.2"
PROTOCOL = 775

# crates/mc-protocol packet ids used by the offline join below.
LOGIN_CLIENTBOUND_DISCONNECT = 0x00
LOGIN_CLIENTBOUND_SUCCESS = 0x02
LOGIN_CLIENTBOUND_SET_COMPRESSION = 0x03
LOGIN_SERVERBOUND_START = 0x00
LOGIN_SERVERBOUND_ACK = 0x03
CONFIG_SERVERBOUND_KNOWN_PACKS = 0x07
CONFIG_SERVERBOUND_ACK_FINISH = 0x03
CONFIG_CLIENTBOUND_FINISH = 0x03
CONFIG_CLIENTBOUND_KNOWN_PACKS = 0x0E
PLAY_CLIENTBOUND_LOGIN = 0x31
PLAY_CLIENTBOUND_KEEPALIVE = 0x2C


def write_varint(value: int) -> bytes:
    value &= 0xFFFFFFFF
    out = bytearray()
    while True:
        part = value & 0x7F
        value >>= 7
        if value:
            out.append(part | 0x80)
        else:
            out.append(part)
            return bytes(out)


def read_exact(sock: socket.socket, length: int) -> bytes:
    result = bytearray()
    while len(result) < length:
        chunk = sock.recv(length - len(result))
        if not chunk:
            raise RuntimeError("peer closed before complete frame")
        result.extend(chunk)
    return bytes(result)


def read_varint(sock: socket.socket) -> int:
    value = 0
    for shift in range(0, 35, 7):
        part = read_exact(sock, 1)[0]
        value |= (part & 0x7F) << shift
        if not part & 0x80:
            return value
    raise RuntimeError("overlong VarInt")


def read_varint_bytes(data: bytes, offset: int = 0) -> tuple[int, int]:
    value = 0
    for shift in range(0, 35, 7):
        if offset >= len(data):
            raise RuntimeError("truncated VarInt")
        part = data[offset]
        offset += 1
        value |= (part & 0x7F) << shift
        if not part & 0x80:
            return value, offset
    raise RuntimeError("overlong VarInt")


def frame(packet_id: int, body: bytes = b"") -> bytes:
    payload = write_varint(packet_id) + body
    return write_varint(len(payload)) + payload


def read_frame(sock: socket.socket) -> tuple[int, bytes]:
    """One uncompressed frame (login before ``Set Compression``)."""
    length = read_varint(sock)
    payload = read_exact(sock, length)
    packet_id, offset = read_varint_bytes(payload)
    return packet_id, payload[offset:]


def mc_string(value: str) -> bytes:
    data = value.encode()
    return write_varint(len(data)) + data


def read_mc_string(data: bytes) -> str:
    length, offset = read_varint_bytes(data)
    end = offset + length
    if end != len(data):
        raise RuntimeError("unexpected trailing bytes in string packet")
    return data[offset:end].decode()


def handshake(host: str, port: int, next_state: int, protocol: int = PROTOCOL) -> bytes:
    return (
        write_varint(protocol)
        + mc_string(host)
        + struct.pack(">H", port)
        + write_varint(next_state)
    )


def read_compressed_frame(sock: socket.socket) -> tuple[int, bytes]:
    """One frame after ``Set Compression``: length, data_length, body."""
    length = read_varint(sock)
    payload = read_exact(sock, length)
    data_length, offset = read_varint_bytes(payload)
    body = payload[offset:] if data_length == 0 else zlib.decompress(payload[offset:])
    packet_id, offset = read_varint_bytes(body)
    return packet_id, body[offset:]


def send_compressed_frame(
    sock: socket.socket, packet_id: int, body: bytes = b""
) -> None:
    payload = write_varint(0) + write_varint(packet_id) + body
    sock.sendall(write_varint(len(payload)) + payload)


def probe_status(address: tuple[str, int], *, timeout: float = 10.0) -> dict[str, Any]:
    """Status request + ping/pong; asserts the supported release metadata."""
    with socket.create_connection(address, timeout=timeout) as sock:
        sock.settimeout(timeout)
        sock.sendall(frame(0, handshake("localhost", 25565, 1)))
        sock.sendall(frame(0))
        packet_id, body = read_frame(sock)
        if packet_id != 0:
            raise RuntimeError(f"unexpected status packet id {packet_id:#x}")
        status = json.loads(read_mc_string(body))
        if status["version"] != {"name": RELEASE, "protocol": PROTOCOL}:
            raise RuntimeError(f"wrong supported metadata: {status['version']!r}")
        sock.sendall(frame(1, struct.pack(">q", 0x102030405060708)))
        packet_id, body = read_frame(sock)
        if packet_id != 1 or body != struct.pack(">q", 0x102030405060708):
            raise RuntimeError("ping/pong was not preserved")
        return status


def join_offline_socket(
    address: tuple[str, int], name: str, *, timeout: float = 120.0
) -> tuple[socket.socket, dict[str, Any]]:
    """Join an offline server through LoginPlay and return the live Play socket.

    The caller owns and must close the returned socket.
    """
    host, port = address
    seen: list[int] = []
    sock = socket.create_connection(address, timeout=timeout)
    try:
        sock.settimeout(timeout)
        sock.sendall(frame(0, handshake(host, port, 2)))
        sock.sendall(
            frame(LOGIN_SERVERBOUND_START, mc_string(name) + uuid.uuid4().bytes)
        )

        packet_id, body = read_frame(sock)
        if packet_id == LOGIN_CLIENTBOUND_DISCONNECT:
            raise RuntimeError(f"login disconnected: {body[:200]!r}")
        if packet_id != LOGIN_CLIENTBOUND_SET_COMPRESSION:
            raise RuntimeError(f"expected SetCompression, got {packet_id:#x}")
        threshold, _ = read_varint_bytes(body)
        packet_id, _ = read_compressed_frame(sock)
        if packet_id != LOGIN_CLIENTBOUND_SUCCESS:
            raise RuntimeError(
                f"expected LoginSuccess, got {packet_id:#x} (compression {threshold})"
            )
        send_compressed_frame(sock, LOGIN_SERVERBOUND_ACK)

        while True:
            packet_id, body = read_compressed_frame(sock)
            seen.append(packet_id)
            if packet_id == CONFIG_CLIENTBOUND_KNOWN_PACKS:
                # Echo the advertised pack: the client then "owns"
                # minecraft:core:<release> and no full registry sidecar is sent.
                send_compressed_frame(sock, CONFIG_SERVERBOUND_KNOWN_PACKS, body)
                break
            if len(seen) > 64:
                raise RuntimeError(f"no KnownPacks in configuration: {seen}")

        seen = []
        while True:
            packet_id, _ = read_compressed_frame(sock)
            seen.append(packet_id)
            if packet_id == CONFIG_CLIENTBOUND_FINISH:
                break
            if len(seen) > 256:
                raise RuntimeError(f"no FinishConfiguration: {seen}")
        send_compressed_frame(sock, CONFIG_SERVERBOUND_ACK_FINISH)
        config_frames = len(seen)

        seen = []
        while True:
            packet_id, _ = read_compressed_frame(sock)
            seen.append(packet_id)
            if packet_id == PLAY_CLIENTBOUND_LOGIN:
                return sock, {
                    "name": name,
                    "compression_threshold": threshold,
                    "configuration_frames": config_frames,
                    "play_frames_before_login": len(seen) - 1,
                }
            if len(seen) > 512:
                raise RuntimeError(f"no LoginPlay after configuration: {seen}")
    except BaseException:
        sock.close()
        raise


def wait_play_loop(sock: socket.socket, *, timeout: float = 120.0) -> None:
    """Block until the first Play KeepAlive.

    The keepalive is written by the play writer after the initial join burst,
    so receiving it proves the connection reached the ordinary play loop and a
    later close takes the normal leave path.
    """
    sock.settimeout(timeout)
    while True:
        packet_id, _ = read_compressed_frame(sock)
        if packet_id == PLAY_CLIENTBOUND_KEEPALIVE:
            return


def join_offline(
    address: tuple[str, int], name: str, *, timeout: float = 120.0
) -> dict[str, Any]:
    """Offline Login -> Configuration -> first Play ``LoginPlay`` frame."""
    sock, result = join_offline_socket(address, name, timeout=timeout)
    sock.close()
    return result
