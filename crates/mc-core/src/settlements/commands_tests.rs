//! Native `/settlement` parse tests: every S1 subcommand shape and refusal.
use crate::settlements::commands::{DEFAULT_RADIUS, SettlementCommand, parse};

#[test]
fn create_defaults_to_registry_radius_and_bounds_checks() {
    assert_eq!(
        parse(&["create", "town"]).unwrap(),
        SettlementCommand::Create {
            name: "town".into(),
            radius: DEFAULT_RADIUS,
        }
    );
    assert_eq!(DEFAULT_RADIUS, 48);
    assert_eq!(
        parse(&["create", "town", "16"]).unwrap(),
        SettlementCommand::Create {
            name: "town".into(),
            radius: 16,
        }
    );
    assert!(parse(&["create", "town", "0"]).is_err());
    assert!(parse(&["create", "town", "257"]).is_err());
    assert!(parse(&["create", "town", "wide"]).is_err());
    assert!(parse(&["create"]).is_err());
    assert!(parse(&["create", "a", "b", "c"]).is_err());
}

#[test]
fn reads_parse_without_arguments_beyond_optional_name() {
    assert_eq!(parse(&["list"]).unwrap(), SettlementCommand::List);
    assert!(parse(&["list", "town"]).is_err());
    assert_eq!(
        parse(&["info"]).unwrap(),
        SettlementCommand::Info { name: None }
    );
    assert_eq!(
        parse(&["info", "town"]).unwrap(),
        SettlementCommand::Info {
            name: Some("town".into())
        }
    );
    assert!(parse(&["info", "a", "b"]).is_err());
}

#[test]
fn mutations_require_exact_names_and_uuids_shape() {
    assert_eq!(
        parse(&["rename", "old", "new"]).unwrap(),
        SettlementCommand::Rename {
            name: "old".into(),
            new_name: "new".into(),
        }
    );
    assert!(parse(&["rename", "only"]).is_err());
    assert_eq!(
        parse(&["remove", "town"]).unwrap(),
        SettlementCommand::Remove {
            name: "town".into()
        }
    );
    assert!(parse(&["remove"]).is_err());
    assert_eq!(
        parse(&["bind", "town", "12345678-1234-1234-1234-123456789012"]).unwrap(),
        SettlementCommand::Bind {
            settlement: "town".into(),
            villager: Some("12345678-1234-1234-1234-123456789012".into()),
        }
    );
    assert_eq!(
        parse(&["bind", "town"]).unwrap(),
        SettlementCommand::Bind {
            settlement: "town".into(),
            villager: None,
        }
    );
    assert_eq!(
        parse(&["unbind", "12345678-1234-1234-1234-123456789012"]).unwrap(),
        SettlementCommand::Unbind {
            villager: Some("12345678-1234-1234-1234-123456789012".into()),
        }
    );
    assert_eq!(
        parse(&["unbind"]).unwrap(),
        SettlementCommand::Unbind { villager: None }
    );
    assert!(parse(&["bind"]).is_err());
    assert!(parse(&["bind", "town", "uuid", "extra"]).is_err());
    assert!(parse(&["unbind", "uuid", "extra"]).is_err());
    assert!(parse(&[]).is_err());
    assert!(parse(&["raze", "town"]).is_err());
}

#[test]
fn root_dispatch_reaches_settlement_through_native_parse() {
    let command = crate::commands::parse("settlement list", false).unwrap();
    assert!(matches!(
        command,
        crate::commands::Command::Settlement(SettlementCommand::List)
    ));
    // Reads stay visible to non-operators; operator-only gating happens at
    // execution per record, not at parse.
    let command = crate::commands::parse("settlement info town", false).unwrap();
    assert!(matches!(
        command,
        crate::commands::Command::Settlement(SettlementCommand::Info { .. })
    ));
    assert!(crate::commands::parse("settlement raze town", true).is_err());
}

#[test]
fn household_relocation_requires_source_identity_and_destination() {
    assert_eq!(
        parse(&["relocate", "town", "1-1", "borough"]).unwrap(),
        SettlementCommand::Relocate {
            settlement: "town".into(),
            household: "1-1".into(),
            destination: "borough".into(),
        }
    );
    assert!(parse(&["relocate", "town", "1-1"]).is_err());
    assert!(parse(&["relocate", "town", "1-1", "borough", "extra"]).is_err());
}

#[test]
fn production_commerce_payroll_and_caravan_are_native_commands() {
    for domain in ["production", "commerce", "payroll", "caravan"] {
        let command = format!("settlement {domain} town status");
        assert!(
            crate::commands::parse(&command, false).is_ok(),
            "{domain} must route through native settlement commands"
        );
    }
}
