use super::*;

#[test]
fn charged_projectile_decode_rejects_depth_before_its_payload() {
    let mut buf = Vec::new();
    for depth in 0..=copied_components::MAX_ITEM_COMPONENT_DEPTH {
        for value in [1, 1, 1, 0, 49] {
            buf.write_varint(value);
        }
        if depth < copied_components::MAX_ITEM_COMPONENT_DEPTH {
            buf.write_varint(1);
        }
    }
    let nested_payload_offset = buf.len();
    for _ in 0..10_000 {
        for value in [1, 1, 1, 1, 0, 49] {
            buf.write_varint(value);
        }
    }
    for payload in [&buf[..nested_payload_offset], &buf[..]] {
        let mut cursor = payload;
        assert!(matches!(
            ItemStack::decode(&mut cursor),
            Err(CodecError::NotSupported("nested item component depth"))
        ));
        assert_eq!(cursor, &payload[nested_payload_offset..]);
    }
}

#[test]
fn charged_projectile_decode_accepts_leaf_components_and_bounds_collection() {
    let mut crossbow = ItemStack::new(1, 1);
    crossbow.extended_mut().charged_projectiles = vec![
        ItemStack::new(2, 1).with_damage(3),
        ItemStack::new(3, 1),
        ItemStack::new(4, 1),
    ];
    let mut bytes = Vec::new();
    crossbow.encode(&mut bytes).unwrap();
    let mut cursor = bytes.as_slice();
    assert_eq!(ItemStack::decode(&mut cursor).unwrap(), crossbow);
    assert!(cursor.is_empty());

    let mut oversized = Vec::new();
    for value in [1, 1, 1, 0, 49, 4, 0, 0, 0, 0] {
        oversized.write_varint(value);
    }
    assert!(matches!(
        ItemStack::decode(&mut oversized.as_slice()),
        Err(CodecError::StringTooLong { .. })
    ));
}

#[test]
fn maximum_supported_component_patch_round_trips_thirteen_components_and_leaf_ammo() {
    use mc_data::item_stack::{ArmorTrim, BannerPattern, FireworkExplosion, Fireworks, StewEffect};
    let id = |name| Identifier::parse(format!("minecraft:{name}")).unwrap();
    let explosion = FireworkExplosion {
        shape: 4,
        colors: vec![0x112233, 0xff0000],
        fade_colors: vec![0x00ff00],
        has_trail: true,
        has_twinkle: true,
    };
    let fireworks = Fireworks {
        flight_duration: 255,
        explosions: vec![explosion],
    };
    let mut arrow = ItemStack::new(2, 1);
    arrow.extended_mut().potion = Some(id("long_poison"));
    let mut rocket = ItemStack::new(3, 1);
    rocket.extended_mut().fireworks = Some(fireworks.clone());
    let mut stack = ItemStack::new(1, 1)
        .with_damage(47)
        .with_enchantment(id("unbreaking"), 3)
        .with_custom_name("Full component payload")
        .with_item_model(Identifier::parse("solaris_loader:component_model").unwrap());
    stack.stew_effects = vec![StewEffect {
        id: id("poison"),
        duration: 220,
    }];
    stack.extended_mut().potion = Some(id("strong_strength"));
    stack.extended_mut().trim = Some(ArmorTrim {
        material: id("diamond"),
        pattern: id("coast"),
    });
    stack.extended_mut().banner_patterns = vec![BannerPattern {
        pattern: id("base"),
        color: 15,
    }];
    stack.extended_mut().repair_cost = 31;
    stack.extended_mut().stored_enchantments = vec![mc_data::ItemEnchantment {
        id: id("mending"),
        level: 1,
    }];
    stack.extended_mut().charged_projectiles = vec![arrow, rocket, ItemStack::new(4, 1)];
    stack.extended_mut().fireworks = Some(fireworks);
    stack.extended_mut().ominous_bottle_amplifier = Some(4);
    let mut bytes = Vec::new();
    stack.encode(&mut bytes).unwrap();
    let mut header = bytes.as_slice();
    assert_eq!(header.read_varint().unwrap(), 1);
    assert_eq!(header.read_varint().unwrap(), 1);
    assert_eq!(header.read_varint().unwrap(), 13);
    let mut cursor = bytes.as_slice();
    assert_eq!(ItemStack::decode(&mut cursor).unwrap(), stack);
    assert!(cursor.is_empty());
    for end in 0..bytes.len() {
        assert!(
            ItemStack::decode(&mut &bytes[..end]).is_err(),
            "truncated component patch at {end}"
        );
    }
}

