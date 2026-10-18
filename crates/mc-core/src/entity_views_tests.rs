use super::*;
use crate::boundary::EntityAppearance;
use std::collections::BTreeMap;
use uuid::Uuid;

fn view(id: i32, x: f64, z: f64, appearance: &Arc<EntityAppearance>) -> EntityView {
    EntityView {
        id: EntityId(id),
        uuid: Uuid::from_u128(id as u128),
        type_id: 30,
        position: [x, 64.0, z],
        velocity: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        head_yaw: 0.0,
        on_ground: true,
        appearance: Arc::clone(appearance),
    }
}

/// Publish `(id, after-image)` candidates; `None` removes the identity.
fn publish(views: &mut EntityViews, mut batch: Vec<(i32, Option<EntityView>)>) {
    batch.sort_by_key(|(id, _)| *id);
    views.publish(
        &batch,
        |(id, image)| (EntityId(*id), image.as_ref()),
        |image| image.clone(),
    );
}

/// Apply the same batch to an id-ordered model and compare the full public surface.
fn apply(model: &mut BTreeMap<i32, EntityView>, batch: &[(i32, Option<EntityView>)]) {
    for (id, image) in batch {
        match image {
            Some(image) => model.insert(*id, image.clone()),
            None => model.remove(id),
        };
    }
}

fn assert_matches_model(views: &EntityViews, model: &BTreeMap<i32, EntityView>) {
    assert_eq!(views.len(), model.len());
    for (id, image) in model {
        assert_eq!(views.get(EntityId(*id)), Some(image));
    }
    let oracle: EntityViews = model.values().cloned().collect::<Vec<_>>().into();
    assert_eq!(
        views.iter().collect::<Vec<_>>(),
        oracle.iter().collect::<Vec<_>>()
    );
    for image in model.values() {
        let key = chunk(image.position);
        let pos = ChunkPos { x: key.0, z: key.1 };
        assert_eq!(
            views.in_chunk(pos).map(|row| row.id).collect::<Vec<_>>(),
            oracle.in_chunk(pos).map(|row| row.id).collect::<Vec<_>>()
        );
    }
}

fn shared_pages(a: &EntityViews, b: &EntityViews) -> usize {
    a.pages
        .iter()
        .zip(b.pages.iter())
        .filter(|(a, b)| Arc::ptr_eq(a, b))
        .count()
}

#[test]
fn retained_versions_survive_sparse_dense_holes_and_slot_reuse() {
    let appearance = Arc::new(EntityAppearance::default());
    let mut model = BTreeMap::new();
    let mut views = EntityViews::default();
    let mut retained = Vec::new();
    let mut keep = |views: &EntityViews, model: &BTreeMap<i32, EntityView>| {
        retained.push((views.clone(), model.clone()));
    };

    // 300 rows: pages 0..=3 full, page 4 partial (44 cells).
    let birth: Vec<_> = (1..=300)
        .map(|id| (id, Some(view(id, f64::from(id), 0.5, &appearance))))
        .collect();
    apply(&mut model, &birth);
    publish(&mut views, birth);
    assert_eq!(views.pages.len(), 5);
    assert_eq!(views.changed().len(), 300);
    keep(&views, &model);

    // Fully dirty page 0 (ids 1..=64 are slots 0..64), one sparse cell in page 1,
    // and holes in page 2 (ids 129..=140).
    let mut batch: Vec<_> = (1..=64)
        .map(|id| (id, Some(view(id, f64::from(id) + 100.0, 0.5, &appearance))))
        .collect();
    batch.push((70, Some(view(70, -40.5, 0.5, &appearance))));
    batch.extend((129..=140).map(|id| (id, None)));
    apply(&mut model, &batch);
    let before = views.clone();
    publish(&mut views, batch);
    assert_matches_model(&views, &model);
    assert_eq!(views.changed().len(), 64 + 1 + 12);
    assert_eq!(views.base_revision(), before.revision());
    assert!(!Arc::ptr_eq(&views.pages[0], &before.pages[0]));
    assert!(!Arc::ptr_eq(&views.pages[1], &before.pages[1]));
    assert!(!Arc::ptr_eq(&views.pages[2], &before.pages[2]));
    for page in 3..5 {
        assert!(Arc::ptr_eq(&views.pages[page], &before.pages[page]));
    }
    // Sparse page 1: the 63 unchanged cells are clones, not rewritten images.
    assert_eq!(views.get(EntityId(71)), before.get(EntityId(71)));
    keep(&views, &model);

    // Reuse the holes (last freed slot first), then append across the page tail.
    let mut batch: Vec<_> = (1000..1012)
        .map(|id| (id, Some(view(id, f64::from(id), 33.5, &appearance))))
        .collect();
    batch.extend((2000..2020).map(|id| (id, Some(view(id, -f64::from(id), 1.5, &appearance)))));
    apply(&mut model, &batch);
    let pages_before = views.pages.len();
    publish(&mut views, batch);
    assert_matches_model(&views, &model);
    assert_eq!(views.len(), 300 - 12 + 12 + 20);
    assert_eq!(views.pages.len(), pages_before, "44 + 20 cells fit page 4");
    let reused: Vec<_> = (1000..1012).map(|id| views.rows[&EntityId(id)]).collect();
    assert!(reused.iter().all(|slot| (128..140).contains(slot)));
    assert_eq!(views.rows[&EntityId(2000)], 300);
    keep(&views, &model);

    // Cross into a new page and move rows between chunks again.
    let batch: Vec<_> = (2030..2060)
        .map(|id| (id, Some(view(id, 8.5, 8.5, &appearance))))
        .chain(
            (1..=300)
                .step_by(7)
                .map(|id| (id, Some(view(id, 500.5, -300.5, &appearance)))),
        )
        .collect();
    let mut model_batch = batch.clone();
    model_batch.retain(|(id, _)| model.contains_key(id) || *id >= 2030);
    apply(&mut model, &model_batch);
    publish(&mut views, model_batch);
    assert_matches_model(&views, &model);
    assert_eq!(views.pages.len(), pages_before + 1);

    for (snapshot, snapshot_model) in &retained {
        assert_matches_model(snapshot, snapshot_model);
    }
}

