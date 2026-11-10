//! Pseudo-console processes: a TUI under ConPTY, created inside its Job Object (D-038).
//!
//! `CreatePseudoConsole` makes a console whose screen comes out of one pipe as VT sequences and
//! whose keyboard goes in through another. `CreateProcessW` then starts the program on it, with
//! two attributes: `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`, and `PROC_THREAD_ATTRIBUTE_JOB_LIST`,
//! which creates the process inside the session's kill-on-close Job Object. So, unlike the
//! suspended start [`super::spawn`] needs, there is no moment at which the process runs outside
//! its job.
//!
//! The pseudo console's own host, `conhost.exe`, is started by `CreatePseudoConsole` as a child
//! of `burrowd`, not of the program, so it isn't in the job. It exits when the pseudo console is
//! closed ([`PseudoConsole::close`]), and when `burrowd` dies, because its pipes break.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::watch;
use tracing::debug;
use windows_sys::Win32::Foundation::{HANDLE, S_OK, WAIT_OBJECT_0};
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::windows::Job;
use super::{ExitInfo, LaunchSpec, PtySize};

/// A pseudo console. Closing it ends its `conhost.exe`; it is closed when dropped.
#[derive(Debug)]
pub struct Console {
    handle: Mutex<Option<HPCON>>,
}

impl Console {
    /// Changes the console's size. The program sees a window-size change.
    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        let handle = self.handle.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(handle) = *handle else {
            return Err(io::Error::other("the pseudo console is closed"));
        };
        // SAFETY: `handle` is a live pseudo console; it is only closed under this lock.
        let result = unsafe { ResizePseudoConsole(handle, coord(size)) };
        hresult(result)
    }

    /// Closes the console. Its host paints a last frame into the output pipe and exits, so the
    /// output must still be read meanwhile, or this can wait until it is. Closing again does
    /// nothing.
    pub fn close(&self) {
        let handle = self
            .handle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(handle) = handle {
            // SAFETY: `handle` came from CreatePseudoConsole and was taken out, so it is closed
            // exactly once.
            unsafe { ClosePseudoConsole(handle) };
        }
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        self.close();
    }
}

/// What [`spawn_pty`] started, before it is wrapped up as a [`super::PtyProcess`].
pub struct Spawned {
    pub pid: u32,
    pub job: Job,
    pub exit: watch::Receiver<Option<ExitInfo>>,
    pub output: File,
    pub input: File,
    pub console: Arc<Console>,
}

/// Starts `spec` on a new pseudo console of `size`, inside a new kill-on-close job.
pub fn spawn_pty(spec: &LaunchSpec, size: PtySize) -> io::Result<Spawned> {
    let job = Job::kill_on_close()?;

    // The program reads `input_read` and writes `output_write`; burrowd keeps the other ends.
    let (input_read, input) = pipe()?;
    let (output, output_write) = pipe()?;
    let mut console: HPCON = 0;
    // SAFETY: both handles are valid pipe ends; `console` receives the new handle.
    let result = unsafe {
        CreatePseudoConsole(
            coord(size),
            input_read.as_raw_handle(),
            output_write.as_raw_handle(),
            0,
            &mut console,
        )
    };
    hresult(result)?;
    let console = Arc::new(Console {
        handle: Mutex::new(Some(console)),
    });
    // The console host has its own copies now. Ours must close, or reading the output would
    // never end.
    drop(input_read);
    drop(output_write);

    let raw_console = console
        .handle
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .expect("just created");
    let process = create_process(spec, raw_console, &job)?;
    let pid = process.pid;
    debug!(process = %spec.label, pid, program = %spec.program.display(), "started on a pseudo console");

    let (exit_sender, exit) = watch::channel(None);
    let label = spec.label.clone();
    std::thread::Builder::new()
        .name(format!("wait {label}"))
        .spawn(move || {
            let info = wait(&process.handle);
            debug!(process = %label, exit = %info.description, "main process exited");
            exit_sender.send_replace(Some(info));
        })?;

    Ok(Spawned {
        pid,
        job,
        exit,
        output: File::from(output),
        input: File::from(input),
        console,
    })
}

struct Process {
    pid: u32,
    handle: OwnedHandle,
}

