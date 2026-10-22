"""Loopback proxy that records the light the server actually put on the wire.

The server has no live light query, so the authoritative server side of the
light-seam gate is its own `LevelChunkWithLight`/`LightUpdate` stream. Chunk
state follows the native client's receive rules (mask layers replace, empty-mask
layers become zero, a chunk packet replaces the whole chunk) and the same
vanilla storage fallbacks, so a client/server mismatch is a transport or
reconciliation defect rather than a decoder difference.
"""
from __future__ import annotations

import json
import select
import socket
import struct
import threading
import zlib
from pathlib import Path
from typing import Any, Callable

LEVEL_CHUNK_WITH_LIGHT = 0x2D
LIGHT_UPDATE = 0x30
FORGET_LEVEL_CHUNK = 0x25
LAYER_BYTES = 2048
MIN_Y = -64


class Reader:
    def __init__(self, data: bytes, offset: int = 0) -> None:
        self.data, self.offset = data, offset

    def take(self, count: int) -> bytes:
        if count < 0 or self.offset + count > len(self.data):
            raise ValueError("truncated wire packet")
        chunk = self.data[self.offset:self.offset + count]
        self.offset += count
        return chunk

    def varint(self) -> int:
        value = 0
        for index in range(5):
            byte = self.take(1)[0]
            value |= (byte & 127) << (7 * index)
            if byte < 128:
                return value - (1 << 32) if value & (1 << 31) else value
        raise ValueError("oversized wire VarInt")

    def int32(self) -> int:
        return struct.unpack(">i", self.take(4))[0]

    def longs(self) -> list[int]:
        return [struct.unpack(">q", self.take(8))[0] for _ in range(self.varint())]

    def byte_array(self) -> bytes:
        return self.take(self.varint())

    def skip_nbt(self) -> None:
        """Skip one nameless network NBT root (heightmaps never use it; block entities do)."""
        kind = self.take(1)[0]
        if kind:
            self._skip_payload(kind)

    def _skip_payload(self, kind: int) -> None:
        fixed = {1: 1, 2: 2, 3: 4, 4: 8, 5: 4, 6: 8}
        if kind in fixed:
            self.take(fixed[kind])
        elif kind == 7:
            self.take(struct.unpack(">i", self.take(4))[0])
        elif kind == 8:
            self.take(struct.unpack(">H", self.take(2))[0])
        elif kind == 9:
            element = self.take(1)[0]
            for _ in range(struct.unpack(">i", self.take(4))[0]):
                self._skip_payload(element)
        elif kind == 10:
            while (child := self.take(1)[0]) != 0:
                self.take(struct.unpack(">H", self.take(2))[0])
                self._skip_payload(child)
        elif kind in (11, 12):
            self.take(struct.unpack(">i", self.take(4))[0] * (4 if kind == 11 else 8))
        else:
            raise ValueError(f"unknown NBT tag {kind}")


def _bits(longs: list[int]) -> list[int]:
    return [index * 64 + bit for index, word in enumerate(longs) for bit in range(64)
            if (word & 0xFFFFFFFFFFFFFFFF) >> bit & 1]


def read_light(reader: Reader) -> dict[str, Any]:
    masks = [reader.longs() for _ in range(4)]
    lists = []
    for mask in masks[:2]:
        layers = [reader.byte_array() for _ in range(reader.varint())]
        if len(layers) != len(_bits(mask)) or any(len(layer) != LAYER_BYTES for layer in layers):
            raise ValueError("light mask and arrays disagree")
        lists.append(dict(zip(_bits(mask), layers)))
    return {"sky": lists[0], "block": lists[1],
            "empty_sky": _bits(masks[2]), "empty_block": _bits(masks[3])}


def read_chunk_with_light(payload: bytes) -> tuple[int, int, dict[str, Any]]:
    reader = Reader(payload)
    x, z = reader.int32(), reader.int32()
    for _ in range(reader.varint()):
        reader.varint()
        reader.longs()
    reader.byte_array()
    for _ in range(reader.varint()):
        reader.take(1)
        reader.take(2)
        reader.varint()
        reader.skip_nbt()
    light = read_light(reader)
    if reader.offset != len(payload):
        raise ValueError("trailing bytes after chunk light")
    return x, z, light


