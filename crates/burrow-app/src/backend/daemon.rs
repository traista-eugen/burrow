//! Starting `burrowd` when nothing serves the pipe (D-029).
//!
//! `burrowd` is a per-user service that outlives the app (D-002), so it is started detached:
//! - no console (`DETACHED_PROCESS`), in its own process group;
//! - no inherited handles, so it holds none of the app's pipes or files open;
//! - outside any Job Object the app runs in (`CREATE_BREAKAWAY_FROM_JOB`), so closing the app's
//!   job, for example a terminal's, doesn't take `burrowd` with it. A job that forbids breaking
//!   away refuses that flag; then `burrowd` starts inside the job, with a warning.

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

    use tracing::warn;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED};
    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT,
        CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION, STARTUPINFOW,
    };

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    pub fn spawn(program: &Path, args: &[String]) -> io::Result<u32> {
        let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT;
        match create(program, args, base | CREATE_BREAKAWAY_FROM_JOB) {
            Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                warn!(
                    "this process's Job Object doesn't allow breaking away; burrowd starts inside \
                     it and ends when the job does"
                );
                create(program, args, base)
            }
            result => result,
        }
    }

    fn create(program: &Path, args: &[String], flags: u32) -> io::Result<u32> {
        let application = wide(program.as_os_str());
        let mut command_line = wide(super::command_line(program, args).as_ref());
        let directory = program.parent().map(|dir| wide(dir.as_os_str()));
        let startup = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: every pointer is valid for the call; the command line buffer is mutable and
        // NUL-terminated as CreateProcessW requires; handles aren't inherited.
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
                &startup,
                &mut info,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both handles were just returned by CreateProcessW and are closed once.
        unsafe {
            CloseHandle(info.hThread);
            CloseHandle(info.hProcess);
        }
        Ok(info.dwProcessId)
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

    #[test]
    fn a_missing_program_is_not_found() {
        let launch = DaemonLaunch {
            program: PathBuf::from(r"Z:\definitely\not\burrowd.exe"),
            args: Vec::new(),
        };
        assert_eq!(spawn(&launch).unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
