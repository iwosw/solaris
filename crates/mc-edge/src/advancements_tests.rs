use super::*;
use mc_core::boundary::{ConnectionId, EntityId, PlayerChange, PlayerInput};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundRecipeBookAdd, ClientboundSelectAdvancementsTab, ServerboundSeenAdvancements,
};
use std::sync::Arc;

fn body(packet: &crate::plan::PlannedPacket) -> bytes::Bytes {
    let mut bytes = bytes::BytesMut::from(&packet.data[..]);
    mc_protocol::frame::try_decode_frame(
        &mut bytes,
        mc_protocol::frame::Compression::Threshold(256),
    )
    .unwrap()
    .unwrap()
    .body
}
fn fixture() -> (crate::plan::ViewerState, Arc<AdvancementTree>) {
    let id = Identifier::parse("minecraft:story/root").unwrap();
    let mut tree = AdvancementTree::default();
    tree.definitions.insert(
        id.to_string(),
        mc_data::advancements::AdvancementDefinition {
            requirements: vec![vec!["tick".into()]],
            ..Default::default()
        },
    );
    let catalog = AdvancementPackets {
        definitions: BTreeMap::from([(
            id.to_string(),
            AdvancementDefinition {
                id,
                parent: None,
                display: None,
                requirements: vec![vec!["tick".into()]],
                sends_telemetry: false,
            },
        )]),
        recipes: BTreeMap::new(),
    };
    let mut viewer = crate::plan::ViewerState::new(ConnectionId(7), EntityId(1), [0.0; 3]);
    viewer.advancements = Arc::new(catalog);
    (viewer, Arc::new(tree))
}
#[test]
fn reset_tree_and_progress_delta_are_reliable_and_viewer_scoped() {
    let (mut viewer, tree) = fixture();
    let progress = serde_json::json!({"minecraft:story/root":{"criteria":{"tick":"1970-01-01 00:00:01 +0000"},"done":true}});
    let change = PlayerChange::Advancements {
        reset: true,
        definitions: tree,
        progress: progress.clone(),
    };
    let packets = crate::plan::plan_player_changes(&[change], &mut viewer);
    assert_eq!(packets.len(), 1);
    assert!(packets[0].reliable);
    let reset = ClientboundUpdateAdvancements::decode(&mut &body(&packets[0])[..]).unwrap();
    assert!(reset.reset);
    assert_eq!(reset.added.len(), 1);
    assert_eq!(
        reset.progress[&Identifier::parse("minecraft:story/root").unwrap()]["tick"],
        Some(1000)
    );
    let delta = PlayerChange::Advancements {
        reset: false,
        definitions: Arc::default(),
        progress,
    };
    let packets = crate::plan::plan_player_changes(std::slice::from_ref(&delta), &mut viewer);
    let delta_packet = ClientboundUpdateAdvancements::decode(&mut &body(&packets[0])[..]).unwrap();
    assert!(!delta_packet.reset);
    assert!(delta_packet.added.is_empty());
    assert_eq!(delta_packet.progress, reset.progress);
}

