use super::*;

#[test]
fn sessions_remain_visible_after_a_writer_unwinds() {
    let sessions = Sessions::default();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut entries = sessions.0.write();
        entries.insert("player".into(), 1);
        panic!("interrupted session update");
    }));
    assert_eq!(sessions.session_of("player"), Some(1));
    assert_eq!(sessions.session_of("not connected"), None);
    sessions.0.write().insert("later".into(), 2);
    assert_eq!(sessions.session_of("later"), Some(2));
}
