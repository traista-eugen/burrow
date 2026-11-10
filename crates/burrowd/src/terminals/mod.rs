//! Terminals: agents' own TUIs on pseudo consoles, emulated in `burrowd` so any client can
//! attach to the same screen (M4, D-038 to D-040).
//!
//! # One terminal
//!
//! A [`Terminal`] is a program on a pseudo console ([`burrow_agents::launcher::spawn_pty`]),
//! inside its own Job Object, plus a [`Screen`] that emulates what it draws. It runs as:
//!
//! - **The reader**, a thread that reads the console's output and feeds it to the screen under
//!   a short lock, then bumps the terminal's version. It never waits for a client: clients read
//!   the screen when they are ready, so a slow one only ever misses intermediate states (D-039).
//! - **The writer**, a thread that writes input to the console: what clients type, and the
//!   replies the emulator owes the program (cursor position reports and the like). Input waiting
//!   for a program that doesn't read is capped at [`MAX_PENDING_INPUT`].
//! - **The supervisor**, a task that waits for the program to exit, kills the rest of its tree,
//!   closes the console and waits for the last of its output. Then the terminal is closed: its
//!   screen stays readable, but it takes no input.
//! - **The sync timer**, a task that ends a synchronized update (mode 2026) whose end never
//!   came, as terminals do after 150 ms.
//!
//! # Clients
//!
//! [`client::Updates`] sends one attached client its `terminal/update`s: at most one every
//! [`client::FRAME`], with how many lines scrolled and only the lines that differ from that
//! client's copy, and only once the previous update has been written to its pipe (D-039). It
//! also answers that client's `terminal/scrollback` requests, in order with its updates. Resize
//! follows the "latest wins" rule (Q7, D-040): the connection keeps the size each client last
//! asked for.
//!
//! [`Terminals`] holds every terminal by id. Closed ones stay attachable, with their last
//! screen and scrollback, until their session is archived or more than [`KEEP_CLOSED`] have
//! closed since.

pub mod client;
pub mod screen;

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use burrow_agents::launcher::{self, LaunchSpec, ProcessHandle, PseudoConsole, PtySize};
use burrow_protocol::{
    ErrorKind, ErrorObject, TerminalExit, TerminalScrollbackResult, TerminalSize, TerminalSnapshot,
    TerminalUpdate,
};
use tokio::sync::{Notify, watch};
use tracing::{debug, info, warn};

use crate::ids;
pub use screen::{Frame, Screen};

/// Input waiting for a program that doesn't read it, beyond which `terminal/input` is refused.
pub const MAX_PENDING_INPUT: usize = 1024 * 1024;
/// Closed terminals kept attachable.
pub const KEEP_CLOSED: usize = 32;
/// How long the console's output may take to end once the console is closed.
const OUTPUT_WAIT: Duration = Duration::from_secs(5);
/// How much output the reader takes at once.
const READ_BUFFER: usize = 64 * 1024;
/// The most lines `terminal/scrollback` returns.
pub const MAX_SCROLLBACK_PAGE: u32 = 1000;
/// Lines `terminal/scrollback` returns by default.
pub const DEFAULT_SCROLLBACK_PAGE: u32 = 100;

/// Every terminal, by id.
#[derive(Debug, Default)]
pub struct Terminals {
    inner: Mutex<Registry>,
}

#[derive(Debug, Default)]
struct Registry {
    terminals: HashMap<String, Arc<Terminal>>,
    /// Closed terminals, oldest first.
    closed: VecDeque<String>,
}

