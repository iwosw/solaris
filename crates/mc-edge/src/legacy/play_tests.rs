//! Byte layouts of the translated Play packets (26.3 -> 26.1.2 and back).

use mc_protocol::codec::{ReadMc, WriteMc};

use super::super::tables::{IdMap, Tables};
use super::super::test_session;
use super::*;

fn session() -> Session {
    test_session(Tables {
        block_states: IdMap::by_name([("a", 0), ("b", 9)], [("a", 0), ("b", 4)], |_| None),
        items: IdMap::by_name([("stick", 10)], [("stick", 3)], |_| None),
        ..Tables::default()
    })
}

#[test]
fn login_drops_online_mode_and_narrows_game_modes() {
    let mut s = session();
    let mut body = Vec::new();
    body.write_i32(7);
    body.write_bool(false);
    body.write_varint(1);
    body.write_string("minecraft:overworld", 32767).unwrap();
    for v in [20, 10, 6] {
        body.write_varint(v);
    }
    for b in [false, true, false] {
        body.write_bool(b);
    }
    let spawn_start = body.len();
    body.write_varint(0); // dimension type holder
    body.write_string("minecraft:overworld", 32767).unwrap();
    body.write_i64(42);
    body.write_varint(1); // creative
    body.write_varint(0); // no previous mode
    body.write_bool(false);
    body.write_bool(false);
    body.write_bool(false); // no death location
    body.write_varint(0);
    body.write_varint(63);
    let spawn_end = body.len();
    body.write_bool(true); // onlineMode (26.3 only)
    body.write_bool(false); // enforcesSecureChat
    let out = clientbound(&mut s, "minecraft:login", &body)
        .unwrap()
        .unwrap();
    let mut expected = body[..spawn_start].to_vec();
    expected.write_varint(0);
    expected.write_string("minecraft:overworld", 32767).unwrap();
    expected.write_i64(42);
    expected.write_u8(1);
    expected.write_i8(-1);
    expected.extend_from_slice(&body[spawn_end - 5..spawn_end]);
    expected.write_bool(false);
    assert_eq!(out, expected);
}

#[test]
fn stepped_entity_moves_collapse_into_one_delta() {
    let mut s = session();
    let mut body = Vec::new();
    body.write_varint(5);
    body.write_varint(1 | (2 << 1)); // on ground, two steps
    for (ticks, delta) in [(1, [10i16, -2, 3]), (2, [5, 0, -3])] {
        body.write_varint(ticks);
        for d in delta {
            body.write_i16(d);
        }
    }
    body.write_u8(64);
    body.write_u8(32);
    let out = clientbound(&mut s, "minecraft:move_entity_pos_rot", &body)
        .unwrap()
        .unwrap();
    let mut r = &out[..];
    assert_eq!(r.read_varint().unwrap(), 5);
    assert_eq!(
        [
            r.read_i16().unwrap(),
            r.read_i16().unwrap(),
            r.read_i16().unwrap()
        ],
        [15, -2, 0]
    );
    assert_eq!([r.read_u8().unwrap(), r.read_u8().unwrap()], [64, 32]);
    assert!(r.read_bool().unwrap());
    assert!(r.is_empty());
}

#[test]
fn block_updates_and_slots_use_legacy_ids() {
    let mut s = session();
    let mut body = Vec::new();
    body.write_i64(123);
    body.write_varint(9);
    let out = clientbound(&mut s, "minecraft:block_update", &body)
        .unwrap()
        .unwrap();
    let mut r = &out[..];
    assert_eq!(r.read_i64().unwrap(), 123);
    assert_eq!(r.read_varint().unwrap(), 4);

    let mut body = Vec::new();
    body.write_varint(0); // window
    body.write_varint(3); // state id
    body.write_i16(36);
    body.write_varint(2); // count
    body.write_varint(10); // stick
    body.write_varint(0);
    body.write_varint(0);
    let out = clientbound(&mut s, "minecraft:container_set_slot", &body)
        .unwrap()
        .unwrap();
    assert_eq!(out[out.len() - 3], 3, "stick has its 26.1.2 item id");
}

#[test]
fn animations_and_team_parameters_follow_the_old_layout() {
    let mut s = session();
    let mut body = Vec::new();
    body.write_varint(9);
    body.write_varint(1); // off hand
    body.write_varint(1);
    body.write_varint(6);
    let out = clientbound(&mut s, "minecraft:swing_animation", &body)
        .unwrap()
        .unwrap();
    assert_eq!(out, [9, 3]);
    assert_eq!(
        legacy_name("minecraft:swing_animation"),
        "minecraft:animate"
    );
    let out = clientbound(&mut s, "minecraft:animate", &[9, 1])
        .unwrap()
        .unwrap();
    assert_eq!(out, [9, 4], "critical hit is 4 in 26.1.2");

    let text = |s: &str| {
        let mut out = Vec::new();
        mc_nbt::write_network_value(&mut out, &mc_nbt::Tag::String(s.into())).unwrap();
        out
    };
    let mut body = Vec::new();
    body.write_string("red", 32767).unwrap();
    body.write_u8(2);
    body.extend_from_slice(&text("Red"));
    body.extend_from_slice(&text("[R] "));
    body.extend_from_slice(&text(""));
    body.write_varint(1);
    body.write_varint(2);
    body.write_bool(true);
    body.write_varint(12);
    body.write_u8(3);
    let out = clientbound(&mut s, "minecraft:set_player_team", &body)
        .unwrap()
        .unwrap();
    let mut expected = Vec::new();
    expected.write_string("red", 32767).unwrap();
    expected.write_u8(2);
    expected.extend_from_slice(&text("Red"));
    expected.write_u8(3);
    expected.write_varint(1);
    expected.write_varint(2);
    expected.write_varint(12);
    expected.extend_from_slice(&text("[R] "));
    expected.extend_from_slice(&text(""));
    assert_eq!(out, expected);
}