#[test]
fn equal_images_publish_nothing_and_share_every_page() {
    let appearance = Arc::new(EntityAppearance::default());
    let mut views = EntityViews::default();
    let rows: Vec<_> = (1..=200)
        .map(|id| (id, Some(view(id, f64::from(id), 0.5, &appearance))))
        .collect();
    publish(&mut views, rows.clone());
    let before = views.clone();
    // Same values behind a distinct appearance allocation: equal by value.
    let copies: Vec<_> = rows
        .into_iter()
        .map(|(id, image)| {
            let mut image = image.unwrap();
            image.appearance = Arc::new((*image.appearance).clone());
            (id, Some(image))
        })
        .collect();
    publish(&mut views, copies);
    assert_eq!(views.revision(), before.revision());
    assert_eq!(views.changed(), before.changed());
    assert_eq!(shared_pages(&views, &before), before.pages.len());
    assert!(Arc::ptr_eq(&views.rows, &before.rows));
    publish(&mut views, vec![(5000, None)]);
    assert_eq!(
        views.revision(),
        before.revision(),
        "absent twice is a no-op"
    );
}

#[test]
fn slot_freed_and_reused_inside_one_batch_changes_both_identities() {
    let appearance = Arc::new(EntityAppearance::default());
    let mut views = EntityViews::default();
    let rows: Vec<_> = (1..=10)
        .map(|id| (id, Some(view(id, f64::from(id), 0.5, &appearance))))
        .collect();
    publish(&mut views, rows);
    let retained = views.clone();
    let slot = views.rows[&EntityId(4)];
    // Id 4 leaves (frees its slot), id 9 is rewritten in place, id 50 is born
    // later in id order and takes slot of 4.
    publish(
        &mut views,
        vec![
            (4, None),
            (9, Some(view(9, 90.5, 0.5, &appearance))),
            (50, Some(view(50, 50.5, 0.5, &appearance))),
        ],
    );
    assert_eq!(views.changed(), &[EntityId(4), EntityId(9), EntityId(50)]);
    assert!(views.get(EntityId(4)).is_none());
    assert_eq!(views.rows[&EntityId(50)], slot);
    assert_eq!(views.get(EntityId(50)).unwrap().position[0], 50.5);
    assert!(retained.get(EntityId(4)).is_some());
    assert!(retained.get(EntityId(50)).is_none());
    assert_eq!(retained.get(EntityId(9)).unwrap().position[0], 9.0);
    assert_eq!(views.len(), 10);
}

