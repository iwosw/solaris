"""Block-level comparison of two saved Anvil overworlds over one chunk rectangle.

Both worlds are read from `<world>/dimensions/minecraft/overworld/region`. Every
chunk of the rectangle is decoded into global block-state and biome ids and
compared cell by cell, streaming one chunk at a time.

Metric spaces (each block state maps to one canonical block name):
- `exact`: full block state, properties included.
- `name`: block name only; air, cave_air and void_air are one "air".
- `no_decor`: `name` with small plants/decorations (grass, flowers, kelp, vines,
  snow layers, mushrooms...) replaced by air (or water when submerged).
- `terrain`: `no_decor` with trees (logs, leaves, wood) also replaced.
- `shape`: `terrain` folded to four classes: air, water, lava, solid.
"""
from __future__ import annotations

from collections import Counter
from pathlib import Path

import numpy as np

from tools.harness.quirk_parity.anvil import read_region

MIN_SECTION = -4
SECTIONS = 24
MIN_Y = MIN_SECTION * 16
CELLS = SECTIONS * 4096
AIR = 'minecraft:air'
AIR_LIKE = {AIR, 'minecraft:cave_air', 'minecraft:void_air'}
CLASSES = ('air', 'water', 'lava', 'solid')

_DECOR = {
    'short_grass', 'tall_grass', 'fern', 'large_fern', 'dead_bush', 'bush', 'firefly_bush',
    'short_dry_grass', 'tall_dry_grass', 'dandelion', 'poppy', 'blue_orchid', 'allium',
    'azure_bluet', 'red_tulip', 'orange_tulip', 'white_tulip', 'pink_tulip', 'oxeye_daisy',
    'cornflower', 'lily_of_the_valley', 'sunflower', 'lilac', 'rose_bush', 'peony',
    'pink_petals', 'wildflowers', 'leaf_litter', 'torchflower', 'pitcher_plant',
    'open_eyeblossom', 'closed_eyeblossom', 'cactus_flower', 'golden_dandelion',
    'vine', 'glow_lichen', 'moss_carpet', 'pale_moss_carpet', 'pale_hanging_moss',
    'hanging_roots', 'spore_blossom', 'cave_vines', 'cave_vines_plant', 'sweet_berry_bush',
    'sugar_cane', 'cactus', 'brown_mushroom', 'red_mushroom', 'snow', 'pumpkin', 'melon',
    'bamboo', 'cocoa', 'small_dripleaf', 'big_dripleaf', 'big_dripleaf_stem',
    'pointed_dripstone', 'azalea', 'flowering_azalea', 'mangrove_propagule', 'lily_pad',
    'seagrass', 'tall_seagrass', 'kelp', 'kelp_plant', 'sea_pickle',
}
_UNDERWATER = {'seagrass', 'tall_seagrass', 'kelp', 'kelp_plant'}
_TREE_SUFFIXES = ('_log', '_leaves', '_wood', '_stem', '_hyphae')
_TREE = {'mangrove_roots', 'bee_nest', 'brown_mushroom_block', 'red_mushroom_block',
         'mushroom_stem', 'creaking_heart'}


def _plain(name: str) -> str:
    return name.removeprefix('minecraft:')


def is_decor(name: str) -> bool:
    plain = _plain(name)
    return (plain in _DECOR or plain.endswith(('_coral', '_coral_fan', '_coral_wall_fan',
                                               '_sapling')))


def is_tree(name: str) -> bool:
    plain = _plain(name)
    return plain in _TREE or (plain.endswith(_TREE_SUFFIXES) and not plain.startswith('stripped_'))


