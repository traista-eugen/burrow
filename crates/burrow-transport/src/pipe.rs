//! The named pipe's name: `\\.\pipe\burrow-<user SID>`.
//!
//! Putting the user's SID in the name gives each Windows user their own `burrowd`. The name alone
//! doesn't keep other users out; `burrowd` also sets a security descriptor that admits only the
//! current user, and rejects remote clients.

use std::io;

/// The prefix every local named pipe path starts with.
pub const PIPE_PREFIX: &str = r"\\.\pipe\";

/// The full path of a pipe.
///
/// A name that already starts with `\\.\pipe\` (in any case) is returned as it is; anything else
/// is taken as a bare name and prefixed. So `burrow-dev` becomes `\\.\pipe\burrow-dev`.
pub fn pipe_path(name: &str) -> String {
    let has_prefix = name
        .get(..PIPE_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PIPE_PREFIX));
    if has_prefix {
        name.to_owned()
    } else {
        format!("{PIPE_PREFIX}{name}")
    }
}

/// The current user's `burrowd` pipe: `\\.\pipe\burrow-<user SID>`.
pub fn default_pipe_name() -> io::Result<String> {
    Ok(format!("{PIPE_PREFIX}burrow-{}", current_user_sid()?))
}

/// The SID of the user this process runs as, in string form, for example
/// `S-1-5-21-1004336348-1177238915-682003330-1001`.
#[cfg(windows)]
pub fn current_user_sid() -> io::Result<String> {
    sys::current_user_sid()
}

/// The SID of the user this process runs as. Only Windows has SIDs, so this always fails
/// elsewhere.
#[cfg(not(windows))]
pub fn current_user_sid() -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "burrow's local transport is Windows-only for now",
    ))
}

/// Whether the process serving the pipe that `pipe` (a client handle) is connected to runs as
/// the current user.
///
/// A client calls this before sending anything. Another account could create a pipe with
/// `burrowd`'s name first ("pipe squatting") and read everything the client sends; the pipe's
/// name alone proves nothing. The check asks Windows for the server's process id
/// (`GetNamedPipeServerProcessId`), opens that process's token, and compares its user SID with
/// this process's. An error, for example access denied to another account's process, means the
/// server can't be trusted either.
#[cfg(windows)]
pub fn pipe_server_is_current_user(pipe: std::os::windows::io::RawHandle) -> io::Result<bool> {
    sys::pipe_server_is_current_user(pipe)
}

/// Whether process `pid` runs as the current user. See [`pipe_server_is_current_user`].
#[cfg(windows)]
pub fn process_is_current_user(pid: u32) -> io::Result<bool> {
    sys::process_is_current_user(pid)
}

#[cfg(windows)]
mod sys {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
    use std::ptr;

    use windows_sys::Win32::Foundation::{HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// A process token's `TOKEN_USER`, in an 8-byte aligned buffer that owns the SID it points
    /// to.
    struct TokenUserBuffer(Vec<u64>);

    impl TokenUserBuffer {
        /// The user of `process`'s token.
        fn of_process(process: HANDLE) -> io::Result<Self> {
            // SAFETY: plain Win32 calls. Every out-pointer points at a live local, the token
            // handle is closed by `OwnedHandle`, and the buffer is sized by the first call.
            unsafe {
                let mut token: HANDLE = ptr::null_mut();
                if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let token = OwnedHandle::from_raw_handle(token);

                // The first call only reports the size needed.
                let mut size = 0u32;
                GetTokenInformation(
                    token.as_raw_handle(),
                    TokenUser,
                    ptr::null_mut(),
                    0,
                    &mut size,
                );
                if size == 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
                if GetTokenInformation(
                    token.as_raw_handle(),
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    size,
                    &mut size,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(Self(buffer))
            }
        }

        fn current() -> io::Result<Self> {
            // SAFETY: the pseudo-handle of the current process needs no closing.
            Self::of_process(unsafe { GetCurrentProcess() })
        }

        fn user(&self) -> &TOKEN_USER {
            // SAFETY: GetTokenInformation(TokenUser) filled the buffer with a TOKEN_USER, and
            // the buffer is aligned for it.
            unsafe { &*self.0.as_ptr().cast::<TOKEN_USER>() }
        }
    }

    pub fn current_user_sid() -> io::Result<String> {
        let buffer = TokenUserBuffer::current()?;
        // SAFETY: the SID lives in `buffer`, which outlives these calls; the string is freed
        // with LocalFree as its docs require.
        unsafe {
            let mut wide: *mut u16 = ptr::null_mut();
            if ConvertSidToStringSidW(buffer.user().User.Sid, &mut wide) == 0 {
                return Err(io::Error::last_os_error());
            }
            let len = (0..).take_while(|&i| *wide.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(wide, len));
            LocalFree(wide.cast());
            Ok(sid)
        }
    }

    pub fn pipe_server_is_current_user(pipe: RawHandle) -> io::Result<bool> {
        let mut pid = 0u32;
        // SAFETY: `pipe` is a valid pipe handle for the call; `pid` is a live out-pointer.
        if unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        process_is_current_user(pid)
    }

    pub fn process_is_current_user(pid: u32) -> io::Result<bool> {
        // SAFETY: a plain open; the handle is owned and closed by `OwnedHandle`.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a fresh, valid handle that nothing else owns.
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        let theirs = TokenUserBuffer::of_process(process.as_raw_handle())?;
        let ours = TokenUserBuffer::current()?;
        // SAFETY: both SIDs live in their buffers for the call.
        Ok(unsafe { EqualSid(theirs.user().User.Sid, ours.user().User.Sid) } != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_path_adds_the_prefix_once() {
        assert_eq!(pipe_path("burrow-dev"), r"\\.\pipe\burrow-dev");
        assert_eq!(pipe_path(r"\\.\pipe\burrow-dev"), r"\\.\pipe\burrow-dev");
        assert_eq!(pipe_path(r"\\.\PIPE\burrow-dev"), r"\\.\PIPE\burrow-dev");
        assert_eq!(pipe_path(""), r"\\.\pipe\");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_pipe_served_by_this_process_passes_the_user_check() {
        use std::os::windows::io::AsRawHandle;

        use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

        let name = format!(r"\\.\pipe\burrow-transport-unit-{}", std::process::id());
        let server = ServerOptions::new().create(&name).unwrap();
        let client = ClientOptions::new().open(&name).unwrap();
        assert!(pipe_server_is_current_user(client.as_raw_handle()).unwrap());
        drop(server);
    }

    #[cfg(windows)]
    #[test]
    fn processes_of_other_accounts_fail_the_user_check() {
        assert!(process_is_current_user(std::process::id()).unwrap());
        // Process 4 is the kernel's System process. Opening it is denied, or its token belongs
        // to SYSTEM; either way it isn't us.
        assert!(!process_is_current_user(4).unwrap_or(false));
    }

    #[cfg(windows)]
    #[test]
    fn the_default_pipe_is_named_after_the_user_sid() {
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-"), "{sid}");
        assert_eq!(
            default_pipe_name().unwrap(),
            format!(r"\\.\pipe\burrow-{sid}")
        );
    }
}