/// A batch above every parallel threshold must be indistinguishable from the
/// serial one-candidate-at-a-time replacement in the same ID order.
#[test]
fn parallel_batch_matches_one_at_a_time_publication() {
    let appearance = Arc::new(EntityAppearance::default());
    let initial: Vec<_> = (0..20_000)
        .map(|id| {
            (
                id * 2,
                Some(view(
                    id * 2,
                    f64::from(id % 400),
                    f64::from(id / 400),
                    &appearance,
                )),
            )
        })
        .collect();
    let mut parallel = EntityViews::default();
    let mut serial = EntityViews::default();
    for one in initial.chunks(1) {
        publish(&mut serial, one.to_vec());
    }
    publish(&mut parallel, initial.clone());
    let mut model: BTreeMap<i32, EntityView> = initial
        .iter()
        .map(|(id, v)| (*id, v.clone().unwrap()))
        .collect();
    let before = parallel.clone();

    let mut batch = Vec::new();
    for id in 0..44_000 {
        match id % 11 {
            // Mostly moving, some crossing chunks.
            0..=6 if id % 2 == 0 => {
                batch.push((
                    id,
                    Some(view(
                        id,
                        f64::from(id % 97) * 3.0,
                        f64::from(id % 13),
                        &appearance,
                    )),
                ));
            }
            7 if id % 2 == 0 => batch.push((id, None)),
            8 | 9 if id % 2 == 1 => {
                batch.push((
                    id,
                    Some(view(id, f64::from(id), -f64::from(id % 50), &appearance)),
                ));
            }
            _ => {}
        }
    }
    assert!(batch.len() >= PARALLEL_PAGE_MIN_ENTRIES && batch.len() >= PARALLEL_RESOLVE_MIN);
    apply(&mut model, &batch);
    for single in &batch {
        publish(&mut serial, vec![single.clone()]);
    }
    publish(&mut parallel, batch);
    assert_matches_model(&parallel, &model);
    assert_eq!(
        *parallel.rows, *serial.rows,
        "slot assignment is serial and ordered"
    );
    assert_eq!(*parallel.free, *serial.free);
    assert_eq!(
        parallel
            .pages
            .iter()
            .flat_map(|p| p.iter())
            .collect::<Vec<_>>(),
        serial
            .pages
            .iter()
            .flat_map(|p| p.iter())
            .collect::<Vec<_>>()
    );
    assert!(parallel.changed().windows(2).all(|pair| pair[0] < pair[1]));
    assert_matches_model(
        &before,
        &initial
            .iter()
            .map(|(id, v)| (*id, v.clone().unwrap()))
            .collect(),
    );
}

/// Native-like census: `cargo test -p mc-core --release views_change_census -- --ignored --nocapture`.
#[test]
#[ignore = "prints a measurement census"]
fn views_change_census() {
    let appearance = Arc::new(EntityAppearance::default());
    let rows = 100_000;
    let mut views = EntityViews::default();
    let birth: Vec<_> = (1..=rows)
        .map(|id| {
            (
                id,
                Some(view(
                    id,
                    f64::from(id % 1600) * 0.5,
                    f64::from(id / 1600) * 0.5,
                    &appearance,
                )),
            )
        })
        .collect();
    publish(&mut views, birth);
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for (moving_percent, noop_percent) in [(90, 20), (50, 20), (5, 0)] {
        let before = views.clone();
        let mut batch = Vec::new();
        let mut moved = 0;
        for id in 1..=rows {
            let roll = (next() % 100) as u32;
            if roll < moving_percent {
                let old = before.get(EntityId(id)).unwrap();
                batch.push((
                    id,
                    Some(view(
                        id,
                        old.position[0] + 0.2,
                        old.position[2] + 0.1,
                        &appearance,
                    )),
                ));
                moved += 1;
            } else if roll < moving_percent + noop_percent {
                batch.push((id, Some(before.get(EntityId(id)).unwrap().clone())));
            }
        }
        let candidates = batch.len();
        publish(&mut views, batch);
        let touched: Vec<usize> = (0..views.pages.len())
            .filter(|&page| !Arc::ptr_eq(&views.pages[page], &before.pages[page]))
            .collect();
        let mut histogram = [0usize; 7];
        for &page in &touched {
            let rows = views.pages[page]
                .iter()
                .zip(before.pages[page].iter())
                .filter(|(a, b)| a != b)
                .count();
            histogram[match rows {
                0 => 0,
                1..=8 => 1,
                9..=16 => 2,
                17..=32 => 3,
                33..=48 => 4,
                49..=63 => 5,
                _ => 6,
            }] += 1;
        }
        println!(
            "census moving={moving_percent}% noop={noop_percent}%: candidates={candidates} moved={moved} changed={} pages={} touched_pages={} rows/page buckets [0,1-8,9-16,17-32,33-48,49-63,64]={histogram:?}",
            views.changed().len(),
            views.pages.len(),
            touched.len(),
        );
    }
}