class Registry:
    """Global ids for block states/biomes plus per-metric lookup tables."""

    def __init__(self):
        self.states = [AIR]
        self.state_ids = {AIR: 0}
        self.names = [AIR]
        self.name_ids = {AIR: 0}
        self.biomes: list[str] = []
        self.biome_ids: dict[str, int] = {}
        self._tables: dict[str, np.ndarray] = {}

    def state(self, text: str) -> int:
        found = self.state_ids.get(text)
        if found is None:
            found = self.state_ids[text] = len(self.states)
            self.states.append(text)
            self._tables.clear()
        return found

    def name(self, text: str) -> int:
        found = self.name_ids.get(text)
        if found is None:
            found = self.name_ids[text] = len(self.names)
            self.names.append(text)
        return found

    def biome(self, text: str) -> int:
        found = self.biome_ids.get(text)
        if found is None:
            found = self.biome_ids[text] = len(self.biomes)
            self.biomes.append(text)
        return found

    def table(self, metric: str) -> np.ndarray:
        """State id -> canonical state/name/class id, with air always id zero."""
        if metric not in self._tables:
            if metric == 'no_decor_state':
                water = self.state('minecraft:water[level=0]')
                out = []
                for index, state in enumerate(self.states):
                    name = state.split('[', 1)[0]
                    if name in AIR_LIKE:
                        index = 0
                    elif is_decor(name):
                        wet = 'waterlogged=true' in state or _plain(name) in _UNDERWATER
                        index = water if wet else 0
                    out.append(index)
                self._tables[metric] = np.array(out, dtype=np.int32)
                return self._tables[metric]
            out = []
            for state in self.states:
                name = state.split('[', 1)[0]
                if name in AIR_LIKE:
                    name = AIR
                wet = 'waterlogged=true' in state or _plain(name) in _UNDERWATER
                if metric in ('no_decor', 'terrain', 'shape') and is_decor(name):
                    name = 'minecraft:water' if wet else AIR
                if metric in ('terrain', 'shape') and is_tree(name):
                    name = 'minecraft:water' if wet else AIR
                if metric == 'shape':
                    plain = _plain(name)
                    out.append(0 if name == AIR else 1 if plain in ('water', 'bubble_column')
                               else 2 if plain == 'lava' else 3)
                else:
                    out.append(self.name(name))
            self._tables[metric] = np.array(out, dtype=np.int32)
        return self._tables[metric]


def _unpack(longs, bits: int, count: int) -> np.ndarray:
    words = np.array(longs, dtype=np.int64).view(np.uint64)
    per = 64 // bits
    shifts = np.arange(per, dtype=np.uint64) * np.uint64(bits)
    values = (words[:, None] >> shifts[None, :]) & np.uint64((1 << bits) - 1)
    return values.reshape(-1)[:count].astype(np.int32)


def _state_text(entry: dict) -> str:
    props = entry.get('Properties')
    if not props:
        return entry['Name']
    return entry['Name'] + '[' + ','.join(f'{k}={props[k]}' for k in sorted(props)) + ']'


def decode_chunk(nbt: dict, registry: Registry):
    """(block ids [24*4096] in y,z,x order, biome ids [24*64], structure starts)."""
    blocks = np.zeros(CELLS, dtype=np.int32)
    biomes = np.full(SECTIONS * 64, -1, dtype=np.int32)
    for section in nbt.get('sections', []):
        index = section['Y'] - MIN_SECTION
        if not 0 <= index < SECTIONS:
            continue
        states = section.get('block_states')
        if states and states.get('palette'):
            palette = np.array([registry.state(_state_text(e)) for e in states['palette']],
                               dtype=np.int32)
            if len(palette) == 1:
                blocks[index * 4096:(index + 1) * 4096] = palette[0]
            else:
                bits = max(4, (len(palette) - 1).bit_length())
                blocks[index * 4096:(index + 1) * 4096] = palette[_unpack(states['data'], bits,
                                                                          4096)]
        biome = section.get('biomes')
        if biome and biome.get('palette'):
            palette = np.array([registry.biome(b) for b in biome['palette']], dtype=np.int32)
            if len(palette) == 1:
                biomes[index * 64:(index + 1) * 64] = palette[0]
            else:
                bits = (len(palette) - 1).bit_length()
                biomes[index * 64:(index + 1) * 64] = palette[_unpack(biome['data'], bits, 64)]
    starts = []
    for name, start in (nbt.get('structures') or {}).get('starts', {}).items():
        if start.get('id', name) != 'INVALID':
            starts.append(name)
    return blocks, biomes, starts


