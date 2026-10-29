use super::bundle;
use super::click_tests::{BUCKET, BUNDLE, STONE, Session, WATER_BUCKET, stack};
use super::{ItemRules, MenuKind};
use crate::boundary::ClickMode;
use mc_data::ItemStack;

fn contents(s: &Session, bundle_stack: &ItemStack) -> Vec<ItemStack> {
    let rules: ItemRules<'_> = s.data.rules();
    bundle::read(&rules, bundle_stack).expect("bundle contents decode")
}

#[test]
fn cursor_stack_clicked_on_a_bundle_goes_inside_and_the_bundle_stays() {
    let mut s = Session::new(MenuKind::Player);
    s.put(9, stack(BUNDLE, 1));
    s.menu.carried = stack(STONE, 10);
    s.click(9, 0, ClickMode::Pickup);
    assert!(s.menu.carried.is_empty(), "cobblestone left the cursor");
    assert_eq!(
        s.slot(9).item_id,
        BUNDLE,
        "bundle did not swap with the cursor"
    );
    assert_eq!(contents(&s, &s.slot(9)), vec![stack(STONE, 10)]);
}

#[test]
fn bundle_on_the_cursor_clicked_on_a_stack_takes_it_in() {
    let mut s = Session::new(MenuKind::Player);
    s.put(9, stack(STONE, 10));
    s.menu.carried = stack(BUNDLE, 1);
    s.click(9, 0, ClickMode::Pickup);
    assert!(s.slot(9).is_empty());
    assert_eq!(s.menu.carried.item_id, BUNDLE);
    assert_eq!(contents(&s, &s.menu.carried), vec![stack(STONE, 10)]);
}

#[test]
fn weight_caps_insertion_and_merges_equal_stacks() {
    let mut s = Session::new(MenuKind::Player);
    s.put(9, stack(BUNDLE, 1));
    s.menu.carried = stack(STONE, 50);
    s.click(9, 0, ClickMode::Pickup);
    s.menu.carried = stack(STONE, 30);
    s.click(9, 0, ClickMode::Pickup);
    // 64 stone fill a bundle (1/64 each); the surplus stays on the cursor.
    assert_eq!(s.menu.carried, stack(STONE, 16));
    assert_eq!(contents(&s, &s.slot(9)), vec![stack(STONE, 64)]);
    // A full bundle refuses even a single non-stackable bucket (weight 1).
    s.menu.carried = stack(WATER_BUCKET, 1);
    s.click(9, 0, ClickMode::Pickup);
    assert_eq!(s.menu.carried, stack(WATER_BUCKET, 1));
    assert_eq!(contents(&s, &s.slot(9)), vec![stack(STONE, 64)]);
}

#[test]
fn right_click_with_empty_cursor_takes_the_newest_stack_out() {
    let mut s = Session::new(MenuKind::Player);
    s.put(9, stack(BUNDLE, 1));
    s.menu.carried = stack(STONE, 10);
    s.click(9, 0, ClickMode::Pickup);
    s.menu.carried = stack(BUCKET, 2);
    s.click(9, 0, ClickMode::Pickup);
    s.click(9, 1, ClickMode::Pickup);
    assert_eq!(s.menu.carried, stack(BUCKET, 2));
    assert_eq!(contents(&s, &s.slot(9)), vec![stack(STONE, 10)]);
    s.menu.carried = ItemStack::EMPTY;
    s.click(9, 1, ClickMode::Pickup);
    assert_eq!(s.menu.carried, stack(STONE, 10));
    assert!(contents(&s, &s.slot(9)).is_empty());
    assert!(
        s.slot(9).extended().preserved_components.is_empty(),
        "an emptied bundle equals a fresh one"
    );
}

#[test]
fn bundle_on_the_cursor_right_clicked_on_an_empty_slot_places_the_newest_stack() {
    let mut s = Session::new(MenuKind::Player);
    let mut full = stack(BUNDLE, 1);
    let rules = s.data.rules();
    assert!(bundle::write(&rules, &mut full, &[stack(STONE, 5)]));
    s.menu.carried = full;
    s.click(10, 1, ClickMode::Pickup);
    assert_eq!(s.slot(10), stack(STONE, 5));
    assert!(contents(&s, &s.menu.carried).is_empty());
}

#[test]
fn left_click_with_an_empty_cursor_still_picks_the_bundle_up() {
    let mut s = Session::new(MenuKind::Player);
    s.put(9, stack(BUNDLE, 1));
    s.click(9, 0, ClickMode::Pickup);
    assert_eq!(
        s.menu.carried.item_id, BUNDLE,
        "left click with an empty cursor picks it up"
    );
    assert!(s.slot(9).is_empty());
}

#[test]
fn nesting_beyond_what_the_wire_encodes_is_refused_like_a_full_bundle() {
    let s = Session::new(MenuKind::Player);
    let rules = s.data.rules();
    // Each pass wraps the previous bundle in a fresh one: levels of non-empty bundles.
    let mut inner = stack(BUNDLE, 1);
    for level in 1..=8 {
        let mut outer = stack(BUNDLE, 1);
        let contents = bundle::read(&rules, &outer).unwrap();
        assert!(
            bundle::room(&rules, &contents, &inner) > 0,
            "level {level} is within the wire depth"
        );
        let mut contents = contents;
        assert_eq!(bundle::insert(&rules, &mut contents, &mut inner), 1);
        assert!(bundle::write(&rules, &mut outer, &contents));
        inner = outer;
    }
    // `inner` now has 8 nested non-empty levels: one more would not encode.
    let contents = bundle::read(&rules, &stack(BUNDLE, 1)).unwrap();
    assert_eq!(bundle::room(&rules, &contents, &inner), 0);
}

#[test]
fn clicking_a_too_deep_bundle_changes_nothing() {
    let mut s = Session::new(MenuKind::Player);
    let rules = s.data.rules();
    let mut inner = stack(BUNDLE, 1);
    for _ in 0..8 {
        let mut outer = stack(BUNDLE, 1);
        let mut contents = Vec::new();
        bundle::insert(&rules, &mut contents, &mut inner);
        bundle::write(&rules, &mut outer, &contents);
        inner = outer;
    }
    s.put(9, inner.clone());
    s.menu.carried = stack(BUNDLE, 1);
    s.click(9, 0, ClickMode::Pickup);
    // The empty cursor bundle refuses to take the too-deep bundle, and nothing moved.
    assert_eq!(s.slot(9), inner);
    assert_eq!(s.menu.carried, stack(BUNDLE, 1));
}
