//! Stop-path diagnostics shared by the native core owners.

use std::any::Any;
use std::error::Error;
use std::fmt;

pub(super) fn install_panic_hook() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            tracing::error!(
                thread = thread.name().unwrap_or("unnamed"),
                panic = %info,
                "core process panic"
            );
            // Preserve the application's stderr/backtrace or custom panic hook.
            previous(info);
        }));
    });
}

pub(super) fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload")
}

pub(super) fn join(handle: std::thread::JoinHandle<()>) {
    let thread = handle.thread().clone();
    if let Err(payload) = handle.join() {
        tracing::error!(
            thread = thread.name().unwrap_or("unnamed"),
            panic = panic_message(&*payload),
            "core owner join failed"
        );
    }
}

/// Display the complete causal chain without allocating an intermediate string.
pub(super) struct Chain<'a>(pub(super) &'a (dyn Error + 'static));

impl fmt::Display for Chain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)?;
        let mut source = self.0.source();
        while let Some(error) = source {
            write!(f, ": {error}")?;
            source = error.source();
        }
        Ok(())
    }
}