impl Terminals {
    /// No terminals yet.
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts `spec` on a new pseudo console of `size` and starts emulating it. Must be called
    /// on the tokio runtime.
    pub fn open(
        self: &Arc<Self>,
        spec: &LaunchSpec,
        size: TerminalSize,
    ) -> io::Result<Arc<Terminal>> {
        let pty = launcher::spawn_pty(
            spec,
            PtySize {
                cols: size.cols,
                rows: size.rows,
            },
        )?;
        let id = ids::new_id("trm");
        let (input, queue) = std_mpsc::channel::<Vec<u8>>();
        let (exit, _) = watch::channel(None);
        let terminal = Arc::new(Terminal {
            id: id.clone(),
            label: spec.label.clone(),
            state: Mutex::new(State {
                screen: Screen::new(size),
                exit: None,
            }),
            changed: watch::channel(0).0,
            input,
            pending: Arc::new(AtomicUsize::new(0)),
            process: pty.handle,
            console: pty.console,
            stopping: AtomicBool::new(false),
            exit,
            sync: Notify::new(),
        });

        let pending = terminal.pending.clone();
        let label = spec.label.clone();
        let input_file = pty.input;
        std::thread::Builder::new()
            .name(format!("input {label}"))
            .spawn(move || write_input(input_file, &queue, &pending, &label))?;
        let (output_done, output_ended) = tokio::sync::oneshot::channel();
        let reader = terminal.clone();
        let output = pty.output;
        std::thread::Builder::new()
            .name(format!("output {}", spec.label))
            .spawn(move || {
                read_output(&reader, output);
                let _ = output_done.send(());
            })?;
        tokio::spawn(supervise(
            terminal.clone(),
            output_ended,
            Arc::downgrade(self),
        ));
        tokio::spawn(sync_timer(terminal.clone()));

        self.registry()
            .terminals
            .insert(id.clone(), terminal.clone());
        info!(terminal = id, process = %spec.label, pid = terminal.process.pid(), cols = size.cols, rows = size.rows, "terminal opened");
        Ok(terminal)
    }

    /// The terminal with this id.
    pub fn get(&self, id: &str) -> Result<Arc<Terminal>, ErrorObject> {
        self.registry()
            .terminals
            .get(id)
            .cloned()
            .ok_or_else(|| ErrorObject::new(ErrorKind::NotFound, format!("no terminal {id}")))
    }

    /// Forgets a terminal, for example when its session is archived. Clients still attached
    /// keep their copy until they detach.
    pub fn remove(&self, id: &str) {
        let mut registry = self.registry();
        registry.terminals.remove(id);
        registry.closed.retain(|closed| closed != id);
    }

    /// Keeps at most [`KEEP_CLOSED`] closed terminals.
    fn closed(&self, id: &str) {
        let mut registry = self.registry();
        registry.closed.push_back(id.to_owned());
        while registry.closed.len() > KEEP_CLOSED {
            if let Some(oldest) = registry.closed.pop_front() {
                registry.terminals.remove(&oldest);
                debug!(terminal = oldest, "forgot a closed terminal");
            }
        }
    }
}

/// One terminal: a program on a pseudo console, and its emulated screen.
#[derive(Debug)]
pub struct Terminal {
    id: String,
    label: String,
    state: Mutex<State>,
    /// Bumped after every change to the screen; attached clients wait on it.
    changed: watch::Sender<u64>,
    input: std_mpsc::Sender<Vec<u8>>,
    /// Input bytes the writer hasn't written yet.
    pending: Arc<AtomicUsize>,
    process: ProcessHandle,
    console: PseudoConsole,
    /// Burrow ended the program, so its exit code means nothing.
    stopping: AtomicBool,
    /// Set once the terminal is closed.
    exit: watch::Sender<Option<TerminalExit>>,
    /// Wakes the sync timer.
    sync: Notify,
}

#[derive(Debug)]
struct State {
    screen: Screen,
    exit: Option<TerminalExit>,
}

