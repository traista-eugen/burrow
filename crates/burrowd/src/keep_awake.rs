//! Keeping Windows awake while agents work.
//!
//! `SetThreadExecutionState` applies to the calling thread and lapses when that thread exits.
//! tokio moves tasks between worker threads, so the call can't be made from a task. Instead one
//! dedicated thread owns the state: [`KeepAwake::set`] sends it the wanted state, and it calls
//! `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)` to hold it, or
//! `SetThreadExecutionState(ES_CONTINUOUS)` to let go.

use std::sync::mpsc;
use std::thread;

use tracing::{info, warn};

/// Asks Windows not to sleep while it is held.
#[derive(Debug)]
pub struct KeepAwake {
    wanted: mpsc::Sender<bool>,
}

impl KeepAwake {
    /// Starts the thread that holds the state. Not held at first.
    pub fn start() -> std::io::Result<Self> {
        let (wanted, requests) = mpsc::channel::<bool>();
        thread::Builder::new()
            .name("burrow-keep-awake".into())
            .spawn(move || {
                let mut held = false;
                // Ends when the sender is dropped; the state lapses with the thread.
                while let Ok(mut want) = requests.recv() {
                    // Only the latest request matters.
                    while let Ok(newer) = requests.try_recv() {
                        want = newer;
                    }
                    if want != held {
                        if set(want) {
                            held = want;
                            info!(held, "keep-awake changed");
                        } else {
                            warn!(want, "SetThreadExecutionState failed");
                        }
                    }
                }
                if held {
                    set(false);
                }
            })?;
        Ok(Self { wanted })
    }

    /// Holds or releases the request. Cheap; repeating the current state does nothing.
    pub fn set(&self, held: bool) {
        let _ = self.wanted.send(held);
    }
}

#[cfg(windows)]
fn set(held: bool) -> bool {
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };
    let flags = if held {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    // SAFETY: a plain call with valid flags. It returns the previous state, or 0 on failure.
    unsafe { SetThreadExecutionState(flags) != 0 }
}

#[cfg(not(windows))]
fn set(_held: bool) -> bool {
    true
}

#[cfg(all(test, windows))]
mod tests {
    use std::time::{Duration, Instant};

    use windows_sys::Win32::System::Power::{
        CallNtPowerInformation, ES_SYSTEM_REQUIRED, SystemExecutionState,
    };

    use super::*;

    /// The system's execution state: every process's requests combined.
    fn system_state() -> u32 {
        let mut state = 0u32;
        // SAFETY: SystemExecutionState takes no input and writes one ULONG.
        let status = unsafe {
            CallNtPowerInformation(
                SystemExecutionState,
                std::ptr::null(),
                0,
                (&raw mut state).cast(),
                size_of::<u32>() as u32,
            )
        };
        assert_eq!(status, 0, "CallNtPowerInformation failed");
        state
    }

    /// A smoke test of the thread and the call. The system state combines every program's
    /// requests, so where another program already keeps the PC awake this passes regardless.
    #[test]
    fn holding_asks_windows_to_stay_awake() {
        let keep_awake = KeepAwake::start().unwrap();
        keep_awake.set(true);
        let deadline = Instant::now() + Duration::from_secs(5);
        while system_state() & ES_SYSTEM_REQUIRED == 0 {
            assert!(
                Instant::now() < deadline,
                "ES_SYSTEM_REQUIRED was never set"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // Releasing can't be checked the same way: other programs may hold it too.
        keep_awake.set(false);
    }
}
