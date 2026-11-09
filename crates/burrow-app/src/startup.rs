//! Measuring startup and memory.
//!
//! **Startup** is the time from the process's creation to the first frame on screen. The clock
//! starts when Windows created the process (`GetProcessTimes`), so the loader and everything
//! before `main` count too. Every run logs it at `info`:
//! `first frame 112 ms after process start (98 ms after main)`.
//!
//! **`--measure <file>`** ([`measure`]) also waits until the app is in use: connected, the lists
//! and agents loaded, and every session opened with `--open` loaded and replayed. It lets the
//! window draw that, then writes one JSON object to `<file>` and quits:
//!
//! ```json
//! {"firstFrameMs": 112, "firstFrameAfterMainMs": 98, "connectedMs": 140, "readyMs": 610,
//!  "timedOut": false, "sessionsListed": 50, "sessionsOpen": 3, "eventsLoaded": 4812,
//!  "workingSetMb": 98.4, "privateMb": 71.2, "peakWorkingSetMb": 101.0, "startedDaemonPid": null}
//! ```
//!
//! `cargo xtask measure` runs it against a seeded `burrowd`.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{App, Entity};

use crate::backend::Backend;
use crate::state::AppState;
use crate::views::workspace::Workspace;

/// How long `--measure` waits for the app to be ready before writing what it has.
const READY_TIMEOUT: Duration = Duration::from_secs(120);
/// How long it lets the window draw what loaded before reading the memory.
const SETTLE: Duration = Duration::from_millis(1500);

/// How long ago the process was created, if the OS says.
pub fn since_process_start() -> Option<Duration> {
    imp::since_process_start()
}

/// Startup timings of the first frame.
#[derive(Clone, Copy, Debug)]
pub struct FirstFrame {
    /// Since the process was created, if the OS says.
    pub since_start: Option<Duration>,
    /// Since `main` began.
    pub after_main: Duration,
}

/// Logs the startup time. Call from the first frame's `on_next_frame` callback.
pub fn report_first_frame(main_started: Instant) -> FirstFrame {
    let after_main = main_started.elapsed();
    let since_start = since_process_start();
    match since_start {
        Some(total) => tracing::info!(
            "first frame {} ms after process start ({} ms after main)",
            total.as_millis(),
            after_main.as_millis()
        ),
        None => tracing::info!("first frame {} ms after main", after_main.as_millis()),
    }
    FirstFrame {
        since_start,
        after_main,
    }
}

/// The process's memory, in bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Memory {
    /// Physical memory in use now.
    pub working_set: u64,
    /// Memory committed to this process alone.
    pub private: u64,
    /// The most physical memory it has used.
    pub peak_working_set: u64,
}

/// The process's memory now, if the OS says.
pub fn memory() -> Option<Memory> {
    imp::memory()
}

/// Where `--measure` keeps the first frame's timings until it writes them.
pub type FirstFrameCell = Rc<Cell<Option<FirstFrame>>>;

/// Runs `--measure`: waits until the app is ready, lets it draw, writes the numbers to `out`,
/// and quits. `first_frame` is filled by the first frame's callback.
pub fn measure(
    out: PathBuf,
    first_frame: FirstFrameCell,
    state: Entity<AppState>,
    workspace: Entity<Workspace>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        let started = Instant::now();
        let since_start = || since_process_start().map(|d| d.as_millis() as u64);
        let mut connected_ms = None;
        let ready = loop {
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
            let ready = cx.update(|cx| {
                let app = state.read(cx);
                let connected = app.connection().is_connected();
                if connected && connected_ms.is_none() {
                    connected_ms = since_start();
                }
                let sessions_ready = workspace
                    .read(cx)
                    .open_sessions()
                    .iter()
                    .all(|view| view.read(cx).is_caught_up(cx));
                connected
                    && app.index().is_loaded()
                    && app.agents_loaded()
                    && sessions_ready
                    && first_frame.get().is_some()
            });
            if ready {
                break true;
            }
            if started.elapsed() > READY_TIMEOUT {
                break false;
            }
        };
        let ready_ms = since_start();
        cx.background_executor().timer(SETTLE).await;
        cx.update(|cx| {
            let app = state.read(cx);
            let workspace = workspace.read(cx);
            let frame = first_frame.get();
            let memory = memory().unwrap_or_default();
            let mb = |bytes: u64| (bytes as f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0;
            let report = serde_json::json!({
                "firstFrameMs": frame.and_then(|f| f.since_start).map(|d| d.as_millis() as u64),
                "firstFrameAfterMainMs": frame.map(|f| f.after_main.as_millis() as u64),
                "connectedMs": connected_ms,
                "readyMs": ready_ms,
                "timedOut": !ready,
                "sessionsListed": app.index().sessions(false).len(),
                "sessionsOpen": workspace.open_sessions().len(),
                "eventsLoaded": workspace
                    .open_sessions()
                    .iter()
                    .map(|view| view.read(cx).event_count(cx))
                    .sum::<usize>(),
                "workingSetMb": mb(memory.working_set),
                "privateMb": mb(memory.private),
                "peakWorkingSetMb": mb(memory.peak_working_set),
                "startedDaemonPid": Backend::global(cx).started_daemon(),
            });
            tracing::info!(%report, "measured");
            if let Err(error) = std::fs::write(&out, format!("{report}\n")) {
                tracing::error!(%error, path = %out.display(), "couldn't write the measurements");
            }
            cx.quit();
        });
    })
    .detach();
}

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    use super::Memory;

    fn ticks(time: FILETIME) -> u64 {
        (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
    }

    pub fn since_process_start() -> Option<Duration> {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: the pseudo-handle of the current process is always valid; the out-pointers
        // point at live locals.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        };
        if ok == 0 {
            return None;
        }
        let mut now = zero;
        // SAFETY: writes one FILETIME to a live local.
        unsafe { GetSystemTimePreciseAsFileTime(&mut now) };
        // Both are in 100 ns units since 1601.
        let elapsed = ticks(now).checked_sub(ticks(created))?;
        Some(Duration::from_nanos(elapsed * 100))
    }

    pub fn memory() -> Option<Memory> {
        // SAFETY: the counters are plain data; all-zero is a valid value to overwrite.
        let mut counters: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
        counters.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        // SAFETY: the pseudo-handle is valid; the pointer and size describe `counters`, which
        // starts with the PROCESS_MEMORY_COUNTERS layout the function writes.
        let ok = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
                counters.cb,
            )
        };
        (ok != 0).then_some(Memory {
            working_set: counters.WorkingSetSize as u64,
            private: counters.PrivateUsage as u64,
            peak_working_set: counters.PeakWorkingSetSize as u64,
        })
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn since_process_start() -> Option<std::time::Duration> {
        None
    }

    pub fn memory() -> Option<super::Memory> {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_process_started_in_the_past() {
        let since = super::since_process_start().unwrap();
        assert!(since > std::time::Duration::ZERO);
        assert!(since < std::time::Duration::from_secs(3_600));
    }

    #[test]
    fn memory_is_read() {
        let memory = super::memory().unwrap();
        assert!(memory.working_set > 0);
        assert!(memory.private > 0);
        assert!(memory.peak_working_set >= memory.working_set);
    }
}
