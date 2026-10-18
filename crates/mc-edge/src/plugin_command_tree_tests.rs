use crate::command_tree::{help, permitted, suggestions, tree};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, CommandArgumentParser, CommandNodeKind, CommandStringKind,
};

#[test]
fn loaded_plugin_roots_share_permissions_tree_help_and_suggestions() {
    let plugins = vec![("settlement".to_owned(), 0), ("audit".to_owned(), 2)];
    assert!(permitted("/settlement info oak", 0, &plugins));
    assert!(!permitted("audit inspect", 0, &plugins));
    assert!(permitted("audit inspect", 2, &plugins));
    assert!(!permitted("settlement info oak", 4, &[]));

    let packet = tree(0, &plugins);
    let roots: Vec<_> = packet.nodes[packet.root_index as usize]
        .children
        .iter()
        .filter_map(|&index| match &packet.nodes[index as usize].kind {
            CommandNodeKind::Literal(root) => Some(root.as_str()),
            _ => None,
        })
        .collect();
    assert!(roots.contains(&"settlement"));
    assert!(!roots.contains(&"audit"));
    let node = packet
        .nodes
        .iter()
        .find(|node| matches!(&node.kind, CommandNodeKind::Literal(root) if root == "settlement"))
        .unwrap();
    assert!(node.executable);
    assert!(matches!(
        packet.nodes[node.children[0] as usize].kind,
        CommandNodeKind::Argument {
            parser: CommandArgumentParser::String(CommandStringKind::GreedyPhrase),
            ..
        }
    ));
    assert!(
        help(None, 0, &plugins)
            .unwrap()
            .iter()
            .any(|line| line.starts_with("/settlement"))
    );
    let completions = suggestions(7, "/sett", 0, &[], &[], &[], &[], &plugins);
    assert_eq!(completions.suggestions[0].text, "settlement");
    assert!(
        suggestions(8, "/aud", 0, &[], &[], &[], &[], &plugins)
            .suggestions
            .is_empty()
    );

    let mut encoded = Vec::new();
    packet.encode(&mut encoded).unwrap();
    let decoded = ClientboundCommands::decode(&mut encoded.as_slice()).unwrap();
    assert_eq!(decoded, packet);
}

#[test]
fn plugin_directory_config_reaches_authoritative_core_without_path_reinterpretation() {
    let config = crate::config::EdgeConfig::from_toml(
        "[plugins]\ndirectory = 'packages'\n",
        Some(std::path::Path::new("config/server.toml")),
    )
    .unwrap();
    assert_eq!(
        config.plugins.directory.as_deref(),
        Some(std::path::Path::new("packages"))
    );
    assert_eq!(
        crate::server::core_config(&config).plugin_directory,
        config.plugins.directory
    );
    assert!(
        crate::config::EdgeConfig::default()
            .plugins
            .directory
            .is_none()
    );
}

#[test]
fn manifest_owned_native_literals_have_one_permission_filtered_plugin_branch() {
    let plugins = vec![("msg".to_owned(), 2), ("list".to_owned(), 0)];
    assert!(!permitted("/msg NativePlayer hello", 0, &plugins));
    assert!(permitted("/msg NativePlayer hello", 2, &plugins));
    assert!(permitted("tell NativePlayer hello", 0, &plugins));
    assert!(help(Some("msg"), 0, &plugins).is_err());
    assert_eq!(
        help(Some("msg"), 2, &plugins).unwrap(),
        ["/msg [<arguments>]"]
    );
    assert!(
        help(Some("list"), 0, &plugins)
            .unwrap()
            .iter()
            .all(|usage| !usage.contains("uuids"))
    );
    let packet = tree(2, &plugins);
    let roots: Vec<_> = packet.nodes[packet.root_index as usize]
        .children
        .iter()
        .map(|&index| &packet.nodes[index as usize])
        .collect();
    let msg: Vec<_> = roots
        .iter()
        .filter(|node| matches!(&node.kind, CommandNodeKind::Literal(root) if root == "msg"))
        .collect();
    assert_eq!(msg.len(), 1);
    assert!(msg[0].executable);
    assert!(matches!(&packet.nodes[msg[0].children[0] as usize].kind,
        CommandNodeKind::Argument {
            name, parser: CommandArgumentParser::String(CommandStringKind::GreedyPhrase), ..
        } if name == "arguments"));
    assert!(roots.iter().any(|node| matches!(&node.kind,
        CommandNodeKind::Literal(root) if root == "tell")
        && node.redirect.is_some()));
    assert!(
        suggestions(1, "/msg", 0, &[], &[], &[], &[], &plugins)
            .suggestions
            .is_empty()
    );
    assert_eq!(
        suggestions(2, "/msg", 2, &[], &[], &[], &[], &plugins)
            .suggestions
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>(),
        ["msg"]
    );
    assert!(
        suggestions(
            3,
            "/msg Native",
            2,
            &["NativePlayer".into()],
            &[],
            &[],
            &[],
            &plugins
        )
        .suggestions
        .is_empty()
    );
}
