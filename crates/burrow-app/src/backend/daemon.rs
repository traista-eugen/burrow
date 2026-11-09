//! Starting `burrowd` when nothing serves the pipe (D-029).
//!
//! `burrowd` is a per-user service that outlives the app (D-002), so it is started detached:
//! - no console (`DETACHED_PROCESS`), in its own process group;
//! - no inherited handles, so it holds none of the app's pipes or files open;
//! - outside the app's process tree and any Job Object the app runs in, so whatever started the
//!   app can't take `burrowd` with it when it goes. A terminal that closes a tab ends the whole
//!   tree under the tab's shell, job or not (D-037). So, in order:
//!   1. as a child of the desktop shell, Explorer (`PROC_THREAD_ATTRIBUTE_PARENT_PROCESS`). A
//!      process created with another parent is in that parent's tree and joins its job;
//!   2. without a shell, as the app's child but out of its job (`CREATE_BREAKAWAY_FROM_JOB`);
//!   3. only if the job forbids breaking away too, inside it, with a warning.

use std::io;
use std::path::{Path, PathBuf};

use super::DaemonLaunch;

/// `burrowd.exe` next to the running executable, as in Cargo's target folder and an install.
pub fn next_to_current_exe() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(exe.with_file_name(format!("burrowd{}", std::env::consts::EXE_SUFFIX)))
}

/// Starts `burrowd` as described in the module docs. Returns its process id.
pub fn spawn(launch: &DaemonLaunch) -> io::Result<u32> {
    if !launch.program.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} doesn't exist", launch.program.display()),
        ));
    }
    imp::spawn(&launch.program, &launch.args)
}

