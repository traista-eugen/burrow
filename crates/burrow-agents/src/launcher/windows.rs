//! Job Objects and the suspended start.

use std::collections::{HashMap, HashSet};
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::ptr;

use tokio::process::{Child, Command};
use windows_sys::Win32::Foundation::{FILETIME, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, GetExitCodeProcess, GetProcessTimes, OpenProcess,
    OpenThread, PROCESS_QUERY_LIMITED_INFORMATION, ResumeThread, THREAD_SUSPEND_RESUME,
};

/// A Job Object whose processes all die when its last handle closes.
#[derive(Debug)]
pub struct Job {
    handle: OwnedHandle,
}

impl Job {
    /// A new, unnamed job with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    pub fn kill_on_close() -> io::Result<Self> {
        // SAFETY: no security attributes and no name; the handle is owned from here on.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `raw` is a fresh, valid handle that nothing else owns.
        let job = Self {
            handle: unsafe { OwnedHandle::from_raw_handle(raw) },
        };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` is the struct this information class expects, with its exact size.
        let ok = unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn raw(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }

    /// Puts a process into the job. Processes it starts afterwards join the job too.
    pub fn assign(&self, process: RawHandle) -> io::Result<()> {
        // SAFETY: both handles are valid for the call.
        if unsafe { AssignProcessToJobObject(self.raw(), process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Kills every process in the job.
    pub fn terminate(&self) -> io::Result<()> {
        // SAFETY: the job handle is valid.
        if unsafe { TerminateJobObject(self.raw(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// How many processes in the job are still running.
    pub fn active_processes(&self) -> io::Result<u32> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: `info` is the struct this information class expects, with its exact size.
        let ok = unsafe {
            QueryInformationJobObject(
                self.raw(),
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info.ActiveProcesses)
    }
}

/// Starts `command` suspended, with no console window, puts it into a new kill-on-close job,
/// then resumes it. Nothing the process starts can escape the job, because it can't run before
/// it is in it.
pub fn spawn_in_job(mut command: Command) -> io::Result<(Child, Job)> {
    let job = Job::kill_on_close()?;
    command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    let mut child = command.spawn()?;
    let started = child
        .raw_handle()
        .ok_or_else(|| io::Error::other("the process exited before it started"))
        .and_then(|handle| job.assign(handle))
        .and_then(|()| {
            let pid = child
                .id()
                .ok_or_else(|| io::Error::other("the process has no id"))?;
            resume_process(pid)
        });
    if let Err(error) = started {
        // Still suspended, and maybe outside the job: kill it directly.
        let _ = child.start_kill();
        return Err(error);
    }
    Ok((child, job))
}

/// Resumes every thread of a process that was created suspended. A new process has one.
fn resume_process(pid: u32) -> io::Result<()> {
    let snapshot = Snapshot::take(TH32CS_SNAPTHREAD)?;
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..THREADENTRY32::default()
    };
    let mut resumed = 0;
    // SAFETY: `entry` is initialised with its size, as Thread32First requires.
    let mut more = unsafe { Thread32First(snapshot.raw(), &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: a plain open; the handle is owned and closed below.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `thread` is valid and owned by this value from here on.
            let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
            // SAFETY: the handle has THREAD_SUSPEND_RESUME access.
            if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            resumed += 1;
        }
        // SAFETY: as for Thread32First.
        more = unsafe { Thread32Next(snapshot.raw(), &mut entry) } != 0;
    }
    if resumed == 0 {
        return Err(io::Error::other(format!(
            "process {pid} has no thread to resume"
        )));
    }
    Ok(())
}

/// A Toolhelp snapshot, closed on drop.
struct Snapshot(OwnedHandle);

impl Snapshot {
    fn take(flags: u32) -> io::Result<Self> {
        // SAFETY: a plain call; 0 means all processes.
        let raw = unsafe { CreateToolhelp32Snapshot(flags, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a fresh, valid handle owned from here on.
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(raw) }))
    }

    fn raw(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
}

/// Every running process `pid` started, directly or not.
pub fn descendants(pid: u32) -> Vec<u32> {
    let Ok(snapshot) = Snapshot::take(TH32CS_SNAPPROCESS) else {
        return Vec::new();
    };
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..PROCESSENTRY32W::default()
    };
    // SAFETY: `entry` is initialised with its size, as Process32FirstW requires.
    let mut more = unsafe { Process32FirstW(snapshot.raw(), &mut entry) } != 0;
    while more {
        children
            .entry(entry.th32ParentProcessID)
            .or_default()
            .push(entry.th32ProcessID);
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot.raw(), &mut entry) } != 0;
    }
    let mut found = Vec::new();
    let mut seen = HashSet::from([pid]);
    let mut queue = vec![pid];
    let mut started: HashMap<u32, Option<u64>> = HashMap::new();
    let mut start = |pid: u32| *started.entry(pid).or_insert_with(|| started_at(pid));
    while let Some(parent) = queue.pop() {
        for &child in children.get(&parent).into_iter().flatten() {
            // Process ids are reused: a parent id can name a newer, unrelated process that got
            // the id of the child's real, dead parent. A real child started after its parent.
            // And the System Idle Process is its own parent.
            let stale = matches!((start(parent), start(child)), (Some(p), Some(c)) if c < p);
            if !stale && seen.insert(child) {
                found.push(child);
                queue.push(child);
            }
        }
    }
    found
}

/// When process `pid` started, in 100 ns units since 1601, if it can be opened.
fn started_at(pid: u32) -> Option<u64> {
    // SAFETY: a plain open; the handle is owned and closed below.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return None;
    }
    // SAFETY: a fresh, valid handle owned from here on.
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the handle has PROCESS_QUERY_LIMITED_INFORMATION access, and each pointer is to
    // a FILETIME that outlives the call.
    let ok = unsafe {
        GetProcessTimes(
            process.as_raw_handle(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } != 0;
    ok.then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// Whether a process with this id is running.
pub fn is_running(pid: u32) -> bool {
    // SAFETY: a plain open; the handle is owned and closed below.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return false;
    }
    // SAFETY: a fresh, valid handle owned from here on.
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut code = 0u32;
    // SAFETY: the handle has PROCESS_QUERY_LIMITED_INFORMATION access.
    let ok = unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } != 0;
    ok && code == STILL_ACTIVE as u32
}
