"""Static tests for the light-seam gate's wire decoder and analytic oracle; not client evidence."""
from __future__ import annotations

import socket
import struct
import tempfile
import threading
import unittest
import zlib
from pathlib import Path

from PIL import Image

from .backends import light_pixels, light_wire
from .backends.light_seam_model import SeamModel


def _varint(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 127
        value >>= 7
        out.append(byte | (128 if value else 0))
        if not value:
            return bytes(out)


def _longs(bits: list[int]) -> bytes:
    words = [0] * ((max(bits) // 64 + 1) if bits else 0)
    for bit in bits:
        words[bit // 64] |= 1 << (bit % 64)
    return _varint(len(words)) + b"".join(struct.pack(">Q", word) for word in words)


def _light(sky: dict[int, bytes], block: dict[int, bytes], empty_sky: list[int] = (), empty_block: list[int] = ()) -> bytes:
    out = _longs(sorted(sky)) + _longs(sorted(block)) + _longs(list(empty_sky)) + _longs(list(empty_block))
    for layers in (sky, block):
        out += _varint(len(layers)) + b"".join(_varint(2048) + layers[key] for key in sorted(layers))
    return out


def _layer(value: int) -> bytes:
    return bytes([value | value << 4]) * 2048


def _chunk(x: int, z: int, light: bytes) -> bytes:
    heightmap = _varint(1) + _varint(4) + _varint(1) + struct.pack(">q", 0)
    block_entity = bytes([0x00, 0, 70]) + _varint(7) + bytes([10, 3]) + struct.pack(">H", 1) + b"x" + struct.pack(">i", 5) \
        + bytes([8]) + struct.pack(">H", 1) + b"y" + struct.pack(">H", 2) + b"ok" + bytes([0])
    return (struct.pack(">ii", x, z) + heightmap + _varint(3) + b"\x01\x02\x03"
            + _varint(1) + block_entity + light)


class WireLightTests(unittest.TestCase):
    def test_chunk_packet_with_block_entity_nbt_decodes_masks_and_layers(self) -> None:
        wire = light_wire.WireLight()
        wire.chunk_packet(_chunk(3, -2, _light({5: _layer(1), 20: _layer(15)}, {5: _layer(7)})))
        self.assertEqual(wire.chunks[(3, -2)].sky.keys(), {5, 20})
        # y=2 is layer 5; above the highest received sky layer the client reads 15.
        self.assertEqual(wire.levels(3 * 16 + 1, 4 * 16 + 2 - 64 + 0, -2 * 16 + 3), (1, 7))
        self.assertEqual(wire.levels(48, 400, -32)[0], 15)

    def test_light_update_replaces_masked_and_zeroes_empty_layers_only(self) -> None:
        wire = light_wire.WireLight()
        wire.chunk_packet(_chunk(0, 0, _light({2: _layer(9), 3: _layer(9)}, {2: _layer(4)})))
        update = _varint(0) + _varint(0) + _light({2: _layer(5)}, {}, empty_block=[2])
        wire.light_packet(update)
        chunk = wire.chunks[(0, 0)]
        self.assertEqual(chunk.sky[2][0], 0x55)
        self.assertEqual(chunk.sky[3][0], 0x99)
        self.assertEqual(chunk.block[2], bytes(2048))
        self.assertFalse(chunk.fully_dark())

    def test_update_for_unreceived_chunk_is_dropped_and_forget_removes(self) -> None:
        wire = light_wire.WireLight()
        wire.light_packet(_varint(4) + _varint(5) + _light({1: _layer(1)}, {}))
        self.assertEqual(wire.chunks, {})
        wire.chunk_packet(_chunk(-1, 7, _light({}, {})))
        self.assertTrue(wire.chunks[(-1, 7)].fully_dark())
        wire.forget_packet(struct.pack(">q", ((-1) & 0xFFFFFFFF) | (7 << 32)))
        self.assertEqual(wire.chunks, {})
        self.assertEqual([event["kind"] for event in wire.events], ["light", "chunk", "forget"])

    def test_disagreeing_mask_and_arrays_fail_closed(self) -> None:
        bad = _longs([1]) + _longs([]) + _longs([]) + _longs([]) + _varint(0) + _varint(0)
        with self.assertRaisesRegex(ValueError, "disagree"):
            light_wire.read_light(light_wire.Reader(bad))


def _frame(packet_id: int, payload: bytes, compressed: bool) -> bytes:
    body = _varint(packet_id) + payload
    if compressed:
        body = _varint(len(body)) + zlib.compress(body) if len(body) > 8 else _varint(0) + body
    return _varint(len(body)) + body


class ProxyTests(unittest.TestCase):
    def test_proxy_follows_phases_and_compression_to_play_light_packets(self) -> None:
        server = socket.socket()
        server.bind(("127.0.0.1", 0))
        server.listen()

        def serve() -> None:
            connection, _ = server.accept()
            with connection:
                stream = (_frame(3, _varint(8), False)  # login: set compression
                          + _frame(2, b"", True)  # login success
                          + _frame(3, b"", True)  # finish configuration
                          + _frame(light_wire.LEVEL_CHUNK_WITH_LIGHT, _chunk(2, 3, _light({1: _layer(6)}, {})), True)
                          + _frame(light_wire.LIGHT_UPDATE, _varint(2) + _varint(3) + _light({1: _layer(9)}, {}), True))
                connection.sendall(stream[:7])  # split mid-frame: the proxy must buffer
                connection.sendall(stream[7:])
                connection.recv(1)

        thread = threading.Thread(target=serve, daemon=True)
        thread.start()
        with tempfile.TemporaryDirectory() as directory:
            proxy = light_wire.LightWireProxy(server.getsockname()[1], Path(directory))
            try:
                client = socket.create_connection(("127.0.0.1", int(proxy.address.rsplit(":", 1)[1])))
                deadline = threading.Event()
                for _ in range(200):
                    if proxy.connection_count() and proxy.connections[0].played and \
                            len(proxy.connections[0].events) == 2:
                        break
                    deadline.wait(0.05)
                events = proxy.query(lambda wire: [(e["kind"], tuple(e["chunk"])) for e in wire.events])
                level = proxy.query(lambda wire: wire.levels(2 * 16, -64, 3 * 16))
                client.close()
            finally:
                proxy.close()
                server.close()
        self.assertEqual(events, [("chunk", (2, 3)), ("light", (2, 3))])
        self.assertEqual(level, (9, 0))
        self.assertEqual(proxy.errors, [])


class PixelPredicateTests(unittest.TestCase):
    def _frame(self, directory: str, black_block: bool) -> Path:
        image = Image.new("RGB", (1280, 720), (110, 110, 110))
        if black_block:
            image.paste((12, 12, 12), (500, 300, 580, 380))
        path = Path(directory) / "wall.png"
        image.save(path)
        return path

    def test_uniform_sunlit_wall_passes_and_a_black_block_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            self.assertTrue(light_pixels.dark_tiles(self._frame(directory, False))["passed"])
            dark = light_pixels.dark_tiles(self._frame(directory, True))
        self.assertFalse(dark["passed"])
        self.assertTrue(dark["dark_tiles"])

    def test_uniformly_dark_wall_fails_on_median(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "dark.png"
            Image.new("RGB", (1280, 720), (20, 20, 20)).save(path)
            self.assertFalse(light_pixels.dark_tiles(path)["passed"])


class SeamOracleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.model = SeamModel(160, 200, 32)
        self.expected = self.model.expected()

    def at(self, dx: int, dy: int, dz: int) -> tuple[int, int]:
        return self.expected[(160 + dx, 200 + dy, 32 + dz)]

    def test_torch_block_light_falls_off_across_the_border_under_the_roof(self) -> None:
        self.assertEqual(self.at(3, 1, 5)[1], 14)
        self.assertEqual([self.at(dx, 1, 5)[1] for dx in (2, 1, 0, -1, -2)], [13, 12, 11, 10, 9])

    def test_roof_and_tunnel_shadow_sky_light_from_the_open_edges(self) -> None:
        self.assertEqual(self.at(-1, 6, 5), (15, 0))
        self.assertEqual(self.at(-6, 3, 5)[0], 14)  # one block inside the west roof edge
        self.assertEqual(self.at(-1, 3, 5)[0], 11)  # four columns inside the nearest (z) edge
        self.assertEqual(self.at(-8, 1, 13)[0], 14)  # tunnel mouth beside open sky
        self.assertEqual(self.at(-1, 1, 13)[0], 7)  # seven blocks into the tunnel
        self.assertEqual(self.at(4, 1, 13), (2, 14))  # twelve into the tunnel, on the torch
        self.assertEqual(self.at(5, 1, 13), (1, 13))

    def test_region_omits_solid_cells_and_fixture_commands_fit_fill_limit(self) -> None:
        self.assertNotIn((160, 204, 37), self.expected)
        self.assertNotIn((160, 201, 44), self.expected)
        self.assertIn((160, 201, 45), self.expected)
        self.assertEqual(len(self.model.region()), 24 * 7 * 16)


if __name__ == "__main__":
    unittest.main()
