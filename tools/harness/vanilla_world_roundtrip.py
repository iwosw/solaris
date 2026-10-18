#!/usr/bin/env python3
"""Real Java -> offline import -> native v2 -> offline export -> Java -> native gate.

Requires an existing accepted eula.txt; this driver does not accept legal terms.
Run: python3 tools/harness/vanilla_world_roundtrip.py --eula-file PATH
Receipts and complete process logs are retained under --output.
All runtime worlds are native-only; Java uses separate offline Anvil exports.
The original 18 checks remain, including Java mutations and fresh native import.
The Java-changed native import is also re-exported, with both conversion receipts.
"""
from __future__ import annotations

import argparse
import gzip
import json
import queue
import shutil
import socket
import struct
import subprocess
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TEST = "dimensions::tests::vanilla_headless_world_roundtrip"


def read_nbt(path: Path):
    """Small read-only NBT decoder for the server's saved level.dat evidence."""
    data = memoryview(gzip.decompress(path.read_bytes()))
    offset = 0

    def take(size):
        nonlocal offset
        if size < 0 or offset + size > len(data):
            raise ValueError("truncated NBT")
        value = data[offset:offset + size]
        offset += size
        return value

    def number(fmt):
        return struct.unpack(fmt, take(struct.calcsize(fmt)))[0]

    def text():
        return bytes(take(number(">H"))).decode("utf-8")

    def payload(kind):
        formats = {1: ">b", 2: ">h", 3: ">i", 4: ">q", 5: ">f", 6: ">d"}
        if kind in formats:
            return number(formats[kind])
        if kind == 7:
            return list(bytes(take(number(">i"))))
        if kind == 8:
            return text()
        if kind == 9:
            child, count = number(">B"), number(">i")
            if count < 0:
                raise ValueError("negative NBT list length")
            return [payload(child) for _ in range(count)]
        if kind == 10:
            result = {}
            while (child := number(">B")) != 0:
                name = text()
                result[name] = payload(child)
            return result
        if kind in (11, 12):
            count = number(">i")
            if count < 0:
                raise ValueError("negative NBT array length")
            return [number(">i" if kind == 11 else ">q") for _ in range(count)]
        raise ValueError(f"unsupported NBT tag {kind}")

    kind = number(">B")
    text()
    return payload(kind)


