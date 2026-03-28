pub use arbiter_core::threads::*;

mod input;
pub(crate) mod window;

pub use input::{close as input_close, open, open_below, open_for_line, OnCancel, OnSubmit};
pub use window::{
    append_interrupted, append_learned_rules, append_message, append_revision_summary,
    append_similar_threads, append_status_hl, append_streaming, close as window_close,
    current_thread_id as window_thread_id, handle as window_handle, is_open as window_is_open,
    open as window_open, replace_last_agent_message, set_last_prompt, OnClose, OnReplyRequested,
    OnRevisionSelected, OnSimilarSelected, WindowCallbacks,
};
