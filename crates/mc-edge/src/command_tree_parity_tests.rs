use super::*;
use mc_protocol::Packet;

fn advertised<'a>(packet: &'a ClientboundCommands, path: &[&str]) -> &'a CommandNode {
    let mut node = &packet.nodes[packet.root_index as usize];
    for name in path {
        node = node
            .children
            .iter()
            .map(|i| &packet.nodes[*i as usize])
            .find(|node| match &node.kind {
                CommandNodeKind::Literal(value) => value == name,
                CommandNodeKind::Argument { name: value, .. } => value == name,
                CommandNodeKind::Root => false,
            })
            .unwrap();
    }
    node
}

#[test]
fn border_summon_nbt_and_distinct_time_queries_use_exact_packet_parsers() {
    let packet = tree(2, &[]);
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    let mut input = bytes.as_slice();
    let decoded = ClientboundCommands::decode(&mut input).unwrap();
    assert!(input.is_empty());
    for (path, expected) in [
        (&["worldborder", "center", "pos"][..], Parser::Vec2),
        (
            &["summon", "entity", "pos", "nbt"][..],
            Parser::NbtCompoundTag,
        ),
        (
            &["worldborder", "set", "distance"][..],
            Parser::Double {
                min: Some(1.0_f64.to_bits()),
                max: Some(59_999_968.0_f64.to_bits()),
            },
        ),
    ] {
        let node = advertised(&decoded, path);
        assert!(node.executable);
        let CommandNodeKind::Argument { parser, .. } = &node.kind else {
            panic!("argument");
        };
        assert_eq!(*parser, expected);
    }
    for query in ["time", "daytime", "day", "gametime"] {
        assert!(advertised(&decoded, &["time", "query", query]).executable);
    }
    assert!(advertised(&decoded, &["worldborder", "get"]).executable);
    assert!(permitted("worldborder set 50", 2, &[]));
    assert!(!permitted("worldborder set 50", 1, &[]));
}

#[test]
fn dimension_execution_advertises_only_the_supported_teleport_subset() {
    let packet = tree(2, &[]);
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    let mut input = bytes.as_slice();
    let decoded = ClientboundCommands::decode(&mut input).unwrap();
    assert!(input.is_empty());
    let dimension = advertised(&decoded, &["execute", "in", "dimension"]);
    assert!(matches!(
        &dimension.kind,
        CommandNodeKind::Argument {
            parser: Parser::Dimension,
            ..
        }
    ));
    let run = advertised(&decoded, &["execute", "in", "dimension", "run"]);
    assert_eq!(run.children.len(), 2);
    for name in ["tp", "teleport"] {
        let command = advertised(&decoded, &["execute", "in", "dimension", "run", name]);
        let target = &decoded.nodes[command.redirect.unwrap() as usize];
        assert!(matches!(&target.kind, CommandNodeKind::Literal(name) if name == "teleport"));
    }
    assert!(permitted(
        "execute in minecraft:the_nether run tp @s 0 80 0",
        2,
        &[]
    ));
    assert!(!permitted(
        "execute in minecraft:the_nether run tp @s 0 80 0",
        1,
        &[]
    ));
}