#[test]
fn hero_of_the_village_icon_emits_each_component_once() {
    use mc_protocol::codec::ReadMc;
    let dir = tempfile::tempdir().unwrap();
    let mut config = mc_core::config::CoreConfig::new(dir.path().join("world"), 42);
    config.vanilla_data_dir =
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
    let core = mc_core::Core::open(config).unwrap();
    let catalog = AdvancementPackets::new(core.registry()).unwrap();
    let hero = catalog.definitions["minecraft:adventure/hero_of_the_village"].clone();
    let icon = &hero.display.as_ref().unwrap().icon;
    assert_eq!(icon.extended().banner_patterns.len(), 8);
    let packet = ClientboundUpdateAdvancements {
        reset: true,
        added: vec![hero],
        removed: Vec::new(),
        progress: BTreeMap::new(),
        show_advancements: true,
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    let mut icon_bytes = bytes.as_slice();
    assert!(icon_bytes.read_bool().unwrap());
    assert_eq!(icon_bytes.read_varint().unwrap(), 1);
    icon_bytes.read_string(32767).unwrap();
    if icon_bytes.read_bool().unwrap() {
        icon_bytes.read_string(32767).unwrap();
    }
    assert!(icon_bytes.read_bool().unwrap());
    mc_nbt::read_network_value(&mut icon_bytes).unwrap();
    mc_nbt::read_network_value(&mut icon_bytes).unwrap();
    icon_bytes.read_varint().unwrap(); // template item id
    assert_eq!(icon_bytes.read_varint().unwrap(), 1);
    // item_name, rarity, tooltip_display, banner_patterns; never a fifth
    // station-owned copy of the preserved banner_patterns component.
    assert_eq!(icon_bytes.read_varint().unwrap(), 4);
    assert_eq!(icon_bytes.read_varint().unwrap(), 0);
    let mut input = bytes.as_slice();
    let decoded = ClientboundUpdateAdvancements::decode(&mut input)
        .expect("the exact ominous-banner icon must not duplicate banner_patterns");
    assert!(input.is_empty());
    assert_eq!(decoded.added.len(), 1);
    let actual = &decoded.added[0].display.as_ref().unwrap().icon;
    let expected = &packet.added[0].display.as_ref().unwrap().icon;
    assert_eq!(actual.item_id, expected.item_id);
    assert_eq!(actual.count, expected.count);
    assert_eq!(
        actual.extended().banner_patterns,
        expected.extended().banner_patterns
    );
    // The wire decoder stores banner patterns in their typed field, not the
    // source-NBT preservation baseline. All other components survive verbatim.
    assert!(
        actual.extended().preserved_components.iter().eq(expected
            .extended()
            .preserved_components
            .iter()
            .filter(|component| component.name != "minecraft:banner_patterns"))
    );
    core.shutdown();
}
#[test]
fn seen_tab_decode_and_selected_tab_reply() {
    let (mut viewer, _) = fixture();
    let mut state = crate::decode::DecodeState::default();
    for (packet, tab) in [
        (
            ServerboundSeenAdvancements::OpenedTab(
                Identifier::parse("minecraft:story/root").unwrap(),
            ),
            Some("minecraft:story/root".to_owned()),
        ),
        (ServerboundSeenAdvancements::ClosedScreen, None),
    ] {
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        let inputs = crate::decode::decode_play_frame_for(
            ConnectionId(7),
            ServerboundSeenAdvancements::ID,
            &bytes,
            &mut state,
        )
        .unwrap();
        assert!(
            matches!(&inputs[..], [PlayerInput::SeenAdvancements { connection: ConnectionId(7), tab: decoded }] if decoded == &tab)
        );
        let packets = crate::plan::plan_player_changes(
            &[PlayerChange::AdvancementsTab { tab: tab.clone() }],
            &mut viewer,
        );
        let selected =
            ClientboundSelectAdvancementsTab::decode(&mut &body(&packets[0])[..]).unwrap();
        assert_eq!(selected.tab.map(|id| id.to_string()), tab);
        bytes.push(0);
        assert!(
            crate::decode::decode_play_frame_for(
                ConnectionId(7),
                ServerboundSeenAdvancements::ID,
                &bytes,
                &mut state
            )
            .is_err()
        );
    }
}
#[test]
fn initial_book_is_empty_and_core_unlocks_only_named_recipe_displays() {
    let dir = std::env::temp_dir().join(format!("mc-edge-advancements-{}", uuid::Uuid::new_v4()));
    let core = mc_core::Core::open({
        let mut config = mc_core::config::CoreConfig::new(dir.join("world"), 42);
        config.vanilla_data_dir =
            Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
        config
    })
    .unwrap();
    let recipes = crate::recipes::RecipePackets::new(core.registry()).unwrap();
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(recipes.book.replace);
    assert!(recipes.book.entries.is_empty());
    let (name, entry) = recipes.advancements.recipes.iter().next().unwrap();
    let mut viewer = crate::plan::ViewerState::new(ConnectionId(7), EntityId(1), [0.0; 3]);
    viewer.advancements = Arc::clone(&recipes.advancements);
    for replace in [true, false] {
        let changes = [PlayerChange::RecipesUnlocked {
            recipes: vec![name.clone(), "minecraft:unknown".into()],
            replace,
        }];
        let packets = crate::plan::plan_player_changes(&changes, &mut viewer);
        let book = ClientboundRecipeBookAdd::decode(&mut &body(&packets[0])[..]).unwrap();
        assert_eq!(book.replace, replace);
        let mut expected = entry.clone();
        if replace {
            expected.flags = 0;
        }
        assert_eq!(book.entries, vec![expected]);
    }
}
#[test]
fn displayed_tree_layout_uses_depth_and_distinct_leaf_rows() {
    let mut tree = AdvancementTree::default();
    for (id, parent) in [("root", None), ("a", Some("root")), ("b", Some("root"))] {
        tree.definitions.insert(
            id.into(),
            mc_data::advancements::AdvancementDefinition {
                parent: parent.map(str::to_owned),
                display: Some(serde_json::json!({})),
                ..Default::default()
            },
        );
    }
    let children = BTreeMap::from([(None, vec!["root"]), (Some("root"), vec!["a", "b"])]);
    let mut positions = BTreeMap::new();
    position("root", 0, &mut 0.0, &tree, &children, &mut positions);
    assert_eq!(positions["root"], (0, 0.5));
    assert_eq!(positions["a"], (1, 0.0));
    assert_eq!(positions["b"], (1, 1.0));
}

#[test]
#[ignore = "requires locally extracted vanilla sidecar"]
fn vanilla_recipe_unlock_notifications_respect_recipe_metadata() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let dir =
        std::env::temp_dir().join(format!("mc-edge-vanilla-recipes-{}", uuid::Uuid::new_v4()));
    let mut config = mc_core::config::CoreConfig::new(dir.join("world"), 42);
    config.vanilla_data_dir = Some(root);
    let core = mc_core::Core::open(config).unwrap();
    let catalog = AdvancementPackets::new(core.registry()).unwrap();
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
    let crafting_table = &catalog.recipes["minecraft:crafting_table"];
    assert_eq!(
        crafting_table.flags, 2,
        "crafting table highlights without a toast"
    );
    assert_eq!(catalog.recipes["minecraft:stick"].flags, 3);
    let mut viewer = crate::plan::ViewerState::new(ConnectionId(7), EntityId(1), [0.0; 3]);
    viewer.advancements = Arc::new(catalog);
    for replace in [true, false] {
        let packets = crate::plan::plan_player_changes(
            &[PlayerChange::RecipesUnlocked {
                recipes: vec!["minecraft:crafting_table".into(), "minecraft:stick".into()],
                replace,
            }],
            &mut viewer,
        );
        let book = ClientboundRecipeBookAdd::decode(&mut &body(&packets[0])[..]).unwrap();
        assert_eq!(
            book.entries
                .iter()
                .map(|entry| entry.flags)
                .collect::<Vec<_>>(),
            if replace { vec![0, 0] } else { vec![2, 3] },
        );
    }
}
