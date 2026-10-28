#!/usr/bin/env python3
"""Markdown tables from bench.json files and side-by-side low|medium|high contact sheets.

    client_showcase_report.py table  LABEL=DIR/bench.json ...      # numbers table
    client_showcase_report.py passes LABEL=DIR/bench.json ...      # per-pass GPU ms (p50)
    client_showcase_report.py sheet  OUT.png CAPTURES_DIR POSE...  # CAPTURES_DIR holds low/ medium/ high/ PNGs
"""
import json
import sys
from pathlib import Path


def load(args):
    return [(label, json.loads(Path(path).read_text())) for label, path in (a.split("=", 1) for a in args)]


def f(x, digits=1):
    return "n/a" if x is None else f"{x:.{digits}f}"


def table(args):
    print("| GPU / run | preset | avg FPS | frame ms p50 | p95 | p99 | max | 1% low FPS | GPU ms (sum of passes) p50 |")
    print("|---|---|---:|---:|---:|---:|---:|---:|---:|")
    for label, data in load(args):
        for preset in ("low", "medium", "high"):
            r = data.get(preset)
            if not r:
                continue
            iv = r.get("frame_interval_ms") or {}
            print(f"| {label} | {preset} | {r['avg_fps_exact']:.1f} | {f(iv.get('p50'), 2)} | {f(iv.get('p95'), 2)} | "
                  f"{f(iv.get('p99'), 2)} | {f(iv.get('max'), 1)} | {f(r.get('fps_1pct_low'))} | "
                  f"{f(r['gpu_total_ms']['p50'], 2)} |")


def passes(args):
    runs = load(args)
    names = sorted({k for _, d in runs for r in d.values() for k in r["gpu_pass_ms"]})
    cols = [(label, p) for label, d in runs for p in ("low", "medium", "high") if p in d]
    print("| pass | " + " | ".join(f"{l} {p}" for l, p in cols) + " |")
    print("|---|" + "---:|" * len(cols))
    data = {label: d for label, d in runs}
    for n in names:
        cells = [(data[l][p]["gpu_pass_ms"].get(n) or {}).get("p50") for l, p in cols]
        if any(c for c in cells):
            print(f"| {n} | " + " | ".join(f(c, 2) for c in cells) + " |")


def sheet(args):
    from PIL import Image, ImageDraw
    out, root, *poses = args
    sheet_img = Image.new("RGB", (1920, 360 * len(poses)))
    for row, pose in enumerate(poses):
        for col, preset in enumerate(("low", "medium", "high")):
            im = Image.open(Path(root) / preset / f"{pose}.png").convert("RGB").resize((640, 360))
            ImageDraw.Draw(im).text((8, 8), f"{preset} {pose}", fill=(255, 255, 0))
            sheet_img.paste(im, (col * 640, row * 360))
    sheet_img.save(out)


if __name__ == "__main__":
    {"table": table, "passes": passes, "sheet": sheet}[sys.argv[1]](sys.argv[2:])