impl Terminal {
    /// The terminal's id (`trm_…`).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The program's process id.
    pub fn pid(&self) -> u32 {
        self.process.pid()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn bump(&self) {
        self.changed.send_modify(|version| *version += 1);
    }

    /// The screen now, and how the program ended if it has.
    fn frame(&self) -> (Frame, Option<TerminalExit>) {
        let mut state = self.state();
        (state.screen.frame(), state.exit.clone())
    }

    /// For a client that has `sent`: the screen now, the update that brings the client to it
    /// (`None` when nothing changed), and how the program ended if it has.
    pub fn update_for(
        &self,
        sent: &Frame,
    ) -> (Frame, Option<TerminalUpdate>, Option<TerminalExit>) {
        let mut state = self.state();
        let now = state.screen.frame();
        let update = state.screen.diff(&self.id, sent, &now);
        (now, update, state.exit.clone())
    }

    /// For `terminal/attach`: the whole screen, the frame it was made from, and a receiver
    /// that wakes on every change after it.
    pub fn attach(&self) -> (TerminalSnapshot, Frame, watch::Receiver<u64>) {
        // Subscribed first, so no change can fall between the snapshot and the updates.
        let changed = self.changed.subscribe();
        let (frame, exit) = self.frame();
        let snapshot = TerminalSnapshot {
            terminal_id: self.id.clone(),
            cols: frame.cols,
            rows: frame.rows,
            cursor: frame.cursor.clone(),
            lines: frame.all_lines(),
            modes: frame.modes.clone(),
            title: frame.title.clone(),
            scrollback: frame.scrollback,
            exit,
        };
        (snapshot, frame, changed)
    }

    /// For `terminal/scrollback`: lines above index `before`, relative to `seen`, the screen an
    /// attached client has, or else to the screen now.
    pub fn page(
        &self,
        seen: Option<&Frame>,
        before: Option<i64>,
        limit: Option<u32>,
    ) -> TerminalScrollbackResult {
        let limit = limit
            .unwrap_or(DEFAULT_SCROLLBACK_PAGE)
            .min(MAX_SCROLLBACK_PAGE) as usize;
        let mut state = self.state();
        let now;
        let seen = match seen {
            Some(seen) => seen,
            None => {
                now = state.screen.frame();
                &now
            }
        };
        state.screen.page(seen, before.unwrap_or(0), limit)
    }

    /// Sends what the user typed to the program.
    pub fn input(&self, data: &[u8]) -> Result<(), ErrorObject> {
        if self.state().exit.is_some() {
            return Err(ErrorObject::new(
                ErrorKind::InvalidState,
                format!("terminal {}'s program has exited", self.id),
            ));
        }
        if data.is_empty() {
            return Ok(());
        }
        let pending = self.pending.fetch_add(data.len(), Ordering::AcqRel);
        if pending + data.len() > MAX_PENDING_INPUT {
            self.pending.fetch_sub(data.len(), Ordering::AcqRel);
            return Err(ErrorObject::new(
                ErrorKind::InvalidState,
                format!(
                    "terminal {}'s program isn't reading its input; {pending} bytes are waiting",
                    self.id
                ),
            ));
        }
        if self.input.send(data.to_vec()).is_err() {
            self.pending.fetch_sub(data.len(), Ordering::AcqRel);
            return Err(ErrorObject::new(
                ErrorKind::InvalidState,
                format!("terminal {} no longer takes input", self.id),
            ));
        }
        Ok(())
    }

    /// Changes the size of the screen and the console. Returns whether it changed.
    pub fn resize(&self, size: TerminalSize) -> Result<bool, ErrorObject> {
        if !size.is_valid() {
            return Err(ErrorObject::invalid_params(format!(
                "a terminal has {}–{} columns and {}–{} rows, not {}x{}",
                TerminalSize::MIN_COLS,
                TerminalSize::MAX_COLS,
                TerminalSize::MIN_ROWS,
                TerminalSize::MAX_ROWS,
                size.cols,
                size.rows
            )));
        }
        let mut state = self.state();
        if state.exit.is_some() {
            return Err(ErrorObject::new(
                ErrorKind::InvalidState,
                format!("terminal {}'s program has exited", self.id),
            ));
        }
        if state.screen.size() == size {
            return Ok(false);
        }
        // The screen first, so that what the program draws for the new size lands in it.
        state.screen.resize(size);
        if let Err(error) = self.console.resize(PtySize {
            cols: size.cols,
            rows: size.rows,
        }) {
            warn!(terminal = self.id, %error, "resizing the pseudo console failed");
        }
        drop(state);
        debug!(
            terminal = self.id,
            cols = size.cols,
            rows = size.rows,
            "resized"
        );
        self.bump();
        Ok(true)
    }

    /// Kills the program's tree now, without waiting; the terminal closes shortly after.
    pub fn kill(&self) {
        self.stopping.store(true, Ordering::Release);
        self.process.kill();
    }

    /// Kills the program's tree and waits until the terminal is closed: the tree and the
    /// console's host are gone.
    pub async fn stop(&self) -> TerminalExit {
        self.kill();
        self.closed().await
    }

    /// Waits until the terminal is closed, and says how its program ended.
    pub async fn closed(&self) -> TerminalExit {
        let mut exit = self.exit.subscribe();
        match exit.wait_for(Option::is_some).await {
            Ok(exit) => exit.clone().unwrap_or_default(),
            Err(_) => TerminalExit::default(),
        }
    }

    /// How the program ended, once the terminal is closed.
    pub fn exit(&self) -> Option<TerminalExit> {
        self.exit.borrow().clone()
    }
}

/// The reader: feeds the console's output to the screen until it ends.
fn read_output(terminal: &Terminal, mut output: File) {
    let mut buffer = vec![0u8; READ_BUFFER];
    loop {
        let read = match output.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                // A broken pipe is how the output ends once the console's host exits.
                if error.kind() != io::ErrorKind::BrokenPipe {
                    debug!(terminal = terminal.id, %error, "reading the console failed");
                }
                break;
            }
        };
        let (replies, syncing) = {
            let mut state = terminal.state();
            let replies = state.screen.feed(&buffer[..read]);
            (replies, state.screen.sync_deadline().is_some())
        };
        if !replies.is_empty() {
            let _ = terminal.input(&replies);
        }
        if syncing {
            terminal.sync.notify_one();
        }
        terminal.bump();
    }
    debug!(terminal = terminal.id, "console output ended");
}

