"""Cross-chunk shadow fixture for the light-seam gate and its analytic light oracle.

A floating obsidian platform at `y0` carries (a) a stone roof spanning the chunk
border `bx` with a torch under its east half and (b) a stone mass with a one-wide
tunnel open only on its west (chunk A) side and a torch deep in its east (chunk B)
end. Everything else above the platform is open sky. The oracle is vanilla light
propagation over exactly these cells (stone/obsidian fully opaque, torch emits 14,
one level lost per block), independent of both the server solver and the client.
"""
from __future__ import annotations

from typing import Callable

Cell = tuple[int, int, int]
FLOOR_HALF = 24  # arena platform spans bx-24..bx+23; far standing pads are separate (`pad_commands`)
REGION_X = (-12, 11)
REGION_Y = (1, 7)
REGION_Z = (0, 15)
_DOMAIN_X, _DOMAIN_Z, _DOMAIN_Y = (-80, 80), (-4, 19), (1, 12)


class SeamModel:
    def __init__(self, bx: int, y0: int, z0: int) -> None:
        self.bx, self.y0, self.z0 = bx, y0, z0
        self.torches: tuple[Cell, ...] = ((bx + 3, y0 + 1, z0 + 5), (bx + 4, y0 + 1, z0 + 13))

    def commands(self) -> list[str]:
        bx, y0, z0 = self.bx, self.y0, self.z0
        return [
            f"fill {bx - FLOOR_HALF} {y0} {z0} {bx + FLOOR_HALF - 1} {y0} {z0 + 15} minecraft:obsidian",
            f"fill {bx - FLOOR_HALF} {y0 + 1} {z0} {bx + FLOOR_HALF - 1} {y0 + 10} {z0 + 15} minecraft:air",
            f"fill {bx - 6} {y0 + 4} {z0 + 2} {bx + 5} {y0 + 4} {z0 + 8} minecraft:stone",
            f"fill {bx - 8} {y0 + 1} {z0 + 11} {bx + 7} {y0 + 4} {z0 + 15} minecraft:stone",
            f"fill {bx - 8} {y0 + 1} {z0 + 13} {bx + 5} {y0 + 2} {z0 + 13} minecraft:air",
            *(f"setblock {x} {y} {z} minecraft:torch" for x, y, z in self.torches),
        ]

    def pad_commands(self, centre_x: int) -> list[str]:
        """A 5x5 obsidian standing pad (with three blocks of air above) in the arena's chunk row."""
        y0, z0 = self.y0, self.z0
        return [f"fill {centre_x - 2} {y0} {z0 + 6} {centre_x + 2} {y0} {z0 + 10} minecraft:obsidian",
                f"fill {centre_x - 2} {y0 + 1} {z0 + 6} {centre_x + 2} {y0 + 3} {z0 + 10} minecraft:air"]

    def solid(self, x: int, y: int, z: int) -> bool:
        bx, y0, z0 = self.bx, self.y0, self.z0
        if y == y0:
            return bx - FLOOR_HALF <= x < bx + FLOOR_HALF and z0 <= z <= z0 + 15
        if y == y0 + 4 and bx - 6 <= x <= bx + 5 and z0 + 2 <= z <= z0 + 8:
            return True
        mass = y0 + 1 <= y <= y0 + 4 and bx - 8 <= x <= bx + 7 and z0 + 11 <= z <= z0 + 15
        tunnel = y0 + 1 <= y <= y0 + 2 and z == z0 + 13 and bx - 8 <= x <= bx + 5
        return mass and not tunnel

    def region(self) -> list[Cell]:
        return [(self.bx + dx, self.y0 + dy, self.z0 + dz)
                for dy in range(REGION_Y[0], REGION_Y[1] + 1)
                for dz in range(REGION_Z[0], REGION_Z[1] + 1)
                for dx in range(REGION_X[0], REGION_X[1] + 1)]

    def expected(self) -> dict[Cell, tuple[int, int]]:
        """(sky, block) for every non-solid cell of `region()`."""
        sky = _propagate(self, self._sky_seeds())
        block = _propagate(self, {cell: 14 for cell in self.torches})
        return {cell: (sky.get(cell, 0), block.get(cell, 0))
                for cell in self.region() if not self.solid(*cell)}

    def _domain(self) -> list[Cell]:
        return [(self.bx + dx, self.y0 + dy, self.z0 + dz)
                for dy in range(_DOMAIN_Y[0], _DOMAIN_Y[1] + 1)
                for dz in range(_DOMAIN_Z[0], _DOMAIN_Z[1] + 1)
                for dx in range(_DOMAIN_X[0], _DOMAIN_X[1] + 1)]

    def _sky_seeds(self) -> dict[Cell, int]:
        top = self.y0 + _DOMAIN_Y[1]
        seeds: dict[Cell, int] = {}
        for x, y, z in self._domain():
            if not self.solid(x, y, z) and not any(self.solid(x, above, z) for above in range(y + 1, top + 1)):
                seeds[(x, y, z)] = 15
        return seeds


def _propagate(model: SeamModel, seeds: dict[Cell, int]) -> dict[Cell, int]:
    inside: Callable[[Cell], bool] = lambda cell: (
        model.bx + _DOMAIN_X[0] <= cell[0] <= model.bx + _DOMAIN_X[1]
        and model.y0 + _DOMAIN_Y[0] <= cell[1] <= model.y0 + _DOMAIN_Y[1]
        and model.z0 + _DOMAIN_Z[0] <= cell[2] <= model.z0 + _DOMAIN_Z[1])
    level = dict(seeds)
    for value in range(15, 1, -1):
        for x, y, z in [cell for cell, held in level.items() if held == value]:
            for neighbour in ((x - 1, y, z), (x + 1, y, z), (x, y - 1, z), (x, y + 1, z),
                              (x, y, z - 1), (x, y, z + 1)):
                if inside(neighbour) and not model.solid(*neighbour) and level.get(neighbour, 0) < value - 1:
                    level[neighbour] = value - 1
    return level