def read_light_update(payload: bytes) -> tuple[int, int, dict[str, Any]]:
    reader = Reader(payload)
    x, z = reader.varint(), reader.varint()
    light = read_light(reader)
    if reader.offset != len(payload):
        raise ValueError("trailing bytes after light update")
    return x, z, light


class ChunkLight:
    """One chunk's received light layers; layer 0 is the section below the world."""

    def __init__(self) -> None:
        self.sky: dict[int, bytes] = {}
        self.block: dict[int, bytes] = {}

    def apply(self, light: dict[str, Any]) -> None:
        for store, layers, empty in ((self.sky, light["sky"], light["empty_sky"]),
                                     (self.block, light["block"], light["empty_block"])):
            store.update(layers)
            store.update({layer: bytes(LAYER_BYTES) for layer in empty})

    def fully_dark(self) -> bool:
        return not any(any(layer) for store in (self.sky, self.block) for layer in store.values())

    def levels(self, x: int, y: int, z: int) -> tuple[int, int] | None:
        """(sky, block) with the client's vanilla storage fallbacks; None when unknown."""
        layer = (y - MIN_Y) // 16 + 1
        if not self.sky:
            return None
        if layer > max(self.sky):
            sky = 15
        else:
            source = self.sky.get(layer)
            row = y
            if source is None:
                source = next((self.sky[key] for key in sorted(self.sky) if key >= max(layer, 0)), None)
                if source is None:
                    return None
                row = 0
            sky = _nibble(source, x, row, z)
        block_layer = self.block.get(layer)
        return sky, 0 if block_layer is None else _nibble(block_layer, x, y, z)


