"""Offline unit tests for the chisel gate helpers (no server/client launched)."""
from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

from .backends import chisel


class ChiselHelperTests(unittest.TestCase):
    def test_rewrite_config_substitutes_each_token_once(self) -> None:
        source = ('port = 25565\nworld_dir = ".analysis/test-world-v12"\n'
                  'directory = "plugins"\nseed = 0\n')
        out = chisel.rewrite_config(source, port=1, world="w", plugins="p", seed=81)
        self.assertIn("port = 1", out)
        self.assertIn('world_dir = "w"', out)
        self.assertIn('directory = "p"', out)
        self.assertIn("seed = 81", out)

    def test_rewrite_config_rejects_missing_token(self) -> None:
        with self.assertRaises(RuntimeError):
            chisel.rewrite_config("port = 1\n", port=1, world="w", plugins="p", seed=81)

    def test_pad_fills_fit_the_server_limit(self) -> None:
        limit = 32768  # server max_block_modifications: larger fills reject whole
        for command in chisel._pad_commands(100, 64, -100):
            parts = command.split()
            x0, y0, z0, x1, y1, z1 = (int(v) for v in parts[1:7])
            volume = (x1 - x0 + 1) * (y1 - y0 + 1) * (z1 - z0 + 1)
            self.assertLessEqual(volume, limit, command)

    def test_block_matches_requires_every_wanted_property(self) -> None:
        block = {"block_id": chisel.SLAB, "properties": {"type": "top"}}
        self.assertTrue(chisel.block_matches(block, chisel.SLAB, {"type": "top"}))
        self.assertFalse(chisel.block_matches(block, chisel.SLAB, {"type": "bottom"}))
        self.assertFalse(chisel.block_matches(block, chisel.STAIRS, {}))

    def test_chisel_damage_reads_selected_stick_only(self) -> None:
        inventory = [
            {"item_id": "minecraft:stick", "count": 1, "damage": 3, "selected": False},
            {"item_id": "minecraft:stick", "count": 1, "damage": 4, "selected": True},
        ]
        self.assertEqual(chisel.chisel_damage(inventory), 4)
        self.assertIsNone(chisel.chisel_damage(
            [{"item_id": "minecraft:dirt", "count": 1, "damage": 0, "selected": True}]))

    def test_slab_count_sums_matching_stacks(self) -> None:
        state = {"inventory": [
            {"item_id": chisel.SLAB, "count": 1},
            {"item_id": chisel.SLAB, "count": 2},
            {"item_id": "minecraft:dirt", "count": 64},
        ]}
        self.assertEqual(chisel.slab_count(state), 3)

    def test_copy_package_refuses_missing_layout(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(RuntimeError, "missing"):
                chisel._copy_package(Path(tmp) / "absent", Path(tmp) / "out")

    def test_copy_package_copies_layout(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "chisel"
            (source / "client").mkdir(parents=True)
            (source / "plugin.toml").write_text("x")
            (source / "plugin.wasm").write_bytes(b"\x00")
            (source / "client" / "chisel-content.zip").write_bytes(b"\x00")
            report = chisel._copy_package(source, Path(tmp) / "out" / "chisel")
            self.assertIn("plugin.toml", report["files"])
            self.assertTrue((Path(tmp) / "out" / "chisel" / "plugin.wasm").is_file())

    def test_package_source_honours_override(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            old = os.environ.get("SOLARIS_CHISEL_PACKAGE")
            os.environ["SOLARIS_CHISEL_PACKAGE"] = tmp
            try:
                self.assertEqual(chisel._package_source(), Path(tmp).resolve())
            finally:
                if old is None:
                    del os.environ["SOLARIS_CHISEL_PACKAGE"]
                else:
                    os.environ["SOLARIS_CHISEL_PACKAGE"] = old


if __name__ == "__main__":
    unittest.main()
