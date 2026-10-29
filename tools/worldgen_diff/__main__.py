"""Block-level Solaris-versus-vanilla worldgen diff: one command, JSON + markdown.

    python3 -m tools.worldgen_diff --server-bin target/release/solaris-v2 \
        --vanilla-jar .analysis/server.jar --out .analysis/worldgen-diff/run

Generates the same seeds and spawn-centered chunk rectangles with stock vanilla
26.1.2 (cached under --vanilla-cache) and Solaris (`world pregenerate` +
`world export-anvil`, always fresh), compares them cell by cell and writes
`report.json`, `report.md` and per-seed `map-<seed>.png` / `height-<seed>.png`.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import shlex
import shutil
import subprocess
import sys
import zlib
from pathlib import Path

import numpy as np
from PIL import Image

from . import diff, generate

ROOT = Path(__file__).resolve().parents[2]

_COLORS = {
    'grass_block': (127, 178, 56), 'water': (64, 64, 255), 'bubble_column': (64, 64, 255),
    'sand': (247, 233, 163), 'sandstone': (247, 233, 163), 'red_sand': (213, 125, 50),
    'red_sandstone': (213, 125, 50), 'stone': (112, 112, 112), 'deepslate': (100, 100, 100),
    'andesite': (112, 112, 112), 'diorite': (190, 190, 190), 'granite': (151, 109, 77),
    'tuff': (110, 110, 100), 'calcite': (209, 177, 161), 'gravel': (150, 140, 130),
    'snow': (255, 255, 255), 'snow_block': (255, 255, 255), 'powder_snow': (245, 245, 255),
    'ice': (160, 160, 255), 'packed_ice': (160, 160, 255), 'blue_ice': (140, 140, 255),
    'dirt': (151, 109, 77), 'coarse_dirt': (151, 109, 77), 'rooted_dirt': (151, 109, 77),
    'podzol': (129, 86, 49), 'mud': (87, 92, 95), 'clay': (164, 168, 184),
    'mycelium': (127, 63, 178), 'moss_block': (89, 125, 39), 'lava': (255, 0, 0),
    'terracotta': (152, 94, 67), 'orange_terracotta': (159, 82, 36),
    'yellow_terracotta': (186, 133, 36), 'white_terracotta': (209, 177, 161),
    'red_terracotta': (142, 60, 46), 'brown_terracotta': (76, 50, 35),
    'light_gray_terracotta': (135, 107, 98), 'dripstone_block': (76, 50, 35),
    'cobblestone': (112, 112, 112), 'mossy_cobblestone': (112, 112, 112),
    'pale_moss_block': (200, 200, 190), 'mangrove_roots': (102, 76, 51),
}


def _color(name: str) -> tuple[int, int, int]:
    plain = name.removeprefix('minecraft:')
    if plain in _COLORS:
        return _COLORS[plain]
    if plain.endswith('_leaves'):
        return (0, 124, 0)
    if plain.endswith(('_log', '_wood')):
        return (102, 76, 51)
    if diff.is_decor(name):
        return (100, 160, 50) if plain not in ('kelp', 'kelp_plant', 'seagrass',
                                               'tall_seagrass') else (40, 90, 200)
    digest = zlib.crc32(plain.encode())
    return (digest & 0xFF, (digest >> 8) & 0xFF, (digest >> 16) & 0xFF)


def _render_map(maps, side: int) -> np.ndarray:
    names = maps['names']
    palette = np.array([_color(n) for n in names], dtype=np.float32)
    ids = maps['top_block'][side]
    rgb = palette[ids]
    height = maps['top_y'][side].astype(np.int32)
    north = np.vstack([height[:1], height[:-1]])
    shade = np.where(height > north, 1.0, np.where(height < north, 0.71, 0.86))
    water = np.isin(ids, [i for i, n in enumerate(names)
                          if n in ('minecraft:water', 'minecraft:bubble_column')])
    floor = maps['surface'][side].astype(np.int32)
    depth = np.clip(height - floor, 0, 30)
    shade = np.where(water, 1.0 - depth / 45.0, shade)
    return np.clip(rgb * shade[..., None], 0, 255).astype(np.uint8)


def _write_images(maps, seed: int, out: Path) -> list[str]:
    left, right = _render_map(maps, 0), _render_map(maps, 1)
    gap = np.full((left.shape[0], 8, 3), 255, dtype=np.uint8)
    pair = np.hstack([left, gap, right])
    Image.fromarray(pair).resize((pair.shape[1] * 2, pair.shape[0] * 2),
                                 Image.NEAREST).save(out / f'map-{seed}.png')
    error = (maps['surface'][1].astype(np.int32) - maps['surface'][0].astype(np.int32))
    scale = np.clip(np.abs(error) / 16.0, 0, 1)
    rgb = np.full(error.shape + (3,), 255.0)
    rgb[..., 1] -= 255 * scale
    rgb[..., 2] = np.where(error > 0, rgb[..., 2] - 255 * scale, rgb[..., 2])
    rgb[..., 0] = np.where(error < 0, rgb[..., 0] - 255 * scale, rgb[..., 0])
    Image.fromarray(rgb.astype(np.uint8)).resize((error.shape[1] * 2, error.shape[0] * 2),
                                                 Image.NEAREST).save(out / f'height-{seed}.png')
    return [f'map-{seed}.png', f'height-{seed}.png']


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open('rb') as handle:
        for block in iter(lambda: handle.read(1 << 20), b''):
            digest.update(block)
    return digest.hexdigest()


def _head() -> str:
    out = subprocess.run(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], capture_output=True,
                         text=True)
    return out.stdout.strip()


def _row(seed, d) -> str:
    m = d['metrics']
    s = d['surface_height']
    st = d['structures']
    return (f"| {seed} | {d['exact_state_match_pct']:.2f} | {d['exact_state_match_nonair_pct']:.2f}"
            f" | {m['name']['match_nonair_pct']:.2f} | {m['no_decor_state']['match_nonair_pct']:.2f}"
            f" | {m['terrain']['match_nonair_pct']:.2f} | {m['shape']['match_pct']:.2f}"
            f" | {d['biome_match_pct']:.2f} | {s['mean_abs']:.2f} / {s['p95_abs']} / {s['max_abs']}"
            f" | {s['exact_pct']:.1f} | {st['vanilla_starts']} / {st['solaris_starts']} /"
            f" {st['same_type_same_chunk']} |")


def _markdown(report: dict) -> str:
    lines = [
        '# Solaris vs vanilla 26.1.2 — block-level worldgen diff', '',
        f"Generated {report['created_utc']} by `{report['command']}`.", '',
        f"- Solaris HEAD `{report['solaris_head']}`, server binary sha256 "
        f"`{report['server_bin_sha256'][:16]}…`; vanilla bundle sha256 "
        f"`{report['vanilla_bundle_sha256'][:16]}…`.",
        f"- {report['chunks']} chunks per seed ({report['chunks'] * 98304:,} cells, "
        'Y -64..319), overworld only; each rectangle is centered on vanilla spawn.',
        '- Vanilla: stock server, `tick freeze` in the datapack load function, `forceload` + FULL '
        'halo, `save-all flush`. Solaris: `world pregenerate` + `world export-anvil`.',
        '', '## Summary', '',
        'Percentages are of all cells unless marked *non-air* (cells where either side is not '
        'air in that metric). *name* ignores block properties; *no decor states* preserves '
        'properties except small plants (grass, flowers, kelp, vines, snow layers...) become '
        'air/source water and air variants merge. *terrain* ignores properties and also '
        'treats trees as air; *shape* folds terrain to air/water/lava/solid. Surface = top solid '
        'terrain block per column (Solaris − vanilla).', '',
        '| seed | exact % | exact non-air % | name non-air % | no-decor states non-air % | '
        'terrain non-air % | shape % | biome % | surface err mean/p95/max | surface exact % | '
        'structures V / S / same |',
        '|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---|',
    ]
    for seed, entry in report['seeds'].items():
        lines.append(_row(seed, entry['diff']))
    lines += ['', '## Generation time (this run, shared host; not a controlled benchmark)', '',
              'Vanilla also generates a FULL halo around the requested chunks; Solaris times '
              'exclude world setup and include light and native storage.', '',
              '| seed | vanilla generate s | Solaris generate s | Solaris chunks/s |',
              '|---:|---:|---:|---:|']
    for seed, entry in report['seeds'].items():
        v, s = entry['vanilla'], entry['solaris']
        cached = ' (cached)' if v.get('cached') else ''
        lines.append(f"| {seed} | {v['generate_s']}{cached} | {s['generate_s']} | "
                     f"{s['chunks_per_s']} |")
    for seed, entry in report['seeds'].items():
        d = entry['diff']
        low, high = entry['chunk_min'], entry['chunk_max']
        lines += ['', f'## Seed {seed}', '',
                  f"Vanilla spawn: `{entry['vanilla']['spawn']['pos']}`. "
                  f'Compared chunks `{low}`..`{high}`.', '']
        if entry.get('images'):
            lines += [f'![top-down map seed {seed}: vanilla left, Solaris right]'
                      f"({entry['images'][0]})", '',
                      f'![surface height error seed {seed}: red = Solaris higher, blue = lower]'
                      f"({entry['images'][1]})", '']
        lines += ['Top name confusions (vanilla → Solaris):', '',
                  '| vanilla | Solaris | cells | % of mismatches |', '|---|---|---:|---:|']
        for row in d['confusion_top'][:15]:
            lines.append(f"| {row['vanilla']} | {row['solaris']} | {row['cells']:,} | "
                         f"{row['pct_of_mismatches']:.1f} |")
        lines += ['', 'Largest block-count differences:', '',
                  '| block | vanilla | Solaris |', '|---|---:|---:|']
        for row in d['block_counts_top_diff'][:15]:
            lines.append(f"| {row['block']} | {row['vanilla']:,} | {row['solaris']:,} |")
        lines += ['', 'Match by 16-block layer (name / terrain / shape %):', '',
                  '| y | name | terrain | shape |', '|---|---:|---:|---:|']
        for row in d['layers']:
            lines.append(f"| {row['y_min']}..{row['y_max']} | {row['name_pct']:.1f} | "
                         f"{row['terrain_pct']:.1f} | {row['shape_pct']:.1f} |")
        st = d['structures']
        lines += ['', 'Structure starts in the rectangle (vanilla / Solaris / same chunk):', '']
        for kind, row in st['by_type'].items():
            lines.append(f"- {kind}: {row['vanilla']} / {row['solaris']} / {row['same_chunk']}")
        if not st['by_type']:
            lines.append('- none')
    return '\n'.join(lines) + '\n'


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog='python3 -m tools.worldgen_diff',
                                     description=__doc__.split('\n\n')[0])
    parser.add_argument('--seeds', type=int, nargs='+', default=[0, 81, 497625])
    parser.add_argument('--chunks', type=int, default=32,
                        help='square side in chunks, centered on vanilla spawn (default 32)')
    parser.add_argument('--server-bin', type=Path, default=ROOT / 'target/release/solaris-v2')
    parser.add_argument('--vanilla-jar', type=Path, default=ROOT / '.analysis/server.jar')
    parser.add_argument('--data-dir', type=Path, default=ROOT / 'data/vanilla')
    parser.add_argument('--java', default=shutil.which('java') or 'java')
    parser.add_argument('--heap', default='3G', help='vanilla JVM heap (default 3G)')
    parser.add_argument('--vanilla-cache', type=Path,
                        default=ROOT / '.analysis/worldgen-diff/vanilla-cache')
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--no-images', action='store_true')
    args = parser.parse_args(argv)
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    server_bin = args.server_bin.resolve()
    report = {
        'command': shlex.join([sys.executable, '-m', 'tools.worldgen_diff', *sys.argv[1:]]
                              if argv is None else ['python3', '-m', 'tools.worldgen_diff',
                                                    *argv]),
        'created_utc': datetime.datetime.now(datetime.UTC).strftime('%Y-%m-%dT%H:%M:%SZ'),
        'solaris_head': _head(), 'server_bin': str(server_bin),
        'server_bin_sha256': _sha256(server_bin),
        'vanilla_bundle': str(args.vanilla_jar), 'vanilla_bundle_sha256': _sha256(args.vanilla_jar),
        'chunks': args.chunks * args.chunks, 'seeds': {},
    }
    for seed in args.seeds:
        spawn = generate.vanilla_world(seed, (0, 0), (0, 0), args.vanilla_jar.resolve(),
                                       args.java, args.vanilla_cache.resolve(), args.heap)
        chunk_min, chunk_max = generate.spawn_bounds(spawn, args.chunks)
        vanilla = generate.vanilla_world(seed, chunk_min, chunk_max, args.vanilla_jar.resolve(),
                                         args.java, args.vanilla_cache.resolve(), args.heap)
        print(f'vanilla seed {seed}: {vanilla["world"]}', flush=True)
        solaris = generate.solaris_world(seed, chunk_min, chunk_max, server_bin,
                                         args.data_dir.resolve(), out / f'solaris-{seed}')
        print(f'solaris seed {seed}: {solaris["pregenerate_summary"]}', flush=True)
        result, maps = diff.compare(Path(vanilla['world']), Path(solaris['world']),
                                    chunk_min, chunk_max)
        entry = {'chunk_min': list(chunk_min), 'chunk_max': list(chunk_max),
                 'vanilla': vanilla, 'solaris': solaris, 'diff': result}
        if not args.no_images:
            entry['images'] = _write_images(maps, seed, out)
        report['seeds'][str(seed)] = entry
        print(_row(seed, result), flush=True)
        (out / 'report.json').write_text(json.dumps(report, indent=2))
    (out / 'report.md').write_text(_markdown(report))
    print(f'wrote {out / "report.json"} and {out / "report.md"}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
