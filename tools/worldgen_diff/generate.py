"""Generate one chunk rectangle with stock vanilla 26.1.2 and with Solaris.

Vanilla: the local server bundle runs headless with `tick freeze` sent before
startup completes, so block/fluid/random ticks and entities never change the
generated terrain. `forceload add` requests the rectangle (vanilla loads a FULL
halo around forced chunks, so every compared chunk carries its neighbours'
features). Readiness is a datapack function run from the console once per server
tick until every forced chunk reports loaded; then `save-all flush` and `stop`.

Solaris: `world pregenerate` over the same rectangle, then `world export-anvil`.
Mojang bytes stay in the ignored output directory.
"""
from __future__ import annotations

import gzip
import hashlib
import json
import queue
import shlex
import socket
import subprocess
import threading
import time
import zipfile
from pathlib import Path

from tools.harness.quirk_parity.anvil import parse_nbt

SERVER_TIMEOUT_S = 1800

_TICK_STATES = ('The game is frozen', 'The game is running', 'The game is sprinting')


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def _put(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


class Console:
    """A server process whose stdout lines are queued with monotonic stamps."""

    def __init__(self, argv: list[str], cwd: Path, log: Path):
        _put(cwd / 'command.txt', shlex.join(argv) + '\n')
        self.proc = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                     text=True, bufsize=1)
        self.lines: queue.Queue = queue.Queue()
        self.log = log
        threading.Thread(target=self._collect, daemon=True).start()

    def _collect(self) -> None:
        with self.log.open('w') as log:
            for line in self.proc.stdout:
                log.write(line)
                log.flush()
                self.lines.put(line)
        self.lines.put(None)

    def send(self, command: str) -> None:
        self.proc.stdin.write(command + '\n')
        self.proc.stdin.flush()

    def expect(self, *markers: str) -> str:
        """Block until a line contains one of `markers`; return that marker."""
        deadline = time.monotonic() + SERVER_TIMEOUT_S
        while True:
            line = self.lines.get(timeout=max(0.01, deadline - time.monotonic()))
            if line is None:
                raise RuntimeError(f'server exited waiting for {markers} (see {self.log})')
            for marker in markers:
                if marker in line:
                    return marker

    def wait(self) -> int:
        return self.proc.wait(timeout=SERVER_TIMEOUT_S)


def _runtime(bundle: Path, cache: Path) -> list[Path]:
    """Unbundle the server jar once; return its classpath."""
    with bundle.open('rb') as handle:
        identity = hashlib.file_digest(handle, 'sha256').hexdigest()
    root = cache / f'runtime-{identity[:16]}'
    marker = root / 'classpath.json'
    if not marker.is_file():
        with zipfile.ZipFile(bundle) as archive:
            members = [m for m in archive.namelist()
                       if m.startswith(('META-INF/versions/', 'META-INF/libraries/'))
                       and m.endswith('.jar')]
            archive.extractall(root, members)
        servers = [m for m in members if m.startswith('META-INF/versions/')]
        if len(servers) != 1:
            raise RuntimeError(f'{bundle}: expected one bundled server jar, got {servers}')
        libraries = sorted(m for m in members if m.startswith('META-INF/libraries/'))
        _put(marker, json.dumps(servers + libraries))
    return [root / m for m in json.loads(marker.read_text())]


def _datapack(world: Path, chunk_min, chunk_max) -> None:
    base = world / 'datapacks/wgd'
    _put(base / 'pack.mcmeta', json.dumps({'pack': {
        'description': 'worldgen-diff readiness probe only',
        'min_format': [101, 1], 'max_format': [101, 1]}}))
    lines = ['scoreboard players set #ready wgd 1']
    lines += [f'execute unless loaded {x * 16 + 8} 0 {z * 16 + 8} run '
              f'scoreboard players set #ready wgd 0'
              for x in range(chunk_min[0], chunk_max[0] + 1)
              for z in range(chunk_min[1], chunk_max[1] + 1)]
    lines += ['execute if score #ready wgd matches 1 run say WGD_READY',
              'execute if score #ready wgd matches 0 run say WGD_PENDING']
    _put(base / 'data/wgd/function/check.mcfunction', '\n'.join(lines) + '\n')