#[test]
fn serverbound_actions_signs_and_teleports_gain_26_3_fields() {
    let mut s = session();
    let mut body = Vec::new();
    body.write_varint(2); // 26.1.2 STOP_DESTROY
    body.write_i64(1);
    body.write_u8(1);
    body.write_varint(7);
    let out = serverbound(&mut s, "minecraft:player_action", &body)
        .unwrap()
        .unwrap();
    assert_eq!(out[0], 3, "CHANGE_DESTROY_DIRECTION shifts later actions");

    let mut body = Vec::new();
    body.write_i64(77);
    body.write_bool(true);
    for line in ["a", "b", "c", "d"] {
        body.write_string(line, 384).unwrap();
    }
    let out = serverbound(&mut s, "minecraft:sign_update", &body)
        .unwrap()
        .unwrap();
    let mut r = &out[..];
    assert_eq!(r.read_i64().unwrap(), 77);
    for line in ["a", "b", "c", "d"] {
        assert_eq!(r.read_string(384).unwrap(), line);
    }
    assert_eq!(r.read_varint().unwrap(), 1, "front text slot");

    let mut teleport = Vec::new();
    teleport.write_varint(4);
    for v in [1.5, 70.0, -2.5, 0.0, 0.0, 0.0] {
        teleport.write_f64(v);
    }
    teleport.write_f32(90.0);
    teleport.write_f32(10.0);
    teleport.write_i32(0);
    let mut writer = s.split();
    let echoed = clientbound(&mut writer, "minecraft:player_position", &teleport)
        .unwrap()
        .unwrap();
    assert_eq!(echoed, teleport, "player_position bytes are unchanged");
    let out = serverbound(&mut s, "minecraft:accept_teleportation", &[4])
        .unwrap()
        .unwrap();
    let mut r = &out[..];
    assert_eq!(r.read_varint().unwrap(), 4);
    assert_eq!(
        [
            r.read_f64().unwrap(),
            r.read_f64().unwrap(),
            r.read_f64().unwrap()
        ],
        [1.5, 70.0, -2.5]
    );
    assert_eq!([r.read_f32().unwrap(), r.read_f32().unwrap()], [90.0, 10.0]);
    assert!(r.is_empty());
}

#[test]
fn swing_and_spectator_inputs_keep_working() {
    let mut s = session();
    assert_eq!(
        serverbound(&mut s, "minecraft:swing", &[1]).unwrap(),
        Some(Vec::new())
    );
    let out = serverbound(&mut s, "minecraft:spectate_entity", &[7])
        .unwrap()
        .unwrap();
    assert_eq!(out, [8], "OptionalInt is entity id + 1");
}

#[test]
fn sign_editor_translates_text_slot_to_front_flag() {
    let mut s = session();
    let mut body = Vec::new();
    body.write_i64(77);
    body.write_varint(0); // back
    let out = clientbound(&mut s, "minecraft:open_sign_editor", &body)
        .unwrap()
        .unwrap();
    assert_eq!(&out[..8], &body[..8]);
    assert_eq!(out[8], 0);
}

#[test]
fn block_entity_payload_and_respawn_tail_are_consumed() {
    let mut s = test_session(Tables {
        block_entities: IdMap::by_name([("chest", 7)], [("chest", 1)], |_| None),
        ..Tables::default()
    });
    let mut body = Vec::new();
    body.write_i64(123);
    body.write_varint(7);
    body.extend([10, 0]); // empty network NBT compound
    let out = clientbound(&mut s, "minecraft:block_entity_data", &body)
        .unwrap()
        .unwrap();
    assert_eq!(out[8], 1);
    assert_eq!(&out[9..], &[10, 0]);

    let mut body = vec![0]; // dimension type
    body.write_string("minecraft:overworld", 32767).unwrap();
    body.write_i64(42);
    body.extend([1, 0, 0, 0, 0, 0, 63, 3]);
    let out = clientbound(&mut s, "minecraft:respawn", &body)
        .unwrap()
        .unwrap();
    assert_eq!(*out.last().unwrap(), 3);
}

#[test]
fn new_cube_spawns_as_slime_with_legacy_size_metadata() {
    let tables = Tables {
        entities: IdMap::by_name([("sulfur_cube", 150)], [("slime", 80)], |_| {
            Some("slime".into())
        }),
        entity_metadata: [(150, Metadata::Cube)].into(),
        ..Tables::default()
    };
    let mut s = test_session(tables);
    let mut body = vec![7];
    body.extend([0; 16]); // UUID
    body.write_varint(150);
    for value in [1.0, 65.0, -2.0] {
        body.write_f64(value);
    }
    body.extend([0, 0, 0, 0, 0]); // zero LP velocity, rotations, spawn data
    let out = clientbound(&mut s, "minecraft:add_entity", &body)
        .unwrap()
        .unwrap();
    assert_eq!(out[17], 80);
    let body = [7, 16, 8, 1, 17, 8, 0, 18, 1, 4, 255];
    let out = clientbound(&mut s, "minecraft:set_entity_data", &body)
        .unwrap()
        .unwrap();
    assert_eq!(
        out,
        [7, 16, 1, 4, 255],
        "size moves to index 16; new age fields vanish"
    );
}
