use super::*;

#[test]
fn moderation_selector_uses_native_player_filters_and_source() {
    let (state, _, _) = crate::hostile_tests::setup(0.0);
    let console = crate::command_console::CONNECTION;
    let all = resolve(&state, console, "@a").unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0, ConnectionId(1));
    assert_eq!(resolve(&state, console, "@e[type=player]").unwrap(), all);
    assert!(
        resolve(&state, console, "@a[name=Nobody]")
            .unwrap()
            .is_empty()
    );
    assert!(resolve(&state, console, "@s").unwrap().is_empty());
    assert_eq!(resolve(&state, ConnectionId(1), "@s").unwrap(), all);
    assert!(resolve(&state, console, "@e").is_err());
    assert!(resolve(&state, console, "@n").is_err());
    assert!(resolve(&state, console, "@bogus").is_err());
}
