//! The local transport between `burrowd` and its clients (D-013).
//!
//! Both ends need the same two things, so they live here rather than in either of them:
//!
//! - [`pipe`]: the named pipe's name. Each Windows user gets their own pipe,
//!   `\\.\pipe\burrow-<user SID>`.
//! - [`framing`]: newline-delimited JSON. One message per line; serialized JSON never contains a
//!   raw newline, so the framing needs no escaping. Lines are capped at [`MAX_LINE_BYTES`].
//!
//! Opening the pipe is left to each side: `burrowd` creates it with a security descriptor that
//! admits only the current user, and `burrow-client` connects to it, then checks with
//! [`pipe_server_is_current_user`] that the server runs as the current user before sending
//! anything.

pub mod framing;
pub mod pipe;

pub use framing::{LineReader, MAX_LINE_BYTES, push_line, write_line};
pub use pipe::{PIPE_PREFIX, current_user_sid, default_pipe_name, pipe_path};
#[cfg(windows)]
pub use pipe::{pipe_server_is_current_user, process_is_current_user};
