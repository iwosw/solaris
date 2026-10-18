use super::*;

fn chat(message: &str) -> PlayerChange {
    PlayerChange::Chat {
        message: message.into(),
    }
}

#[test]
fn reliable_handoff_coalesces_fifo_batches_and_keeps_disconnect_last() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let outbox = ReliableOutbox(parking_lot::Mutex::new(receiver));
    let first = ConnectionId(1);
    let second = ConnectionId(2);
    sender
        .send(vec![
            (first, vec![chat("one")]),
            (second, vec![chat("other")]),
        ])
        .unwrap();
    sender
        .send(vec![(
            first,
            vec![
                chat("two"),
                PlayerChange::Disconnect {
                    reason: "sealed".into(),
                },
            ],
        )])
        .unwrap();
    sender
        .send(vec![(first, vec![chat("not delivered")])])
        .unwrap();
    // A stopped authority leaves its terminal batch available to the edge.
    drop(sender);
    let queues: HashMap<_, _> = outbox.drain().into_iter().collect();
    assert_eq!(
        queues[&first],
        vec![
            chat("one"),
            chat("two"),
            PlayerChange::Disconnect {
                reason: "sealed".into()
            }
        ]
    );
    assert_eq!(queues[&second], vec![chat("other")]);
    assert!(outbox.drain().is_empty());
}

#[test]
fn draining_does_not_consume_later_batches() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let outbox = ReliableOutbox(parking_lot::Mutex::new(receiver));
    let connection = ConnectionId(1);
    sender
        .send(vec![(connection, vec![chat("first")])])
        .unwrap();
    assert_eq!(outbox.drain(), vec![(connection, vec![chat("first")])]);
    sender
        .send(vec![(connection, vec![chat("later")])])
        .unwrap();
    assert_eq!(outbox.drain(), vec![(connection, vec![chat("later")])]);
}
