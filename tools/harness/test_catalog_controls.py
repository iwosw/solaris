"""Pure recipe/oracle regressions; no server, world injection, or fixture runs."""
from copy import deepcopy
import json
import unittest

from . import catalog_controls as controls


def state(name, **properties):
    return {"state": 1, "name": name, "properties": properties}


def slots(items, size):
    return [{"slot": index, "item_id": index + 1 if count else 0,
             "name": name if count else "minecraft:air", "count": count}
            for index, (name, count) in enumerate(items + [("minecraft:air", 0)] * (size - len(items)))]


def base_menus():
    containers = {}
    for n in range(64):
        initial = slots([("minecraft:raw_iron", 64), ("minecraft:coal", 64)], 3)
        final = slots([("minecraft:raw_iron", 63), ("minecraft:coal", 63), ("minecraft:iron_ingot", 1)], 3)
        containers[f"furnace_{n}"] = {"initial": initial, "final": final,
            "initial_player_inventory": slots([("minecraft:raw_iron", 64), ("minecraft:coal", 64)], 36)}
        containers[f"hopper_{n}_a"] = {
            "initial": slots([("minecraft:dirt", 64), ("minecraft:dirt", 1)], 5),
            "final": slots([("minecraft:dirt", 63), ("minecraft:dirt", 1)], 5)}
        containers[f"hopper_{n}_b"] = {
            "initial": slots([("minecraft:dirt", 63)], 5),
            "final": slots([("minecraft:dirt", 64)], 5)}
    return containers


def action_receipts(spec):
    return [{"action": index, "kind": action["kind"], "bot": action["bot"],
             "window": action.get("window", "steady"), "offered": 1, "admitted": 1,
             "writes_completed": 1, "completed": 1, "errors": [],
             "dropped_late": 0, "dropped_unavailable": 0, "dropped_capacity": 0}
            for index, action in enumerate(spec["actions"])]




def exploration_report(prepared):
    return {"bots": {"ready_clients": prepared["clients"], "disconnects": 0,
        "chunks_received": 32, "chunk_boundaries_crossed": 32, "walking_distance_blocks": 256,
        "open_loop": {"schedule_complete": True, "elapsed_ms": 50000, "windows": [
            {"name": "steady", "duration_ms": 20000, "received": {"chunks_received": 16}},
            {"name": "burst", "duration_ms": 10000, "received": {"chunks_received": 16}},
            {"name": "recovery", "duration_ms": 20000, "recovery": True, "received": {"chunks_received": 0}}]},
        "mechanics": {"schema_version": 1, "baseline_captured": True, "identity_errors": 0, "position_errors": 0,
            "duplicate_spawns": 0, "ghost_updates": 0, "capacity_errors": [], "actions": [],
            "players": [{"initial_chunk_center": [0, 0], "final_chunk_center": [16, 0],
                "chunk_center_packets": 16, "chunk_center_transitions": 16, "chunk_center_traces_truncated": 0,
                "chunk_center_trace": [{"window": "steady" if n < 8 else "burst",
                                       "previous": [n, 0], "center": [n + 1, 0]} for n in range(16)]}
                for _ in range(prepared["clients"])],
            "entities": {}, "blocks": {"marker_0": {"initial": state("minecraft:bedrock"),
                "final": state("minecraft:bedrock"), "update_count": 0, "fresh_reads": 1}},
            "containers": {}, "windows": {"steady": {}, "burst": {}, "recovery": {}}}}}


def village_report(prepared):
    report = exploration_report(prepared)
    expected = prepared["expected"]
    mechanics = report["bots"]["mechanics"]
    mechanics["blocks"] = {name: {
        "initial": state(kind, **expected.get("stable_properties", {}).get(name, {})),
        "final": state(kind, **expected.get("stable_properties", {}).get(name, {})),
        "update_count": 0, "fresh_reads": 1}
        for name, kind in expected["stable_blocks"].items()}
    identities = expected["entities"]["minecraft:villager"]
    samples = [{"uuid": identity, "initial_position": [0, 64, 0], "final_position": [1, 64, 0],
                "payload": {"id": "minecraft:villager", "CustomName": controls._VILLAGER_CUSTOM_NAME}}
               for identity in identities]
    mechanics["entities"] = {"minecraft:villager": {
        "observed": len(samples), "active": len(samples), "moved": len(samples),
        "removed": 0, "samples": deepcopy(samples)}}
    for phase in ("steady", "burst"):
        mechanics["windows"][phase]["entity_moves"] = len(samples)
    report["native_snapshot"] = {
        "entities": {"minecraft:villager": {"samples": samples}},
        "villagers": [{"uuid": identity, "profession": "minecraft:farmer", "brain_valid": True,
                       "home": expected["poi_positions"]["home"][2 * index],
                       "job_site": expected["poi_positions"]["job_site"][index]}
                      for index, identity in enumerate(identities[:64])]}
    return report


class PublicControlRecipeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.recipes = {name: controls.recipe(name, 16, True) for name in controls.CONTROL_IDS}

    def test_all_twelve_owned_and_unknown_not_owned(self):
        self.assertEqual(len(self.recipes), 12)
        self.assertIsNone(controls.recipe("MIX-01", 16, True))
        for name, prepared in self.recipes.items():
            with self.subTest(name=name):
                self.assertEqual(prepared["id"], name)
                self.assertEqual(prepared["mechanic_spec"]["schema_version"], 1)
                self.assertNotIn("capability", prepared)
                json.dumps(prepared, allow_nan=False)

    def test_client_values_are_not_silently_coerced(self):
        for clients in (0, -1, True, 2.0, "10"):
            with self.subTest(clients=clients), self.assertRaises(ValueError):
                controls.recipe("dense-hostile", clients, True)
        with self.assertRaises(ValueError):
            controls.recipe("explore", 1, "smoke")

    def test_density_and_finite_stock_preserved(self):
        hostile = self.recipes["dense-hostile"]
        self.assertEqual(sum(row["count"] for row in hostile["mechanic_spec"]["entities"]), 1024)
        passive = self.recipes["spread-passive"]
        self.assertEqual(passive["anchors"], [[0, 0], [256, 0], [0, 256], [256, 256]])
        self.assertEqual(passive["group_size"], 4)
        self.assertEqual([row["count"] for row in passive["mechanic_spec"]["entities"]], [256] * 4)
        base = self.recipes["built-base"]
        self.assertEqual(base["expected"]["stocks"]["furnaces"], 64)
        self.assertEqual(base["expected"]["stocks"]["hoppers"], 128)
        self.assertEqual(base["expected"]["stocks"]["raw_iron"], 4096)
        self.assertEqual(len(base["mechanic_spec"]["containers"]), 192)
        self.assertEqual(sum(a["kind"] == "drop_stack" for a in base["preparation_spec"]["actions"]), 64)
        self.assertEqual(sum(p["name"].startswith("button_") for p in base["mechanic_spec"]["blocks"]), 512)
        self.assertEqual(len(self.recipes["farms"]["expected"]["crops"]), 11440)
        self.assertEqual(len(self.recipes["fluids"]["expected"]["fluid_sources"]), 128)
        self.assertEqual(len(self.recipes["village"]["expected"]["entities"]["minecraft:villager"]), 64)
        self.assertEqual(len(self.recipes["combat"]["expected"]["entities"]["minecraft:tnt"]), 96)
        self.assertEqual(len(self.recipes["projectiles"]["expected"]["entities"]["minecraft:arrow"]), 128)
        self.assertEqual(len(self.recipes["projectiles"]["expected"]["entities"]["minecraft:skeleton"]), 128)
        self.assertEqual(sum(len(ids) for ids in self.recipes["long-session"]["expected"]["entities"].values()), 256)

    def test_base_transport_lead_preserves_fixed_work_rate_and_reach(self):
        for name in ("built-base", "long-session", "checkpoint-restart"):
            prepared = controls.recipe(name, 10, True)
            for phase, interval in (("steady", 125), ("burst", 63)):
                actions = [a for a in prepared["mechanic_spec"]["actions"]
                           if a["window"] == phase and a["kind"] in
                           ("command", "open_container", "close_container", "use_block")]
                self.assertEqual(sum(a["kind"] == "open_container" for a in actions), 192)
                self.assertEqual(sum(a["kind"] == "use_block" for a in actions), 256)
                self.assertEqual(sum(a["kind"] == "close_container" for a in actions), 192)
                teleports = [a for a in actions if a["kind"] == "command"
                             and a["command"].startswith("tp @s ")]
                self.assertEqual(len(teleports), 128)
                for bot in range(10):
                    owned = [a for a in actions if a["bot"] == bot]
                    for start in range(0, len(owned), 12):
                        group = owned[start:start + 12]
                        self.assertEqual([a["kind"] for a in group],
                            ["command"] + ["open_container", "close_container"] * 3
                            + ["command"] + ["use_block"] * 4)
                        self.assertEqual(group[1]["at_ms"] - group[0]["at_ms"], 250)
                        for opened, closed in zip(group[1:7:2], group[2:7:2]):
                            self.assertEqual(closed["at_ms"] - opened["at_ms"], 200)
                        for closed, next_open in zip(group[2:6:2], group[3:7:2]):
                            self.assertEqual(next_open["at_ms"] - closed["at_ms"], 63)
                        self.assertEqual(group[7]["at_ms"] - group[6]["at_ms"], 63)
                        self.assertEqual(group[8]["at_ms"] - group[7]["at_ms"], 250)
                        self.assertEqual([a["at_ms"] - group[8]["at_ms"] for a in group[8:]],
                                         [index * interval for index in range(4)])
                        for transport, uses in ((group[0], group[1:7:2]), (group[7], group[8:])):
                            pose = list(map(float, transport["command"].split()[2:]))
                            for action in uses:
                                distance_sq = sum((p + 0.5 - origin) ** 2
                                                  for p, origin in zip(action["position"], pose))
                                self.assertLess(distance_sq, 9)
                self.assertEqual(prepared["constraints"]["minimum_window_ms"][phase],
                                 7 * (500 + 3 * 263 + 4 * interval))

    def test_preparation_closes_every_menu_before_another_open(self):
        for name in ("built-base", "long-session", "checkpoint-restart"):
            opened = False
            for action in controls.recipe(name, 10, True)["preparation_spec"]["actions"]:
                if action["kind"] == "open_container":
                    self.assertFalse(opened, (name, action))
                    opened = True
                elif action["kind"] == "close_container":
                    self.assertTrue(opened, (name, action))
                    opened = False
            self.assertFalse(opened)

    def test_smoke_changes_no_population_or_stock(self):
        for name, smoke in self.recipes.items():
            normal = controls.recipe(name, 16, False)
            with self.subTest(name=name):
                self.assertEqual(normal["commands"], smoke["commands"])
                self.assertEqual(normal["expected"].get("stocks"), smoke["expected"].get("stocks"))
                self.assertEqual(normal["mechanic_spec"], smoke["mechanic_spec"])

    def test_commands_are_public_bounded_and_no_private_nbt_transport(self):
        allowed = {"fill", "setblock", "summon", "gamerule", "weather", "time", "difficulty",
                   "setworldspawn", "clear", "tp", "give", "town", "money", "pay", "econadmin", "save-all"}
        for name, prepared in self.recipes.items():
            commands = list(prepared["commands"])
            for region in prepared.get("post_preparation_regions", []):
                commands.extend(region["commands"])
            for spec in (prepared["preparation_spec"], prepared["mechanic_spec"]):
                commands.extend(action["command"] for action in spec["actions"] if action["kind"] == "command")
            with self.subTest(name=name):
                for command in commands:
                    self.assertFalse(command.startswith("/"))
                    words = command.split()
                    self.assertIn(words[0], allowed)
                    self.assertNotEqual(command, "save-all flush")
                    if words[0] == "fill":
                        coords = list(map(int, words[1:7]))
                        volume = 1
                        for index in range(3):
                            volume *= abs(coords[index + 3] - coords[index]) + 1
                        self.assertLessEqual(volume, 32768)
                    if words[0] == "setblock":
                        self.assertNotIn("{", command)
                    if words[0] == "summon":
                        self.assertNotIn("NoAI", command)
                        self.assertNotIn("Invulnerable", command)
                        self.assertNotIn("Fuse:", command)
                        self.assertNotIn("Item:", command)
                    if words[0] in ("clear", "give"):
                        self.assertEqual(words[1], "@s")
                self.assertTrue(all(region["view_distance"] == 8 for region in prepared["preparation_regions"]))

    def test_uuids_unique_and_recipe_mutations_do_not_alias(self):
        for prepared in self.recipes.values():
            identities = [identity for values in prepared["expected"]["entities"].values() for identity in values]
            self.assertEqual(len(identities), len(set(identities)))
        one = controls.recipe("dense-hostile", 1, True)
        two = controls.recipe("dense-hostile", 1, True)
        one["commands"].clear()
        self.assertTrue(two["commands"])

    def test_only_authored_villagers_receive_persisted_cohort_name(self):
        for prepared in self.recipes.values():
            for command in prepared["commands"]:
                if not command.startswith("summon "):
                    continue
                with self.subTest(recipe=prepared["id"], command=command):
                    if command.startswith("summon minecraft:villager "):
                        self.assertIn(",CustomName:" + json.dumps(controls._VILLAGER_CUSTOM_NAME), command)
                    else:
                        self.assertNotIn("CustomName", command)

    def test_recovery_never_repairs_or_refills(self):
        for name in controls.RECIPE_IDS:
            prepared = controls.recipe(name, 16, True)
            with self.subTest(name=name):
                self.assertFalse(any(prepared["schedule_overrides"]["recovery"].values()))
                self.assertFalse(any(action.get("window") == "recovery" for action in prepared["mechanic_spec"]["actions"]))
                self.assertFalse(any(action.get("window") == "recovery"
                    and action["kind"] in ("click_slot", "drop_stack")
                    for action in prepared["mechanic_spec"]["actions"]))

    def test_physical_probe_addresses_and_names_are_unique(self):
        for name, prepared in self.recipes.items():
            probes = prepared["mechanic_spec"]["blocks"]
            with self.subTest(name=name):
                self.assertEqual(len(probes), len({tuple(probe["position"]) for probe in probes}))
                self.assertEqual(len(probes), len({probe["name"] for probe in probes}))

    def test_plugin_uses_two_town_economy_actors_and_real_chest_custody(self):
        prepared = self.recipes["plugin-heavy"]
        self.assertEqual(prepared["required_plugins"], ["solaris-towns", "solaris-economy"])
        self.assertEqual(prepared["constraints"]["minimum_clients"], 2)
        setup = prepared["preparation_spec"]["actions"]
        self.assertEqual([action["command"] for action in setup[:2]],
                         ["town create load_matrix", "town claim"])
        self.assertEqual(sum(action["command"].startswith("econadmin set ") for action in setup), 2)
        self.assertTrue(all(action.get("expected_feedback") for action in setup))
        self.assertGreater(prepared["preparation_duration_ms"], setup[-1]["at_ms"])
        actions = prepared["mechanic_spec"]["actions"]
        self.assertEqual({action["bot"] for action in actions}, {0, 1})
        for bot in range(2):
            self.assertTrue(any(a.get("command") == f"town create load_actor{bot}"
                                and a["bot"] == bot for a in actions))
            clear = next(a for a in actions if a.get("command") == "clear @s" and a["bot"] == bot)
            grant = next(a for a in actions if a.get("command") == "give @s minecraft:diamond 1"
                         and a["bot"] == bot)
            self.assertLess(clear["at_ms"], grant["at_ms"])
            for phase in ("steady", "burst"):
                own = [a for a in actions if a["bot"] == bot and a["window"] == phase]
                self.assertEqual(sum(a["kind"] == "click_slot" for a in own), 2)
                self.assertEqual([a["slot"] for a in own if a["kind"] == "click_slot"],
                                 [54 if phase == "steady" else 62, 0])
                self.assertEqual(sum(a.get("command") == "money" for a in own), 4)
                self.assertTrue(any(a.get("expected_feedback") ==
                                   f"load_actor{bot}: 1 members, 1 claims." for a in own))
        payments = [a for a in actions if a.get("command", "").startswith("pay ")]
        self.assertEqual(payments[0]["command"], payments[1]["command"])
        self.assertEqual(payments[1]["expected_feedback"], "That transfer token was already committed.")

    def test_retired_settlements_recipe_is_not_owned(self):
        self.assertIsNone(controls.recipe("plugin-settlements", 3, True))
        self.assertNotIn("plugin-settlements", controls.RECIPE_IDS)


    def test_checkpoint_plants_once_after_finite_stock_preparation(self):
        prepared = self.recipes["checkpoint-restart"]
        late = prepared["post_preparation_regions"]
        self.assertEqual(len(late), 1)
        crops = [command for command in late[0]["commands"] if "[age=0]" in command]
        self.assertEqual(len(crops), 11440)
        self.assertFalse(any("[age=0]" in command for command in prepared["commands"]))
        self.assertFalse(any("[age=0]" in command
                             for region in prepared["preparation_regions"] for command in region["commands"]))
        self.assertGreaterEqual(prepared["preparation_duration_ms"], 640000)


    def test_finite_sources_tnt_arrows_are_measurement_timed_not_consumed_in_setup(self):
        fluids = self.recipes["fluids"]
        releases = fluids["mechanic_spec"]["actions"]
        self.assertEqual(len(releases), 128)
        self.assertEqual(sum(a["window"] == "steady" for a in releases), 64)
        self.assertEqual(sum(a["window"] == "burst" for a in releases), 64)
        combat = self.recipes["combat"]
        tnt = [a for a in combat["mechanic_spec"]["actions"] if a["command"].startswith("summon minecraft:tnt ")]
        self.assertEqual(len(tnt), 96)
        self.assertFalse(any(command.startswith("summon minecraft:tnt ") for command in combat["commands"]))
        arrows = [a for a in self.recipes["projectiles"]["mechanic_spec"]["actions"] if a["kind"] == "command"]
        self.assertEqual(len(arrows), 128)
        self.assertTrue(all("Motion:[0.0d,0.0d,-1.5d]" in a["command"] for a in arrows))

    def test_legacy_slow_command_rates_use_integer_wall_intervals(self):
        self.assertEqual(self.recipes["combat"]["schedule_overrides"]["steady"]["command_interval_ms"], 6000)
        self.assertEqual(self.recipes["checkpoint-restart"]["schedule_overrides"]["burst"]["command_interval_ms"], 10000)
        for name in ("combat", "checkpoint-restart"):
            self.assertEqual(self.recipes[name]["schedule_overrides"]["steady"]["command_hz"], 0)