def load_chunks(world: Path, chunk_min, chunk_max):
    """{(x, z): nbt} for the rectangle; regions parsed once each."""
    region_dir = world / 'dimensions/minecraft/overworld/region'
    out = {}
    for rx in range(chunk_min[0] >> 5, (chunk_max[0] >> 5) + 1):
        for rz in range(chunk_min[1] >> 5, (chunk_max[1] >> 5) + 1):
            path = region_dir / f'r.{rx}.{rz}.mca'
            if not path.is_file():
                continue
            for lx, lz, nbt in read_region(path):
                pos = (rx * 32 + lx, rz * 32 + lz)
                if (chunk_min[0] <= pos[0] <= chunk_max[0]
                        and chunk_min[1] <= pos[1] <= chunk_max[1]):
                    out[pos] = nbt
    return out


def _top(mask: np.ndarray) -> np.ndarray:
    """Highest set y per (z, x) column of a (384, 16, 16) mask; MIN_Y-1 if none."""
    flipped = mask[::-1]
    found = flipped.any(axis=0)
    top = SECTIONS * 16 - 1 - flipped.argmax(axis=0) + MIN_Y
    return np.where(found, top, MIN_Y - 1)


def _percent(part, whole) -> float:
    return round(100.0 * part / whole, 3) if whole else 0.0


def _height_stats(errors: np.ndarray) -> dict:
    absolute = np.abs(errors)
    return {
        'columns': int(errors.size),
        'mean_abs': round(float(absolute.mean()), 3),
        'mean_signed_solaris_minus_vanilla': round(float(errors.mean()), 3),
        'rmse': round(float(np.sqrt((errors.astype(np.float64) ** 2).mean())), 3),
        'p50_abs': int(np.percentile(absolute, 50)),
        'p95_abs': int(np.percentile(absolute, 95)),
        'max_abs': int(absolute.max()),
        'exact_pct': _percent(int((absolute == 0).sum()), errors.size),
        'within_1_pct': _percent(int((absolute <= 1).sum()), errors.size),
        'within_4_pct': _percent(int((absolute <= 4).sum()), errors.size),
    }


METRICS = ('name', 'no_decor', 'no_decor_state', 'terrain', 'shape')