def _forceload_commands(chunk_min, chunk_max) -> list[str]:
    """Split the rectangle into <=256-chunk forceload areas (vanilla's cap)."""
    out = []
    for x0 in range(chunk_min[0], chunk_max[0] + 1, 16):
        for z0 in range(chunk_min[1], chunk_max[1] + 1, 16):
            x1, z1 = min(x0 + 15, chunk_max[0]), min(z0 + 15, chunk_max[1])
            out.append(f'forceload add {x0 * 16} {z0 * 16} {x1 * 16 + 15} {z1 * 16 + 15}')
    return out


def vanilla_world(seed: int, chunk_min, chunk_max, bundle: Path, java: str, cache: Path,
                  heap: str) -> dict:
    """Return the cached or freshly generated vanilla world directory + receipt."""
    with bundle.open('rb') as handle:
        identity = hashlib.file_digest(handle, 'sha256').hexdigest()
    key = f'vanilla-{seed}-{chunk_min[0]}_{chunk_min[1]}-{chunk_max[0]}_{chunk_max[1]}'
    directory = cache / key
    receipt_path = directory / 'receipt.json'
    if receipt_path.is_file():
        receipt = json.loads(receipt_path.read_text())
        if receipt.get('bundle_sha256') != identity:
            raise RuntimeError(f'{directory}: cached oracle uses another jar; use a fresh cache')
        receipt['cached'] = True
        return receipt
    if directory.exists():
        raise RuntimeError(f'{directory} exists without receipt.json; remove it to regenerate')
    classpath = _runtime(bundle, cache)
    directory.mkdir(parents=True)
    _put(directory / 'eula.txt', 'eula=true\n')
    props = {
        'online-mode': 'false', 'server-ip': '127.0.0.1', 'server-port': str(_free_port()),
        'level-name': 'world', 'level-seed': str(seed), 'level-type': 'minecraft:normal',
        'generate-structures': 'true', 'view-distance': '2', 'simulation-distance': '2',
        'pause-when-empty-seconds': '-1', 'max-tick-time': '-1', 'max-players': '1',
        'sync-chunk-writes': 'true', 'enable-rcon': 'false', 'enable-query': 'false',
        'spawn-protection': '0',
    }
    _put(directory / 'server.properties', ''.join(f'{k}={v}\n' for k, v in props.items()))
    _datapack(directory / 'world', chunk_min, chunk_max)
    argv = [java, f'-Xms{heap}', f'-Xmx{heap}', '-cp', ':'.join(map(str, classpath)),
            'net.minecraft.server.Main', 'nogui']
    server = Console(argv, directory, directory / 'server.log')
    try:
        # Queued before startup finishes, so it applies on the first tick. Its
        # feedback can fail (the console source predates the overworld), so the
        # state is confirmed with `tick query` once the server is up.
        server.send('tick freeze')
        started = time.monotonic()
        server.expect('Done (')
        ready_at = time.monotonic()
        server.send('tick query')
        if server.expect(*_TICK_STATES) != 'The game is frozen':
            raise RuntimeError(f'vanilla did not freeze (see {directory / "server.log"})')
        server.send('scoreboard objectives add wgd dummy')
        for command in _forceload_commands(chunk_min, chunk_max):
            server.send(command)
        checks = 0
        while True:
            # The console runs one queued command batch per server tick; each
            # check answers exactly once, so this waits on server state, not time.
            server.send('function wgd:check')
            checks += 1
            if server.expect('WGD_READY', 'WGD_PENDING') == 'WGD_READY':
                break
        generated_at = time.monotonic()
        server.send('save-all flush')
        server.expect('Saved the game')
        saved_at = time.monotonic()
        server.send('stop')
        code = server.wait()
    except BaseException:
        if server.proc.poll() is None:
            server.proc.kill()
        raise
    if code != 0:
        raise RuntimeError(f'vanilla server exit {code} (see {directory / "server.log"})')
    chunks = (chunk_max[0] - chunk_min[0] + 1) * (chunk_max[1] - chunk_min[1] + 1)
    level = parse_nbt(gzip.decompress((directory / 'world/level.dat').read_bytes()))['Data']
    receipt = {
        'engine': 'vanilla', 'seed': seed, 'world': str(directory / 'world'),
        'bundle': str(bundle), 'bundle_sha256': identity, 'spawn': level['spawn'],
        'heap': heap, 'startup_s': round(ready_at - started, 3),
        'generate_s': round(generated_at - ready_at, 3),
        'save_s': round(saved_at - generated_at, 3), 'readiness_checks': checks,
        'requested_chunks': chunks,
        'note': 'tick-frozen; forceload also generates a FULL halo beyond the requested '
                'chunks, so requested chunks/s understates raw generator throughput',
    }
    _put(receipt_path, json.dumps(receipt, indent=2))
    return receipt


