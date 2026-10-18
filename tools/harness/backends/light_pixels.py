"""Pixel predicate for a screenshot that is entirely one sunlit opaque face.

The gate stands 1.5 blocks from a flat stone wall whose open-air neighbour cells
are all sky level 15, so every block-sized region of the central crop must render
about equally bright. A face that samples the opaque block's own (zero) light
renders almost black and is far below the median tile.
"""
from __future__ import annotations

from pathlib import Path
from statistics import median
from typing import Any

from PIL import Image

TILE = 64
MIN_RELATIVE = 0.6
MIN_MEDIAN = 50.0


def dark_tiles(path: Path | str) -> dict[str, Any]:
    """Tile means of the central half of the frame (0..255 luminance) and the darkest ones."""
    with Image.open(path) as image:
        gray = image.convert("L")
        width, height = gray.size
        left, top = width // 4, height // 4
        columns, rows = (width // 2) // TILE, (height // 2) // TILE
        means = {}
        for row in range(rows):
            for column in range(columns):
                box = (left + column * TILE, top + row * TILE, left + (column + 1) * TILE, top + (row + 1) * TILE)
                pixels = list(gray.crop(box).getdata())
                means[(column, row)] = sum(pixels) / len(pixels)
    typical = median(means.values())
    dark = {f"{column},{row}": round(mean, 1) for (column, row), mean in means.items()
            if mean < MIN_RELATIVE * typical}
    return {"tile": TILE, "tiles": len(means), "median": round(typical, 1),
            "minimum": round(min(means.values()), 1), "dark_tiles": dark,
            "passed": typical >= MIN_MEDIAN and not dark}
