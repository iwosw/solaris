import unittest
from copy import deepcopy

from . import catalog_mixes as mixes


def receipt(fixture):
    expected = fixture["expected"]
    blocks = {name: {"final": {"name": kind, "properties": {}}, "update_count": 1}
              for name, kind in expected.get("blocks", {}).items()}
    entities = {"minecraft:armor_stand": {"samples": [
        {"uuid": row["uuid"], "active": row["active"], "final_position": row.get("position")}
        for row in expected.get("entities", [])]}}
    mechanics = {"blocks": deepcopy(blocks), "entities": deepcopy(entities), "windows": {
        "burst": {"explosions": expected.get("finite_blasts", 0)}},
        "actions": [{"action": index, "offered": 1, "admitted": 1, "writes_completed": 1, "errors": 0}
                    for index, _ in enumerate(fixture["mechanic_spec"]["actions"])]}
    mechanics["explosions"] = [{"center": [n + .5, 64.0, .5], "bot": 0}
                              for n in range(expected.get("finite_blasts", 0))]
    mechanics["players"] = [
        {"final_blocks": {name: deepcopy(row["final"]) for name, row in blocks.items()},
         "final_observed_entities": [
             {"uuid": row["uuid"], "final_position": row["position"]}
             for row in expected.get("entities", []) if row["active"]]}
        for _ in range(expected["clients"] - expected.get("cohort_clients", 0))]
    native = {"read_only": True, "identity": {"seed": 42, "worldgen_revision": 1},
              "blocks": blocks, "entities": entities}
    result = {"bots": {"ready_clients": expected["clients"], "disconnects": 0,
                       "open_loop": {"schedule_complete": True, "windows": [{"name": "recovery"}]},
                       "mechanics": mechanics},
              "native_snapshot": native, "initial_native_snapshot": {},
              "live_complete": {"event": "schedule_complete", "mechanics": deepcopy(mechanics)}}
    if expected.get("cohort"):
        result["bots"].update(ready_clients=1, chat_echo_clients=1)
        result["windows"] = {"burst": {"start_unix_ns": 100, "end_unix_ns": 1000}}
        result["cohort"] = {"released_unix_ns": 110, "ready": {"ready_unix_ns": 120},
            "completed_unix_ns": 900,
            "complete": {"open_loop": {"schedule_complete": True}, "chat_echo_clients": expected["cohort_clients"]},
            "bots": {"ready_clients": expected["cohort_clients"], "chat_echo_clients": expected["cohort_clients"]}}
    if fixture.get("restart_check"):
        result["restart"] = {"same_world": True, "fixture_regenerated": False, "repair_commands": [],
                             "after": deepcopy(native)}
    return result


