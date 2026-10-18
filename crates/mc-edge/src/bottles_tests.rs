use super::*;

#[test]
fn bottle_sounds_match_verified_fill_empty_identifiers_sources_and_wire_layout() {
    use mc_protocol::packets::play::ClientboundSoundEntity;
    for (empty, cauldron, name, source) in [
        (false, false, "minecraft:item.bottle.fill", 6),
        (false, true, "minecraft:item.bottle.fill", 4),
        (true, true, "minecraft:item.bottle.empty", 4),
    ] {
        let mut packets = Vec::new();
        push_entity_event(
            &mut packets,
            ConnectionId(7),
            BoundaryEvent::Bottle {
                entity: EntityId(41),
                empty,
                cauldron,
            },
        );
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].id, 0x74);
        let body = body_of(&packets[0]);
        // javap: direct sound holder, optional fixed range, enum ordinal,
        // entity VarInt, volume/pitch floats and signed long seed.
        let mut expected = vec![0, u8::try_from(name.len()).unwrap()];
        expected.extend_from_slice(name.as_bytes());
        expected.extend_from_slice(&[0, source, 41]);
        expected.extend_from_slice(&1.0_f32.to_be_bytes());
        expected.extend_from_slice(&1.0_f32.to_be_bytes());
        expected.extend_from_slice(&41_i64.to_be_bytes());
        assert_eq!(body.as_ref(), expected);
        let sound = ClientboundSoundEntity::decode(&mut &body[..]).unwrap();
        assert_eq!(sound.sound.as_str(), name);
        assert_eq!(sound.source, i32::from(source));
        assert_eq!(sound.entity_id, 41);
        assert_eq!((sound.volume, sound.pitch, sound.seed), (1.0, 1.0, 41));
    }
}
