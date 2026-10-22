"""Offline unit tests for the anti-xray gate helpers (no server/client launched)."""
from __future__ import annotations

import unittest

from .backends import anti_xray


def _neighbours(x: int, y: int, z: int) -> list[tuple[int, int, int]]:
    return [(x + 1, y, z), (x - 1, y, z), (x, y + 1, z), (x, y - 1, z),
            (x, y, z + 1), (x, y, z - 1)]


class AntiXrayLayoutTests(unittest.TestCase):
    def test_both_ores_start_fully_enclosed_by_the_stone_block(self) -> None:
        plan = anti_xray.layout(100, 64, -100)
        x0, y0, z0, x1, y1, z1 = plan["fill"]
        inside = lambda p: x0 <= p[0] <= x1 and y0 <= p[1] <= y1 and z0 <= p[2] <= z1
        ores = {plan["ore_a"], plan["ore_b"]}
        for ore in ores:
            self.assertTrue(all(inside(n) and n not in ores for n in _neighbours(*ore)), ore)

    def test_each_mined_block_is_the_face_block_beside_exactly_its_ore(self) -> None:
        plan = anti_xray.layout(0, 70, 0)
        self.assertIn(plan["mine_a"], _neighbours(*plan["ore_a"]))
        self.assertNotIn(plan["mine_a"], _neighbours(*plan["ore_b"]))
        self.assertIn(plan["mine_b"], _neighbours(*plan["ore_b"]))
        self.assertNotIn(plan["mine_b"], _neighbours(*plan["ore_a"]))

    def test_fixture_sits_inside_the_pad_and_scan_is_the_whole_block(self) -> None:
        plan = anti_xray.layout(0, 70, 0)
        x0, y0, z0, x1, y1, z1 = plan["fill"]
        self.assertEqual((x1 - x0 + 1) * (y1 - y0 + 1) * (z1 - z0 + 1), 45)
        for stand in (plan["stand_a"], plan["stand_b"]):
            self.assertLessEqual(abs(stand[0]), 20)

    def test_diamond_rows_selects_only_diamond_ore(self) -> None:
        rows = [{"block_id": anti_xray.STONE}, {"block_id": anti_xray.DIAMOND},
                {"block_id": "minecraft:deepslate_diamond_ore"}]
        self.assertEqual(anti_xray.diamond_rows(rows), [{"block_id": anti_xray.DIAMOND}])


if __name__ == "__main__":
    unittest.main()
