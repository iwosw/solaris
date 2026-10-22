"""Minimal read-only Anvil/NBT reader for the vanilla oracle's saved world.

Only what the parity gate needs: block snapshots out of `region/*.mca` and
entity snapshots out of `entities/*.mca` (26.1.2 layout: one directory per
dimension under `dimensions/minecraft/<dimension>/`).
"""
from __future__ import annotations

import struct
import zlib
from pathlib import Path

_END, _BYTE, _SHORT, _INT, _LONG, _FLOAT, _DOUBLE, _BYTES, _STRING, _LIST, _COMPOUND, _INTS, _LONGS = range(13)


class _Reader:
    def __init__(self, data: bytes):
        self.data, self.pos = data, 0

    def take(self, n: int) -> bytes:
        chunk = self.data[self.pos:self.pos + n]
        if len(chunk) != n:
            raise ValueError("truncated NBT")
        self.pos += n
        return chunk

    def number(self, fmt: str):
        return struct.unpack(fmt, self.take(struct.calcsize(fmt)))[0]

    def text(self) -> str:
        return self.take(self.number(">H")).decode("utf-8", "surrogatepass")

    def payload(self, kind: int):
        simple = {_BYTE: ">b", _SHORT: ">h", _INT: ">i", _LONG: ">q", _FLOAT: ">f", _DOUBLE: ">d"}
        if kind in simple:
            return self.number(simple[kind])
        if kind == _BYTES:
            return list(struct.unpack(f">{(n := self.number('>i'))}b", self.take(n)))
        if kind == _STRING:
            return self.text()
        if kind == _LIST:
            child, count = self.number(">B"), self.number(">i")
            return [self.payload(child) for _ in range(max(count, 0))]
        if kind == _COMPOUND:
            out = {}
            while (child := self.number(">B")) != _END:
                name = self.text()
                out[name] = self.payload(child)
            return out
        if kind == _INTS:
            n = self.number(">i")
            return list(struct.unpack(f">{n}i", self.take(4 * n)))
        if kind == _LONGS:
            n = self.number(">i")
            return list(struct.unpack(f">{n}q", self.take(8 * n)))
        raise ValueError(f"unsupported NBT tag {kind}")


def parse_nbt(data: bytes) -> dict:
    reader = _Reader(data)
    kind = reader.number(">B")
    reader.text()
    return reader.payload(kind)


def read_region(path: Path):
    """Yield (chunk_x, chunk_z, nbt) for every stored chunk of one .mca file."""
    raw = path.read_bytes()
    if len(raw) < 8192:
        return
    for index in range(1024):
        entry = struct.unpack(">I", raw[index * 4:index * 4 + 4])[0]
        offset, sectors = entry >> 8, entry & 0xFF
        if offset == 0 or sectors == 0:
            continue
        start = offset * 4096
        length, compression = struct.unpack(">IB", raw[start:start + 5])
        body = raw[start + 5:start + 4 + length]
        if compression == 2:
            body = zlib.decompress(body)
        elif compression == 1:
            import gzip
            body = gzip.decompress(body)
        elif compression != 3:
            raise ValueError(f"{path}: unsupported region compression {compression}")
        yield index & 31, index >> 5, parse_nbt(body)


def _block_string(entry: dict) -> str:
    props = entry.get("Properties")
    if not props:
        return entry["Name"]
    return entry["Name"] + "[" + ",".join(f"{k}={props[k]}" for k in sorted(props)) + "]"


def _section_blocks(section: dict):
    states = section.get("block_states") or {}
    palette = [_block_string(entry) for entry in states.get("palette", [])]
    if not palette:
        return
    if len(palette) == 1:
        for index in range(4096):
            yield index, palette[0]
        return
    bits = max(4, (len(palette) - 1).bit_length())
    per_long = 64 // bits
    mask = (1 << bits) - 1
    data = [value & 0xFFFFFFFFFFFFFFFF for value in states["data"]]
    for index in range(4096):
        word = data[index // per_long]
        yield index, palette[(word >> ((index % per_long) * bits)) & mask]


def _chunks_of(boxes):
    wanted = set()
    for x1, _y1, z1, x2, _y2, z2 in boxes:
        for chunk_x in range(x1 >> 4, (x2 >> 4) + 1):
            for chunk_z in range(z1 >> 4, (z2 >> 4) + 1):
                wanted.add((chunk_x, chunk_z))
    return wanted


def _regions(directory: Path, wanted):
    """Yield (chunk_x, chunk_z, nbt) for wanted chunks, parsing each region once."""
    by_region: dict[tuple[int, int], set] = {}
    for chunk in wanted:
        by_region.setdefault((chunk[0] >> 5, chunk[1] >> 5), set()).add(chunk)
    for (rx, rz), chunks in by_region.items():
        file = directory / f"r.{rx}.{rz}.mca"
        if not file.is_file():
            continue
        for cx, cz, nbt in read_region(file):
            chunk = (cx + (rx << 5), cz + (rz << 5))
            if chunk in chunks:
                yield chunk, nbt


def read_world(dimension_dir: Path, boxes) -> list[dict]:
    """Per-box {"blocks": {"x,y,z": state}, "entities": [raw NBT]}.

    Blocks are non-air only; entities are those whose position is inside the box.
    """
    cells = [{"blocks": {}, "entities": [], "containers": {}} for _ in boxes]
    wanted = _chunks_of(boxes)
    for (chunk_x, chunk_z), nbt in _regions(dimension_dir / "region", wanted):
        mine = [i for i, b in enumerate(boxes)
                if b[0] >> 4 <= chunk_x <= b[3] >> 4 and b[2] >> 4 <= chunk_z <= b[5] >> 4]
        for entity in nbt.get("block_entities", []):
            items = entity.get("Items")
            if not items:
                continue
            x, y, z = entity["x"], entity["y"], entity["z"]
            for i in mine:
                x1, y1, z1, x2, y2, z2 = boxes[i]
                if x1 <= x <= x2 and y1 <= y <= y2 and z1 <= z <= z2:
                    totals = cells[i]["containers"].setdefault(f"{x},{y},{z}", {})
                    for stack in items:
                        totals[stack["id"]] = totals.get(stack["id"], 0) + stack.get("count", stack.get("Count", 1))
        for section in nbt.get("sections", []):
            base_y = section["Y"] * 16
            relevant = [i for i in mine if boxes[i][1] <= base_y + 15 and boxes[i][4] >= base_y]
            if not relevant:
                continue
            for index, name in _section_blocks(section):
                if name in ("minecraft:air", "minecraft:cave_air", "minecraft:void_air"):
                    continue
                x = chunk_x * 16 + (index & 15)
                z = chunk_z * 16 + ((index >> 4) & 15)
                y = base_y + (index >> 8)
                for i in relevant:
                    x1, y1, z1, x2, y2, z2 = boxes[i]
                    if x1 <= x <= x2 and y1 <= y <= y2 and z1 <= z <= z2:
                        cells[i]["blocks"][f"{x},{y},{z}"] = name
    for (chunk_x, chunk_z), nbt in _regions(dimension_dir / "entities", wanted):
        for entity in nbt.get("Entities", []):
            x, y, z = entity.get("Pos", [0.0, 0.0, 0.0])
            for i, (x1, y1, z1, x2, y2, z2) in enumerate(boxes):
                if x1 <= x < x2 + 1 and y1 <= y < y2 + 1 and z1 <= z < z2 + 1:
                    cells[i]["entities"].append(entity)
    return cells
