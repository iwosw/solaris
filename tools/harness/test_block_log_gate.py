"""Offline unit tests for the block-log gate helpers (no server/client launched)."""
from __future__ import annotations

import math
import unittest

from .backends import block_log


class BlockLogLayoutTests(unittest.TestCase):
    def test_chests_are_single_and_within_ordinary_reach(self) -> None:
        plan = block_log.layout(0, 70, 0)
        chest, full = plan["chest"], plan["full_chest"]
        # Not side by side: two adjacent chests would form one double chest.
        self.assertGreater(abs(chest[0] - full[0]) + abs(chest[2] - full[2]), 1)
        # The client refuses targets beyond 4.5 blocks from the eye to the
        # block centre, whatever the game mode.
        for name, cell in plan.items():
            reach = math.dist((0.5, 71.62, 0.5), (cell[0] + 0.5, cell[1] + 0.5, cell[2] + 0.5))
            self.assertLess(reach, 4.5, name)

    def test_dirt_is_placed_on_the_floor_cell_below_it(self) -> None:
        plan = block_log.layout(10, 64, -3)
        floor, dirt = plan["dirt_floor"], plan["dirt"]
        self.assertEqual((floor[0], floor[1] + 1, floor[2]), dirt)
        self.assertNotIn(dirt, (plan["stone"], plan["chest"], plan["full_chest"]))

    def test_grief_window_excludes_the_setup_gap(self) -> None:
        window = block_log.grief_window_seconds(1_000.0, 1_007.4)
        self.assertGreaterEqual(window, 8)
        # The setup ended GAP_TICKS (20 per second) before the grief started.
        self.assertLess(window - 7.4, block_log.GAP_TICKS / 20)

    def test_chat_has_matches_any_recent_line(self) -> None:
        state = {"recent_chat": ["a", "Rollback complete: 4 change(s) rolled back"]}
        self.assertTrue(block_log.chat_has(state, "4 change(s)"))
        self.assertFalse(block_log.chat_has({}, "4 change(s)"))

    def test_rollback_reply_is_not_a_rolled_back_record_line(self) -> None:
        state = {"recent_chat": [
            "Rollback complete: 4 change(s) rolled back, 0 skipped (changed since)",
            "3s ago - BlockLogGate broke stone at 1 2 3 (rolled back)",
        ]}
        self.assertEqual(block_log.rolled_back_lines(state), 1)


if __name__ == "__main__":
    unittest.main()
