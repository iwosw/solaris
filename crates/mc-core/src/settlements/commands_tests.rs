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
fn claim_uses_live_player_location_not_a_caller_supplied_target() {
    assert_eq!(parse(&["claim"]).unwrap(), SettlementCommand::Claim);
    assert!(parse(&["claim", "remote-village"]).is_err());
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
    assert!(parse(&["raze", "town"]).is_err());
}
#[test]
fn army_verbs_parse_with_target_and_order_words() {
    use super::{ArmyCommand, ArmyOrderKind};
    assert_eq!(
        parse(&["army", "list"]).unwrap(),
        SettlementCommand::Army {
            command: ArmyCommand::List
        }
    );
    assert_eq!(
        parse(&["army", "recruit"]).unwrap(),
        SettlementCommand::Army {
            command: ArmyCommand::Recruit
        }
    );
    let uuid = "12345678-1234-1234-1234-123456789012";
    assert_eq!(
        parse(&["army", "dismiss", uuid]).unwrap(),
        SettlementCommand::Army {
            command: ArmyCommand::Dismiss {
                soldier: uuid.into()
            }
        }
    );
    for (word, kind) in [
        ("follow", ArmyOrderKind::Follow),
        ("guard", ArmyOrderKind::Guard),
        ("patrol", ArmyOrderKind::Patrol),
        ("return", ArmyOrderKind::Return),
    ] {
        assert_eq!(
            parse(&["army", "order", "all", word]).unwrap(),
            SettlementCommand::Army {
                command: ArmyCommand::Order {
                    target: "all".into(),
                    order: kind
                }
            }
        );
        assert_eq!(
            parse(&["army", "order", uuid, word]).unwrap(),
            SettlementCommand::Army {
                command: ArmyCommand::Order {
                    target: uuid.into(),
                    order: kind
                }
            }
        );
    }
    assert!(parse(&["army"]).is_err());
    assert!(parse(&["army", "list", "extra"]).is_err());
    assert!(parse(&["army", "recruit", "extra"]).is_err());
    assert!(parse(&["army", "dismiss"]).is_err());
    assert!(parse(&["army", "dismiss", "a", "b"]).is_err());
    assert!(parse(&["army", "order", "all"]).is_err());
    assert!(parse(&["army", "order", "all", "charge"]).is_err());
    assert!(parse(&["army", "order", "all", "patrol", "extra"]).is_err());
    assert!(parse(&["army", "enlist"]).is_err());
    let command = crate::commands::parse("settlement army list", false).unwrap();
    assert!(matches!(
        command,
        crate::commands::Command::Settlement(SettlementCommand::Army { .. })
    ));
}

#[test]
fn building_commands_parse_success_and_refusals() {
    assert_eq!(
        parse(&[
            "building", "certify", "house", "0", "64", "0", "4", "66", "4"
        ])
        .unwrap(),
        SettlementCommand::BuildingCertify {
            kind: "house".into(),
            min: [0, 64, 0],
            max: [4, 66, 4],
        }
    );
    assert_eq!(
        parse(&["building", "upgrade", "4294967297"]).unwrap(),
        SettlementCommand::BuildingUpgrade { id: 4_294_967_297 }
    );
    assert_eq!(
        parse(&["building", "list"]).unwrap(),
        SettlementCommand::BuildingList
    );
    assert!(parse(&["building"]).is_err());
    assert!(parse(&["building", "raze"]).is_err());
    assert!(parse(&["building", "certify", "house", "0", "64", "0", "4", "66"]).is_err());
    assert!(
        parse(&[
            "building", "certify", "house", "0", "64", "0", "4", "66", "4", "extra"
        ])
        .is_err()
    );
    assert!(
        parse(&[
            "building", "certify", "house", "x", "64", "0", "4", "66", "4"
        ])
        .is_err()
    );
    assert!(parse(&["building", "upgrade"]).is_err());
    assert!(parse(&["building", "upgrade", "nope"]).is_err());
    assert!(parse(&["building", "upgrade", "1", "extra"]).is_err());
    assert!(parse(&["building", "list", "extra"]).is_err());
    let command = crate::commands::parse("settlement building list", false).unwrap();
    assert!(matches!(
        command,
        crate::commands::Command::Settlement(SettlementCommand::BuildingList)
    ));
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

#[test]
fn build_order_parses_blueprint_and_upgrade_and_refuses_shapes() {
    assert_eq!(
        parse(&["build", "house"]).unwrap(),
        SettlementCommand::Build {
            blueprint: "house".into(),
        }
    );
    assert_eq!(
        parse(&["build", "upgrade", "4294967297"]).unwrap(),
        SettlementCommand::BuildUpgrade { id: 4_294_967_297 }
    );
    assert!(parse(&["build"]).is_err());
    assert!(parse(&["build", "house", "extra"]).is_err());
    // `build upgrade` alone is blueprint "upgrade" (ids are free-form);
    // execution reports Unknown blueprint. Only a non-numeric upgrade id or
    // extra words fail at parse.
    assert_eq!(
        parse(&["build", "upgrade"]).unwrap(),
        SettlementCommand::Build {
            blueprint: "upgrade".into(),
        }
    );
    assert!(parse(&["build", "upgrade", "nope"]).is_err());
    assert!(parse(&["build", "upgrade", "1", "extra"]).is_err());
    assert!(parse(&["build", "certify", "house"]).is_err());
    let command = crate::commands::parse("settlement build house", false).unwrap();
    assert!(matches!(
        command,
        crate::commands::Command::Settlement(SettlementCommand::Build { .. })
    ));
    let command = crate::commands::parse("settlement build upgrade 7", true).unwrap();
    assert!(matches!(
        command,
        crate::commands::Command::Settlement(SettlementCommand::BuildUpgrade { .. })
    ));
}
