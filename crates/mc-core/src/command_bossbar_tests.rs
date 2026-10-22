//! `/bossbar` parse tests: all branches, bounds, bad ids.

use super::*;
use crate::bossbar::{BarColor, BarOverlay};

fn parse_full(command: &str) -> Result<BossBarCommand, CommandError> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let (_, args) = words.split_first().expect("root word");
    parse(args, command)
}

#[test]
fn add_keeps_component_tail_and_normalizes_id() {
    let command = parse_full(r#"bossbar add test:bar {"text":"Hi there"}"#).unwrap();
    assert_eq!(
        command,
        BossBarCommand::Add {
            id: "test:bar".into(),
            name: r#"{"text":"Hi there"}"#.into(),
        }
    );
    // Bare ids default to the minecraft namespace.
    let command = parse_full(r#"bossbar add bar {"text":"x"}"#).unwrap();
    assert!(matches!(command, BossBarCommand::Add { id, .. } if id == "minecraft:bar"));
}

#[test]
fn add_rejects_bad_ids_and_missing_name() {
    assert!(parse_full("bossbar add").is_err());
    assert!(parse_full("bossbar add Bad:ID").is_err());
    assert!(parse_full(r#"bossbar add ok:bar"#).is_err());
    assert!(parse_full(r#"bossbar add ok:bar {"text":}"#).is_err());
}

#[test]
fn remove_list_shapes() {
    assert_eq!(
        parse_full("bossbar remove foo:bar").unwrap(),
        BossBarCommand::Remove {
            id: "foo:bar".into()
        }
    );
    assert!(parse_full("bossbar remove").is_err());
    assert!(parse_full("bossbar remove a b").is_err());
    assert_eq!(parse_full("bossbar list").unwrap(), BossBarCommand::List);
    assert!(parse_full("bossbar list extra").is_err());
    assert!(parse_full("bossbar").is_err());
    assert!(parse_full("bossbar frobnicate").is_err());
}

#[test]
fn set_name_keeps_component_tail() {
    let command = parse_full(r#"bossbar set m:b name {"text":"New","color":"red"}"#).unwrap();
    assert_eq!(
        command,
        BossBarCommand::SetName {
            id: "m:b".into(),
            name: r#"{"text":"New","color":"red"}"#.into(),
        }
    );
    assert!(parse_full("bossbar set m:b name").is_err());
}

#[test]
fn set_color_accepts_all_vanilla_colors() {
    for (word, color) in [
        ("pink", BarColor::Pink),
        ("blue", BarColor::Blue),
        ("red", BarColor::Red),
        ("green", BarColor::Green),
        ("yellow", BarColor::Yellow),
        ("purple", BarColor::Purple),
        ("white", BarColor::White),
    ] {
        assert_eq!(
            parse_full(&format!("bossbar set m:b color {word}")).unwrap(),
            BossBarCommand::SetColor {
                id: "m:b".into(),
                color,
            }
        );
    }
    assert!(parse_full("bossbar set m:b color orange").is_err());
    assert!(parse_full("bossbar set m:b color").is_err());
}

#[test]
fn set_style_accepts_all_vanilla_styles() {
    for (word, overlay) in [
        ("progress", BarOverlay::Progress),
        ("notched_6", BarOverlay::Notched6),
        ("notched_10", BarOverlay::Notched10),
        ("notched_12", BarOverlay::Notched12),
        ("notched_20", BarOverlay::Notched20),
    ] {
        assert_eq!(
            parse_full(&format!("bossbar set m:b style {word}")).unwrap(),
            BossBarCommand::SetStyle {
                id: "m:b".into(),
                overlay,
            }
        );
    }
    assert!(parse_full("bossbar set m:b style dashed").is_err());
    assert!(parse_full("bossbar set m:b style").is_err());
}

#[test]
fn set_value_max_visible_bounds() {
    assert_eq!(
        parse_full("bossbar set m:b value 37").unwrap(),
        BossBarCommand::SetValue {
            id: "m:b".into(),
            value: 37,
        }
    );
    assert!(parse_full("bossbar set m:b value -1").is_err());
    assert!(parse_full("bossbar set m:b value many").is_err());
    assert_eq!(
        parse_full("bossbar set m:b max 200").unwrap(),
        BossBarCommand::SetMax {
            id: "m:b".into(),
            max: 200,
        }
    );
    assert!(parse_full("bossbar set m:b max 0").is_err());
    assert_eq!(
        parse_full("bossbar set m:b visible false").unwrap(),
        BossBarCommand::SetVisible {
            id: "m:b".into(),
            visible: false,
        }
    );
    assert!(parse_full("bossbar set m:b visible yes").is_err());
    assert!(parse_full("bossbar set m:b bogus x").is_err());
}

#[test]
fn set_players_shapes() {
    assert_eq!(
        parse_full("bossbar set m:b players").unwrap(),
        BossBarCommand::SetPlayers {
            id: "m:b".into(),
            targets: None,
        }
    );
    assert_eq!(
        parse_full("bossbar set m:b players @a").unwrap(),
        BossBarCommand::SetPlayers {
            id: "m:b".into(),
            targets: Some("@a".into()),
        }
    );
    assert_eq!(
        parse_full("bossbar set m:b players Alice").unwrap(),
        BossBarCommand::SetPlayers {
            id: "m:b".into(),
            targets: Some("Alice".into()),
        }
    );
    assert!(parse_full("bossbar set m:b players @a extra").is_err());
}

#[test]
fn get_shapes() {
    for field in ["value", "max", "visible", "players"] {
        let command = parse_full(&format!("bossbar get m:b {field}")).unwrap();
        let expected = match field {
            "value" => BossBarCommand::GetValue { id: "m:b".into() },
            "max" => BossBarCommand::GetMax { id: "m:b".into() },
            "visible" => BossBarCommand::GetVisible { id: "m:b".into() },
            _ => BossBarCommand::GetPlayers { id: "m:b".into() },
        };
        assert_eq!(command, expected);
    }
    assert!(parse_full("bossbar get m:b").is_err());
    assert!(parse_full("bossbar get m:b everything").is_err());
}
