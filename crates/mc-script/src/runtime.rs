//! Runtime contract: the controls and context a host hands to a script runtime,
//! the error type it returns, and the minimal trait a future engine implements.
//!
//! Split out of `lib.rs` by CP-REFACTOR-040; the crate root re-exports these
//! items, so callers keep using `mc_script::{...}`.

use std::num::{NonZeroU64, NonZeroUsize};
use std::time::Duration;

use crate::{CommandBatch, MAX_SCRIPT_COMMAND_BATCH, ScriptEvent};

/// Future VM resource and lifecycle controls reserved in the host contract.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeControls {
    fuel: Option<NonZeroU64>,
    memory_bytes: Option<NonZeroUsize>,
    timeout: Option<Duration>,
    shutdown_requested: bool,
}

impl RuntimeControls {
    /// Create controls with no active limits or shutdown request.
    pub fn unrestricted() -> Self {
        Self::default()
    }

    /// Set the maximum instruction/fuel budget for a future VM invocation.
    pub fn with_fuel(mut self, fuel: NonZeroU64) -> Self {
        self.fuel = Some(fuel);
        self
    }

    /// Set the maximum script memory budget for a future VM invocation.
    pub fn with_memory_bytes(mut self, memory_bytes: NonZeroUsize) -> Self {
        self.memory_bytes = Some(memory_bytes);
        self
    }

    /// Set the wall-clock timeout budget for a future VM invocation.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Mark that the host is requesting cooperative runtime shutdown.
    pub fn with_shutdown_requested(mut self) -> Self {
        self.shutdown_requested = true;
        self
    }

    pub fn fuel(&self) -> Option<NonZeroU64> {
        self.fuel
    }

    pub fn memory_bytes(&self) -> Option<NonZeroUsize> {
        self.memory_bytes
    }

    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }
}

/// Per-event host context passed into a script runtime invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeContext<'a> {
    controls: &'a RuntimeControls,
    command_limit: NonZeroUsize,
}

impl<'a> RuntimeContext<'a> {
    /// Create a runtime context from host-provided controls and command limit.
    pub fn new(controls: &'a RuntimeControls, command_limit: NonZeroUsize) -> Self {
        let command_limit = NonZeroUsize::new(command_limit.get().min(MAX_SCRIPT_COMMAND_BATCH))
            .expect("script command limit is non-zero");
        Self {
            controls,
            command_limit,
        }
    }

    /// Return the resource and lifecycle controls for this invocation.
    pub fn controls(&self) -> &'a RuntimeControls {
        self.controls
    }

    /// Return the maximum command count this invocation may emit.
    pub fn command_limit(&self) -> NonZeroUsize {
        self.command_limit
    }

    /// Build an empty command batch capped by this context's command limit.
    pub fn command_batch(&self) -> CommandBatch {
        CommandBatch::new(self.command_limit)
    }
}

/// Error returned by a script runtime implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeError {
    Trap { message: String },
    ShutdownRequested,
}

/// Result type returned by script runtime invocations.
pub type RuntimeResult<T> = Result<T, RuntimeError>;

/// Minimal runtime trait implemented by future script engines.
pub trait ScriptRuntime {
    /// Handle one immutable event snapshot and return a bounded command batch.
    fn handle_event(
        &mut self,
        event: &ScriptEvent,
        context: RuntimeContext<'_>,
    ) -> RuntimeResult<CommandBatch>;
}