#[test]
fn cleared_last_extended_component_merges_and_encodes_as_an_absent_patch() {
    let bare = ItemStack::new(1, 1);
    let mut cleared = bare.clone();
    cleared.extended_mut().potion = Some(Identifier::parse("minecraft:strong_poison").unwrap());
    cleared.extended_mut().potion = None;
    assert_eq!(cleared, bare);
    assert!(mc_data::inventory_semantics_26_1_2::can_stack(
        &cleared, &bare
    ));
    let mut bytes = Vec::new();
    cleared.encode(&mut bytes).unwrap();
    assert_eq!(bytes, [1, 1, 0, 0]);
    assert_eq!(ItemStack::decode(&mut bytes.as_slice()).unwrap(), bare);
}

#[test]
fn potion_custom_effects_color_name_and_hidden_details_match_vanilla_wire() {
    use mc_data::item_stack::PotionEffect;
    let id = Identifier::parse("minecraft:speed").unwrap();
    let hidden = PotionEffect {
        id: id.clone(),
        amplifier: 0,
        duration: 20,
        ambient: false,
        show_particles: true,
        show_icon: true,
        hidden: None,
    };
    let effect = PotionEffect {
        id,
        amplifier: 255,
        duration: -1,
        ambient: true,
        show_particles: false,
        show_icon: true,
        hidden: Some(Box::new(hidden)),
    };
    let mut stack = ItemStack::new(1, 1);
    let components = stack.extended_mut();
    components.potion_custom_color = Some(0x123456);
    components.potion_custom_effects.push(effect);
    components.potion_custom_name = Some("custom".into());
    let mut bytes = Vec::new();
    stack.encode(&mut bytes).unwrap();
    assert_eq!(
        bytes,
        [
            1, 1, 1, 0, 51, // stack and one added potion_contents
            0,  // no base potion
            1, 0, 0x12, 0x34, 0x56, // optional i32 color
            1, 0, // one custom effect, speed registry id 0
            0xff, 1, // amplifier 255 VarInt
            0xff, 0xff, 0xff, 0xff, 0x0f, // infinite duration
            1, 0, 1, 1, // ambient, particles, icon, hidden present
            0, 20, 0, 1, 1, 0, // hidden details (no repeated effect id)
            1, 6, b'c', b'u', b's', b't', b'o', b'm', // optional name
        ]
    );
    let mut cursor = bytes.as_slice();
    assert_eq!(ItemStack::decode(&mut cursor).unwrap(), stack);
    assert!(cursor.is_empty());
    for end in 0..bytes.len() {
        assert!(ItemStack::decode(&mut &bytes[..end]).is_err());
    }
}

#[test]
fn potion_effect_wire_rejects_unknown_ids_excessive_counts_and_hidden_depth() {
    use mc_data::item_stack::{MAX_POTION_CUSTOM_EFFECTS, MAX_POTION_HIDDEN_DEPTH};
    let header = [1, 1, 1, 0, 51, 0, 0];
    let mut bytes = header.to_vec();
    bytes.write_varint(MAX_POTION_CUSTOM_EFFECTS as i32 + 1);
    assert_eq!(
        ItemStack::decode(&mut bytes.as_slice()).unwrap_err(),
        CodecError::StringTooLong {
            len: MAX_POTION_CUSTOM_EFFECTS + 1,
            max: MAX_POTION_CUSTOM_EFFECTS
        }
    );
    let mut bytes = header.to_vec();
    bytes.extend_from_slice(&[1, 40, 0, 0, 0, 1, 1, 0, 0]);
    assert_eq!(
        ItemStack::decode(&mut bytes.as_slice()).unwrap_err(),
        CodecError::NotSupported("unknown potion effect")
    );
    let mut bytes = header.to_vec();
    bytes.extend_from_slice(&[1, 0]);
    for _ in 0..MAX_POTION_HIDDEN_DEPTH {
        bytes.extend_from_slice(&[0, 20, 0, 1, 1, 1]);
    }
    assert_eq!(
        ItemStack::decode(&mut bytes.as_slice()).unwrap_err(),
        CodecError::NotSupported("potion hidden effect depth exceeded")
    );
}
