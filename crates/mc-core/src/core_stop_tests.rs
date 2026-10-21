use super::*;
use std::io::Write;

#[derive(Clone, Default)]
struct Log(std::sync::Arc<parking_lot::Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture(action: impl FnOnce()) -> String {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, action);
    String::from_utf8(log.0.lock().clone()).unwrap()
}

#[test]
fn native_writer_channel_close_logs_stop_cause() {
    let root = tempfile::tempdir().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    drop(sender);
    let log = capture(|| {
        native_writer_loop(
            root.path(),
            receiver,
            &std::sync::atomic::AtomicBool::new(false),
            &AtomicU64::new(0),
            &std::sync::atomic::AtomicBool::new(false),
            CheckpointManifest::default(),
        );
    });
    assert!(
        log.contains("ERROR"),
        "missing error-level stop record: {log}"
    );
    assert!(log.contains("native checkpoint writer stopped"), "{log}");
    assert!(log.contains("journal work channel closed"), "{log}");
}

#[test]
fn panic_hook_logs_message_and_source_location() {
    stop::install_panic_hook();
    let log = capture(|| {
        assert!(std::panic::catch_unwind(|| panic!("tick owner test panic")).is_err());
    });
    assert!(log.contains("ERROR"), "{log}");
    assert!(log.contains("core process panic"), "{log}");
    assert!(log.contains("tick owner test panic"), "{log}");
    assert!(log.contains("core_stop_tests.rs:"), "{log}");
}

#[test]
fn failed_owner_join_logs_payload() {
    let owner = std::thread::Builder::new()
        .name("mc-core-test-owner".into())
        .spawn(|| panic!("owner join test panic"))
        .unwrap();
    let log = capture(|| stop::join(owner));
    assert!(log.contains("ERROR"), "{log}");
    assert!(log.contains("mc-core-test-owner"), "{log}");
    assert!(log.contains("owner join test panic"), "{log}");
    assert!(log.contains("core owner join failed"), "{log}");
}

#[test]
fn stop_error_display_includes_source_chain() {
    #[derive(Debug, thiserror::Error)]
    #[error("journal write failed")]
    struct Failure(#[source] std::io::Error);

    let error = Failure(std::io::Error::other("disk full"));
    assert_eq!(
        stop::Chain(&error).to_string(),
        "journal write failed: disk full"
    );
}
