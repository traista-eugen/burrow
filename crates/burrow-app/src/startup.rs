//! Measuring startup: the time from the process's creation to the first frame on screen.
//!
//! The clock starts when Windows created the process (`GetProcessTimes`), so the loader and
//! everything before `main` count too. The result is logged at `info`:
//! `first frame 112 ms after process start (98 ms after main)`.

use std::time::{Duration, Instant};

/// How long ago the process was created, if the OS says.
pub fn since_process_start() -> Option<Duration> {
    imp::since_process_start()
}

/// Logs the startup time. Call from the first frame's `on_next_frame` callback.
pub fn report_first_frame(main_started: Instant) {
    let after_main = main_started.elapsed();
    match since_process_start() {
        Some(total) => tracing::info!(
            "first frame {} ms after process start ({} ms after main)",
            total.as_millis(),
            after_main.as_millis()
        ),
        None => tracing::info!("first frame {} ms after main", after_main.as_millis()),
    }
}

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

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
}

#[cfg(not(windows))]
mod imp {
    pub fn since_process_start() -> Option<std::time::Duration> {
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
}
