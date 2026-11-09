//! The named pipe server: who may connect, a single `burrowd` per pipe, one instance per client.
//!
//! - **Current user only.** Every pipe instance is created with a security descriptor whose DACL
//!   has a single entry, granting access to the current user's SID. The default DACL would also
//!   let other accounts read. Remote clients are rejected (`PIPE_REJECT_REMOTE_CLIENTS`).
//! - **One `burrowd` per pipe.** The first instance is created with
//!   `FILE_FLAG_FIRST_PIPE_INSTANCE`, which fails if the pipe already exists. Then a probe
//!   connection tells a live `burrowd` (exit) from a pipe that only lingers because a client still
//!   holds a handle to a dead server's instance (wait for it to go away).
//! - **Always listening.** As soon as a client connects, the next instance is created, so a new
//!   client never finds the pipe busy for long.

use std::io;
use std::ptr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use tracing::{debug, error, warn};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

use crate::connection::{self, Context};

/// How long to wait for a lingering pipe to disappear before giving up.
const STALE_PIPE_WAIT: Duration = Duration::from_secs(5);

/// Why the pipe couldn't be created.
#[derive(Debug, thiserror::Error)]
pub enum BindError {
    /// Another `burrowd` answers on the pipe.
    #[error("another burrowd is already serving this pipe")]
    AlreadyRunning,
    /// The pipe exists, but nothing answers on it.
    #[error("the pipe is in use, but no burrowd answers on it: {0}")]
    InUse(io::Error),
    /// Creating the pipe failed for another reason.
    #[error("can't create the pipe: {0}")]
    Io(io::Error),
}

/// Accepts clients on the pipe.
#[derive(Debug)]
pub struct Listener {
    path: String,
    security: PipeSecurity,
    /// The instance waiting for the next client.
    next: NamedPipeServer,
}

impl Listener {
    /// Creates the pipe's first instance. Fails with [`BindError::AlreadyRunning`] if another
    /// `burrowd` serves it.
    pub async fn bind(path: &str) -> Result<Self, BindError> {
        let security = PipeSecurity::current_user_only().map_err(BindError::Io)?;
        let deadline = Instant::now() + STALE_PIPE_WAIT;
        loop {
            match security.create(path, true) {
                Ok(first) => {
                    return Ok(Self {
                        path: path.to_owned(),
                        security,
                        next: first,
                    });
                }
                Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                    if ClientOptions::new().open(path).is_ok() {
                        return Err(BindError::AlreadyRunning);
                    }
                    if Instant::now() >= deadline {
                        return Err(BindError::InUse(error));
                    }
                    debug!("the pipe exists but nothing answers yet; waiting for it to go away");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(BindError::Io(error)),
            }
        }
    }

    /// The pipe's full path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Waits for the next client, and leaves a fresh instance listening for the one after.
    async fn accept(&mut self) -> io::Result<NamedPipeServer> {
        self.next.connect().await?;
        let next = self.security.create(&self.path, false)?;
        Ok(std::mem::replace(&mut self.next, next))
    }

    /// Replaces the listening instance, after it failed.
    fn reset(&mut self) -> io::Result<()> {
        self.next = self.security.create(&self.path, false)?;
        Ok(())
    }
}

/// Accepts clients forever, one task per connection.
pub async fn serve(mut listener: Listener, context: Arc<Context>) {
    loop {
        match listener.accept().await {
            Ok(pipe) => {
                tokio::spawn(connection::serve(pipe, context.clone()));
            }
            Err(error) => {
                warn!(%error, "accepting a client failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                if let Err(error) = listener.reset() {
                    error!(%error, "creating a pipe instance failed");
                }
            }
        }
    }
}

/// A security descriptor that grants the current user, and nobody else, access to the pipe.
#[derive(Debug)]
struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
}

// SAFETY: the descriptor is an immutable heap block owned by this value; Windows only reads it.
unsafe impl Send for PipeSecurity {}
// SAFETY: as above; nothing mutates it after creation.
unsafe impl Sync for PipeSecurity {}

impl PipeSecurity {
    fn current_user_only() -> io::Result<Self> {
        let sid = burrow_transport::current_user_sid()?;
        // D:P   a protected DACL, so no entries are inherited;
        // A;;GA allow generic-all access…
        // ;;;S  …to this SID only.
        let sddl = format!("D:P(A;;GA;;;{sid})");
        let wide: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and outlives the call; `descriptor` receives a
        // LocalAlloc'd block, freed in Drop.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { descriptor })
    }

    /// Creates a pipe instance with this descriptor. `first` asks for the pipe's first instance.
    fn create(&self, path: &str, first: bool) -> io::Result<NamedPipeServer> {
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: `attributes` and the descriptor it points to are valid for the whole call.
        unsafe { options.create_with_security_attributes_raw(path, (&raw mut attributes).cast()) }
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW, freed once.
        unsafe { LocalFree(self.descriptor) };
    }
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

    use super::*;

    fn unique_pipe(name: &str) -> String {
        format!(r"\\.\pipe\burrowd-unit-{}-{name}", std::process::id())
    }

    /// The DACL of a pipe instance, as SDDL.
    fn dacl(pipe: &NamedPipeServer) -> String {
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let mut text: *mut u16 = ptr::null_mut();
        // SAFETY: test-only Win32 calls with valid out-pointers; both buffers are freed.
        unsafe {
            let status = GetSecurityInfo(
                pipe.as_raw_handle(),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            );
            assert_eq!(status, 0, "GetSecurityInfo failed");
            let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                ptr::null_mut(),
            );
            assert_ne!(
                ok, 0,
                "ConvertSecurityDescriptorToStringSecurityDescriptorW failed"
            );
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            LocalFree(descriptor);
            sddl
        }
    }

    #[tokio::test]
    async fn only_the_current_user_may_open_the_pipe() {
        let listener = Listener::bind(&unique_pipe("dacl")).await.unwrap();
        let sid = burrow_transport::current_user_sid().unwrap();
        // GA (generic all) is stored as FA (file all access) once mapped for a pipe.
        assert_eq!(dacl(&listener.next), format!("D:P(A;;FA;;;{sid})"));
    }

    #[tokio::test]
    async fn a_second_bind_on_a_live_pipe_reports_already_running() {
        let path = unique_pipe("twice");
        let _first = Listener::bind(&path).await.unwrap();
        let second = Listener::bind(&path).await.unwrap_err();
        assert!(matches!(second, BindError::AlreadyRunning), "{second:?}");
    }
}