def _nibble(layer: bytes, x: int, y: int, z: int) -> int:
    index = (y % 16) * 256 + (z % 16) * 16 + (x % 16)
    return (layer[index // 2] >> ((index % 2) * 4)) & 15


class WireLight:
    """Chunk-light table plus the ordered chunk/light/forget packet history."""

    def __init__(self) -> None:
        self.chunks: dict[tuple[int, int], ChunkLight] = {}
        self.events: list[dict[str, Any]] = []
        self.played = False

    def chunk_packet(self, payload: bytes) -> None:
        x, z, light = read_chunk_with_light(payload)
        self.chunks[(x, z)] = fresh = ChunkLight()
        fresh.apply(light)
        self._record("chunk", x, z, light)

    def light_packet(self, payload: bytes) -> None:
        x, z, light = read_light_update(payload)
        # An update for a chunk the client never received is dropped, like the client.
        if (x, z) in self.chunks:
            self.chunks[(x, z)].apply(light)
        self._record("light", x, z, light)

    def forget_packet(self, payload: bytes) -> None:
        packed = struct.unpack(">q", Reader(payload).take(8))[0]  # ChunkPos.pack: x low 32 bits, z high 32
        x, z = struct.unpack(">i", struct.pack(">I", packed & 0xFFFFFFFF))[0], packed >> 32
        self.chunks.pop((x, z), None)
        self.events.append({"index": len(self.events), "kind": "forget", "chunk": [x, z]})

    def _record(self, kind: str, x: int, z: int, light: dict[str, Any]) -> None:
        self.events.append({"index": len(self.events), "kind": kind, "chunk": [x, z],
                            "sky_layers": sorted(light["sky"]), "block_layers": sorted(light["block"]),
                            "empty_sky": light["empty_sky"], "empty_block": light["empty_block"]})

    def levels(self, x: int, y: int, z: int) -> tuple[int, int] | None:
        chunk = self.chunks.get((x // 16, z // 16))
        return None if chunk is None else chunk.levels(x, y, z)


class LightWireProxy:
    """Unmodified forwarding listener; every server Play light packet feeds `WireLight`."""

    def __init__(self, server_port: int, artifact: Path,
                 packet_observer: Callable[[str, int, bytes], None] | None = None,
                 client_packet_observer: Callable[[str, int, bytes], None] | None = None) -> None:
        self.server_port = server_port
        self.path = artifact / "light-wire-trace.json"
        self.packet_observer = packet_observer
        self.client_packet_observer = client_packet_observer
        self.lock = threading.Lock()
        self.connections: list[WireLight] = []
        self.errors: list[str] = []
        self.threads: list[threading.Thread] = []
        self.wake, self.stop = socket.socketpair()
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen()
        self.address = f"127.0.0.1:{self.listener.getsockname()[1]}"
        self.worker = threading.Thread(target=self._accept, daemon=True)
        self.worker.start()

    def query(self, fn: Any) -> Any:
        """Run `fn(latest connection's WireLight)` while the receive thread is excluded."""
        with self.lock:
            return fn(next(wire for wire in reversed(self.connections) if wire.played))

    def connection_count(self) -> int:
        with self.lock:
            return len(self.connections)

    def _accept(self) -> None:
        while self.listener in select.select([self.listener, self.wake], [], [])[0]:
            client, _ = self.listener.accept()
            wire = WireLight()
            with self.lock:
                self.connections.append(wire)
            thread = threading.Thread(target=self._forward, args=(client, wire), daemon=True)
            self.threads.append(thread)
            thread.start()

    def _forward(self, client: socket.socket, wire: WireLight) -> None:
        try:
            with client, socket.create_connection(("127.0.0.1", self.server_port), timeout=10) as server:
                buffer = bytearray()
                client_buffer = bytearray() if self.client_packet_observer is not None else None
                phase, compressed = "login", False
                while True:
                    ready = select.select([client, server, self.wake], [], [])[0]
                    if self.wake in ready:
                        return
                    for source, target in ((client, server), (server, client)):
                        if source not in ready:
                            continue
                        data = source.recv(65536)
                        if not data:
                            return
                        if source is client:
                            if client_buffer is not None:
                                client_buffer.extend(data)
                                self._frames(client_buffer, wire, phase, compressed, serverbound=True)
                            target.sendall(data)
                            continue
                        buffer.extend(data)
                        phase, compressed = self._frames(buffer, wire, phase, compressed)
                        target.sendall(data)
        except ConnectionError:
            return  # an ordinary client/server disconnect (restart, shutdown), not a decode failure
        except Exception as error:
            self.errors.append(f"{type(error).__name__}: {error}")

    def _frames(self, buffer: bytearray, wire: WireLight, phase: str, compressed: bool,
                *, serverbound: bool = False) -> tuple[str, bool]:
        while True:
            try:
                header = Reader(bytes(buffer[:5]))
                length = header.varint()
            except ValueError:
                return phase, compressed
            if length > 16 * 1024 * 1024:
                raise ValueError("oversized local server frame")
            if len(buffer) < header.offset + length:
                return phase, compressed
            body = bytes(buffer[header.offset:header.offset + length])
            del buffer[:header.offset + length]
            if compressed:
                inner = Reader(body)
                raw_length = inner.varint()
                body = zlib.decompress(body[inner.offset:]) if raw_length else body[inner.offset:]
            reader = Reader(body)
            packet_id = reader.varint()
            payload = body[reader.offset:]
            if serverbound:
                with self.lock:
                    self.client_packet_observer(phase, packet_id, payload)
                continue
            with self.lock:
                if self.packet_observer is not None:
                    self.packet_observer(phase, packet_id, payload)
                if phase == "play" and packet_id == LEVEL_CHUNK_WITH_LIGHT:
                    wire.chunk_packet(payload)
                elif phase == "play" and packet_id == LIGHT_UPDATE:
                    wire.light_packet(payload)
                elif phase == "play" and packet_id == FORGET_LEVEL_CHUNK:
                    wire.forget_packet(payload)
            if phase == "login" and packet_id == 3:
                compressed = True
            elif phase == "login" and packet_id == 2:
                phase = "configuration"
            elif phase == "configuration" and packet_id == 3:
                phase = "play"
                wire.played = True

    def close(self) -> None:
        self.stop.send(b"x")
        self.worker.join(timeout=10)
        for thread in self.threads:
            thread.join(timeout=10)
        self.listener.close()
        self.wake.close()
        self.stop.close()
        self.path.write_text(json.dumps({
            "origin": "unmodified_loopback_server_stream", "errors": self.errors,
            "connections": [wire.events for wire in self.connections]}) + "\n")
