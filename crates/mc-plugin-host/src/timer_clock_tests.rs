use super::{TimerRefusal, TimerSchedule};

#[test]
fn restored_clock_preserves_relative_deadlines_and_tie_order() {
    let mut schedule = TimerSchedule::default();
    schedule.schedule("later".into(), 5).unwrap();
    schedule.schedule("b".into(), 1).unwrap();
    schedule.schedule("a".into(), 1).unwrap();
    schedule.rebase_deadlines(10_000).unwrap();
    assert_eq!(schedule.next_due(10_000), None);
    assert_eq!(schedule.take_next_due(10_001), Some((10_001, "a".into())));
    assert_eq!(schedule.take_next_due(10_001), Some((10_001, "b".into())));
    assert_eq!(schedule.next_due(10_004), None);
    assert!(schedule.cancel("later"));
    assert_eq!(schedule.next_due(u64::MAX), None);
}

#[test]
fn initial_zero_clock_keeps_the_schedule_unchanged() {
    let mut schedule = TimerSchedule::default();
    schedule.schedule("first".into(), 1).unwrap();
    schedule.rebase_deadlines(0).unwrap();
    assert_eq!(schedule.next_due(0), None);
    assert_eq!(schedule.take_next_due(1), Some((1, "first".into())));
}

#[test]
fn restored_clock_overflow_changes_neither_timer_index() {
    let mut schedule = TimerSchedule::default();
    schedule.schedule("first".into(), 1).unwrap();
    schedule.schedule("overflow".into(), 10).unwrap();
    assert_eq!(
        schedule.rebase_deadlines(u64::MAX - 5),
        Err(TimerRefusal::DeadlineOverflow)
    );
    assert_eq!(schedule.take_next_due(1), Some((1, "first".into())));
    assert!(schedule.cancel("overflow"));
    assert_eq!(schedule.next_due(u64::MAX), None);
}