def compare(vanilla: Path, solaris: Path, chunk_min, chunk_max, top: int = 25):
    """(JSON-ready result, maps) for the inclusive chunk rectangle.

    `maps` holds per-column arrays shaped (2, depth, width), index 0 vanilla and
    1 Solaris: `surface` (top solid terrain y), `top_y`/`top_block` (highest
    non-air cell and its block name), plus `names` to decode `top_block`.
    """
    registry = Registry()
    left = load_chunks(vanilla, chunk_min, chunk_max)
    right = load_chunks(solaris, chunk_min, chunk_max)
    wanted = [(x, z) for z in range(chunk_min[1], chunk_max[1] + 1)
              for x in range(chunk_min[0], chunk_max[0] + 1)]
    missing = {'vanilla': [p for p in wanted if p not in left],
               'solaris': [p for p in wanted if p not in right]}
    not_full = [p for p in wanted if p in left and left[p].get('Status') != 'minecraft:full']
    if any(missing.values()):
        raise ValueError(f'comparison rectangle has missing chunks: {missing}')
    if not_full:
        raise ValueError(f'vanilla chunks have not reached FULL status: {not_full}')
    chunks = [p for p in wanted if p in left and p in right]
    totals = {'cells': 0, 'exact': 0, 'exact_nonair': 0, 'nonair_either': 0}
    per_metric = {m: {'match': 0, 'nonair_either': 0, 'match_nonair': 0} for m in METRICS}
    layers = np.zeros((SECTIONS, 4), dtype=np.int64)  # cells, name, terrain, shape matches
    class_confusion = np.zeros((4, 4), dtype=np.int64)
    pairs: Counter = Counter()
    counts_left: Counter = Counter()
    counts_right: Counter = Counter()
    surface_errors, world_surface_errors = [], []
    biome_cells = biome_match = 0
    biome_pairs: Counter = Counter()
    starts = {'vanilla': [], 'solaris': []}
    width = (chunk_max[0] - chunk_min[0] + 1) * 16
    depth = (chunk_max[1] - chunk_min[1] + 1) * 16
    heightmaps = np.full((2, depth, width), MIN_Y - 1, dtype=np.int16)
    top_y = np.full((2, depth, width), MIN_Y - 1, dtype=np.int16)
    top_block = np.zeros((2, depth, width), dtype=np.int32)
    columns = np.arange(256)
    for pos in chunks:
        vb, vbio, vstarts = decode_chunk(left[pos], registry)
        sb, sbio, sstarts = decode_chunk(right[pos], registry)
        starts['vanilla'] += [(s, *pos) for s in vstarts]
        starts['solaris'] += [(s, *pos) for s in sstarts]
        name = registry.table('name')
        vn, sn = name[vb], name[sb]
        nonair = (vn != 0) | (sn != 0)
        exact = vb == sb
        totals['cells'] += CELLS
        totals['exact'] += int(exact.sum())
        totals['exact_nonair'] += int((exact & nonair).sum())
        totals['nonair_either'] += int(nonair.sum())
        mapped = {}
        for metric in METRICS:
            table = registry.table(metric)
            a, b = table[vb], table[sb]
            mapped[metric] = (a, b)
            eq = a == b
            either = (a != 0) | (b != 0)
            stats = per_metric[metric]
            stats['match'] += int(eq.sum())
            stats['nonair_either'] += int(either.sum())
            stats['match_nonair'] += int((eq & either).sum())
        layers[:, 0] += 4096
        for column, metric in ((1, 'name'), (2, 'terrain'), (3, 'shape')):
            a, b = mapped[metric]
            layers[:, column] += (a == b).reshape(SECTIONS, 4096).sum(axis=1)
        va, sa = mapped['shape']
        class_confusion += np.bincount(va * 4 + sa, minlength=16).reshape(4, 4)
        wrong = vn != sn
        if wrong.any():
            keys, freq = np.unique(vn[wrong].astype(np.int64) * 1_000_000 + sn[wrong],
                                   return_counts=True)
            for key, n in zip(keys.tolist(), freq.tolist()):
                pairs[(key // 1_000_000, key % 1_000_000)] += n
        for ids, counter in ((vn, counts_left), (sn, counts_right)):
            freq = np.bincount(ids)
            for key in np.nonzero(freq)[0].tolist():
                counter[key] += int(freq[key])
        vsurf = _top((va == 3).reshape(384, 16, 16))
        ssurf = _top((sa == 3).reshape(384, 16, 16))
        surface_errors.append((ssurf - vsurf).reshape(-1))
        ox, oz = (pos[0] - chunk_min[0]) * 16, (pos[1] - chunk_min[1]) * 16
        heightmaps[0, oz:oz + 16, ox:ox + 16] = vsurf
        heightmaps[1, oz:oz + 16, ox:ox + 16] = ssurf
        for side, ids in enumerate((vn, sn)):
            highest = _top((ids != 0).reshape(384, 16, 16))
            top_y[side, oz:oz + 16, ox:ox + 16] = highest
            row = np.clip(highest.reshape(-1) - MIN_Y, 0, 383)
            top_block[side, oz:oz + 16, ox:ox + 16] = (
                ids.reshape(384, 256)[row, columns].reshape(16, 16))
        world_surface_errors.append((top_y[1, oz:oz + 16, ox:ox + 16]
                                     - top_y[0, oz:oz + 16, ox:ox + 16]).reshape(-1)
                                    .astype(np.int32))
        both = (vbio >= 0) & (sbio >= 0)
        biome_cells += int(both.sum())
        same = both & (vbio == sbio)
        biome_match += int(same.sum())
        for a, b in zip(vbio[both & ~same].tolist(), sbio[both & ~same].tolist()):
            biome_pairs[(a, b)] += 1
    maps = {'surface': heightmaps, 'top_y': top_y, 'top_block': top_block,
            'names': list(registry.names)}
    names = registry.names
    mismatches = sum(pairs.values())
    result = {
        'chunk_min': list(chunk_min), 'chunk_max': list(chunk_max),
        'chunks_compared': len(chunks),
        'chunks_missing': {k: [list(p) for p in v] for k, v in missing.items()},
        'vanilla_not_full': [list(p) for p in not_full],
        'cells': totals['cells'],
        'exact_state_mismatch_cells': totals['cells'] - totals['exact'],
        'exact_state_match_pct': _percent(totals['exact'], totals['cells']),
        'exact_state_match_nonair_pct': _percent(totals['exact_nonair'],
                                                 totals['nonair_either']),
        'metrics': {m: {'match_pct': _percent(s['match'], totals['cells']),
                        'match_nonair_pct': _percent(s['match_nonair'], s['nonair_either']),
                        'nonair_either_cells': s['nonair_either']}
                    for m, s in per_metric.items()},
        'layers': [{'y_min': MIN_Y + 16 * i, 'y_max': MIN_Y + 16 * i + 15,
                    'name_pct': _percent(int(row[1]), int(row[0])),
                    'terrain_pct': _percent(int(row[2]), int(row[0])),
                    'shape_pct': _percent(int(row[3]), int(row[0]))}
                   for i, row in enumerate(layers) if row[0]],
        'shape_confusion': {'classes': list(CLASSES),
                            'rows_vanilla_cols_solaris': class_confusion.tolist()},
        'surface_height': _height_stats(np.concatenate(surface_errors)) if chunks else {},
        'top_nonair_height': _height_stats(np.concatenate(world_surface_errors)) if chunks
        else {},
        'name_mismatch_cells': mismatches,
        'confusion_top': [{'vanilla': names[a], 'solaris': names[b], 'cells': n,
                           'pct_of_mismatches': _percent(n, mismatches)}
                          for (a, b), n in pairs.most_common(top)],
        'block_counts_top_diff': sorted(
            ({'block': names[k], 'vanilla': counts_left.get(k, 0),
              'solaris': counts_right.get(k, 0)}
             for k in set(counts_left) | set(counts_right) if k != 0),
            key=lambda row: -abs(row['vanilla'] - row['solaris']))[:top],
        'biome_cells': biome_cells,
        'biome_mismatch_cells': biome_cells - biome_match,
        'biome_match_pct': _percent(biome_match, biome_cells),
        'biome_confusion_top': [{'vanilla': registry.biomes[a], 'solaris': registry.biomes[b],
                                 'cells': n} for (a, b), n in biome_pairs.most_common(10)],
        'structures': _structures(starts),
    }
    return result, maps


def _structures(starts) -> dict:
    vanilla = set(starts['vanilla'])
    solaris = set(starts['solaris'])
    kinds = sorted({s for s, *_ in vanilla | solaris})
    return {
        'vanilla_starts': len(vanilla), 'solaris_starts': len(solaris),
        'same_type_same_chunk': len(vanilla & solaris),
        'by_type': {k: {'vanilla': sum(1 for s in vanilla if s[0] == k),
                        'solaris': sum(1 for s in solaris if s[0] == k),
                        'same_chunk': sum(1 for s in vanilla & solaris if s[0] == k)}
                    for k in kinds},
        'vanilla': sorted([list(s) for s in vanilla]),
        'solaris': sorted([list(s) for s in solaris]),
    }