/// Pre-streaming `build_page`: stages every cell in a 64-wide array, then copies.
fn build_page_reference<S: Copy>(
    old: Option<&[Option<EntityView>]>,
    page: usize,
    group: &[Entry],
    resolved: &[Resolved<S>],
    project: &impl Fn(S) -> EntityView,
) -> PageOut {
    let base = page * ROWS_PER_PAGE;
    let mut staged: [Option<Option<EntityView>>; ROWS_PER_PAGE] = std::array::from_fn(|_| None);
    let mut changed = Vec::with_capacity(group.len());
    let mut moves = Vec::new();
    let mut len = old.map_or(0, <[_]>::len);
    for entry in group {
        let cell = entry.slot - base;
        let row = &resolved[entry.index as usize];
        let current = match &staged[cell] {
            Some(staged) => staged.as_ref(),
            None => old.and_then(|old| old.get(cell)?.as_ref()),
        };
        let before = current.map(|view| chunk(view.position));
        if entry.clear {
            staged[cell] = Some(None);
            changed.push(row.id);
            if before.is_some() {
                moves.push(ChunkMove {
                    id: row.id,
                    before,
                    after: None,
                });
            }
        } else {
            let next = project(row.src.expect("set entry has a source"));
            if current.is_some_and(|current| view_eq(current, &next)) {
                continue;
            }
            let after = chunk(next.position);
            staged[cell] = Some(Some(next));
            len = len.max(cell + 1);
            changed.push(row.id);
            if before != Some(after) {
                moves.push(ChunkMove {
                    id: row.id,
                    before,
                    after: Some(after),
                });
            }
        }
    }
    if changed.is_empty() {
        return PageOut {
            page,
            next: None,
            changed,
            moves,
        };
    }
    let mut cells = Vec::with_capacity(len);
    for (cell, staged) in staged.iter_mut().take(len).enumerate() {
        cells.push(match staged.take() {
            Some(image) => image,
            None => old.expect("unassigned cell has a previous page")[cell].clone(),
        });
    }
    PageOut {
        page,
        next: Some(Arc::new(cells)),
        changed,
        moves,
    }
}

struct PageCase {
    old: Option<Vec<Option<EntityView>>>,
    page: usize,
    entries: Vec<Entry>,
    resolved: Vec<Resolved<(i32, i32)>>,
}

fn shared_appearance() -> &'static Arc<EntityAppearance> {
    static SHARED: std::sync::LazyLock<Arc<EntityAppearance>> =
        std::sync::LazyLock::new(|| Arc::new(EntityAppearance::default()));
    &SHARED
}

fn project_case(&(id, x): &(i32, i32)) -> EntityView {
    view(id, f64::from(x), 0.0, shared_appearance())
}

/// Random page + candidate groups: equal, moved, cleared, cleared-then-reused
/// and appended cells, in stable slot order like `publish` builds them.
fn page_case(next: &mut impl FnMut() -> u64, appearance: &Arc<EntityAppearance>) -> PageCase {
    let page = (next() % 3) as usize;
    let base = page * ROWS_PER_PAGE;
    let has_old = !next().is_multiple_of(8);
    let old_len = if has_old {
        (next() % (ROWS_PER_PAGE as u64 + 1)) as usize
    } else {
        0
    };
    let mut old: Vec<Option<EntityView>> = (0..old_len)
        .map(|cell| {
            (!next().is_multiple_of(5)).then(|| {
                view(
                    (base + cell) as i32 + 1,
                    (next() % 64) as f64 * 16.0,
                    0.0,
                    appearance,
                )
            })
        })
        .collect();
    let mut entries = Vec::new();
    let mut resolved = Vec::new();
    let mut push = |slot: usize, clear: bool, id: i32, x: i32, set: bool| {
        entries.push(Entry {
            slot,
            index: resolved.len() as u32,
            clear,
        });
        resolved.push(Resolved {
            id: EntityId(id),
            src: set.then_some((id, x)),
            slot: Some(slot),
        });
    };
    let density = next() % 100;
    for (cell, image) in old.iter().enumerate() {
        if next() % 100 >= density {
            continue;
        }
        let slot = base + cell;
        let id = slot as i32 + 1;
        match (image, next() % 5) {
            (Some(image), 0) => push(slot, false, id, image.position[0] as i32, true),
            (Some(_), 1) => push(slot, false, id, (next() % 64) as i32 * 16, true),
            (Some(_), 2) => push(slot, true, id, 0, false),
            (Some(_), 3) => {
                push(slot, true, id, 0, false);
                push(slot, false, id + 1000, (next() % 64) as i32 * 16, true);
            }
            (None, _) => push(slot, false, id, (next() % 64) as i32 * 16, true),
            (Some(_), _) => {}
        }
    }
    for cell in old_len..old_len + (next() % 4) as usize {
        if cell < ROWS_PER_PAGE {
            let slot = base + cell;
            push(
                slot,
                false,
                slot as i32 + 1,
                (next() % 64) as i32 * 16,
                true,
            );
        }
    }
    PageCase {
        old: has_old.then(|| std::mem::take(&mut old)),
        page,
        entries,
        resolved,
    }
}