/// The writer: writes queued input to the console, in order, until the terminal is gone or the
/// console stops taking it.
fn write_input(
    mut input: File,
    queue: &std_mpsc::Receiver<Vec<u8>>,
    pending: &AtomicUsize,
    label: &str,
) {
    for data in queue {
        let written = input.write_all(&data).and_then(|()| input.flush());
        pending.fetch_sub(data.len(), Ordering::AcqRel);
        if let Err(error) = written {
            debug!(process = label, %error, "the console stopped taking input");
            break;
        }
    }
    // Input that arrives later is dropped with the queue.
    for data in queue.try_iter() {
        pending.fetch_sub(data.len(), Ordering::AcqRel);
    }
}

/// Waits for the program to exit, then closes the terminal.
async fn supervise(
    terminal: Arc<Terminal>,
    output_ended: tokio::sync::oneshot::Receiver<()>,
    terminals: Weak<Terminals>,
) {
    let exit = terminal.process.exited().await;
    let stopped = terminal.stopping.load(Ordering::Acquire);
    // What the program started goes with it, as for structured sessions (D-007).
    terminal.process.stop(Duration::ZERO).await;
    // Closing the console makes its host paint a last frame and exit; the reader keeps
    // reading meanwhile, which closing needs.
    let console = terminal.console.clone();
    let _ = tokio::task::spawn_blocking(move || console.close()).await;
    if tokio::time::timeout(OUTPUT_WAIT, output_ended)
        .await
        .is_err()
    {
        warn!(
            terminal = terminal.id,
            "the console's output didn't end after it was closed"
        );
    }
    let exit = TerminalExit {
        exit_code: if stopped { None } else { exit.code },
    };
    terminal.state().exit = Some(exit.clone());
    terminal.bump();
    terminal.exit.send_replace(Some(exit.clone()));
    info!(terminal = terminal.id, process = %terminal.label, exit_code = ?exit.exit_code, stopped, "terminal closed");
    if let Some(terminals) = terminals.upgrade() {
        terminals.closed(&terminal.id);
    }
}

/// Ends synchronized updates whose end didn't come in time.
async fn sync_timer(terminal: Arc<Terminal>) {
    let mut closed = terminal.exit.subscribe();
    loop {
        tokio::select! {
            () = terminal.sync.notified() => {}
            _ = closed.wait_for(Option::is_some) => return,
        }
        loop {
            let Some(deadline) = terminal.state().screen.sync_deadline() else {
                break;
            };
            tokio::time::sleep_until(deadline.into()).await;
            let replies = {
                let mut state = terminal.state();
                match state.screen.sync_deadline() {
                    Some(deadline) if deadline <= std::time::Instant::now() => {
                        state.screen.end_sync()
                    }
                    _ => continue,
                }
            };
            if !replies.is_empty() {
                let _ = terminal.input(&replies);
            }
            terminal.bump();
        }
    }
}
