use super::*;

#[test]
fn empty_roster_reads_and_direct_admission_do_not_wait_for_the_writer() {
    let authority = Arc::new(PrecommitAuthority::new());
    let _writer = authority.writer.lock().unwrap();
    assert!(!authority.has(HookKind::Build));
    assert!(!authority.has(HookKind::Damage));
    assert_eq!(authority.generation(), 0);
    let context = HookContext::Build(
        BuildContext::try_new(HookActor::Environment, "minecraft:overworld", Vec::new()).unwrap(),
    );
    let mut pending = begin(
        &authority,
        context,
        Instant::now() + Duration::from_secs(30),
        None,
        |_| panic!("an empty roster must not publish a request"),
    )
    .unwrap();
    let mut approval = pending.try_resolve().unwrap().unwrap();
    assert_eq!(approval.consume(), Ok(HookDecision::Keep));
    assert_eq!(authority.outstanding.load(Ordering::Acquire), 0);
}

#[test]
fn concurrent_terminal_permit_releases_and_drop_return_exactly_one_slot() {
    let authority = PrecommitAuthority::new();
    let permit = authority.reserve().unwrap();
    assert_eq!(authority.outstanding.load(Ordering::Acquire), 1);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| permit.release());
        }
    });
    assert_eq!(authority.outstanding.load(Ordering::Acquire), 0);
    drop(permit);
    assert_eq!(authority.outstanding.load(Ordering::Acquire), 0);
    let abandoned = authority.reserve().unwrap();
    assert_eq!(authority.outstanding.load(Ordering::Acquire), 1);
    drop(abandoned);
    assert_eq!(authority.outstanding.load(Ordering::Acquire), 0);
}