class ReceivedControlOracleTests(unittest.TestCase):
    def test_missing_state_is_infrastructure_not_pass_or_unsupported(self):
        for name in controls.CONTROL_IDS:
            with self.subTest(name=name):
                prepared = controls.recipe(name, 16, True)
                proof = controls.validate(name, {}, prepared)
                self.assertFalse(proof["passed"])
                self.assertTrue(proof["infra_errors"])
                self.assertNotIn("UNSUPPORTED_MECHANIC", json.dumps(proof))

    def test_native_villager_census_ignores_only_unrelated_natural_population(self):
        prepared = controls.recipe("village", 16, True)
        report = village_report(prepared)
        samples = report["native_snapshot"]["entities"]["minecraft:villager"]["samples"]
        samples.extend([
            {"uuid": "00000000-0000-0000-0000-000000000002",
             "initial_position": [400, 70, 400], "final_position": [401, 70, 400],
             "payload": {"id": "minecraft:villager"}},
            {"uuid": "00000000-0000-0000-0000-000000000003",
             "initial_position": [400, 70, 401], "final_position": [401, 70, 401],
             "payload": {"id": "minecraft:villager", "CustomName": '{"text":"native resident"}'}}])
        proof = controls.validate("village", report, prepared)
        self.assertTrue(proof["passed"], proof)
        for change in ("missing", "missing_marker", "extra_named", "substituted"):
            receipt = deepcopy(report)
            cohort = receipt["native_snapshot"]["entities"]["minecraft:villager"]["samples"]
            if change == "missing":
                cohort.pop(0)
            elif change == "missing_marker":
                cohort[0]["payload"].pop("CustomName")
            elif change == "extra_named":
                cohort[-1]["payload"]["CustomName"] = controls._VILLAGER_CUSTOM_NAME
            else:
                cohort[0]["uuid"] = "00000000-0000-0000-0000-000000000004"
            proof = controls.validate("village", receipt, prepared)
            with self.subTest(change=change):
                self.assertFalse(proof["passed"])
                self.assertIn("minecraft:villager: authoritative population lost or duplicated",
                              proof["failures"])


    def test_exploration_positive_and_writes_only_negative(self):
        prepared = controls.recipe("explore", 1, True)
        report = exploration_report(prepared)
        self.assertTrue(controls.validate("explore", report, prepared)["passed"])
        report["bots"].update(movements_sent=100000, chunk_boundaries_crossed=100000, walking_distance_blocks=100000)
        player = report["bots"]["mechanics"]["players"][0]
        player.update(final_chunk_center=[0, 0], chunk_center_packets=0, chunk_center_transitions=0, chunk_center_trace=[])
        proof = controls.validate("explore", report, prepared)
        self.assertFalse(proof["passed"])
        self.assertIn("explore: no actual work floor for received_displacement_lower_bound", proof["failures"])

    def test_exploration_requires_complete_received_center_chain(self):
        prepared = controls.recipe("explore", 1, True)
        for change in ("missing_baseline", "wrong_previous", "wrong_phase", "truncated", "counter_only"):
            report = exploration_report(prepared)
            player = report["bots"]["mechanics"]["players"][0]
            if change == "missing_baseline":
                player["initial_chunk_center"] = None
            elif change == "wrong_previous":
                player["chunk_center_trace"][1]["previous"] = [100, 0]
            elif change == "wrong_phase":
                player["chunk_center_trace"][0]["window"] = "warm"
            elif change == "truncated":
                player["chunk_center_traces_truncated"] = 1
            else:
                player["chunk_center_trace"] = []
            self.assertFalse(controls.validate("explore", report, prepared)["passed"], change)

    def test_run_baseline_and_measured_windows_cannot_be_lifetime_fallbacks(self):
        prepared = controls.recipe("explore", 1, True)
        report = exploration_report(prepared)
        report["bots"]["mechanics"].pop("baseline_captured")
        self.assertIn("received mechanic Run baseline missing",
                      controls.validate("explore", report, prepared)["infra_errors"])
        for identifier, metric, lifetime, failure in (
                ("dense-hostile", "health_damage", "health_damage_received",
                 "survival combat: no received health damage"),
                ("combat", "explosions", "explosions_received", "combat: no received explosion"),
                ("projectiles", "projectile_spawns", "projectiles_spawned",
                 "projectiles: startup 128 arrows do not prove continued skeleton launches")):
            fixture = controls.recipe(identifier, 10, True)
            receipt = exploration_report(fixture)
            receipt["bots"][lifetime] = 1000000
            receipt["bots"]["mechanics"]["windows"] = {
                phase: {metric: 0, "health_damage": 0}
                for phase in ("steady", "burst", "recovery")}
            self.assertIn(failure, controls.validate(identifier, receipt, fixture)["failures"])
            receipt["bots"]["mechanics"]["windows"] = None
            proof = controls.validate(identifier, receipt, fixture)
            self.assertIn("measured mechanic receive windows missing", proof["infra_errors"])

    def test_ready_client_denominator_and_recovery_actions_fail_closed(self):
        prepared = controls.recipe("explore", 2, True)
        report = exploration_report(prepared)
        report["bots"]["ready_clients"] = 1
        report["bots"]["open_loop"]["windows"][-1]["movement"] = {"offered": 1}
        proof = controls.validate("explore", report, prepared)
        self.assertIn("not every requested client was ready", proof["failures"])
        self.assertIn("recovery offered/admitted new gameplay stimulus", proof["failures"])

    def test_entity_identity_and_nonfinite_position_cannot_pass(self):
        oracle = controls._Oracle()
        identity = "e06a523c-cc37-48f4-b673-9855e2266865"
        sample = {"uuid": identity, "initial_position": [0, 64, 0], "final_position": [1, 64, 0]}
        controls._entity_samples(oracle, {"samples": [sample, sample]}, "minecraft:cow")
        self.assertIn("minecraft:cow: duplicate entity identity", oracle.failures)
        oracle = controls._Oracle()
        sample = {**sample, "final_position": [float("nan"), 64, 0]}
        controls._entity_samples(oracle, {"samples": [sample]}, "minecraft:cow")
        self.assertTrue(oracle.infra_errors)

    def test_static_furnaces_and_hoppers_are_not_work(self):
        oracle = controls._Oracle()
        containers = base_menus()
        for row in containers.values():
            row["initial"] = deepcopy(row["final"])
        controls._base_oracle(oracle, {"containers": containers})
        self.assertIn("base: no received new cooked output", oracle.failures)
        self.assertIn("base: no received circulating hopper transfer", oracle.failures)

    def test_real_cooking_and_hopper_receipts_pass_stock_oracle(self):
        oracle = controls._Oracle()
        controls._base_oracle(oracle, {"containers": base_menus()})
        self.assertTrue(oracle.result()["passed"])
        self.assertEqual(oracle.completed["cooked_output_delta"], 64)
        self.assertEqual(oracle.completed["changed_hopper_menus"], 128)

    def test_net_zero_hopper_flow_requires_actual_container_slot_changes(self):
        for container_updates, generic_updates, passed in ((2, 2, True), (0, 99, False)):
            containers = base_menus()
            for name, row in containers.items():
                if name.startswith("hopper_"):
                    row["initial"] = deepcopy(row["final"])
            containers["hopper_0_a"].update(container_slot_updates=container_updates,
                                             updates=generic_updates, versions=[0, 1, 2])
            with self.subTest(container_updates=container_updates):
                oracle = controls._Oracle()
                controls._base_oracle(oracle, {"containers": containers})
                self.assertEqual(oracle.result()["passed"], passed)

    def test_finite_smelting_stock_wrong_identity_and_extra_output_fail(self):
        for change in ("count", "name"):
            containers = base_menus()
            containers["furnace_0"]["final"][2][change] = 2 if change == "count" else "minecraft:gold_ingot"
            oracle = controls._Oracle()
            controls._base_oracle(oracle, {"containers": containers})
            self.assertFalse(oracle.result()["passed"])

    def test_atomic_durable_stock_rejects_one_item_duplication(self):
        containers = base_menus()
        containers["hopper_0_a"]["final"][1]["count"] = 2
        oracle = controls._Oracle()
        controls._durable_stock_oracle(oracle, {"containers": containers})
        self.assertIn("durable base: exact seeded+dropped dirt conservation failed", oracle.failures)

    def test_packet_write_never_substitutes_menu_click_completion(self):
        spec = {"actions": [{"bot": 0, "kind": "click_slot", "slot": 30, "mode": 1}]}
        receipts = action_receipts(spec)
        receipts[0]["completed"] = 0
        oracle = controls._Oracle()
        controls._action_oracle(oracle, {"actions": receipts}, spec)
        self.assertIn("finite menu/item/block action lacks received completion", oracle.failures)

    def test_dropped_or_unoffered_finite_stimulus_cannot_hide(self):
        spec = {"actions": [{"bot": 0, "kind": "command", "command": "setblock 0 69 0 air"}]}
        for change in ({"offered": 0}, {"dropped_late": 1}, {"errors": ["out of reach"]}):
            receipts = action_receipts(spec)
            receipts[0].update(change)
            oracle = controls._Oracle()
            controls._action_oracle(oracle, {"actions": receipts}, spec)
            self.assertFalse(oracle.result()["passed"])

    def test_deferred_menu_close_still_requires_one_actual_write(self):
        spec = {"actions": [{"bot": 0, "kind": "close_container"}]}
        for written in (0, 1):
            receipts = action_receipts(spec)
            receipts[0].update(awaited_menu_content=True, writes_completed=written, completed=0)
            oracle = controls._Oracle()
            controls._action_oracle(oracle, {"actions": receipts}, spec)
            self.assertEqual(oracle.result()["passed"], written == 1)

    def test_fresh_cells_cannot_be_replaced_by_heightmap_or_unknown_identity(self):
        oracle = controls._Oracle()
        self.assertIsNone(controls._state(oracle, {"final": {"heightmap": 64}}, "final", "crop"))
        self.assertTrue(oracle.infra_errors)
        oracle = controls._Oracle()
        ledger = slots([("minecraft:dirt", 1)], 5)
        ledger[0]["name"] = None
        self.assertIsNone(controls._slots(oracle, {"final": ledger}, "final", "hopper", 5))
        self.assertTrue(oracle.infra_errors)

    def test_slot_duplicate_and_overstock_fail_closed(self):
        for ledger in (slots([("minecraft:dirt", 65)], 5), slots([("minecraft:dirt", 1)], 5) + [slots([], 1)[0]]):
            oracle = controls._Oracle()
            controls._slots(oracle, {"final": ledger}, "final", "hopper", 5)
            self.assertTrue(oracle.failures)

    def test_restart_requires_real_checkpoint_and_snapshots(self):
        oracle = controls._Oracle()
        controls._restart_oracle(oracle, {"before": {}, "after": {}},
                                 {"families": [], "entities": {}, "stable_blocks": {}})
        self.assertTrue(oracle.infra_errors)
        self.assertIn("restart: actual Saved-game completion receipt missing", oracle.infra_errors)

    def test_restart_final_only_native_stock_keeps_conservation_gates(self):
        containers = {name: {"final": deepcopy(row["final"])}
                      for name, row in base_menus().items()}
        restart = {"checkpoint": {"checkpoint_event": {"event": "fixture_checkpoint_saved"}},
                   "same_world": True, "fixture_regenerated": False,
                   "world_before": {"sha256": "before"}, "world_after": {"sha256": "after"},
                   "before": {"containers": deepcopy(containers)},
                   "after": {"containers": containers}}
        expected = {"families": ["base"], "entities": {}, "stable_blocks": {}}
        oracle = controls._Oracle()
        oracle.completed.update(cooked_output_delta=7, changed_hopper_menus=3)
        controls._restart_oracle(oracle, restart, expected)
        self.assertTrue(oracle.result()["passed"])
        self.assertEqual(oracle.completed["cooked_output_delta"], 7)
        self.assertEqual(oracle.completed["changed_hopper_menus"], 3)
        for change in ("missing_final", "duplicate_stock", "wrong_output"):
            mutated = deepcopy(restart)
            after = mutated["after"]["containers"]
            if change == "missing_final":
                after["furnace_0"].pop("final")
            elif change == "duplicate_stock":
                after["hopper_0_a"]["final"][1]["count"] += 1
            else:
                after["furnace_0"]["final"][2]["name"] = "minecraft:gold_ingot"
            with self.subTest(change=change):
                oracle = controls._Oracle()
                controls._restart_oracle(oracle, mutated, expected)
                self.assertFalse(oracle.result()["passed"])
                if change == "missing_final":
                    self.assertIn("furnace_0: missing final slot ledger", oracle.infra_errors)
                elif change == "duplicate_stock":
                    self.assertIn("durable base: exact seeded+dropped dirt conservation failed",
                                  oracle.failures)
                else:
                    self.assertIn("base: finite raw-iron/ingot conservation failed", oracle.failures)

    def test_plugin_is_real_prerequisite_not_missing_villager_fallback(self):
        prepared = controls.recipe("plugin-heavy", 16, True)
        self.assertEqual(prepared["required_plugins"], ["solaris-towns", "solaris-economy"])
        report = exploration_report(prepared)
        report["bots"]["commands_sent"] = 100000
        report["bots"]["command_feedback_received"] = 100000
        proof = controls.validate("plugin-heavy", report, prepared)
        self.assertFalse(proof["passed"])
        self.assertIn("plugin: deployed WASM/host and durable town/claim probes missing", proof["infra_errors"])

    def test_plugin_admitted_chat_is_not_committed_native_work(self):
        prepared = controls.recipe("plugin-heavy", 16, True)
        report = exploration_report(prepared)
        report["plugin"] = {"runtime_ready": True,
            "town_name": "load_matrix", "durable": True, "successful_replies": 100,
            "host_operations": 1000, "wasm_invocations": 1000,
            "journal": {owner: {"admitted": 100, "journaled_messages": 100, "committed": 0}
                        for owner in prepared["required_plugins"]}}
        proof = controls.validate("plugin-heavy", report, prepared)
        self.assertIn("plugin: no actual committed native/storage operation", proof["failures"])
        self.assertIn("plugin: per-actor state-query evidence missing", proof["infra_errors"])

    def test_plugin_inventory_writes_cannot_replace_received_custody(self):
        prepared = controls.recipe("plugin-heavy", 2, True)
        report = exploration_report(prepared)
        report["bots"]["mechanics"]["actions"] = action_receipts(prepared["mechanic_spec"])
        proof = controls.validate("plugin-heavy", report, prepared)
        self.assertTrue(any("plugin_chest_0" in error for error in proof["infra_errors"]))

    def test_plugin_chest_conservation_uses_menu_player_slot_offsets(self):
        prepared = controls.recipe("plugin-heavy", 2, True)
        report = exploration_report(prepared)
        mechanics = report["bots"]["mechanics"]
        mechanics["actions"] = action_receipts(prepared["mechanic_spec"])
        inventory = [{**item, "slot": item["slot"] + 27}
                     for item in slots([("minecraft:diamond", 1)], 36)]
        for bot in range(2):
            mechanics["containers"][f"plugin_chest_{bot}"] = {
                "initial": slots([], 27), "final": slots([], 27),
                "initial_player_inventory": deepcopy(inventory),
                "final_player_inventory": deepcopy(inventory),
                "container_slot_updates": 4, "carried": {"count": 0}}
        proof = controls.validate("plugin-heavy", report, prepared)
        self.assertFalse(any("plugin gameplay:" in error for error in proof["failures"]))
        self.assertFalse(any("plugin_chest_" in error for error in proof["infra_errors"]))
        mechanics["containers"]["plugin_chest_1"]["final_player_inventory"][0]["count"] = 2
        proof = controls.validate("plugin-heavy", report, prepared)
        self.assertIn("plugin gameplay: one granted diamond was not conserved", proof["failures"])

    def test_crop_age_type_and_substrate_are_physical_not_heightmap(self):
        prepared = controls.recipe("farms", 16, True)
        name, kind = next(iter(prepared["expected"]["crops"].items()))
        report = exploration_report(prepared)
        cells = report["bots"]["mechanics"]["blocks"]
        cells[name] = {"initial": state(kind, age="0"), "final": state(kind, age="99"), "fresh_reads": 1}
        cells[name.replace("crop_", "soil_", 1)] = {
            "initial": state("minecraft:farmland", moisture="7"),
            "final": state("minecraft:dirt"), "fresh_reads": 1}
        proof = controls.validate("farms", report, prepared)
        self.assertIn("farm: illegal crop type/age", proof["failures"])
        self.assertIn("farm: crop substrate lost", proof["failures"])

    def test_fluid_source_cannot_masquerade_as_non_source_propagation(self):
        prepared = controls.recipe("fluids", 16, True)
        report = exploration_report(prepared)
        cells = report["bots"]["mechanics"]["blocks"]
        cells["source_0"] = {"initial": state("minecraft:water", level="0"),
                             "final": state("minecraft:water", level="0"), "fresh_reads": 1}
        cells["flow_0_69"] = {"initial": state("minecraft:glass"),
                             "final": state("minecraft:water", level="0"),
                             "update_count": 1, "fresh_reads": 1}
        proof = controls.validate("fluids", report, prepared)
        self.assertIn("fluids: natural falling-column topology mismatch", proof["failures"])
        self.assertIn("fluids: no received non-source fluid propagation", proof["failures"])

    def test_granted_fuel_stock_requires_actual_player_slot_ledger(self):
        spec = {"actions": []}
        containers = base_menus()
        containers["furnace_0"]["initial_player_inventory"][1]["count"] = 1
        oracle = controls._Oracle()
        controls._action_oracle(oracle, {"actions": [], "containers": containers}, spec, preparation=True)
        self.assertIn("preparation: each furnace must receive exactly64 raw iron and64 coal", oracle.failures)

    def test_duplicate_action_identity_and_missing_physical_cell_fail_closed(self):
        spec = {"actions": [{"bot": 0, "kind": "use_block", "position": [0, 64, 0]}]}
        receipts = action_receipts(spec)
        oracle = controls._Oracle()
        controls._action_oracle(oracle, {"actions": receipts + receipts}, spec)
        self.assertIn("duplicate finite action identity", oracle.failures)
        oracle = controls._Oracle()
        controls._fresh_probe_oracle(oracle, {"blocks": {}},
                                    {"blocks": [{"name": "crop_0", "position": [0, 64, 0]}]})
        self.assertTrue(oracle.infra_errors)
        self.assertEqual(oracle.completed["fresh_physical_cells"], 0)

    def test_public_validator_accepts_contract_recipe_keyword(self):
        prepared = controls.recipe("explore", 1, True)
        self.assertTrue(controls.validate(scenario_id="explore", report=exploration_report(prepared),
                                          recipe=prepared)["passed"])

    def test_projectile_damage_joins_real_owner_victim_and_same_uuid_removal(self):
        arrow = "e06a523c-cc37-48f4-b673-9855e2266865"
        owner = "f09c5964-45c9-4343-a0b5-43b03d533e8f"
        expected = {"clients": 1, "entities": {"minecraft:arrow": [], "minecraft:skeleton": [owner]}}
        arrows = {"samples": [{"uuid": arrow, "initial_position": [0, 65, 5],
                               "final_position": [0, 65, 0], "remove_count": 1}]}
        hit = {"projectile_uuid": arrow, "owner_uuid": owner, "target_bot": 0, "window": "steady"}
        trace_status = {"damage_events_unresolved": 0, "damage_events_truncated": 0,
                        "projectile_duplicate_hits": 0}
        oracle = controls._Oracle()
        controls._projectile_trace_oracle(oracle, {**trace_status, "projectile_hits": [hit]}, arrows, expected)
        self.assertTrue(oracle.result()["passed"])
        self.assertEqual(oracle.completed["natural_projectile_hit_removal_chains"], 1)
        for change in ({"owner_uuid": None}, {"target_bot": 1}, {"window": "preparation"}):
            oracle = controls._Oracle()
            controls._projectile_trace_oracle(oracle, {**trace_status, "projectile_hits": [{**hit, **change}]}, arrows, expected)
            self.assertFalse(oracle.result()["passed"])
        oracle = controls._Oracle()
        controls._projectile_trace_oracle(oracle, {**trace_status, "projectile_hits": [hit, hit]}, arrows, expected)
        self.assertIn("projectiles: duplicate hit for non-piercing arrow", oracle.failures)
        arrows["samples"][0]["remove_count"] = 0
        oracle = controls._Oracle()
        controls._projectile_trace_oracle(oracle, {**trace_status, "projectile_hits": [hit]}, arrows, expected)
        self.assertFalse(oracle.result()["passed"])
        arrows["samples"][0]["remove_count"] = 1
        for failure in ("damage_events_unresolved", "damage_events_truncated", "projectile_duplicate_hits"):
            oracle = controls._Oracle()
            controls._projectile_trace_oracle(oracle,
                {**trace_status, failure: 1, "projectile_hits": [hit]}, arrows, expected)
            self.assertFalse(oracle.result()["passed"])

    def test_feedback_counts_never_replace_observed_durable_save(self):
        expected = {"families": [], "entities": {}, "stable_blocks": {}}
        restart = {"checkpoint": {"feedback_count": 100000}, "same_world": True,
                   "fixture_regenerated": False, "world_before": {"sha256": "before"},
                   "world_after": {"sha256": "after"}, "before": {}, "after": {}}
        oracle = controls._Oracle()
        controls._restart_oracle(oracle, restart, expected)
        self.assertIn("restart: actual Saved-game completion receipt missing", oracle.infra_errors)
        restart["checkpoint"] = {"checkpoint_event": {"event": "fixture_checkpoint_saved"}}
        oracle = controls._Oracle()
        controls._restart_oracle(oracle, restart, expected)
        self.assertTrue(oracle.result()["passed"])

    def test_long_session_consumes_actual_normalized_resource_windows(self):
        prepared = controls.recipe("long-session", 16, True)
        report = exploration_report(prepared)
        report["raw_samples"] = "recorded/samples.jsonl"
        report["windows"] = {phase: {
            "edge_send_queue": {"samples": 2, "snapshots": [{"reliable": {}}, {"reliable": {}}]},
            "memory": {process: {"rss_max_bytes": 1024, "rss_final_bytes": 512}
                       for process in ("server", "bots")}}
            for phase in ("steady", "burst", "recovery")}
        proof = controls.validate("long-session", report, prepared)
        self.assertFalse(any("normalized process RSS" in error or "queue time series" in error
                             for error in proof["infra_errors"]))
        self.assertIn("raw RSS time series retained by backend", proof["completed"]["resource_scope"])


if __name__ == "__main__":
    unittest.main()
