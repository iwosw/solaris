"""Unit contracts for the bench pipeline's pure parts; no server, systemd or JVM launch."""
import json
from pathlib import Path
import tempfile
import unittest

from tools.bench import common, profile_runs, worldgen_runs

# 6 cores / 12 threads, siblings (0,1) (2,3) ...: the shape of the reference host (lscpu -e).
SMT = {c: [c // 2 * 2, c // 2 * 2 + 1] for c in range(12)}


def sibling_of(cpu):
    return SMT[cpu]


class CommonTests(unittest.TestCase):
    def test_sizes(self):
        self.assertEqual(common.parse_size("4G"), 4 * 2**30)
        self.assertEqual(common.parse_size("512m"), 512 * 2**20)
        with self.assertRaises(common.BenchError):
            common.parse_size("4 gigs")

    def test_cpu_lists_roundtrip(self):
        self.assertEqual(common.parse_cpu_list("0-2,8,10-11"), [0, 1, 2, 8, 10, 11])
        self.assertEqual(common.cpu_list([0, 1, 8]), "0,1,8")

    def test_logical_series_includes_smt_siblings(self):
        pool = list(range(12))
        two = common.split_cpus(pool, 2, "logical", sibling_of)
        self.assertEqual((two["server"], two["physical_cores_touched"]), ("0,1", 1))  # "2 vCPU" = one physical core
        six = common.split_cpus(pool, 6, "logical", sibling_of)
        self.assertEqual((six["server"], six["physical_cores_touched"]), ("0,1,2,3,4,5", 3))  # 0-5 is 3 cores, not 6
        self.assertEqual(six["bots"], "6,7,8,9,10,11")

    def test_physical_series_is_one_cpu_per_core_and_bots_avoid_sibling_cores(self):
        pool = list(range(12))
        two = common.split_cpus(pool, 2, "physical", sibling_of)
        self.assertEqual((two["server"], two["physical_cores_touched"]), ("0,2", 2))
        self.assertEqual(two["bots"], "4,5,6,7,8,9,10,11")  # not the idle siblings 1,3 of the server cores
        six = common.split_cpus(pool, 6, "physical", sibling_of)
        self.assertEqual(six["server"], "0,2,4,6,8,10")
        with self.assertRaisesRegex(common.BenchError, "only 6 available"):
            common.split_cpus(pool, 8, "physical", sibling_of)

    def test_full_pool_shares_cpus_with_bots(self):
        full = common.split_cpus(list(range(12)), 12, "logical", sibling_of)
        self.assertTrue(full["bots_share_server_cpus"])
        self.assertEqual(full["bots"], full["server"])

    def test_more_cpus_than_pool_is_a_clear_error(self):
        with self.assertRaisesRegex(common.BenchError, "only 4 available"):
            common.split_cpus([0, 1, 2, 3], 8, "logical", lambda c: [c])

    def test_scope_prefix_caps_memory_without_swap(self):
        prefix = common.scope_prefix("4G", "solaris-heavy.slice")
        self.assertIn("MemoryMax=4G", prefix)
        self.assertIn("MemorySwapMax=0", prefix)
        self.assertIn("--slice=solaris-heavy.slice", prefix)
        self.assertEqual(common.shell_prefix(None, None), "")
        self.assertTrue(common.shell_prefix("4G", None).endswith(" --"))


class WorldgenTests(unittest.TestCase):
    OUTPUT = """pregenerate: 0/1089 chunks (0.0%), 0 generated, 0 already stored, 2.5s
pregenerate: 300/1089 chunks (27.5%), 300 generated, 0 already stored, 7.5s
pregenerate complete: chunks [-16,-16]..[16,16] around block (0,0), 1089 chunks, 1089 generated, 0 already stored, 12.5s
"""

    def test_rate_excludes_world_open(self):
        parsed = worldgen_runs.parse_pregenerate(self.OUTPUT)
        self.assertEqual(parsed["chunks"], 1089)
        self.assertAlmostEqual(parsed["generation_seconds"], 10.0)
        self.assertAlmostEqual(parsed["chunks_per_s"], 108.9)
        self.assertAlmostEqual(parsed["chunks_per_s_incl_setup"], 87.12)

    def test_missing_complete_line_fails_loudly(self):
        with self.assertRaises(common.BenchError):
            worldgen_runs.parse_pregenerate("pregenerate: 0/4 chunks (0.0%), 0 generated, 0 already stored, 1.0s\n")

    def test_forceload_tiles_respect_vanilla_limit(self):
        commands = worldgen_runs.forceload_commands([-16, -16], [16, 16])
        self.assertEqual(len(commands), 9)  # 33x33 chunks -> 3x3 tiles of <= 16x16
        covered = set()
        for command in commands:
            _, _, x1, z1, x2, z2 = command.split()
            x1, z1, x2, z2 = (int(v) // 16 for v in (x1, z1, x2, z2))
            self.assertLessEqual((x2 - x1 + 1) * (z2 - z1 + 1), worldgen_runs.MAX_FORCELOAD_CHUNKS)
            covered |= {(x, z) for x in range(x1, x2 + 1) for z in range(z1, z2 + 1)}
        self.assertEqual(len(covered), 33 * 33)

    def test_time_file_parsing(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "time.txt"
            path.write_text("\tUser time (seconds): 3.5\n\tSystem time (seconds): 0.5\n"
                            "\tMaximum resident set size (kbytes): 2048\n")
            parsed = worldgen_runs.parse_time_file(path)
        self.assertEqual(parsed, {"peak_rss_mib": 2.0, "cpu_seconds": 4.0})

    def test_median_picks_middle_run(self):
        runs = [{"status": "ok", "chunks_per_s": v} for v in (30.0, 10.0, 20.0)] + [{"status": "failed"}]
        median = worldgen_runs.median_by_engine(runs)
        self.assertEqual(median["chunks_per_s"], 20.0)
        self.assertEqual((median["repeats_ok"], median["repeats"]), (3, 4))


class ProfileRunTests(unittest.TestCase):
    def receipt(self, directory: Path, **fields):
        (directory / "result.json").write_text(json.dumps(fields))

    def test_measured_receipt_is_normalised_to_mib_and_ms(self):
        split = common.split_cpus([0, 1, 2, 3], 2, "logical", lambda c: [c])
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            summary = {"duration_seconds": 25.0, "ticks": 500, "tps": 20.0,
                       "mspt": {"p50": 1.5, "p95": 2.0, "p99": 3.0, "max": 9.0},
                       "rss": {"server": {"peak_lifetime_bytes": 300 * 2**20, "steady_median_bytes": 200 * 2**20},
                               "bots": {"peak_lifetime_bytes": 50 * 2**20}},
                       "population_observed": {"first": 1, "last": 2},
                       "resident_chunks_observed": {"first": 100, "last": 150}}
            self.receipt(directory, status="measured", measurement={"summary": summary})
            row = profile_runs.normalise("explore-20", split, "4G", directory, 0, None, 30.0)
        self.assertEqual(row["status"], "ok")
        self.assertEqual(row["server_rss_mib"], {"peak": 300.0, "steady_median": 200.0})
        self.assertAlmostEqual(row["resident_chunks_per_s_proxy"], 2.0)
        self.assertEqual(row["cpus_server"], "0,1")

    def test_failed_run_carries_error_and_no_numbers(self):
        split = common.split_cpus([0, 1], 1, "logical", lambda c: [c])
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            self.receipt(directory, status="failed", error="RuntimeError: server exited")
            row = profile_runs.normalise("idle", split, None, directory, 1, None, 3.0)
        self.assertEqual(row["status"], "failed")
        self.assertIn("server exited", row["error"])
        self.assertNotIn("tps", row)


class RemoteSessionTests(unittest.TestCase):
    LOCAL = '''[server]
name = "catalog-load-isolated"
max_players = 20
[network]
bind_address = "127.0.0.1"
port = 41000
[auth]
online_mode = false
[data]
world_dir = "/local/world"
seed = 42
vanilla_data_dir = "/local/vanilla"
[dashboard]
enabled = true
bind_address = "127.0.0.1"
port = 41001
allow_remote = false
'''

    def session(self, **options):
        from tools.bench.remote_session import RemoteSession
        import argparse
        session = object.__new__(RemoteSession)
        session.args = argparse.Namespace(remote_data_dir="/opt/content/26.1.2", remote_tunnel_game=False,
                                          remote_game_port=25590, remote_dash_port=25591, **options)
        session.rdir = "/var/tmp/solaris-bench-1"
        with tempfile.TemporaryDirectory() as directory:
            session.config = Path(directory) / "server.toml"
            session.config.write_text(self.LOCAL)
            return session.remote_config()

    def test_paths_and_ports_are_rewritten_everything_else_kept(self):
        import tomllib
        remote = tomllib.loads(self.session())
        self.assertEqual(remote["data"], {"world_dir": "/var/tmp/solaris-bench-1/world", "seed": 42,
                                          "vanilla_data_dir": "/opt/content/26.1.2"})
        self.assertEqual((remote["network"]["bind_address"], remote["network"]["port"]), ("0.0.0.0", 25590))
        self.assertEqual((remote["dashboard"]["bind_address"], remote["dashboard"]["port"]), ("127.0.0.1", 25591))
        self.assertEqual(remote["server"]["max_players"], 20)


if __name__ == "__main__":
    unittest.main()