_COMPLETE = 'pregenerate complete:'


def solaris_world(seed: int, chunk_min, chunk_max, server_bin: Path, data_dir: Path,
                  directory: Path) -> dict:
    """Pregenerate + export a fresh Solaris world into `directory`."""
    if directory.exists():
        raise RuntimeError(f'{directory} already exists; Solaris worlds are never reused')
    (directory / 'plugins').mkdir(parents=True)
    config = directory / 'server.toml'
    _put(config, f'''[plugins]
directory = "{directory / 'plugins'}"
[data]
world_dir = "{directory / 'world'}"
backup_dir = "{directory / 'backups'}"
vanilla_data_dir = "{data_dir}"
seed = {seed}
''')
    # pregenerate takes a block radius around a block center (inclusive edges).
    radius = (chunk_max[0] - chunk_min[0] + 1) * 8 - 8
    if (chunk_max[1] - chunk_min[1] + 1) * 8 - 8 != radius:
        raise ValueError('Solaris pregenerate needs a square rectangle')
    center = (chunk_min[0] * 16 + radius, chunk_min[1] * 16 + radius)
    base = [str(server_bin), '--config', str(config), 'world']
    pregenerate = base + ['pregenerate', f'--radius={radius}', f'--center={center[0]},{center[1]}']
    started = time.monotonic()
    out = subprocess.run(pregenerate, cwd=directory, capture_output=True, text=True)
    elapsed = time.monotonic() - started
    _put(directory / 'pregenerate.log', out.stdout + out.stderr)
    if out.returncode != 0:
        raise RuntimeError(f'pregenerate failed ({out.returncode}): {out.stderr[-2000:]}')
    lines = (out.stdout + out.stderr).splitlines()
    summary = next((line for line in lines if _COMPLETE in line), '')
    if f'[{chunk_min[0]},{chunk_min[1]}]..[{chunk_max[0]},{chunk_max[1]}]' not in summary:
        raise RuntimeError(f'pregenerate covered an unexpected rectangle: {summary!r}')
    # The first progress line (0 generated) closes world setup; the summary's
    # elapsed closes generation.
    setup = next(float(line.rsplit(' ', 1)[1].rstrip('s')) for line in lines
                 if line.startswith('pregenerate: ') and ', 0 generated,' in line)
    total = float(summary.rsplit(' ', 1)[1].rstrip('s'))
    export = base + ['export-anvil', '--source', str(directory / 'world'),
                     '--destination', str(directory / 'anvil'),
                     '--receipt', str(directory / 'export-receipt.json')]
    out = subprocess.run(export, cwd=directory, capture_output=True, text=True)
    _put(directory / 'export.log', out.stdout + out.stderr)
    if out.returncode != 0:
        raise RuntimeError(f'export-anvil failed ({out.returncode}): {out.stderr[-2000:]}')
    chunks = (chunk_max[0] - chunk_min[0] + 1) * (chunk_max[1] - chunk_min[1] + 1)
    return {
        'engine': 'solaris', 'seed': seed, 'world': str(directory / 'anvil'),
        'server_bin': str(server_bin), 'pregenerate_wall_s': round(elapsed, 3),
        'setup_s': setup, 'generate_s': round(total - setup, 3),
        'pregenerate_summary': summary.strip(), 'requested_chunks': chunks,
        'chunks_per_s': round(chunks / max(total - setup, 1e-3), 2),
    }
