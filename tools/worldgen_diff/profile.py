"""Interleave two frozen mc-worldgen release test binaries (approved window only).

Run inside mem-run -m 6G; do not put builds in the measurement window:
  python3 -m tools.worldgen_diff.profile --before /path/to/before-test \
      --after /path/to/after-test --out .analysis/worldgen-diff/ab
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import platform
import statistics
import subprocess

TEST = 'terrain::profile_tests::parity_stages'


def samples(stdout: str) -> list[dict]:
    rows = []
    for line in stdout.splitlines():
        if line.startswith('STAGES '):
            row = {}
            for pair in line.split()[1:]:
                key, value = pair.split('=', 1)
                row[key] = int(value) if key in ('seed', 'run', 'chunks') else float(value)
            rows.append(row)
    if [row['seed'] for row in rows] != [0, 81, 497625]:
        raise ValueError(f'{TEST}: missing or unexpected seed samples')
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--repetitions', type=int, default=3)
    args = parser.parse_args()
    if args.repetitions < 3:
        parser.error('at least three interleaved repetitions are required')
    args.out.mkdir(parents=True, exist_ok=False)
    binaries = {'before': args.before.resolve(), 'after': args.after.resolve()}
    fingerprints = {}
    for name, path in binaries.items():
        with path.open('rb') as handle:
            fingerprints[name] = hashlib.file_digest(handle, 'sha256').hexdigest()
    report = {
        'environment': platform.platform(),
        'scope': 'contended, shared lane; single-threaded in-process, no lighting in timer',
        'test': TEST,
        'binary_sha256': fingerprints,
        'runs': [],
    }
    for repetition in range(args.repetitions):
        for name, binary in binaries.items():
            command = [str(binary), '--ignored', '--exact', TEST, '--nocapture',
                       '--test-threads=1']
            result = subprocess.run(command, capture_output=True, text=True, check=True)
            log = f'{repetition}-{name}.log'
            (args.out / log).write_text(result.stdout + result.stderr)
            report['runs'].append({'repetition': repetition, 'variant': name,
                                   'command': command, 'log': log, 'samples': samples(result.stdout)})
    report['seeds'] = {}
    for seed in (0, 81, 497625):
        entry = {}
        for name in binaries:
            values = [row['ms_per_chunk'] for run in report['runs'] if run['variant'] == name
                      for row in run['samples'] if row['seed'] == seed]
            median = statistics.median(values)
            entry[name] = {'ms_per_chunk': values, 'median_ms': median,
                           'min_ms': min(values), 'max_ms': max(values),
                           'median_chunks_per_s': 1000.0 / median}
        delta = entry['before']['median_ms'] - entry['after']['median_ms']
        spread = max(entry[name]['max_ms'] - entry[name]['min_ms'] for name in binaries)
        entry['time_reduction_pct'] = 100.0 * delta / entry['before']['median_ms']
        entry['spread_ms'] = spread
        entry['delta_exceeds_spread'] = delta > spread
        report['seeds'][str(seed)] = entry
    (args.out / 'measurements.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report['seeds'], indent=2))


if __name__ == '__main__':
    main()
