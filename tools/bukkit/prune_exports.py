"""Retain only the Java callbacks needed by the WIT adapter before Binaryen DCE."""

from pathlib import Path
import sys


def read_uint(data: bytes, offset: int) -> tuple[int, int]:
    value = 0
    for shift in range(0, 35, 7):
        byte = data[offset]
        offset += 1
        value |= (byte & 127) << shift
        if not byte & 128:
            return value, offset
    raise ValueError("invalid Wasm u32")


def uint(value: int) -> bytes:
    output = bytearray()
    while value >= 128:
        output.append((value & 127) | 128)
        value >>= 7
    output.append(value)
    return bytes(output)


def prune(data: bytes, keep: set[str]) -> bytes:
    if data[:8] != b"\x00asm\x01\x00\x00\x00":
        raise ValueError("expected a core Wasm module")
    output = bytearray(data[:8])
    offset = 8
    found = set()
    while offset < len(data):
        section = data[offset]
        size, start = read_uint(data, offset + 1)
        end = start + size
        if end > len(data):
            raise ValueError("truncated Wasm section")
        payload = data[start:end]
        if section == 7:
            count, position = read_uint(payload, 0)
            exports = []
            for _ in range(count):
                entry_start = position
                length, position = read_uint(payload, position)
                name = payload[position:position + length].decode("utf-8")
                position += length + 1  # export kind
                _, position = read_uint(payload, position)
                if name in keep:
                    exports.append(payload[entry_start:position])
                    found.add(name)
            if position != len(payload):
                raise ValueError("malformed export section")
            payload = uint(len(exports)) + b"".join(exports)
        output.append(section)
        output.extend(uint(len(payload)))
        output.extend(payload)
        offset = end
    if found != keep:
        raise ValueError(f"missing required Java exports: {sorted(keep - found)}")
    return bytes(output)


if __name__ == "__main__":
    source, destination, *exports = sys.argv[1:]
    Path(destination).write_bytes(prune(Path(source).read_bytes(), set(exports)))
