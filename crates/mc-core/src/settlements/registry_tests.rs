use super::*;

fn create(registry: &mut Registry, sequence: &mut u64, name: &str) -> SettlementId {
    registry
        .create(sequence, name.into(), "owner".into(), [0, 64, 0], 48)
        .unwrap()
}

#[test]
fn settlement_ids_never_reuse_deleted_or_replayed_ids() {
    let mut registry = Registry::default();
    let mut sequence = 0;
    let first = create(&mut registry, &mut sequence, "first");
    assert!(registry.remove(first));
    let mut reopened = Registry::default();
    for image in registry.take_images() {
        reopened.apply_image(image);
    }
    reopened.set_high_water(registry.high_water());
    assert_eq!(reopened.records().count(), 0);
    let second = create(&mut reopened, &mut sequence, "second");
    assert!(second.0 > first.0);
    assert_eq!(reopened.high_water(), second.0);
}

#[test]
fn touched_record_images_coalesce_without_capturing_unchanged_records() {
    let mut registry = Registry::default();
    let mut sequence = 0;
    let first = create(&mut registry, &mut sequence, "first");
    let second = create(&mut registry, &mut sequence, "second");
    assert_eq!(registry.take_images().len(), 2);
    assert!(registry.take_images().is_empty());
    registry.rename(first, "first".into()).unwrap();
    assert!(registry.take_images().is_empty());
    registry.rename(first, "renamed".into()).unwrap();
    registry.rename(first, "final".into()).unwrap();
    let images = registry.take_images();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].id, first);
    assert_eq!(images[0].value.as_ref().unwrap().name, "final");
    assert_eq!(registry.get(second).unwrap().revision, 1);
    registry.remove(first);
    assert_eq!(
        registry.take_images(),
        vec![RecordImage {
            id: first,
            value: None
        }]
    );
}

#[test]
fn refused_registry_mutations_do_not_spend_identity_or_touch_records() {
    let mut registry = Registry::default();
    let mut sequence = 0;
    let first = create(&mut registry, &mut sequence, "first");
    registry.take_images();
    for name in ["", "has spaces", "first", "../escape"] {
        assert!(
            registry
                .create(&mut sequence, name.into(), "owner".into(), [0; 3], 48)
                .is_err()
        );
    }
    assert_eq!(sequence, first.0);
    assert!(registry.take_images().is_empty());
    let second = create(&mut registry, &mut sequence, "second");
    registry.take_images();
    assert!(registry.rename(second, "first".into()).is_err());
    assert!(registry.take_images().is_empty());
    sequence = u64::MAX;
    assert!(
        registry
            .create(
                &mut sequence,
                "exhausted".into(),
                "owner".into(),
                [0; 3],
                48
            )
            .is_err()
    );
    assert_eq!(registry.records().count(), 2);
}

#[test]
fn record_images_replay_idempotently_and_derived_counts_are_not_durable() {
    let mut registry = Registry::default();
    let mut sequence = 0;
    let id = create(&mut registry, &mut sequence, "town");
    registry.count_member(id);
    assert_eq!(registry.member_count(id), 1);
    let images = registry.images();
    let mut reopened = Registry::default();
    for image in images.iter().chain(&images) {
        reopened.apply_image(image.clone());
    }
    assert_eq!(reopened.images(), images);
    assert_eq!(reopened.member_count(id), 0);
    assert!(reopened.take_images().is_empty());
}
