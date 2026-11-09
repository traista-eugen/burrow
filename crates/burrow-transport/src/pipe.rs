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

#[cfg(windows)]
mod sys {
    use std::io;
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use std::ptr;

    use windows_sys::Win32::Foundation::{HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    pub fn current_user_sid() -> io::Result<String> {
        // SAFETY: plain Win32 calls. Every out-pointer points at a live local, the token handle
        // is closed by `OwnedHandle`, the TOKEN_USER buffer is sized by the first call and
        // 8-byte aligned, and the SID string is freed with LocalFree as its docs require.
        unsafe {
            let mut token: HANDLE = ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let token = OwnedHandle::from_raw_handle(token);
            let token_handle = std::os::windows::io::AsRawHandle::as_raw_handle(&token);

            // The first call only reports the size needed.
            let mut size = 0u32;
            GetTokenInformation(token_handle, TokenUser, ptr::null_mut(), 0, &mut size);
            if size == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            if GetTokenInformation(
                token_handle,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();

            let mut wide: *mut u16 = ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut wide) == 0 {
                return Err(io::Error::last_os_error());
            }
            let len = (0..).take_while(|&i| *wide.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(wide, len));
            LocalFree(wide.cast());
            Ok(sid)
        }
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
