use super::super::tables::IdMap;
use super::*;

fn tables() -> Tables {
    Tables {
        items: IdMap::by_name([("stick", 10)], [("stick", 3)], |_| None),
        argument_types: IdMap::by_name([("uuid", 61)], [("uuid", 58)], |_| None),
        argument_names: {
            let mut names = vec![String::new(); 62];
            names[61] = "minecraft:uuid".into();
            names
        },
        registry_ids: [(
            "custom_stat".into(),
            IdMap::by_name([("jump", 9)], [("jump", 8)], |_| None),
        )]
        .into(),
        ..Tables::default()
    }
}
fn translate(name: &str, body: &[u8]) -> Vec<u8> {
    let mut r = body;
    let mut out = Vec::new();
    clientbound(&tables(), name, &mut r, &mut out).unwrap();
    assert!(r.is_empty());
    out
}
#[test]
fn command_parser_ids_follow_names_not_ordinals() {
    // One argument node with no children and a UUID singleton parser.
    let body = [1, 2, 0, 1, b'x', 61, 0];
    assert_eq!(translate("commands", &body), [1, 2, 0, 1, b'x', 58, 0]);
}
#[test]
fn recipes_remap_templates_and_ingredients() {
    let body = [1, 1, b'x', 1, 10, 1, 2, 10, 5, 10, 2, 0, 0];
    let expected = [1, 1, b'x', 1, 3, 1, 2, 3, 5, 3, 2, 0, 0];
    assert_eq!(translate("update_recipes", &body), expected);
    // One shapeless entry with item ingredient and template output.
    let body = [
        1, 7, 0, 1, 4, 10, 5, 10, 2, 0, 0, 1, 0, 0, 1, 1, 2, 10, 3, 1,
    ];
    let expected = [1, 7, 0, 1, 4, 3, 5, 3, 2, 0, 0, 1, 0, 0, 1, 1, 2, 3, 3, 1];
    assert_eq!(translate("recipe_book_add", &body), expected);
}
#[test]
fn custom_stat_insertions_do_not_relabel_counters() {
    assert_eq!(translate("award_stats", &[1, 8, 9, 12]), [1, 8, 8, 12]);
}
#[test]
fn advancement_coordinates_move_back_inside_display_info() {
    let mut body = vec![1, 1];
    body.write_string("test:a", 32767).unwrap();
    body.extend([0, 1]); // no parent, display present
    mc_nbt::write_network_value(&mut body, &mc_nbt::Tag::String("Title".into())).unwrap();
    mc_nbt::write_network_value(&mut body, &mc_nbt::Tag::String("Description".into())).unwrap();
    body.extend([10, 1, 0, 0, 0]); // icon template, frame
    body.write_i32(0);
    let display_end = body.len();
    body.extend([0, 0]); // no requirements, no telemetry
    body.write_f32(2.5);
    body.write_f32(-1.0);
    body.extend([0, 0, 1]); // removed, progress, show
    let out = translate("update_advancements", &body);
    let mut expected = body[..display_end].to_vec();
    // Icon item id was 10, now 3; it is five bytes before the flags.
    expected[display_end - 9] = 3;
    expected.extend_from_slice(&body[display_end + 2..display_end + 10]);
    expected.extend_from_slice(&body[display_end..display_end + 2]);
    expected.extend_from_slice(&body[display_end + 10..]);
    assert_eq!(out, expected);
}