/// Quotes one argument for a Windows command line, the way `CommandLineToArgvW` and the C
/// runtime read it back.
fn quote_arg(arg: &str, out: &mut String) {
    let needs_quotes = arg.is_empty() || arg.contains([' ', '\t', '\n', '\u{b}', '"']);
    if !needs_quotes {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
}

/// The whole command line: the program, then its arguments.
fn command_line(program: &Path, args: &[String]) -> String {
    let mut line = String::new();
    quote_arg(&program.to_string_lossy(), &mut line);
    for arg in args {
        line.push(' ');
        quote_arg(arg, &mut line);
    }
    line
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr;

    use tracing::{info, warn};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, HANDLE};
    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT,
        CreateProcessW, DETACHED_PROCESS, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList, OpenProcess,
        PROC_THREAD_ATTRIBUTE_PARENT_PROCESS, PROCESS_CREATE_PROCESS, PROCESS_INFORMATION,
        STARTUPINFOEXW, STARTUPINFOW, UpdateProcThreadAttribute,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    pub fn spawn(program: &Path, args: &[String]) -> io::Result<u32> {
        let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT;
        // Under the desktop shell: out of whatever tree and job the app was started in.
        match ShellProcess::open() {
            Ok(Some(shell)) => match create(program, args, base, Some(shell.handle)) {
                Ok(pid) => {
                    info!(
                        pid,
                        shell = shell.pid,
                        "started burrowd under the desktop shell"
                    );
                    return Ok(pid);
                }
                Err(error) => warn!(%error, "couldn't start burrowd under the desktop shell"),
            },
            Ok(None) => info!("there is no desktop shell to start burrowd under"),
            Err(error) => warn!(%error, "couldn't open the desktop shell's process"),
        }
        // As the app's child, out of its job. Closing the app's process tree still ends it.
        match create(program, args, base | CREATE_BREAKAWAY_FROM_JOB, None) {
            Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                warn!(
                    "this process's Job Object doesn't allow breaking away; burrowd starts \
                     inside it and ends when the job does"
                );
                create(program, args, base, None)
            }
            result => result,
        }
    }

    /// The desktop shell's process (Explorer), opened to be another process's parent.
    pub(super) struct ShellProcess {
        /// Opened with `PROCESS_CREATE_PROCESS` only.
        pub(super) handle: HANDLE,
        pub(super) pid: u32,
    }

    impl ShellProcess {
        /// `None` when there is no shell window, as on a server without a desktop shell.
        pub(super) fn open() -> io::Result<Option<Self>> {
            // SAFETY: plain calls; a null window means no shell.
            let window = unsafe { GetShellWindow() };
            if window.is_null() {
                return Ok(None);
            }
            let mut pid = 0u32;
            // SAFETY: `window` is a window handle and `pid` is a valid out pointer.
            unsafe { GetWindowThreadProcessId(window, &mut pid) };
            if pid == 0 {
                return Ok(None);
            }
            // SAFETY: a plain open; the handle is owned by the returned value.
            let handle = unsafe { OpenProcess(PROCESS_CREATE_PROCESS, 0, pid) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Some(Self { handle, pid }))
        }
    }

    impl Drop for ShellProcess {
        fn drop(&mut self) {
            // SAFETY: the handle came from OpenProcess and is closed once.
            unsafe { CloseHandle(self.handle) };
        }
    }

    /// Creates the process; with `parent`, as that process's child, joining its Job Object.
    pub(super) fn create(
        program: &Path,
        args: &[String],
        mut flags: u32,
        parent: Option<HANDLE>,
    ) -> io::Result<u32> {
        let application = wide(program.as_os_str());
        let mut command_line = wide(super::command_line(program, args).as_ref());
        let directory = program.parent().map(|dir| wide(dir.as_os_str()));
        // SAFETY: STARTUPINFOEXW is plain data, for which all zeroes is valid.
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOW>() as u32;

        // The attribute list that names the parent. Kept alive until CreateProcessW returns,
        // together with the handle it points at.
        let parent_handle = parent;
        let mut list_buffer: Vec<usize> = Vec::new();
        if let Some(parent_handle) = parent_handle.as_ref() {
            let mut size = 0usize;
            // SAFETY: a size query with a null list, as documented; it fails with
            // ERROR_INSUFFICIENT_BUFFER and sets `size`.
            unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size) };
            list_buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
            let list = list_buffer.as_mut_ptr().cast();
            // SAFETY: `list` points at `size` writable, pointer-aligned bytes.
            if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `list` was initialised for one attribute; the value is a process handle
            // that outlives the CreateProcessW call below.
            let ok = unsafe {
                UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_PARENT_PROCESS as usize,
                    (parent_handle as *const HANDLE).cast(),
                    size_of::<HANDLE>(),
                    ptr::null_mut(),
                    ptr::null(),
                )
            };
            if ok == 0 {
                let error = io::Error::last_os_error();
                // SAFETY: the list was initialised above and is deleted once.
                unsafe { DeleteProcThreadAttributeList(list) };
                return Err(error);
            }
            startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            startup.lpAttributeList = list;
            flags |= EXTENDED_STARTUPINFO_PRESENT;
        }

        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: every pointer is valid for the call; the command line buffer is mutable and
        // NUL-terminated as CreateProcessW requires; handles aren't inherited. With
        // EXTENDED_STARTUPINFO_PRESENT the startup info is a STARTUPINFOEXW, whose first member
        // is the STARTUPINFOW passed here.
        let ok = unsafe {
            CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                flags,
                ptr::null(),
                directory.as_ref().map_or(ptr::null(), |dir| dir.as_ptr()),
                &startup.StartupInfo,
                &mut info,
            )
        };
        let result = if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: both handles were just returned by CreateProcessW and are closed once.
            unsafe {
                CloseHandle(info.hThread);
                CloseHandle(info.hProcess);
            }
            Ok(info.dwProcessId)
        };
        if !startup.lpAttributeList.is_null() {
            // SAFETY: the list was initialised above and is deleted once, after its last use.
            unsafe { DeleteProcThreadAttributeList(startup.lpAttributeList) };
        }
        drop(list_buffer);
        result
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;
    use std::path::Path;
    use std::process::{Command, Stdio};

    pub fn spawn(program: &Path, args: &[String]) -> io::Result<u32> {
        let child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(child.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quoted(arg: &str) -> String {
        let mut out = String::new();
        quote_arg(arg, &mut out);
        out
    }

    #[test]
    fn arguments_are_quoted_for_the_c_runtime() {
        assert_eq!(quoted("--dev"), "--dev");
        assert_eq!(quoted(""), "\"\"");
        assert_eq!(
            quoted(r"C:\Program Files\Burrow"),
            r#""C:\Program Files\Burrow""#
        );
        assert_eq!(quoted(r"a\\b c\"), r#""a\\b c\\""#);
        assert_eq!(quoted(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(
            command_line(
                Path::new(r"D:\b\burrowd.exe"),
                &["--pipe".into(), "burrow test".into()]
            ),
            r#"D:\b\burrowd.exe --pipe "burrow test""#
        );
    }

    /// How `burrowd` leaves the app's process tree and job (D-037): a process started under the
    /// desktop shell is the shell's child and shares its job membership, not this test's.
    #[cfg(windows)]
    #[test]
    fn a_process_started_under_the_shell_leaves_our_job() {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, OpenProcess,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
        };

        fn in_any_job(process: HANDLE) -> bool {
            let mut in_job = 0;
            // SAFETY: a valid process handle and out pointer; null means "any job".
            assert_ne!(
                unsafe { IsProcessInJob(process, std::ptr::null_mut(), &mut in_job) },
                0
            );
            in_job != 0
        }

        fn parent_of(pid: u32) -> Option<u32> {
            // SAFETY: a snapshot of all processes, closed below.
            let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
            let mut entry = PROCESSENTRY32W {
                dwSize: size_of::<PROCESSENTRY32W>() as u32,
                // SAFETY: plain data, for which all zeroes is valid.
                ..unsafe { std::mem::zeroed() }
            };
            let mut found = None;
            // SAFETY: `entry` carries its size, as Process32FirstW requires.
            let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
            while more {
                if entry.th32ProcessID == pid {
                    found = Some(entry.th32ParentProcessID);
                    break;
                }
                // SAFETY: as above.
                more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
            }
            // SAFETY: the snapshot handle is closed once.
            unsafe { CloseHandle(snapshot) };
            found
        }

        let Some(shell) = imp::ShellProcess::open().expect("open the desktop shell") else {
            return; // No desktop shell here: nothing to start under.
        };
        let shell_pid = shell.pid;
        let ping = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
            .join(r"System32\PING.EXE");
        // A hidden console rather than DETACHED_PROCESS: ping exits at once without one.
        let pid = imp::create(
            &ping,
            &["-n".into(), "30".into(), "127.0.0.1".into()],
            CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP,
            Some(shell.handle),
        )
        .expect("start ping under the desktop shell");

        // SAFETY: plain opens; both handles are closed below.
        let child = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                0,
                pid,
            )
        };
        assert!(
            !child.is_null(),
            "open ping: {}",
            io::Error::last_os_error()
        );
        let shell_query = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, shell_pid) };
        assert!(
            !shell_query.is_null(),
            "open the shell: {}",
            io::Error::last_os_error()
        );
        let child_in_job = in_any_job(child);
        let shell_in_job = in_any_job(shell_query);
        let parent = parent_of(pid);
        // SAFETY: valid handles, each closed once.
        unsafe {
            TerminateProcess(child, 0);
            CloseHandle(child);
            CloseHandle(shell_query);
        }
        assert_eq!(parent, Some(shell_pid), "the child's parent is the shell");
        assert_eq!(
            child_in_job, shell_in_job,
            "the child shares the shell's job membership"
        );
    }

    #[test]
    fn a_missing_program_is_not_found() {
        let launch = DaemonLaunch {
            program: PathBuf::from(r"Z:\definitely\not\burrowd.exe"),
            args: Vec::new(),
        };
        assert_eq!(spawn(&launch).unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