fn create_process(spec: &LaunchSpec, console: HPCON, job: &Job) -> io::Result<Process> {
    let (application, mut command_line) = command_line(&spec.program, &spec.args)?;
    let environment = environment(spec);
    let cwd = spec.cwd.as_deref().map(wide);

    let attributes = AttributeList::new(2)?;
    // Both values are read when the process is created, so they must outlive CreateProcessW.
    let jobs: [HANDLE; 1] = [job.raw()];
    attributes.set(
        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
        console as *const std::ffi::c_void,
        size_of::<HPCON>(),
    )?;
    attributes.set(
        PROC_THREAD_ATTRIBUTE_JOB_LIST,
        jobs.as_ptr().cast(),
        size_of_val(&jobs),
    )?;

    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    // With no handles given, the program gets the pseudo console's, never burrowd's own.
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.lpAttributeList = attributes.raw();

    let mut flags = EXTENDED_STARTUPINFO_PRESENT;
    if environment.is_some() {
        flags |= CREATE_UNICODE_ENVIRONMENT;
    }
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: every pointer is to a value that lives until the call returns: the strings, the
    // environment block, the startup info and its attribute list (and through it `jobs`).
    let ok = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            flags,
            environment
                .as_ref()
                .map_or(ptr::null(), |block| block.as_ptr().cast()),
            cwd.as_ref().map_or(ptr::null(), |cwd| cwd.as_ptr()),
            &startup.StartupInfo,
            &mut info,
        )
    };
    if ok == 0 {
        let error = io::Error::last_os_error();
        return Err(io::Error::new(
            error.kind(),
            format!("starting {} failed: {error}", spec.program.display()),
        ));
    }
    // SAFETY: CreateProcessW succeeded, so both handles are valid and ours to close.
    let (handle, thread) = unsafe {
        (
            OwnedHandle::from_raw_handle(info.hProcess),
            OwnedHandle::from_raw_handle(info.hThread),
        )
    };
    drop(thread);
    Ok(Process {
        pid: info.dwProcessId,
        handle,
    })
}

/// Waits for a process to exit and reads its exit code.
fn wait(process: &OwnedHandle) -> ExitInfo {
    // SAFETY: the handle is valid and has SYNCHRONIZE access, as CreateProcessW gives.
    let waited = unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) };
    if waited != WAIT_OBJECT_0 {
        return ExitInfo {
            code: None,
            description: format!("waiting for it failed: {}", io::Error::last_os_error()),
        };
    }
    let mut code = 0u32;
    // SAFETY: the handle has PROCESS_QUERY_INFORMATION access, as CreateProcessW gives.
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 {
        return ExitInfo {
            code: None,
            description: format!("its exit code is unknown: {}", io::Error::last_os_error()),
        };
    }
    // Exit codes are DWORDs; NTSTATUS crash codes such as 0xC0000005 come out negative, as
    // std's `ExitStatus::code` reports them.
    let code = code as i32;
    ExitInfo {
        code: Some(code),
        description: format!("exit code {code}"),
    }
}

/// A `PROC_THREAD_ATTRIBUTE_LIST`, deleted on drop.
struct AttributeList {
    buffer: Box<[usize]>,
}

impl AttributeList {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0usize;
        // SAFETY: the documented size query: a null list fails and sets `size`.
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &mut size) };
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        // usize-aligned storage, as the list holds pointers.
        let buffer = vec![0usize; size.div_ceil(size_of::<usize>())].into_boxed_slice();
        let list = Self { buffer };
        // SAFETY: the buffer is at least `size` bytes.
        if unsafe { InitializeProcThreadAttributeList(list.raw(), count, 0, &mut size) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(list)
    }

    fn raw(&self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buffer.as_ptr().cast_mut().cast()
    }

    fn set(&self, attribute: u32, value: *const std::ffi::c_void, size: usize) -> io::Result<()> {
        // SAFETY: the list is initialized; the caller keeps `value` alive while the list is used.
        let ok = unsafe {
            UpdateProcThreadAttribute(
                self.raw(),
                0,
                attribute as usize,
                value,
                size,
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: initialized in `new`, deleted once.
        unsafe { DeleteProcThreadAttributeList(self.raw()) };
    }
}

/// An anonymous pipe: (read end, write end). Neither end is inheritable.
fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let (mut read, mut write): (HANDLE, HANDLE) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: plain out-parameters; no security attributes, the default buffer size.
    if unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both are fresh handles, owned from here on.
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        )
    })
}

fn coord(size: PtySize) -> COORD {
    COORD {
        X: size.cols.min(i16::MAX as u16) as i16,
        Y: size.rows.min(i16::MAX as u16) as i16,
    }
}

fn hresult(result: i32) -> io::Result<()> {
    if result == S_OK {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(result))
    }
}

fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

