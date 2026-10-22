//! Item presentation for clients that cannot resolve non-vanilla item models.
//!
//! A connection that finished Configuration without a Loader session is marked
//! here. Packets planned for it encode item stacks without any
//! `minecraft:item_model` outside the `minecraft` namespace
//! (`mc_protocol::packets::play::with_vanilla_item_models`), so a plugin item such
//! as the chisel shows as its carrier item with its custom name instead of the
//! vanilla client's missing-model cube. Server-side identity is untouched.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

use bytes::BufMut;
use mc_core::boundary::ConnectionId;
use mc_protocol::CodecError;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::with_vanilla_item_models;

static MARKED: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(Mutex::default);
/// Lets the common all-Loader case skip the lock on every planned packet.
static MARKED_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn mark(connection: ConnectionId) {
    let mut marked = MARKED.lock().expect("vanilla item connection set");
    if marked.insert(connection.0) {
        MARKED_COUNT.store(marked.len(), Ordering::Release);
    }
}

pub(crate) fn unmark(connection: ConnectionId) {
    if MARKED_COUNT.load(Ordering::Acquire) == 0 {
        return;
    }
    let mut marked = MARKED.lock().expect("vanilla item connection set");
    if marked.remove(&connection.0) {
        MARKED_COUNT.store(marked.len(), Ordering::Release);
    }
}

fn marked(connection: ConnectionId) -> bool {
    MARKED_COUNT.load(Ordering::Acquire) != 0
        && MARKED
            .lock()
            .expect("vanilla item connection set")
            .contains(&connection.0)
}

/// Encode `packet` for `connection` under its item-model presentation.
pub(crate) fn encode<P: Packet, B: BufMut>(
    connection: ConnectionId,
    packet: &P,
    body: &mut B,
) -> Result<(), CodecError> {
    with_vanilla_item_models(marked(connection), || packet.encode(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::ItemStack;
    use mc_protocol::packets::play::ClientboundSetCursorItem;

    fn cursor(model: &str) -> ClientboundSetCursorItem {
        ClientboundSetCursorItem {
            item_stack: ItemStack::new(1, 1)
                .with_item_model(mc_data::Identifier::parse(model).unwrap())
                .with_custom_name("Chisel"),
        }
    }

    fn bytes(connection: ConnectionId, packet: &ClientboundSetCursorItem) -> Vec<u8> {
        let mut out = Vec::new();
        encode(connection, packet, &mut out).unwrap();
        out
    }

    #[test]
    fn non_vanilla_models_are_omitted_only_for_marked_connections() {
        let (plain, marked_id) = (ConnectionId(9_000_001), ConnectionId(9_000_002));
        let chisel = cursor("chisel:chisel");
        let full = bytes(plain, &chisel);
        mark(marked_id);
        let stripped = bytes(marked_id, &chisel);
        assert!(
            stripped.len() < full.len(),
            "the model component is omitted"
        );
        assert_eq!(
            full,
            bytes(plain, &chisel),
            "other connections are unaffected"
        );
        let vanilla = cursor("minecraft:diamond");
        assert_eq!(
            bytes(plain, &vanilla),
            bytes(marked_id, &vanilla),
            "a vanilla model id still resolves on a vanilla client"
        );
        unmark(marked_id);
        assert_eq!(
            bytes(marked_id, &chisel),
            full,
            "unmarking restores the model"
        );
    }
}
