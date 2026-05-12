//! Output bounded context — keyboard injection, window focus, retype state.

pub mod backend;
pub mod retype;
pub mod window;

pub use backend::{auto_detect, select_backend_kind, BackendKind};
pub use retype::{KeyboardSink, RetypeState, RetypeStep};
pub use window::{
    capture as capture_window, is_excluded, restore as restore_window, Platform, WindowSnapshot,
};
