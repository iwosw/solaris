use super::*;
use std::num::NonZeroUsize;

fn batch(count: usize) -> CommandBatch {
    let mut batch = CommandBatch::new(NonZeroUsize::new(count).unwrap());
    for index in 0..count {
        batch
            .try_push(ScriptCommand::BroadcastChatMessage {
                message: index.to_string(),
            })
            .unwrap();
    }
    batch
}

#[test]
fn unpublished_capacity_refusal_returns_the_exact_original_batch() {
    let ledger = Arc::new(HostAdmissionLedger::default());
    let mut issued = Vec::new();
    for _ in 0..MAX_SCRIPT_COMMAND_QUEUE_CAPACITY / 32 {
        issued.extend(
            ledger
                .issue(Arc::from("owner"), batch(32), Some(7))
                .unwrap(),
        );
    }
    let refused = batch(2);
    assert_eq!(
        ledger.issue(Arc::from("owner"), refused.clone(), Some(7)),
        Err(refused)
    );
    assert_eq!(ledger.pending_len(), MAX_SCRIPT_COMMAND_QUEUE_CAPACITY);
    drop(issued.pop());
    assert_eq!(ledger.pending_len(), MAX_SCRIPT_COMMAND_QUEUE_CAPACITY - 1);
    let recovered = ledger.issue(Arc::from("owner"), batch(1), Some(7)).unwrap();
    drop(recovered);
    drop(issued);
    assert_eq!(ledger.pending_len(), 0);
}

#[test]
fn nonce_exhaustion_refuses_without_publishing_or_changing_the_batch() {
    let ledger = Arc::new(HostAdmissionLedger::default());
    ledger.next_nonce.store(u64::MAX, Ordering::Relaxed);
    let refused = batch(1);
    assert_eq!(
        ledger.issue(Arc::from("owner"), refused.clone(), None),
        Err(refused)
    );
    assert_eq!(ledger.pending_len(), 0);
}
