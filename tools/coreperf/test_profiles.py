"""Unit contracts for public profile receipts; no server launch."""
import argparse
import unittest

from tools.coreperf import profiles


class ProfilesTests(unittest.TestCase):
    def test_shared_heavy_fixture_exact_counts_and_loaded_regions(self):
        recipe = profiles.fixture(100000, 20000)
        self.assertLessEqual(len(recipe["regions"]), 100)
        commands = [c for r in recipe["regions"] for c in r["commands"]]
        for kind, count in recipe["expected"].items():
            self.assertEqual(sum(c.startswith(f"summon {kind} ") for c in commands), count)
        self.assertEqual(recipe["expected"]["minecraft:villager"], 20000)
        self.assertEqual(sum("minecraft:composter" in c for c in commands), 20000)
        self.assertFalse(any("NoAI" in c or "Invulnerable" in c for c in commands))
        self.assertIn("gamerule randomTickSpeed 3", commands)
        self.assertIn("gamerule doDaylightCycle true", commands)

    def test_native_fixture_has_eight_equal_mob_kinds_and_no_villagers(self):
        recipe = profiles.fixture(100000, 0)
        self.assertEqual(recipe["expected"]["minecraft:villager"], 0)
        self.assertEqual([n for k, n in recipe["expected"].items() if k != "minecraft:villager"], [12500] * 8)

    def test_idle_and_explore_do_not_summon_entities(self):
        recipe = profiles.fixture(0, 0)
        self.assertTrue(recipe["regions"])
        self.assertFalse(any(c.startswith("summon ") for r in recipe["regions"] for c in r["commands"]))
        for name, movement, edits in (("idle", 0, 0), ("explore-20", 20, 0), ("native-100", 20, 2)):
            args = argparse.Namespace(profile=name, mobs=profiles.PROFILES[name][1], warmup_seconds=30, measure_seconds=120)
            plan = profiles.schedule(args)["windows"]
            self.assertEqual([w["duration_ms"] for w in plan], [30000, 120000])
            self.assertEqual(plan[-1]["movement_hz"], movement)
            self.assertEqual(plan[-1]["edit_hz"], edits)

    def test_half_open_window_tps_and_server_only_memory(self):
        samples = [{"wall_start_unix_ns": at, "wall_us": wall} for at, wall in
                   ((999, 999999), (1000, 1000), (2000, 2000), (3000, 3000), (4000, 999999))]
        processes = [{"captured_unix_ns": at, "processes": {
            "server": {"rss_bytes": rss, "peak_rss_bytes": 100},
            "bots": {"rss_bytes": 1000, "peak_rss_bytes": 2000}},
            "stats": {"entities": {"total": 3}, "chunks": {"loaded_total": 4}}}
            for at, rss in ((999, 999999), (1000, 10), (2000, 20), (3000, 30), (4000, 999999))]
        result = profiles.summarize(samples, processes, 1000, 4000)
        self.assertEqual(result["ticks"], 3)
        self.assertEqual(result["tps"], 1000000)
        self.assertEqual(result["mspt"], {"p50": 2, "p95": 3, "p99": 3, "max": 3})
        self.assertEqual(result["rss"]["server"]["steady_median_bytes"], 20)
        self.assertEqual(result["rss"]["server"]["peak_lifetime_bytes"], 100)
        self.assertIsNone(result["chunk_generation_per_second"])
        with self.assertRaises(ValueError):
            profiles.summarize([], processes, 1000, 4000)

    def test_missing_reset_overrun_and_noncontiguous_ticks_fail(self):
        initial = {"schema_version": 1, "instance_started_unix_ns": 1, "source_skipped_ticks_total": 0}
        metric = dict(initial, samples=[{"tick": 11, "wall_start_unix_ns": 100, "wall_us": 20}])
        self.assertEqual(profiles.validate_metrics(metric, initial, 10), 11)
        for changes in ({"overrun": True}, {"instance_started_unix_ns": 2},
                        {"source_skipped_ticks_total": 1}, {"schema_version": 2},
                        {"samples": [{"tick": 12, "wall_start_unix_ns": 100, "wall_us": 20}]},
                        {"samples": [{"tick": 11, "wall_start_unix_ns": 100, "wall_us": -1}]}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                profiles.validate_metrics(dict(metric, **changes), initial, 10)


if __name__ == "__main__":
    unittest.main()