fn xorshift(seed: u64) -> impl FnMut() -> u64 {
    let mut seed = seed;
    move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    }
}

fn moves_of(out: &PageOut) -> Vec<(EntityId, Option<ChunkKey>, Option<ChunkKey>)> {
    out.moves
        .iter()
        .map(|step| (step.id, step.before, step.after))
        .collect()
}

#[test]
fn streaming_build_page_matches_reference() {
    let appearance = Arc::clone(shared_appearance());
    let mut next = xorshift(0x1234_5678_9abc_def1);
    let mut unchanged = 0;
    for _ in 0..20_000 {
        let case = page_case(&mut next, &appearance);
        let old = case.old.as_deref();
        let project = |src: (i32, i32)| project_case(&src);
        let a = build_page(old, case.page, &case.entries, &case.resolved, &project);
        let b = build_page_reference(old, case.page, &case.entries, &case.resolved, &project);
        assert_eq!(a.page, b.page);
        assert_eq!(a.next.as_deref(), b.next.as_deref());
        assert_eq!(a.changed, b.changed);
        assert_eq!(moves_of(&a), moves_of(&b));
        unchanged += usize::from(a.next.is_none());
    }
    assert!(unchanged > 100, "fixture must exercise unchanged pages");
}

/// B,A,B,A page-build micro: `cargo test -p mc-core --release streaming_build_page_micro -- --ignored --nocapture`.
#[test]
#[ignore = "prints a measurement"]
fn streaming_build_page_micro() {
    let appearance = Arc::clone(shared_appearance());
    let mut next = xorshift(0xfeed_beef_cafe_f00d);
    // Native-like: dense pages, ~90% of cells moving.
    let cases: Vec<PageCase> = (0..2048)
        .map(|page| {
            let base = page * ROWS_PER_PAGE;
            let old: Vec<_> = (0..ROWS_PER_PAGE)
                .map(|cell| {
                    Some(view(
                        (base + cell) as i32 + 1,
                        f64::from((next() % 1024) as u32),
                        f64::from((next() % 1024) as u32),
                        &appearance,
                    ))
                })
                .collect();
            let mut entries = Vec::new();
            let mut resolved = Vec::new();
            for cell in 0..ROWS_PER_PAGE {
                if next() % 100 < 90 {
                    let slot = base + cell;
                    entries.push(Entry {
                        slot,
                        index: resolved.len() as u32,
                        clear: false,
                    });
                    resolved.push(Resolved {
                        id: EntityId(slot as i32 + 1),
                        src: Some((slot as i32 + 1, (next() % 1024) as i32)),
                        slot: Some(slot),
                    });
                }
            }
            PageCase {
                old: Some(old),
                page,
                entries,
                resolved,
            }
        })
        .collect();
    let project = |src: (i32, i32)| project_case(&src);
    let run = |streaming: bool| {
        let start = std::time::Instant::now();
        let mut sink = 0;
        for _ in 0..20 {
            for case in &cases {
                let old = case.old.as_deref();
                let out = if streaming {
                    build_page(old, case.page, &case.entries, &case.resolved, &project)
                } else {
                    build_page_reference(old, case.page, &case.entries, &case.resolved, &project)
                };
                sink += out.changed.len();
            }
        }
        assert!(sink > 0);
        start.elapsed().as_secs_f64() * 1000.0 / 20.0
    };
    for streaming in [false, true, false, true] {
        println!(
            "{}: {:.3} ms per 2048 pages",
            if streaming { "streaming" } else { "reference" },
            run(streaming)
        );
    }
}
