use crate::command_tree::{help, permitted, suggestions, tree};
use mc_protocol::packets::play::{CommandArgumentParser as Parser, CommandNodeKind};

#[test]
fn settlement_is_native_player_command_with_typed_arguments_and_help() {
    assert!(permitted("/settlement list", 0, &[]));
    let packet = tree(0, &[]);
    let node = |path: &[&str]| {
        let mut index = packet.root_index;
        for part in path {
            index = *packet.nodes[index as usize]
                .children
                .iter()
                .find(|&&child| match &packet.nodes[child as usize].kind {
                    CommandNodeKind::Literal(name) | CommandNodeKind::Argument { name, .. } => {
                        name == part
                    }
                    CommandNodeKind::Root => false,
                })
                .unwrap_or_else(|| panic!("missing settlement branch {path:?}"));
        }
        &packet.nodes[index as usize]
    };
    assert!(node(&["settlement", "guide"]).executable);
    assert!(node(&["settlement", "list"]).executable);
    assert!(node(&["settlement", "info"]).executable);
    assert!(node(&["settlement", "bind", "settlement"]).executable);
    assert!(node(&["settlement", "unbind"]).executable);
    assert!(node(&["settlement", "create", "name"]).executable);
    assert!(matches!(
        node(&["settlement", "create", "name", "radius"]).kind,
        CommandNodeKind::Argument {
            parser: Parser::Integer {
                min: Some(1),
                max: Some(256)
            },
            ..
        }
    ));
    assert!(matches!(
        &node(&["settlement", "bind", "settlement"]).kind,
        CommandNodeKind::Argument {
            parser: Parser::String(_),
            suggestions: None,
            ..
        }
    ));
    assert!(matches!(
        node(&["settlement", "bind", "settlement", "villager"]).kind,
        CommandNodeKind::Argument {
            parser: Parser::Uuid,
            ..
        }
    ));
    for domain in ["production", "commerce", "payroll", "caravan"] {
        assert!(node(&["settlement", domain, "settlement", "status"]).executable);
    }
    for (domain, action) in [
        ("production", "start"),
        ("commerce", "quote"),
        ("payroll", "employ"),
        ("caravan", "open"),
    ] {
        let action = node(&["settlement", domain, "settlement", action]);
        let arguments = &packet.nodes[action.children[0] as usize];
        assert!(arguments.executable);
        assert!(matches!(
            arguments.kind,
            CommandNodeKind::Argument {
                parser: Parser::String(mc_protocol::packets::play::CommandStringKind::GreedyPhrase),
                ..
            }
        ));
    }
    let help = help(Some("settlement"), 0, &[]).unwrap();
    for sub in [
        "guide",
        "create",
        "list",
        "info",
        "rename",
        "remove",
        "bind",
        "unbind",
        "relocate",
        "production",
        "commerce",
        "payroll",
        "caravan",
    ] {
        assert!(help.iter().any(|line| line.starts_with(&format!("/settlement {sub}")) && line.contains(" — ")), "missing description for {sub}: {help:?}");
    }
    assert!(
        help.iter()
            .any(|line| line.contains("2000-8999") && line.contains("48"))
    );
}

#[test]
fn settlement_completion_covers_subcommands_without_name_lookup() {
    let complete = |input: &str| {
        suggestions(3, input, 0, &[], &[], &[], &[], &[])
            .suggestions
            .into_iter()
            .map(|entry| entry.text)
            .collect::<Vec<_>>()
    };
    assert_eq!(complete("/sett"), ["settlement"]);
    assert!(complete("/settlement ").contains(&"create".to_owned()));
    assert_eq!(complete("/settlement gui"), ["guide"]);
    assert!(complete("/settlement info O").is_empty());
    assert!(complete("/settlement create O").is_empty());
    assert!(complete("/settlement rename Oak O").is_empty());
    assert!(complete("/settlement unbind O").is_empty());
    assert!(complete("/settlement caravan Oak ").contains(&"status".to_owned()));
}