class CatalogMixesTests(unittest.TestCase):
    def test_exact_owned_ids_are_runnable_with_real_stimuli(self):
        for identifier in mixes.IDS:
            fixture = mixes.recipe(identifier, 2, True)
            self.assertIsNone(fixture["capability"]["outcome"])
            self.assertTrue(fixture["commands"])
            self.assertTrue(fixture["mechanic_spec"]["actions"])
            self.assertEqual(fixture["expected"]["scenario_id"], identifier)
            self.assertEqual(fixture["schedule_overrides"]["recovery"]["movement_hz"], 0)
        for identifier in ("MIX-03", "mix-18", "MIX-180"):
            self.assertIsNone(mixes.recipe(identifier, 2, True))

    def test_invalid_client_counts(self):
        for clients in (True, 0, -1, 1.5, "2"):
            with self.subTest(clients=clients), self.assertRaises(ValueError):
                mixes.recipe("MIX-18", clients, True)

    def test_transport_only_cannot_pass(self):
        for identifier in mixes.IDS:
            proof = mixes.validate(identifier, {"bots": {"chunks_received": 10000}}, mixes.recipe(identifier, 2, True))
            self.assertFalse(proof["passed"])
            self.assertEqual(proof["infra_errors"], [])

    def test_pinned_adhesive_and_gravity_image_detects_loss_or_duplication(self):
        fixture = mixes.recipe("MIX-04", 2, True)
        report = receipt(fixture)
        self.assertTrue(mixes.validate("MIX-04", report, fixture)["passed"])
        report["native_snapshot"]["blocks"]["piston_0_15_65"]["final"]["name"] = "minecraft:gold_block"
        self.assertFalse(mixes.validate("MIX-04", report, fixture)["passed"])

    def test_final_mutations_require_live_not_shutdown_convergence(self):
        fixture = mixes.recipe("MIX-21", 2, True)
        report = receipt(fixture)
        self.assertTrue(mixes.validate("MIX-21", report, fixture)["passed"])
        report["live_complete"]["mechanics"]["blocks"]["last_block"]["final"]["name"] = "minecraft:air"
        proof = mixes.validate("MIX-21", report, fixture)
        self.assertTrue(any("live recovery" in failure for failure in proof["failures"]))
        report = receipt(fixture)
        report["live_complete"]["mechanics"]["entities"]["minecraft:armor_stand"]["samples"][1]["active"] = True
        self.assertFalse(mixes.validate("MIX-21", report, fixture)["passed"])

    def test_every_reader_must_converge_not_only_the_primary_image(self):
        fixture = mixes.recipe("MIX-21", 2, True)
        for stale in ("block", "position", "removal"):
            report = receipt(fixture)
            reader = report["live_complete"]["mechanics"]["players"][1]
            if stale == "block":
                reader["final_blocks"]["last_block"]["name"] = "minecraft:air"
            elif stale == "position":
                reader["final_observed_entities"][0]["final_position"] = [16.5, 64.0, -8.5]
            else:
                reader["final_observed_entities"].append({"uuid": fixture["expected"]["entities"][1]["uuid"]})
            proof = mixes.validate("MIX-21", report, fixture)
            self.assertFalse(proof["passed"])
            self.assertTrue(any("reader 1" in failure for failure in proof["failures"]))

    def test_explosion_fanout_is_not_distinct_finite_blast_work(self):
        fixture = mixes.recipe("MIX-21", 2, True)
        report = receipt(fixture)
        report["bots"]["mechanics"]["explosions"] = [{"center": [.5, 64.0, .5]}] * 192
        proof = mixes.validate("MIX-21", report, fixture)
        self.assertFalse(proof["passed"])
        self.assertEqual(proof["completed"]["unique_explosions"], 1)

    def test_restart_identity_and_native_readback_are_additional(self):
        fixture = mixes.recipe("MIX-21", 2, True)
        for change in ("identity", "regenerate", "block"):
            report = receipt(fixture)
            if change == "identity":
                report["restart"]["after"]["identity"]["seed"] = 7
            elif change == "regenerate":
                report["restart"]["fixture_regenerated"] = True
            else:
                report["restart"]["after"]["blocks"]["last_block"]["final"]["name"] = "minecraft:air"
            self.assertFalse(mixes.validate("MIX-21", report, fixture)["passed"])

    def test_zero_input_recovery_and_skipped_stimulus_fail(self):
        fixture = mixes.recipe("MIX-04", 2, True)
        report = receipt(fixture)
        report["bots"]["open_loop"]["windows"][0]["command"] = {"offered": 1}
        self.assertFalse(mixes.validate("MIX-04", report, fixture)["passed"])
        report = receipt(fixture)
        report["bots"]["mechanics"]["actions"][0]["admitted"] = 0
        self.assertFalse(mixes.validate("MIX-04", report, fixture)["passed"])

    def test_village_requires_reassignment_and_positive_work(self):
        fixture = mixes.recipe("MIX-01", 2, True)
        report = receipt(fixture)
        jobs = [row["position"] for row in fixture["mechanic_spec"]["blocks"] if row["name"].startswith("job_")]
        before = [{"uuid": identity, "job_site": jobs[n], "brain": {"memories": {}}}
                  for n, identity in enumerate(fixture["expected"]["villager_uuids"])]
        final = deepcopy(before)
        for n in range(16):
            if n % 2 == 0:
                final[n]["job_site"] = [jobs[n][0] + 3, jobs[n][1], jobs[n][2]]
            final[n]["brain"]["memories"]["minecraft:last_worked_at_poi"] = {"value": 100}
        report["initial_native_snapshot"]["villagers"] = before
        report["native_snapshot"]["villagers"] = final
        self.assertTrue(mixes.validate("MIX-01", report, fixture)["passed"])
        natural = {"uuid": "native-generated-villager", "job_site": jobs[0], "brain": {"memories": {}}}
        report["initial_native_snapshot"]["villagers"] = before + [natural]
        report["native_snapshot"]["villagers"] = final + [natural]
        self.assertTrue(mixes.validate("MIX-01", report, fixture)["passed"])
        report["native_snapshot"]["villagers"] = final[1:] + [natural]
        proof = mixes.validate("MIX-01", report, fixture)
        self.assertIn("native villager population lost or duplicated", proof["failures"])
        report["native_snapshot"]["villagers"] = final + [natural]
        report["initial_native_snapshot"]["villagers"] = deepcopy(final)
        proof = mixes.validate("MIX-01", report, fixture)
        self.assertFalse(proof["passed"])
        self.assertTrue(any("re-acquired" in row for row in proof["failures"]))
        self.assertTrue(any("positive" in row for row in proof["failures"]))

    def test_items_count_each_owner_once(self):
        fixture = mixes.recipe("MIX-02", 2, True)
        report = receipt(fixture)
        report["native_snapshot"].update(containers={"free_chest": {"final": {"slots": [
            {"name": "minecraft:dirt", "count": 64}]}},
            "blocked_chest": {"final": [{"name": "minecraft:dirt", "count": 64}]},
            "shared_chest_1": {"final": [{"name": "minecraft:dirt", "count": 64}]}},
            players=[{"payload": {"Inventory": [{"id": "minecraft:dirt", "count": 1664}]}}],
            entity_records=[{"active": True, "payload": {"Item": {"id": "minecraft:dirt", "count": 64}}}])
        self.assertTrue(mixes.validate("MIX-02", report, fixture)["passed"])
        report["native_snapshot"]["entity_records"][0]["payload"]["Item"]["count"] = 65
        self.assertFalse(mixes.validate("MIX-02", report, fixture)["passed"])

    def test_reader_requires_position_progress_and_final_identity_convergence(self):
        fixture = mixes.recipe("MIX-08", 2, True)
        report = receipt(fixture)
        identities = [{"uuid": identity, "entity_id": n + 1, "kind": "minecraft:cow"}
                      for n, identity in enumerate(fixture["expected"]["mob_uuids"])]
        report["native_snapshot"]["entities"]["minecraft:cow"] = {
            "samples": [{"uuid": row["uuid"]} for row in identities]}
        players = [{"reader_progress_by_window": {"burst": {
            "frames": 10, "decoded_bytes": 100, "position_updates": 1}}},
            {"deaths": 1, "respawns": 1, "reader_bytes_per_second": None,
             "final_observed_entities": identities}]
        report["live_complete"]["mechanics"]["players"] = players
        self.assertTrue(mixes.validate("MIX-08", report, fixture)["passed"])
        players[0]["reader_progress_by_window"]["burst"]["position_updates"] = 0
        self.assertFalse(mixes.validate("MIX-08", report, fixture)["passed"])
        players[0]["reader_progress_by_window"]["burst"]["position_updates"] = 1
        players[1]["final_observed_entities"] = identities + [identities[0]]
        self.assertFalse(mixes.validate("MIX-08", report, fixture)["passed"])

    def test_every_actor_has_distinct_fixed_poll_slots(self):
        for identifier in mixes.IDS:
            slots = {}
            fixture = mixes.recipe(identifier, 2, True)
            for action in fixture["mechanic_spec"]["actions"]:
                slots.setdefault((action["bot"], action["window"]), []).append(action["at_ms"])
            for times in slots.values():
                ordered = sorted(times)
                self.assertTrue(all(right - left >= 50 for left, right in zip(ordered, ordered[1:])))

    def test_cold_route_uses_fixed_public_waypoints_not_terrain_walking(self):
        fixture = mixes.recipe("MIX-18", 2, True)
        self.assertEqual(fixture["client_args"], ["--no-edits"])
        self.assertTrue(all(window["movement_hz"] == 0
                            for window in fixture["schedule_overrides"].values()))
        self.assertEqual(fixture["constraints"]["minimum_window_ms"],
                         {"steady": 20000, "burst": 10000})
        self.assertEqual(fixture["initial_mechanic_spec"],
                         {"schema_version": 1, "entities": [], "blocks": [], "containers": [], "actions": []})
        # Preparing a full runway would make the generated-route oracle vacuous.
        self.assertEqual(fixture["commands"], [
            "gamerule doMobSpawning false", "gamerule doDaylightCycle false",
            "fill -16 63 -16 16 63 16 minecraft:stone", "setworldspawn 0 64 0", "time set day"])
        actions = fixture["mechanic_spec"]["actions"]
        self.assertEqual([(row["id"], row["window"], row["at_ms"], row["command"]) for row in actions], [
            ("outbound", "steady", 0, "tp @a 64.5 128 0.5"),
            ("outbound_middle", "steady", 5000, "tp @a 128.5 128 0.5"),
            ("outbound_far", "steady", 10000, "tp @a 192.5 128 0.5"),
            ("return", "burst", 0, "tp @a 128.5 128 0.5"),
            ("return_target", "burst", 4000, "tp @a 64.5 128 0.5"),
            ("edit", "burst", 8000, "setblock 64 64 0 minecraft:diamond_block"),
        ])
        self.assertTrue(all(row["kind"] == "command" and row["bot"] == 0 for row in actions))
        self.assertEqual(fixture["expected"]["route"], [
            {"window": "steady", "center": [4, 0]},
            {"window": "steady", "center": [8, 0]},
            {"window": "steady", "center": [12, 0]},
            {"window": "burst", "center": [8, 0]},
            {"window": "burst", "center": [4, 0]},
        ])
        self.assertGreater(12 - 4, fixture["view_distance"])

    def test_outbound_return_requires_cold_target_and_accepted_restart_edit(self):
        fixture = mixes.recipe("MIX-18", 2, True)
        self.assertEqual(fixture["view_distance"], 2)
        report = receipt(fixture)
        report["initial_native_snapshot"]["persisted_chunks"] = []
        report["bots"]["mechanics"]["players"][0]["chunk_center_trace"] = deepcopy(fixture["expected"]["route"])
        self.assertTrue(mixes.validate("MIX-18", report, fixture)["passed"])
        for center in ([4, 0], [8, 0], [12, 0]):
            with self.subTest(prewarmed=center):
                report["initial_native_snapshot"]["persisted_chunks"] = [{"position": center}]
                self.assertFalse(mixes.validate("MIX-18", report, fixture)["passed"])

    def test_cold_route_requires_exact_ordered_outbound_and_reverse_centers(self):
        fixture = mixes.recipe("MIX-18", 2, True)
        route = fixture["expected"]["route"]
        wrong_axis = deepcopy(route)
        wrong_axis[0]["center"][1] = -2
        traces = {
            "terrain_auto_turn": [
                {"window": "steady", "center": center} for center in ([-1, -2], [0, -2], [1, -2], [1, -1])
            ] + [{"window": "burst", "center": center} for center in ([0, -1], [-1, -1], [-1, -2], [0, -2])],
            "wrong_axis": wrong_axis,
            "missing_outbound_target": route[1:],
            "missing_far_departure": route[:2] + route[3:],
            "outbound_out_of_order": [route[2], route[0], route[1], *route[3:]],
            "missing_return_middle": route[:3] + route[4:],
            "missing_return_target": route[:-1],
            "return_out_of_order": route[:3] + list(reversed(route[3:])),
        }
        for name, trace in traces.items():
            with self.subTest(trace=name):
                report = receipt(fixture)
                report["initial_native_snapshot"]["persisted_chunks"] = []
                report["bots"]["mechanics"]["players"][0]["chunk_center_trace"] = trace
                proof = mixes.validate("MIX-18", report, fixture)
                self.assertFalse(proof["passed"])
                self.assertTrue(any("route never received ordered chunk center" in failure
                                    for failure in proof["failures"]))

    def test_cold_route_does_not_replace_accepted_edit_and_restart_proof(self):
        fixture = mixes.recipe("MIX-18", 2, True)
        for missing in ("initial_index", "centers", "live_edit", "live_update", "native_edit", "restart_edit",
                        "same_world", "unrepaired_restart", "regenerated_fixture"):
            with self.subTest(missing=missing):
                report = receipt(fixture)
                report["initial_native_snapshot"]["persisted_chunks"] = []
                report["bots"]["mechanics"]["players"][0]["chunk_center_trace"] = deepcopy(fixture["expected"]["route"])
                if missing == "initial_index":
                    del report["initial_native_snapshot"]["persisted_chunks"]
                elif missing == "centers":
                    report["bots"]["mechanics"]["players"][0]["chunk_center_trace"] = []
                elif missing == "live_update":
                    report["live_complete"]["mechanics"]["blocks"]["returned_edit"]["update_count"] = 0
                elif missing in ("live_edit", "native_edit", "restart_edit"):
                    image = {"live_edit": report["live_complete"]["mechanics"],
                             "native_edit": report["native_snapshot"],
                             "restart_edit": report["restart"]["after"]}[missing]
                    image["blocks"]["returned_edit"]["final"]["name"] = "minecraft:air"
                elif missing == "same_world":
                    report["restart"]["same_world"] = False
                elif missing == "unrepaired_restart":
                    report["restart"]["repair_commands"] = ["setblock 64 64 0 minecraft:diamond_block"]
                else:
                    report["restart"]["fixture_regenerated"] = True
                self.assertFalse(mixes.validate("MIX-18", report, fixture)["passed"])

    def test_long_uses_reachable_mechanics_and_retains_explicit_duration(self):
        fixture = mixes.recipe("mixed-survival-long", 100, False)
        self.assertEqual(fixture["constraints"]["minimum_measurement_ms"], 600000)
        self.assertTrue(fixture["restart_check"])
        self.assertIn("villagers", fixture["expected"])
        self.assertIn("overlap_0_piston_0_16_64", fixture["expected"]["blocks"])
        self.assertEqual(fixture["preparation_duration_ms"], 3000)

    def test_long_origin_edits_follow_a_settled_resident_tp(self):
        # The compound scene anchors every resident at a village quarter, so the
        # origin rig is resident only if the actor first moves there and the
        # chunks have had time to load; /setblock refuses unloaded positions.
        fixture = mixes.recipe("mixed-survival-long", 100, False)
        view = fixture["view_distance"]
        chunk = lambda value: int(value // 16)
        actions = sorted((row for row in fixture["mechanic_spec"]["actions"]
                          if row["bot"] == 0 and row["window"] == "burst"
                          and row["kind"] == "command"), key=lambda row: row["at_ms"])
        anchor = fixture["anchors"][0]
        here, arrived = (chunk(anchor[0]), chunk(anchor[1])), None
        checked = 0
        for row in actions:
            words = row["command"].split()
            if words[0] == "tp" and words[1] in ("@s", "@a") and len(words) == 5:
                here, arrived = (chunk(float(words[2])), chunk(float(words[4]))), row["at_ms"]
            elif words[0] == "setblock" and chunk(int(words[1])) < 20:
                checked += 1
                target = (chunk(int(words[1])), chunk(int(words[3])))
                self.assertLessEqual(max(abs(target[0] - here[0]), abs(target[1] - here[1])), view,
                                     row["id"])
                self.assertIsNotNone(arrived, row["id"])
                if here != (chunk(anchor[0]), chunk(anchor[1])):
                    self.assertGreaterEqual(row["at_ms"] - arrived, mixes._RESIDENCY_SETTLE_MS, row["id"])
        self.assertGreaterEqual(checked, 17)

    def test_rejected_unloaded_position_command_fails_with_a_named_cause(self):
        fixture = mixes.recipe("MIX-04", 2, True)
        report = receipt(fixture)
        self.assertTrue(mixes.validate("MIX-04", report, fixture)["passed"])
        report["bots"]["mechanics"]["command_replies"] = [
            {"bot": 0, "text": "Changed 1 block(s)", "window": "burst"},
            {"bot": 0, "text": "Position is not loaded", "window": "burst"}]
        proof = mixes.validate("MIX-04", report, fixture)
        self.assertFalse(proof["passed"])
        self.assertIn("origin command rejected: target chunk not resident (1 reply)", proof["failures"])

    def test_water_drops_are_contained_without_weakening_piston_air_pins(self):
        for identifier, smoke, clients in (
            ("MIX-04", True, 2), ("MIX-04", False, 25),
            ("mixed-survival-long", True, 2), ("mixed-survival-long", False, 100),
        ):
            with self.subTest(identifier=identifier, smoke=smoke):
                fixture = mixes.recipe(identifier, clients, smoke)
                levels = (64,) if smoke else (64, 72, 80, 88)
                air_pins = []
                for y in levels:
                    for dx, dz in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                        self.assertIn(
                            f"fill {20 + dx} {y} {4 + dz} {20 + dx} {y + 3} {4 + dz} minecraft:glass",
                            fixture["commands"])
                for name, kind in fixture["expected"]["blocks"].items():
                    if "piston_" in name and "_17_" in name:
                        self.assertEqual(kind, "minecraft:air")
                        air_pins.append(name)
                self.assertEqual(len(air_pins), 4 if smoke else 16)

    def test_full_scale_is_not_the_smoke_fixture(self):
        village = mixes.recipe("MIX-01", 100, False)
        self.assertEqual(village["scale"]["villagers"], 10_000)
        self.assertEqual(village["scale"]["village_quarters"], 25)
        self.assertEqual(len(village["expected"]["villager_uuids"]), 10_000)
        self.assertEqual(village["expected"]["cohort_clients"], 75)
        self.assertEqual(village["constraints"]["minimum_clients"], 100)
        self.assertEqual(sum(row["name"].startswith("bed_")
                             for row in village["mechanic_spec"]["blocks"]), 10_000)
        self.assertEqual(len(village["anchors"]), 25)
        stock = mixes.recipe("MIX-02", 25, False)
        self.assertEqual(len(stock["expected"]["stock_modules"]), 256)
        self.assertEqual(stock["preparation_clients"], 25)
        self.assertEqual(sum(row["input_count"] for row in stock["expected"]["stock_modules"]), 16_384)
        self.assertLess(len(stock["preparation_spec"]["actions"]), 4096)
        pistons = mixes.recipe("MIX-04", 25, False)
        self.assertEqual(pistons["scale"]["piston_machines"], 8)
        readers = mixes.recipe("MIX-08", 100, False)
        self.assertEqual(len(readers["expected"]["mob_uuids"]), 128)
        self.assertEqual(readers["constraints"]["minimum_clients"], 100)
        route = mixes.recipe("MIX-18", 25, False)
        self.assertEqual(len(route["expected"]["route"]), 63)
        self.assertEqual(route["scale"]["route_blocks"], 4096)
        self.assertEqual(route["scale"]["generated_save_buffer_capacity"], 1024)
        self.assertEqual(route["constraints"]["minimum_window_ms"], {"steady": 33000, "burst": 33000})
        burst = mixes.recipe("MIX-21", 100, False)
        self.assertEqual(burst["scale"]["combat_mobs"], 1024)
        self.assertEqual(burst["expected"]["finite_blasts"], 96)
        self.assertEqual(mixes.recipe("MIX-01", 2, True)["expected"]["villagers"], 16)
        self.assertEqual(len(mixes.recipe("MIX-08", 2, True)["expected"]["mob_uuids"]), 2)

    def test_scale_receipt_counts_observed_population_instead_of_copying_demand(self):
        fixture = mixes.recipe("MIX-01", 100, False)
        self.assertIsNone(mixes.scale_receipt(fixture, {})["observed"])
        identities = fixture["expected"]["villager_uuids"]
        native = {"villagers": [{"uuid": identity} for identity in identities[:400]] +
                  [{"uuid": "native-generated-villager"}], "blocks": {
                      "bed_0": {"final": {"name": "minecraft:white_bed"}},
                      "bed_1": {"final": {"name": "minecraft:air"}}}}
        census = mixes.scale_receipt(fixture, native, ready_clients=100)
        self.assertEqual(census["declared"]["villagers"], 10_000)
        self.assertEqual(census["observed"]["fixture_villagers"], 400)
        self.assertEqual(census["observed"]["other_native_villagers"], 1)
        self.assertEqual(census["observed"]["complete_village_quarters"], 1)
        self.assertEqual(census["observed"]["beds"], 1)
        self.assertEqual(census["observed"]["ready_clients"], 100)

    def test_scale_receipt_does_not_invent_missing_harvest_or_route_chunks(self):
        stock = mixes.recipe("MIX-02", 25, False)
        module = stock["expected"]["stock_modules"][0]
        hopper, chest = module["hopper_probe"], module["chest_probe"]
        native = {"blocks": {
            hopper: {"final": {"name": "minecraft:hopper"}},
            chest: {"final": {"name": "minecraft:chest"}}},
            "containers": {
                hopper: {"final": [{"name": "minecraft:nether_wart", "count": 1}]},
                chest: {"final": [{"name": "minecraft:nether_wart", "count": 63}]}}}
        observed = mixes.scale_receipt(stock, native)["observed"]
        self.assertEqual(observed["sorter_modules"], 1)
        self.assertEqual(observed["sorter_modules_with_received_harvest"], 1)
        self.assertEqual(observed["module_harvest_items"], 64)
        self.assertEqual(observed["finite_stock_by_item"], {
            "minecraft:dirt": 0, "minecraft:nether_wart": 64})
        route = mixes.recipe("MIX-18", 25, False)
        observed = mixes.scale_receipt(route, {"persisted_chunks": [
            {"position": [8, 0]}, {"position": [16, 0]}, {"position": [0, 0]}]})["observed"]
        self.assertEqual(observed["persisted_chunks"], 3)
        self.assertEqual(observed["persisted_route_centers"], 2)


if __name__ == "__main__":
    unittest.main()
