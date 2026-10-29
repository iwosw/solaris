"""Army QA fixture invariants + wedged-owner watchdog for the bind loop."""
from __future__ import annotations

import re
import unittest
from unittest.mock import Mock, patch

from .backends import army_qa

# Server `max_block_modifications` default: a larger fill is rejected whole.
FILL_LIMIT = 32768
FILL = re.compile(r"fill (-?\d+) (-?\d+) (-?\d+) (-?\d+) (-?\d+) (-?\d+) (\S+)")


class ArmyQaPadTests(unittest.TestCase):
    def test_pad_fills_fit_the_server_limit_and_cover_the_pad(self) -> None:
        """An oversized air fill is silently rejected, leaving a hillside that
        buries the barracks door one block below grade (soldiers cannot leave)."""
        ox, y0, oz = 192, 77, -448
        cleared: set[tuple[int, int, int]] = set()
        for command in army_qa._pad_commands(ox, y0, oz):
            match = FILL.fullmatch(command)
            self.assertIsNotNone(match, command)
            x0, y1, z0, x1, y2, z1 = (int(v) for v in match.groups()[:6])
            volume = (x1 - x0 + 1) * (y2 - y1 + 1) * (z1 - z0 + 1)
            self.assertLessEqual(volume, FILL_LIMIT, command)
            if match.group(7) == "minecraft:air" and y1 >= y0:
                cleared.update((x, y, z) for x in range(x0, x1 + 1)
                               for y in range(y1, y2 + 1) for z in range(z0, z1 + 1))
        half = army_qa.PAD_HALF
        want = {(x, y, z) for x in range(ox - half, ox + half + 1)
                for y in range(y0, y0 + 9) for z in range(oz - half, oz + half + 1)}
        self.assertEqual(want - cleared, set(), "air above the pad must be fully cleared")


class GuardLureTests(unittest.TestCase):
    def test_lure_waits_for_all_guards_and_leaves_room_for_excursion(self) -> None:
        post = (176.5, 77.0, -445.5)
        near = {"entity_uuid": "guard-a", "x": 177.5, "z": -445.5}
        far = {"entity_uuid": "guard-b", "x": 192.5, "z": -445.5}
        arrived = {**far, "x": 175.5}
        probe = Mock()

        def wait(_probe, predicate, label, timeout):
            self.assertEqual(label, "guard_lure_post_settled")
            with patch.object(army_qa, "_villagers", return_value=[near, far]):
                self.assertFalse(predicate({}), "returning guards must settle first")
            with patch.object(army_qa, "_villagers", return_value=[near]):
                self.assertFalse(predicate({}), "missing guards are not settled")
            with patch.object(army_qa, "_villagers", return_value=[near, arrived]):
                self.assertTrue(predicate({}))

        with patch.object(army_qa, "_bwait", side_effect=wait) as settled:
            lure = army_qa._prepare_guard_lure(probe, {"guard-a", "guard-b"}, post)
        settled.assert_called_once()
        self.assertEqual(lure, (187.5, 77.0, -445.5))
        self.assertGreater(lure[0] - post[0], 2 * 4.0 + 2.0)
        self.assertLess(lure[0] - post[0], 12.0)


class CommandOrderRosterTests(unittest.TestCase):
    def test_old_ordered_chat_does_not_prove_patrol(self) -> None:
        detail = {}
        with self.assertRaisesRegex(RuntimeError, "missing for"):
            army_qa._verify_order_coverage(
                detail, "Patrol",
                {"recent_chat": ["Ordered 3 soldiers: Patrol.",
                                 "36373a87 — 20 HP — Return"]},
                ["36373a87"])
        self.assertFalse(detail["order_Patrol_covered"])

    def test_every_recruited_soldier_must_show_patrol(self) -> None:
        detail = {}
        rows = ["36373a87 — 20 HP — Patrol", "b6ac53fe — 20 HP — Patrol"]
        army_qa._verify_order_coverage(
            detail, "Patrol", {"recent_chat": rows}, ["36373a87", "b6ac53fe"])
        self.assertTrue(detail["order_Patrol_covered"])
        with self.assertRaisesRegex(RuntimeError, "missing for"):
            army_qa._verify_order_coverage(
                detail, "Patrol", {"recent_chat": rows[:1]}, ["36373a87", "b6ac53fe"])


class WatchdogTests(unittest.TestCase):
    def test_fast_send_passes_through(self) -> None:
        from .backends.army_qa_watchdog import send_guarded
        self.assertEqual(send_guarded(lambda: {"ok": True}, "probe", timeout=5.0), {"ok": True})

    def test_error_propagates(self) -> None:
        from .backends.army_qa_watchdog import WedgedOwnerError, send_guarded

        def boom() -> None:
            raise RuntimeError("minecraft_send_chat failed: owner_result_unknown")

        with self.assertRaises(RuntimeError):
            send_guarded(boom, "probe", timeout=5.0)
        with self.assertRaises(WedgedOwnerError):
            send_guarded(lambda: __import__("time").sleep(30.0), "probe", timeout=0.5)


if __name__ == "__main__":
    unittest.main()
