import hashlib
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

from tools.worldgen_diff import generate


class OracleCacheTests(unittest.TestCase):
    def test_comparison_rectangle_contains_and_is_centered_on_actual_spawn(self):
        low, high = generate.spawn_bounds({'spawn': {'pos': [-576, 68, 432]}}, 32)
        self.assertEqual(low, (-52, 11))
        self.assertEqual(high, (-21, 42))
        low, high = generate.spawn_bounds({'spawn': {'pos': [-1, 70, -17]}}, 3)
        self.assertEqual(low, (-2, -3))
        self.assertEqual(high, (0, -1))

    def test_load_function_freezes_before_world_ticks_not_an_early_console_command(self):
        with tempfile.TemporaryDirectory() as temp:
            world = Path(temp)
            generate._datapack(world, (-1, -1), (0, 0))
            pack = world / 'datapacks/wgd/data'
            load = json.loads((pack / 'minecraft/tags/function/load.json').read_text())
            self.assertEqual(load['values'], ['wgd:freeze'])
            self.assertEqual((pack / 'wgd/function/freeze.mcfunction').read_text(), 'tick freeze\n')

    def test_bundles_get_separate_extracted_classpaths(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            classpaths = []
            for version in ('a', 'b'):
                jar = root / f'{version}.jar'
                with zipfile.ZipFile(jar, 'w') as archive:
                    archive.writestr('META-INF/versions/test/server.jar', version)
                    archive.writestr('META-INF/libraries/test/library.jar', 'library')
                classpath = generate._runtime(jar, root / 'cache')
                self.assertEqual(classpath[0].read_text(), version)
                self.assertEqual(classpath[1].read_text(), 'library')
                self.assertEqual(generate._runtime(jar, root / 'cache'), classpath)
                classpaths.append(classpath)
            self.assertNotEqual(classpaths[0], classpaths[1])

    def test_cached_world_rejects_a_changed_or_unidentified_bundle(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            jar = root / 'server.jar'
            jar.write_bytes(b'local test jar')
            cache = root / 'vanilla-0--1_-1-0_0'
            cache.mkdir()
            receipt = cache / 'receipt.json'
            receipt.write_text('{}')
            with self.assertRaisesRegex(RuntimeError, 'another jar'):
                generate.vanilla_world(0, (-1, -1), (0, 0), jar, 'java', root, '1G')
            receipt.write_text(json.dumps({
                'bundle_sha256': hashlib.sha256(jar.read_bytes()).hexdigest()}))
            self.assertTrue(generate.vanilla_world(
                0, (-1, -1), (0, 0), jar, 'java', root, '1G')['cached'])

    def test_forceload_tiles_cover_rectangle_without_gaps_or_duplicates(self):
        found = []
        for command in generate._forceload_commands((-16, -16), (15, 15)):
            _, _, x0, z0, x1, z1 = command.split()
            x0, z0, x1, z1 = (int(v) // 16 for v in (x0, z0, x1, z1))
            area = [(x, z) for x in range(x0, x1 + 1) for z in range(z0, z1 + 1)]
            self.assertLessEqual(len(area), 256)
            found.extend(area)
        self.assertEqual(len(found), 1024)
        self.assertEqual(set(found), {(x, z) for x in range(-16, 16) for z in range(-16, 16)})


if __name__ == '__main__':
    unittest.main()
