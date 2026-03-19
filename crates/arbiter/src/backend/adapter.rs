//! Adapter trait for backend CLI implementations.
//!
//! Abstracts over Cursor and Claude CLI. Implementors build args,
//! spawn the process, and schedule the callback.

use crate::types::{BackendOpts, OnComplete, OnStream};

/// Backend CLI adapter. Spawns processes and returns results via callback.
pub(crate) trait Adapter: Send + Sync {
    /// Executes a CLI call. Spawns on a background thread; callback runs
    /// on the main thread via `dispatch::schedule`.
    fn execute(&self, opts: BackendOpts, on_stream: Option<OnStream>, callback: OnComplete);
}
