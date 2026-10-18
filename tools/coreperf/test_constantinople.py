import copy
import random
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools.coreperf.constantinople import (
    census, population_plan, section_cells, nbt, stored_item_total, preservation, region_chunks,
    native_settlement_commands,
)


def terrain_fixture():
    return {"beds": [], "workstations": [], "farmland": [{"position": [0, 63, -2], "above": "air"}],
            "outdoor": [[x, 64, z, "grass_block"] for x in range(-24, 25) for z in range(-24, 25)]}


class ConstantinopleTests(unittest.TestCase):
    def test_empty_region_placeholder_is_not_a_truncated_record(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "r.-1.-10.mca"
            path.write_bytes(b"")
            self.assertEqual(list(region_chunks(path)), [])
            path.write_bytes(b"\0")
            with self.assertRaisesRegex(ValueError, "truncated region"):
                list(region_chunks(path))

    def test_placement_is_seeded_and_input_order_independent(self):
        terrain = terrain_fixture()
        before = copy.deepcopy(terrain)
        first = population_plan(terrain, 12, 8, 3, 1453)
        random.Random(5).shuffle(terrain["outdoor"])
        second = population_plan(terrain, 12, 8, 3, 1453)
        self.assertEqual(first, second)
        self.assertEqual(before["beds"], terrain["beds"])
        self.assertEqual(first["seeded_farmland"], 1)
        self.assertNotEqual(first["villagers"], population_plan(before, 12, 8, 3, 1454)["villagers"])
        self.assertEqual(len({v["uuid"] for v in first["villagers"] + first["animals"]}), 20)
        self.assertEqual(len({tuple(v["home"]) for v in first["villagers"]}), 12)
        self.assertEqual(len({tuple(v["job_site"]) for v in first["villagers"]}), 12)

    def test_target_tops_up_preserved_villagers_and_does_not_steal_their_pois(self):
        terrain = terrain_fixture()
        terrain["existing_villagers"] = [{"uuid": "00000000-0000-0000-0000-000000000001",
                                          "position": [10.5, 64, 10.5], "home": [10, 64, 10],
                                          "job_site": [12, 64, 10]}]
        terrain["beds"] = [{"position": [10, 64, 10], "facing": "east"}]
        terrain["workstations"] = [{"position": [12, 64, 10], "profession": "farmer"}]
        result = population_plan(terrain, 5, 0, 1, 1453)
        self.assertEqual(result["target_villagers"], 5)
        self.assertEqual(len(result["existing_villagers"]), 1)
        self.assertEqual(len(result["villagers"]), 4)
        self.assertNotIn([10, 64, 10], [v["home"] for v in result["villagers"]])
        self.assertNotIn([12, 64, 10], [v["job_site"] for v in result["villagers"]])
        with self.assertRaisesRegex(ValueError, "below"):
            population_plan(terrain, 0, 0, 1, 1453)

    def test_reused_home_spawns_never_overlap_later_module_blocks(self):
        terrain = terrain_fixture()
        homes = [[x, 64, z] for x in (-16, -8, 0, 8, 16) for z in (-8, 8)]
        terrain["beds"] = [{"position": home, "facing": "east"} for home in homes]
        terrain["workstations"] = [{"position": [x + 2, y, z], "profession": "fisherman"}
                                   for x, y, z in homes]
        occupied = {(x + dx, z) for x, _, z in homes for dx in (-1, 0, 2)}
        terrain["outdoor"] = [p for p in terrain["outdoor"] if (p[0], p[2]) not in occupied]
        result = population_plan(terrain, 100, 0, 1, 0)
        spawns = {tuple(v["position"]) for v in result["villagers"]}
        self.assertEqual(len(spawns), 100)
        self.assertTrue(spawns.isdisjoint(tuple(v["job_site"]) for v in result["villagers"]))

    def test_existing_farmland_plants_are_not_obstructions(self):
        for plant in ("melon_stem", "attached_melon_stem", "pumpkin_stem",
                      "attached_pumpkin_stem", "torchflower_crop", "pitcher_crop"):
            with self.subTest(plant=plant):
                terrain = terrain_fixture()
                terrain["farmland"][0]["above"] = plant
                result = population_plan(terrain, 0, 0, 1, 1453)
                self.assertEqual(result["seeded_farmland"], 1)

    def test_census_requires_canonical_export_entity_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            world = Path(directory)
            (world / "region").mkdir()
            (world / "region/r.0.0.mca").write_bytes(bytes(8192))
            with self.assertRaisesRegex(ValueError, "entities"):
                census(world)

    def test_counts_only_matches_full_census_without_retaining_cell_terrain(self):
        palette = [{"Name": "minecraft:" + name} for name in
                   ("air", "grass_block", "red_bed", "composter", "farmland", "wheat")]
        palette[2]["Properties"] = {"part": "head", "facing": "east"}
        cells = [1] * 256 + [0] * (4096 - 256)
        cells[2], cells[256], cells[257], cells[258] = 4, 2, 3, 5
        words = [sum(value << (4 * shift) for shift, value in enumerate(cells[start:start + 16]))
                 for start in range(0, 4096, 16)]
        chunk = {"DataVersion": 3955, "xPos": 0, "zPos": 0,
                 "sections": [{"Y": 4, "block_states": {"palette": palette, "data": words}}]}
        villagers = {"Entities": [{"id": "minecraft:villager", "UUID": [0, 0, 0, 1],
                                    "Pos": [0.5, 66, 0.5]}, {"id": "minecraft:pig"}]}
        with tempfile.TemporaryDirectory() as directory:
            world = Path(directory)
            for name in ("region", "entities"):
                (world / name).mkdir()
                (world / name / "r.0.0.mca").touch()
            with patch("tools.coreperf.constantinople.region_chunks",
                       side_effect=lambda path: iter([(0, 0, chunk if path.parent.name == "region" else villagers)])):
                full, terrain = census(world)
                counts, omitted = census(world, collect_terrain=False)
        self.assertEqual(counts, full)
        self.assertIsNone(omitted)
        self.assertEqual(counts["counts"], {"beds": 1, "workstations": 1, "street_cells": 253,
                         "chunks": 1, "regions": 1, "farmland": 1, "crop_blocks": 1,
                         "existing_villagers": 1, "existing_entities": 2})
        self.assertEqual(len(terrain["farmland"]), 1)
        self.assertEqual(len(terrain["outdoor"]), 253)

    def test_preservation_counts_legacy_and_modern_records_across_dimensions(self):
        identity = "00000000-0000-0000-0000-000000000007"
        player = {"UUID": [0, 0, 0, 7], "Inventory": [{"id": "minecraft:stone", "Count": 7}]}
        shadow = {"UUID": player["UUID"], "Inventory": [{"id": "minecraft:stone", "Count": 99}]}
        bodies = [{"id": "minecraft:villager", "Inventory": [{"id": "minecraft:bread", "Count": 3}],
                   "Passengers": [{"id": "minecraft:pig"}]}]
        blocks = [{"id": "minecraft:chest", "Items": [{"id": "minecraft:stone", "Count": 5}]},
                  {"id": "minecraft:sign"}]
        nether = {"block_entities": [{"id": "minecraft:chest",
                                      "Items": [{"id": "minecraft:stone", "count": 2}]}]}
        with tempfile.TemporaryDirectory() as directory:
            source, native = (Path(directory) / name for name in ("source", "native"))
            records = {
                source / "region/r.0.0.mca": {"Level": {"Entities": bodies, "TileEntities": blocks}},
                source / "DIM-1/region/r.0.0.mca": nether,
                native / "dimensions/minecraft/overworld/region/r.0.0.mca": {"block_entities": blocks},
                native / "dimensions/minecraft/overworld/entities/r.0.0.mca": {"Entities": bodies},
                native / "dimensions/minecraft/the_nether/region/r.0.0.mca": nether,
            }
            for path in records:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.touch()
            for world, level in ((source, b"embedded"), (native, b"empty")):
                (world / "level.dat").write_bytes(level)
            for world, name, raw in ((source, "playerdata", b"shadow"), (native, "players/data", b"player")):
                path = world / name / (identity + ".dat")
                path.parent.mkdir(parents=True)
                path.write_bytes(raw)
            decoded = {b"embedded": {"Data": {"Player": player}}, b"empty": {},
                       b"shadow": shadow, b"player": player}
            with patch("tools.coreperf.constantinople.nbt", side_effect=decoded.__getitem__), \
                 patch("tools.coreperf.constantinople.region_chunks",
                       side_effect=lambda path: iter([(0, 0, records[path])])):
                result = preservation(source, native)
                self.assertEqual(result["status"], "PASS")
                self.assertEqual(result["source"]["counts"], {"entities": 2, "block_entities": 3,
                                 "container_items": 10, "players": 1, "player_items": 7})
                decoded[b"player"] = {**player, "Inventory": [{"id": "minecraft:stone", "count": 6}]}
                result = preservation(source, native)
                self.assertEqual(result["status"], "FAIL")
                self.assertEqual(result["differences"]["player_items"], {"source": 7, "native": 6})
                self.assertIn("player_items_by_uuid", result["differences"])

    def test_preservation_counts_nested_containers_and_equipment_not_derived_items(self):
        old = {"Items": [{"id": "minecraft:shulker_box", "Count": 1, "tag": {
            "BlockEntityTag": {"Items": [{"id": "minecraft:stone", "Count": 8}]}}}],
               "ArmorItems": [{}, {"id": "minecraft:iron_helmet", "Count": 1}],
               "SelectedItem": {"id": "minecraft:stone", "Count": 64}}
        modern = {"Items": [{"id": "minecraft:shulker_box", "count": 1, "components": {
            "minecraft:container": [{"slot": 0, "item": {"id": "minecraft:stone", "count": 8}}]}}],
                  "equipment": {"head": {"id": "minecraft:iron_helmet", "count": 1}}}
        self.assertEqual(stored_item_total(old, "source"), 10)
        self.assertEqual(stored_item_total(modern, "native"), 10)
        with self.assertRaisesRegex(ValueError, r"chest\.Items\[0\]"):
            stored_item_total({"Items": [{"id": "minecraft:stone", "Count": "8"}]}, "chest")

    def test_preservation_counts_legacy_equipment_physical_items_once(self):
        armor = {"id": "minecraft:diamond_horse_armor", "Count": 1}
        saddle = {"id": "minecraft:saddle", "Count": 1}
        old = {"id": "minecraft:horse", "ArmorItem": armor, "SaddleItem": saddle,
               "ArmorItems": [{}, {}, armor, {}]}
        modern = {"id": "minecraft:horse", "equipment": {
            "body": {"id": "minecraft:diamond_horse_armor", "count": 1},
            "saddle": {"id": "minecraft:saddle", "count": 1}}}
        self.assertEqual(stored_item_total(old, "source"), 2)
        self.assertEqual(stored_item_total(modern, "native"), 2)
        distinct = {**old, "ArmorItems": [{}, {}, {"id": "minecraft:stone", "Count": 1}, {}]}
        self.assertEqual(stored_item_total(distinct, "distinct"), 3)
        self.assertEqual(stored_item_total({**old, "id": "mod:horse"}, "opaque"), 3)
        for kind in ("minecraft:pig", "minecraft:strider"):
            with self.subTest(kind=kind):
                self.assertEqual(stored_item_total({"id": kind, "Saddle": 1}, "legacy"), 1)
                self.assertEqual(stored_item_total({"id": kind, "Saddle": 0}, "unsaddled"), 0)
                self.assertEqual(stored_item_total({"id": kind, "Saddle": 1,
                    "equipment": {"saddle": {"id": "minecraft:saddle", "count": 1}}}, "modern"), 1)

    def test_routes_are_closed_cardinal_edges_and_avoid_authored_pois(self):
        result = population_plan(terrain_fixture(), 20, 8, 3, 1453)
        pois = {tuple(v[field]) for v in result["villagers"] for field in ("home", "job_site")}
        for route in result["street_routes"]:
            self.assertGreater(len(route), 2)
            for a, b in zip(route, route[1:] + route[:1]):
                self.assertEqual(abs(a[0] - b[0]) + abs(a[2] - b[2]), 1)
                self.assertLessEqual(abs(a[1] - b[1]), 1)
                self.assertNotIn(tuple(a), pois)
        self.assertEqual({a["kind"] for a in result["animals"]}, {"sheep", "chicken", "pig", "cow"})

    def test_obstructed_farmland_and_insufficient_space_fail_closed(self):
        terrain = terrain_fixture()
        terrain["farmland"][0]["above"] = "stone"
        with self.assertRaisesRegex(ValueError, "obstructed"):
            population_plan(terrain, 0, 0, 1, 1)
        terrain["outdoor"] = [[x, 64, 0, "stone"] for x in range(20)]
        terrain["farmland"] = []
        with self.assertRaisesRegex(ValueError, "only 0 villagers fit"):
            population_plan(terrain, 1, 0, 1, 1)

    def test_modern_padded_palettes_do_not_cross_word_boundaries(self):
        palette = [{"Name": f"minecraft:state_{i}"} for i in range(17)]
        expected = [i % 17 for i in range(4096)]
        words = []
        for start in range(0, 4096, 12):
            word = sum(value << (5 * shift) for shift, value in enumerate(expected[start:start + 12]))
            words.append(word)
        self.assertEqual(section_cells({"block_states": {"palette": palette, "data": words}})[1], expected)

    def test_nbt_rejects_truncation_and_duplicate_names(self):
        self.assertEqual(nbt(b"\x0a\x00\x00\x00"), {})
        with self.assertRaises(ValueError):
            nbt(b"\x0a\x00\x00")
        with self.assertRaisesRegex(ValueError, "duplicate"):
            nbt(b"\x0a\x00\x00\x01\x00\x01a\x00\x01\x00\x01a\x00\x00")

    def test_native_population_uses_real_create_bind_commands_and_restores_spawn(self):
        population = {"count": 2, "spawn_position": [8, 311, 9],
                      "settlement_anchors": [{"name": "first", "anchor": [1, 311, 2], "radius": 8},
                                             {"name": "second", "anchor": [3, 311, 4], "radius": 8}],
                      "cohort": [{"uuid": "a", "settlement": "first"},
                                 {"uuid": "b", "settlement": "second"}]}
        self.assertEqual(native_settlement_commands(population), [
            "setworldspawn 1 311 2", "settlement create first 8",
            "setworldspawn 3 311 4", "settlement create second 8",
            "settlement bind first a", "settlement bind second b",
            "setworldspawn 8 311 9", "settlement info first", "settlement info second"])
        with self.assertRaisesRegex(ValueError, "complete bounded"):
            native_settlement_commands({**population, "count": 3})


if __name__ == "__main__":
    unittest.main()
