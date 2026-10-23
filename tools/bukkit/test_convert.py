import json
from pathlib import Path
import tempfile
import unittest
import zipfile

from convert import entry_source, material_source, metadata


class ConversionTests(unittest.TestCase):
    def manifest(self, extra):
        with tempfile.TemporaryDirectory() as directory:
            jar = Path(directory) / "plugin.jar"
            with zipfile.ZipFile(jar, "w") as archive:
                archive.writestr("plugin.yml", "name: Example\nmain: example.Plugin\nversion: '1.0.0'\n" + extra)
            return metadata(jar)

    def test_dependencies_refused_instead_of_silently_lost(self):
        with self.assertRaisesRegex(ValueError, "unsupported plugin.yml keys"):
            self.manifest("depend: [Vault]\n")

    def test_aliases_and_permissions_not_silently_downgraded(self):
        with self.assertRaisesRegex(ValueError, "unsupported command metadata"):
            self.manifest("commands:\n  admin:\n    permission: admin.use\n")

    def test_entry_is_derived_from_jar_not_fixture_name(self):
        source = entry_source(self.manifest("commands:\n  greet: {}\n"), [])
        self.assertIn("new example.Plugin()", source)
        self.assertIn('new PluginCommand("greet", Runtime.plugin)', source)
        self.assertNotIn("HelloPlugin", source)

    def test_handler_priority_and_ignore_cancelled_are_compiled(self):
        handlers = [
            ["example.Late", "receive", "org.bukkit.event.block.BlockBreakEvent", "HIGH", "true"],
            ["example.Early", "receive", "org.bukkit.event.block.BlockBreakEvent", "LOW", "false"],
        ]
        source = entry_source(self.manifest(""), handlers)
        self.assertLess(source.index("listener instanceof example.Early"), source.index("listener instanceof example.Late"))
        self.assertIn("!((org.bukkit.event.Cancellable) event).isCancelled()", source)

    def test_material_lookup_preserves_gaps_and_air_variants(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "blocks.json"
            report.write_text(json.dumps({
                "minecraft:air": {"states": [{"id": 0}]},
                "minecraft:cave_air": {"states": [{"id": 1}]},
                "minecraft:void_air": {"states": [{"id": 2}]},
                "minecraft:diamond_ore": {"states": [{"id": 42}, {"id": 43}]},
            }))
            enum, mapping = material_source(report)
            self.assertIn("this == CAVE_AIR", enum)
            self.assertIn("id >= 42 && id <= 43", mapping)
            self.assertIn('throw new IllegalArgumentException("unknown deployment block state")', mapping)
            self.assertNotIn("id >= 3", mapping)


if __name__ == "__main__":
    unittest.main()