def seed_startup_tickets(world: Path):
    """Configure Java chunk residency, not any round-trip feature payload.

    26.1.2 TicketStorage.TYPE is minecraft:chunk_tickets. Its CODEC uses
    tickets[{chunk_pos: IntArray[x,z], type: String, level: Int}]; FORCED
    has no timeout and ChunkMap.FORCED_TICKET_LEVEL is 31.
    """
    def text(value):
        encoded = value.encode("utf-8")
        return struct.pack(">H", len(encoded)) + encoded

    def field(kind, name, payload):
        return bytes([kind]) + text(name) + payload

    ticket = field(11, "chunk_pos", struct.pack(">iii", 2, 0, 0))
    ticket += field(8, "type", text("minecraft:forced"))
    ticket += field(3, "level", struct.pack(">i", 31)) + b"\0"
    data = field(9, "tickets", b"\x0a" + struct.pack(">i", 1) + ticket) + b"\0"
    root = b"\x0a\0\0" + field(10, "data", data)
    root += field(3, "DataVersion", struct.pack(">i", 4790)) + b"\0"
    for dimension in ("overworld", "the_nether", "the_end"):
        path = world / "dimensions/minecraft" / dimension / "data/minecraft/chunk_tickets.dat"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(gzip.compress(root))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--jar", type=Path, default=ROOT / ".analysis/server.jar")
    parser.add_argument("--java", default="java")
    parser.add_argument("--eula-file", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--timeout", type=int, default=180)
    args = parser.parse_args()
    output = (args.output or ROOT / ".analysis/vanilla-world-roundtrip" / time.strftime("%Y%m%dT%H%M%S")).resolve()
    output.mkdir(parents=True, exist_ok=False)
    receipt = {"profile": "vanilla-26.1.2-world-roundtrip", "passed": False, "graphical_clients": 0,
               "world": str(output / "server/world"), "checks": {}, "logs": {}}
    server = None
    try:
        if not args.jar.is_file():
            raise RuntimeError(f"missing runnable vanilla bundler jar: {args.jar}")
        if not args.eula_file.is_file() or "eula=true" not in args.eula_file.read_text().splitlines():
            raise RuntimeError("--eula-file must point to an already accepted eula.txt")
        home = output / "server"
        home.mkdir()
        shutil.copyfile(args.eula_file, home / "eula.txt")
        world = home / "world"
        runner = ROOT / ".analysis/tools/mem-run.sh"

        def v2(phase, native, anvil):
            command = [str(runner), "-x", "-m", "8G", "env", "CARGO_BUILD_JOBS=6",
                       "CARGO_PROFILE_DEV_DEBUG=0", "CARGO_PROFILE_TEST_DEBUG=0",
                       f"SOLARIS_VANILLA_ROUNDTRIP_WORLD={native}", f"SOLARIS_VANILLA_ROUNDTRIP_PHASE={phase}",
                       f"SOLARIS_VANILLA_ROUNDTRIP_ANVIL={anvil}",
                       f"SOLARIS_VANILLA_ROUNDTRIP_RECEIPT={output / (phase + '-conversion.json')}",
                       f"SOLARIS_VANILLA_ROUNDTRIP_EXPORT={world if phase == 'create' else output / 'reexport'}",
                       f"SOLARIS_VANILLA_ROUNDTRIP_EXPORT_RECEIPT={output / (phase + '-export.json')}",
                       "cargo", "test", "-p", "mc-core", TEST, "--", "--ignored", "--exact", "--nocapture"]
            receipt.setdefault("conversion_receipts", {})[phase] = {
                "import": str(output / (phase + "-conversion.json")),
                "export": str(output / (phase + "-export.json")),
            }
            log = output / f"v2-{phase}.log"
            with log.open("w") as handle:
                result = subprocess.run(command, cwd=ROOT, stdout=handle, stderr=subprocess.STDOUT,
                                        timeout=args.timeout * 4, check=False)
            receipt["logs"][f"v2_{phase}"] = str(log)
            if result.returncode or f"SOLARIS_VANILLA_ROUNDTRIP_{phase.upper()}_OK" not in log.read_text():
                raise RuntimeError(f"v2 {phase} failed (exit {result.returncode}); see {log}")
            receipt["checks"][f"v2_{phase}"] = True

        bootstrap = output / "java-source"
        bootstrap.mkdir()
        shutil.copyfile(args.eula_file, bootstrap / "eula.txt")
        (bootstrap / "server.properties").write_text(
            "server-ip=127.0.0.1\nserver-port=0\nonline-mode=false\nlevel-name=world\n"
            "level-seed=42\nview-distance=2\nsimulation-distance=2\nmax-players=1\n"
            "generate-structures=false\nspawn-protection=0\nenable-rcon=false\nenable-query=false\n")
        bootstrap_log = output / "java-bootstrap.log"
        receipt["logs"]["java_bootstrap"] = str(bootstrap_log)
        server = subprocess.Popen([str(runner), "-m", "4G", args.java, "-Xms512M", "-Xmx2G", "-jar",
                                   str(args.jar.resolve()), "nogui"], cwd=bootstrap, stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)
        bootstrap_lines = queue.Queue()

        def collect_bootstrap():
            with bootstrap_log.open("w") as handle:
                for line in server.stdout:
                    handle.write(line)
                    handle.flush()
                    bootstrap_lines.put(line)
                bootstrap_lines.put(None)

        bootstrap_thread = threading.Thread(target=collect_bootstrap, daemon=True)
        bootstrap_thread.start()
        deadline = time.monotonic() + args.timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError(f"Java bootstrap timed out; see {bootstrap_log}")
            line = bootstrap_lines.get(timeout=remaining)
            if line is None:
                raise RuntimeError(f"Java bootstrap exited before startup; see {bootstrap_log}")
            if 'Done (' in line:
                break
        server.stdin.write("save-all flush\nstop\n")
        server.stdin.flush()
        server.wait(timeout=60)
        bootstrap_thread.join(timeout=5)
        if server.returncode:
            raise RuntimeError(f"Java bootstrap exit {server.returncode}; see {bootstrap_log}")
        native = output / "native-create"
        receipt["native_create_world"] = str(native)
        v2("create", native, bootstrap / "world")
        seed_startup_tickets(world)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        (home / "server.properties").write_text(
            f"server-ip=127.0.0.1\nserver-port={port}\nonline-mode=false\nlevel-name=world\n"
            "view-distance=2\nsimulation-distance=2\nmax-players=1\nspawn-protection=0\n"
            "enable-rcon=false\nenable-query=false\n")
        vanilla_log = output / "vanilla.log"
        receipt["logs"]["vanilla"] = str(vanilla_log)
        lines = queue.Queue()
        server = subprocess.Popen([str(runner), "-m", "4G", args.java, "-Xms512M", "-Xmx2G", "-jar",
                                   str(args.jar.resolve()), "nogui"], cwd=home, stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)

        def collect():
            with vanilla_log.open("w") as handle:
                for line in server.stdout:
                    handle.write(line)
                    handle.flush()
                    lines.put(line.rstrip())
                lines.put(None)

        thread = threading.Thread(target=collect, daemon=True)
        thread.start()

        def expect(fragment, timeout=None):
            deadline = time.monotonic() + (timeout or args.timeout)
            observed = []
            while time.monotonic() < deadline:
                try:
                    line = lines.get(timeout=max(0.01, deadline - time.monotonic()))
                except queue.Empty:
                    break
                if line is None:
                    raise RuntimeError(f"vanilla exited awaiting {fragment!r}: {observed[-10:]}")
                observed.append(line)
                if fragment in line:
                    return line
            raise RuntimeError(f"vanilla timed out awaiting {fragment!r}: {observed[-10:]}")

        def send(command):
            server.stdin.write(command + "\n")
            server.stdin.flush()

        expect("Starting minecraft server version 26.1.2")
        # The harness seeded minecraft:chunk_tickets in all three dimensions.
        # MinecraftServer.prepareLevels activates saved tickets and awaits
        # ChunkLoadCounter.pendingChunks == 0 before this startup event.
        expect('Done (')
        for dimension in ("overworld", "the_nether", "the_end"):
            marker = f"LAYOUT_BLOCK_{dimension}_OK"
            send(f"execute in minecraft:{dimension} if block 4 80 4 minecraft:diamond_block run say {marker}")
            receipt["checks"][f"vanilla_block_{dimension}"] = expect(marker, 20)
        send('execute in minecraft:overworld if entity @e[type=minecraft:cow,name=LayoutMarker] run say LAYOUT_ENTITY_OK')
        receipt["checks"]["vanilla_named_entity"] = expect("LAYOUT_ENTITY_OK", 20)
        send("execute if score LayoutPlayer layout_gate matches 37 run say LAYOUT_SCOREBOARD_OK")
        receipt["checks"]["vanilla_scoreboard"] = expect("LAYOUT_SCOREBOARD_OK", 20)
        send("gamerule minecraft:advance_time")
        receipt["checks"]["vanilla_gamerule"] = expect("Gamerule advance_time is currently set to: false", 20)
        send("time query time")
        receipt["checks"]["vanilla_daytime"] = expect("Clock minecraft:overworld is at 18000 tick(s)", 20)
        send("save-all flush")
        expect("Saved the game", 60)
        initial_spawn = read_nbt(world / "level.dat")["Data"]["spawn"]
        if initial_spawn["dimension"] != "minecraft:overworld" or initial_spawn["pos"] != [8, 80, 8]:
            raise RuntimeError(f"vanilla did not decode v2 world spawn: {initial_spawn}")
        receipt["checks"]["vanilla_initial_spawn"] = initial_spawn
        for dimension in ("overworld", "the_nether", "the_end"):
            send(f"execute in minecraft:{dimension} run setblock 4 80 4 minecraft:emerald_block")
            marker = f"LAYOUT_CHANGED_BLOCK_{dimension}_OK"
            send(f"execute in minecraft:{dimension} if block 4 80 4 minecraft:emerald_block run say {marker}")
            receipt["checks"][f"vanilla_changed_block_{dimension}"] = expect(marker, 20)
        send("setblock 8 79 9 minecraft:stone")
        send("tp @e[type=minecraft:cow,name=LayoutMarker,limit=1] 8.0 80.0 9.0")
        send('data merge entity @e[type=minecraft:cow,name=LayoutMarker,limit=1] {CustomName:"LayoutMarkerVanilla"}')
        send("execute if entity @e[type=minecraft:cow,name=LayoutMarkerVanilla,x=8,y=80,z=9,distance=..1] run say LAYOUT_CHANGED_ENTITY_OK")
        receipt["checks"]["vanilla_changed_entity"] = expect("LAYOUT_CHANGED_ENTITY_OK", 20)
        send("scoreboard players set LayoutPlayer layout_gate 41")
        send("execute if score LayoutPlayer layout_gate matches 41 run say LAYOUT_CHANGED_SCORE_OK")
        receipt["checks"]["vanilla_changed_score"] = expect("LAYOUT_CHANGED_SCORE_OK", 20)
        send("setworldspawn 9 81 9")
        send("time set 19000")
        send("time query time")
        receipt["checks"]["vanilla_changed_daytime"] = expect("Clock minecraft:overworld is at 19000 tick(s)", 20)
        send("save-all flush")
        expect("Saved the game", 60)
        send("stop")
        server.wait(timeout=60)
        thread.join(timeout=5)
        if server.returncode:
            raise RuntimeError(f"vanilla exit {server.returncode}")
        saved = read_nbt(world / "level.dat")["Data"]
        spawn = saved["spawn"]
        if spawn["dimension"] != "minecraft:overworld" or spawn["pos"] != [9, 81, 9]:
            raise RuntimeError(f"vanilla-saved spawn mismatch: {spawn}")
        receipt["checks"]["vanilla_saved_spawn"] = spawn
        receipt["checks"]["vanilla_version"] = saved["Version"]
        if saved["Version"]["Name"] != "26.1.2":
            raise RuntimeError(f"unexpected vanilla version: {saved['Version']}")
        fresh = output / "fresh-import"
        # Import only Java-owned domains into a separate, fresh native destination.
        source_world = output / "java-saved-source"
        source_world.mkdir()
        for name in ("level.dat", "dimensions", "data", "players"):
            source, target = world / name, source_world / name
            if source.is_dir():
                shutil.copytree(source, target)
            elif source.is_file():
                shutil.copyfile(source, target)
        receipt["fresh_import_world"] = str(fresh)
        receipt["java_saved_source"] = str(source_world)
        receipt["reexport_world"] = str(output / "reexport")
        v2("verify", fresh, source_world)
        receipt["passed"] = True
    except Exception as error:
        receipt["error"] = str(error)
    finally:
        if server is not None and server.poll() is None:
            try:
                server.stdin.write("stop\n")
                server.stdin.flush()
                server.wait(timeout=30)
            except Exception:
                server.kill()
                server.wait(timeout=10)
        (output / "result.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, indent=2))
    return 0 if receipt["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