/// The application to run and its command line, both NUL-terminated UTF-16. A `.cmd` or `.bat`
/// runs through `cmd.exe`, like `std::process::Command` runs one.
fn command_line(program: &Path, args: &[OsString]) -> io::Result<(Vec<u16>, Vec<u16>)> {
    let is_batch = program
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"));
    if !is_batch {
        let mut line = OsString::new();
        append_arg(&mut line, program.as_os_str());
        for arg in args {
            line.push(" ");
            append_arg(&mut line, arg);
        }
        return Ok((wide(program), wide(line)));
    }
    let cmd = std::env::var_os("ComSpec")
        .filter(|path| Path::new(path).is_absolute())
        .or_else(|| {
            std::env::var_os("SystemRoot")
                .map(|root| Path::new(&root).join("System32\\cmd.exe").into_os_string())
        })
        .ok_or_else(|| io::Error::other("cmd.exe wasn't found to run a batch file"))?;
    let mut inner = OsString::from("\"");
    inner.push(program.as_os_str());
    inner.push("\"");
    for arg in args {
        inner.push(" ");
        append_batch_arg(&mut inner, arg)?;
    }
    let mut line = OsString::new();
    append_arg(&mut line, &cmd);
    line.push(" /e:ON /v:OFF /d /c \"");
    line.push(inner);
    line.push("\"");
    Ok((wide(&cmd), wide(line)))
}

/// Appends one argument as the C runtime parses command lines: quoted when it has a space, a
/// tab or a quote, or is empty; quotes and the backslashes before them escaped.
fn append_arg(line: &mut OsString, arg: &OsStr) {
    let text = arg.to_string_lossy();
    if !text.is_empty() && !text.contains([' ', '\t', '"']) {
        line.push(arg);
        return;
    }
    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for c in text.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                // n backslashes before a quote become 2n + 1: n escaped, one for the quote.
                quoted.extend(std::iter::repeat_n('\\', backslashes + 1));
            }
            backslashes = 0;
        }
        quoted.push(c);
    }
    // Backslashes before the closing quote are doubled.
    quoted.extend(std::iter::repeat_n('\\', backslashes));
    quoted.push('"');
    line.push(quoted);
}

/// Appends one argument for a batch file. `cmd.exe` has no escaping that works everywhere, so
/// characters it would interpret are refused; the rest is quoted when needed.
fn append_batch_arg(line: &mut OsString, arg: &OsStr) -> io::Result<()> {
    let text = arg
        .to_str()
        .ok_or_else(|| io::Error::other("a batch file argument isn't valid Unicode"))?;
    if text.contains(['"', '%', '!', '^', '\r', '\n']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{text:?} can't be passed to a batch file safely"),
        ));
    }
    if text.is_empty() || text.contains([' ', '\t', '&', '|', '<', '>', '(', ')', ',', ';', '=']) {
        line.push(format!("\"{text}\""));
    } else {
        line.push(text);
    }
    Ok(())
}

/// The environment block, when the program's environment differs from `burrowd`'s.
fn environment(spec: &LaunchSpec) -> Option<Vec<u16>> {
    if spec.env.is_empty() && spec.env_remove.is_empty() {
        return None;
    }
    let key = |name: &OsStr| name.to_string_lossy().to_uppercase();
    let removed: Vec<String> = spec.env_remove.iter().map(|name| key(name)).collect();
    let mut vars: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| !removed.contains(&key(name)))
        .filter(|(name, _)| !spec.env.iter().any(|(set, _)| key(set) == key(name)))
        .collect();
    vars.extend(spec.env.iter().cloned());
    let mut block = Vec::new();
    for (name, value) in vars {
        block.extend(name.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    Some(block)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(program: &str, args: &[&str]) -> String {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let (_, line) = command_line(Path::new(program), &args).unwrap();
        String::from_utf16_lossy(&line[..line.len() - 1])
    }

    #[test]
    fn command_lines_quote_like_the_c_runtime() {
        assert_eq!(
            line("C:\\a\\b.exe", &["x", "y z"]),
            "C:\\a\\b.exe x \"y z\""
        );
        assert_eq!(
            line("C:\\Program Files\\b.exe", &[""]),
            "\"C:\\Program Files\\b.exe\" \"\""
        );
        assert_eq!(line("b.exe", &["say \"hi\""]), "b.exe \"say \\\"hi\\\"\"");
        assert_eq!(line("b.exe", &["dir\\ x\\"]), "b.exe \"dir\\ x\\\\\"");
        assert_eq!(line("b.exe", &["a\\\\b"]), "b.exe a\\\\b");
    }

    #[test]
    fn batch_files_run_through_cmd_with_safe_arguments() {
        let text = line("C:\\npm\\claude.cmd", &["--model", "a b", "x=y"]);
        assert!(
            text.ends_with(
                " /e:ON /v:OFF /d /c \"\"C:\\npm\\claude.cmd\" --model \"a b\" \"x=y\"\""
            ),
            "{text}"
        );
        let args = [OsString::from("100%")];
        assert!(command_line(Path::new("x.bat"), &args).is_err());
    }
}
