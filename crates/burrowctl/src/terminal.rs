//! `burrowctl terminal`: a second client for terminal sessions.
//!
//! `attach` puts this console in raw mode on its alternate screen, draws the terminal's screen
//! with ANSI sequences as `terminal/update`s arrive, and forwards keystrokes as
//! `terminal/input`. The terminal's input modes (application cursor keys, bracketed paste,
//! mouse reporting, focus events) are set on this console too, so it encodes keys and the mouse
//! the way the program asked, and the bytes are forwarded unchanged. Ctrl+] then `d` detaches;
//! Ctrl+] twice sends one Ctrl+].
//!
//! Resizing follows "latest wins" (D-040): attaching doesn't change the terminal's size; the
//! first keystroke, and any resize of this console, makes it this console's size.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::time::Duration;

use burrow_client::{Client, Notifications};
use burrow_protocol::NotificationMethod;
use burrow_protocol::methods::{
    SessionGet, TerminalAttach, TerminalDetach, TerminalInput, TerminalResize, TerminalScrollback,
};
use burrow_protocol::notifications::{TerminalClosedNotification, TerminalUpdated};
use burrow_protocol::{
    Color, CursorShape, Event, MouseTracking, SessionIdParams, StreamName, TerminalCursor,
    TerminalIdParams, TerminalInputParams, TerminalLine, TerminalModes, TerminalResizeParams,
    TerminalRun, TerminalScrollbackParams, TerminalSize, TerminalSnapshot, TerminalUpdate,
};
use tokio::sync::mpsc;

/// Ctrl+], the first key of the detach chord.
const ESCAPE: u8 = 0x1d;
/// How often the console's size is checked.
const SIZE_POLL: Duration = Duration::from_millis(200);
/// How long a session's stream may take to announce its terminal.
const ANNOUNCE_WAIT: Duration = Duration::from_secs(10);

/// The terminal `id` names: a terminal id as it is, or a session's terminal: its `terminalId`,
/// or what its stream announced, from an older `burrowd`.
pub async fn resolve(client: &Client, id: &str) -> Result<String, String> {
    if id.starts_with("trm_") {
        return Ok(id.to_owned());
    }
    let session = client
        .call::<SessionGet>(&SessionIdParams {
            session_id: id.to_owned(),
        })
        .await
        .map_err(|error| error.to_string())?;
    if let Some(terminal) = session.terminal_id {
        return Ok(terminal);
    }
    let mut events = client
        .subscribe(StreamName::session(id), Some(0))
        .await
        .map_err(|error| error.to_string())?;
    let found = tokio::time::timeout(ANNOUNCE_WAIT, async {
        while let Some(event) = events.next().await {
            if let Event::TerminalOpened(opened) = event.event {
                return Some(opened.terminal_id);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    let _ = events.unsubscribe().await;
    found.ok_or_else(|| format!("{id} has no terminal: is it a terminal session?"))
}

/// This console's size, to start a terminal session at with `session new --attach`, so its
/// program draws its first screen at the size it is shown. `None` outside a console.
pub fn console_size() -> Option<TerminalSize> {
    let (cols, rows) = console::Console::open().ok()?.size();
    Some(TerminalSize {
        cols: cols.clamp(TerminalSize::MIN_COLS, TerminalSize::MAX_COLS),
        rows: rows.clamp(TerminalSize::MIN_ROWS, TerminalSize::MAX_ROWS),
    })
}

/// `terminal show`: the screen now, as plain text.
pub async fn show(client: &Client, terminal: &str) -> Result<TerminalSnapshot, String> {
    let params = TerminalIdParams {
        terminal_id: terminal.to_owned(),
    };
    let snapshot = client
        .call::<TerminalAttach>(&params)
        .await
        .map_err(|error| error.to_string())?;
    client
        .call::<TerminalDetach>(&params)
        .await
        .map_err(|error| error.to_string())?;
    Ok(snapshot)
}

/// `terminal scrollback`: every scrollback line, oldest first, up to `limit`.
pub async fn scrollback(
    client: &Client,
    terminal: &str,
    limit: Option<usize>,
) -> Result<Vec<TerminalLine>, String> {
    let mut lines: Vec<TerminalLine> = Vec::new();
    let mut before = 0;
    loop {
        let page = client
            .call::<TerminalScrollback>(&TerminalScrollbackParams {
                terminal_id: terminal.to_owned(),
                before: Some(before),
                limit: Some(1000),
            })
            .await
            .map_err(|error| error.to_string())?;
        let complete = page.complete || page.lines.is_empty();
        if let Some(first) = page.lines.first() {
            before = first.index;
        }
        let mut older = page.lines;
        older.append(&mut lines);
        lines = older;
        if complete || limit.is_some_and(|limit| lines.len() >= limit) {
            break;
        }
    }
    if let Some(limit) = limit {
        let excess = lines.len().saturating_sub(limit);
        lines.drain(..excess);
    }
    Ok(lines)
}

/// How an attachment ended.
pub enum Ended {
    /// The user detached.
    Detached,
    /// The program exited, with its exit code if it had one.
    Closed(Option<i32>),
}

/// `terminal attach`: draws the terminal in this console and types into it until the user
/// detaches or the program exits.
pub async fn attach(client: &Client, terminal: &str) -> Result<Ended, String> {
    let mut console = console::Console::open()
        .map_err(|error| format!("`terminal attach` needs to run in a console window: {error}"))?;
    // Listen first: notifications that arrive while nobody listens are dropped.
    let notifications = client.notifications();
    let snapshot = client
        .call::<TerminalAttach>(&TerminalIdParams {
            terminal_id: terminal.to_owned(),
        })
        .await
        .map_err(|error| error.to_string())?;
    if let Some(exit) = &snapshot.exit {
        return Ok(Ended::Closed(exit.exit_code));
    }
    console.raw().map_err(|error| error.to_string())?;
    let mut view = View::new(terminal, snapshot, console.size());
    view.start();
    let ended = view.run(client, notifications, &console).await;
    view.finish();
    drop(console);
    let _ = client
        .call::<TerminalDetach>(&TerminalIdParams {
            terminal_id: terminal.to_owned(),
        })
        .await;
    ended
}

/// The terminal as this console shows it.
struct View {
    terminal: String,
    lines: Vec<TerminalLine>,
    cursor: TerminalCursor,
    modes: TerminalModes,
    title: Option<String>,
    /// This console's size, in cells.
    console: (u16, u16),
    /// The size this client last asked for; `None` until it types or resizes.
    asked: Option<TerminalSize>,
    /// Ctrl+] was pressed: the next key decides.
    escaped: bool,
}

impl View {
    fn new(terminal: &str, snapshot: TerminalSnapshot, console: (u16, u16)) -> Self {
        Self {
            terminal: terminal.to_owned(),
            lines: snapshot.lines,
            cursor: snapshot.cursor,
            modes: snapshot.modes,
            title: snapshot.title,
            console,
            asked: None,
            escaped: false,
        }
    }

    /// The alternate screen, without autowrap so long rows are cut at the console's edge, then
    /// the whole terminal.
    fn start(&mut self) {
        let mut out = String::from("\x1b[?1049h\x1b[?7l");
        self.draw_all(&mut out);
        write_title(&mut out, self.title.as_deref(), &self.terminal);
        let modes = std::mem::take(&mut self.modes);
        set_modes(&mut out, &modes, &TerminalModes::default());
        self.modes = modes;
        flush(&out);
    }

    /// Restores this console: its modes, autowrap, the cursor and the main screen.
    fn finish(&self) {
        let mut out = String::new();
        set_modes(&mut out, &TerminalModes::default(), &self.modes);
        out.push_str("\x1b[0m\x1b[0 q\x1b[?25h\x1b[?7h\x1b[?1049l");
        flush(&out);
    }

    async fn run(
        &mut self,
        client: &Client,
        mut notifications: Notifications,
        console: &console::Console,
    ) -> Result<Ended, String> {
        let mut keys = read_keys();
        let mut poll = tokio::time::interval(SIZE_POLL);
        loop {
            tokio::select! {
                notification = notifications.next() => {
                    let Some(notification) = notification else {
                        return Err("burrowd closed the connection".into());
                    };
                    match notification.method.as_str() {
                        TerminalUpdated::NAME => {
                            if let Ok(update) = notification.params_as::<TerminalUpdated>()
                                && update.terminal_id == self.terminal
                            {
                                self.apply(update);
                            }
                        }
                        TerminalClosedNotification::NAME => {
                            if let Ok(closed) = notification.params_as::<TerminalClosedNotification>()
                                && closed.terminal_id == self.terminal
                            {
                                return Ok(Ended::Closed(closed.exit_code));
                            }
                        }
                        _ => {}
                    }
                }
                keys = keys.recv() => {
                    let Some(keys) = keys else {
                        return Ok(Ended::Detached);
                    };
                    let Some(data) = self.keys(&keys) else {
                        return Ok(Ended::Detached);
                    };
                    if !data.is_empty() {
                        if self.asked.is_none() {
                            self.ask_size(client).await?;
                        }
                        client
                            .call::<TerminalInput>(&TerminalInputParams {
                                terminal_id: self.terminal.clone(),
                                data,
                            })
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                }
                _ = poll.tick() => {
                    let size = console.size();
                    if size != self.console {
                        self.console = size;
                        self.ask_size(client).await?;
                        let mut out = String::from("\x1b[2J");
                        self.draw_all(&mut out);
                        flush(&out);
                    }
                }
            }
        }
    }

    /// Asks for this console's size (D-040).
    async fn ask_size(&mut self, client: &Client) -> Result<(), String> {
        let size = TerminalSize {
            cols: self
                .console
                .0
                .clamp(TerminalSize::MIN_COLS, TerminalSize::MAX_COLS),
            rows: self
                .console
                .1
                .clamp(TerminalSize::MIN_ROWS, TerminalSize::MAX_ROWS),
        };
        self.asked = Some(size);
        client
            .call::<TerminalResize>(&TerminalResizeParams {
                terminal_id: self.terminal.clone(),
                cols: size.cols,
                rows: size.rows,
            })
            .await
            .map(drop)
            .map_err(|error| error.to_string())
    }

    /// What to send for typed bytes; `None` to detach.
    fn keys(&mut self, keys: &[u8]) -> Option<String> {
        let mut data = Vec::with_capacity(keys.len());
        for &key in keys {
            if self.escaped {
                self.escaped = false;
                match key {
                    b'd' | b'D' => return None,
                    ESCAPE => data.push(ESCAPE),
                    other => data.extend([ESCAPE, other]),
                }
            } else if key == ESCAPE {
                self.escaped = true;
            } else {
                data.push(key);
            }
        }
        Some(String::from_utf8_lossy(&data).into_owned())
    }

    fn apply(&mut self, update: TerminalUpdate) {
        let mut out = String::new();
        let mut redraw_all = false;
        if update.reset
            && let Some(rows) = update.rows
        {
            self.lines = (0..i64::from(rows)).map(blank).collect();
            out.push_str("\x1b[2J");
            redraw_all = true;
        }
        if update.scrolled > 0 {
            let rows = self.lines.len();
            let moved = (update.scrolled as usize).min(rows);
            self.lines.drain(..moved);
            self.lines.extend((0..moved).map(|_| blank(0)));
            redraw_all = true;
        }
        let mut changed = Vec::new();
        for line in update.lines {
            // The scrollback isn't shown here.
            if let Ok(row) = usize::try_from(line.index)
                && row < self.lines.len()
            {
                changed.push(row);
                self.lines[row] = line;
            }
        }
        if redraw_all {
            self.draw_rows(&mut out, 0..self.lines.len());
        } else {
            for row in changed {
                self.draw_rows(&mut out, row..row + 1);
            }
        }
        if let Some(modes) = update.modes {
            set_modes(&mut out, &modes, &self.modes);
            self.modes = modes;
        }
        if let Some(title) = update.title {
            self.title = (!title.is_empty()).then_some(title);
            write_title(&mut out, self.title.as_deref(), &self.terminal);
        }
        if update.bell {
            out.push('\x07');
        }
        self.cursor = update.cursor;
        self.draw_cursor(&mut out);
        flush(&out);
    }

    fn draw_all(&self, out: &mut String) {
        self.draw_rows(out, 0..self.lines.len());
        self.draw_cursor(out);
    }

    /// Draws rows, as far as this console has room.
    fn draw_rows(&self, out: &mut String, rows: std::ops::Range<usize>) {
        for row in rows {
            if row >= usize::from(self.console.1) {
                break;
            }
            let _ = write!(out, "\x1b[{};1H", row + 1);
            for run in &self.lines[row].runs {
                style(out, run);
                // Control characters never reach this console as such.
                out.extend(
                    run.text
                        .chars()
                        .map(|c| if c.is_control() { '\u{fffd}' } else { c }),
                );
            }
            out.push_str("\x1b[0m\x1b[K");
        }
    }

    fn draw_cursor(&self, out: &mut String) {
        let cursor = &self.cursor;
        let shape = match cursor.shape {
            CursorShape::Underline => 4,
            CursorShape::Bar => 6,
            _ => 2,
        };
        let _ = write!(
            out,
            "\x1b[{};{}H\x1b[{shape} q{}",
            cursor.row + 1,
            cursor.col + 1,
            if cursor.visible {
                "\x1b[?25h"
            } else {
                "\x1b[?25l"
            }
        );
    }
}

fn blank(index: i64) -> TerminalLine {
    TerminalLine {
        index,
        ..TerminalLine::default()
    }
}

/// SGR for a run's style, from the default.
fn style(out: &mut String, run: &TerminalRun) {
    out.push_str("\x1b[0");
    for (bit, code) in [
        (TerminalRun::BOLD, ";1"),
        (TerminalRun::DIM, ";2"),
        (TerminalRun::ITALIC, ";3"),
        (TerminalRun::UNDERLINE, ";4"),
        (TerminalRun::UNDERCURL, ";4:3"),
        (TerminalRun::INVERSE, ";7"),
        (TerminalRun::HIDDEN, ";8"),
        (TerminalRun::STRIKETHROUGH, ";9"),
    ] {
        if run.flags & bit != 0 {
            out.push_str(code);
        }
    }
    if let Some(fg) = run.fg {
        color(out, fg, 30, 90, 38);
    }
    if let Some(bg) = run.bg {
        color(out, bg, 40, 100, 48);
    }
    out.push('m');
}

fn color(out: &mut String, color: Color, base: u8, bright: u8, extended: u8) {
    let _ = match color {
        Color::Indexed(index @ 0..=7) => write!(out, ";{}", base + index),
        Color::Indexed(index @ 8..=15) => write!(out, ";{}", bright + index - 8),
        Color::Indexed(index) => write!(out, ";{extended};5;{index}"),
        Color::Rgb(r, g, b) => write!(out, ";{extended};2;{r};{g};{b}"),
    };
}

/// Sets this console's input modes from `old` to `new`.
fn set_modes(out: &mut String, new: &TerminalModes, old: &TerminalModes) {
    let mut set = |on: bool, was: bool, mode: &str| {
        if on != was {
            let _ = write!(out, "\x1b[?{mode}{}", if on { 'h' } else { 'l' });
        }
    };
    set(new.app_cursor, old.app_cursor, "1");
    set(new.bracketed_paste, old.bracketed_paste, "2004");
    set(new.focus_events, old.focus_events, "1004");
    set(new.mouse_sgr, old.mouse_sgr, "1006");
    set(new.alternate_scroll, old.alternate_scroll, "1007");
    let mouse = |tracking: &Option<MouseTracking>| match tracking {
        Some(MouseTracking::Click) => Some("1000"),
        Some(MouseTracking::Drag) => Some("1002"),
        Some(MouseTracking::Motion) => Some("1003"),
        _ => None,
    };
    if new.mouse != old.mouse {
        if let Some(mode) = mouse(&old.mouse) {
            let _ = write!(out, "\x1b[?{mode}l");
        }
        if let Some(mode) = mouse(&new.mouse) {
            let _ = write!(out, "\x1b[?{mode}h");
        }
    }
    if new.app_keypad != old.app_keypad {
        out.push_str(if new.app_keypad { "\x1b=" } else { "\x1b>" });
    }
}

fn write_title(out: &mut String, title: Option<&str>, terminal: &str) {
    let title: String = title
        .unwrap_or(terminal)
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let _ = write!(out, "\x1b]0;{title} (Ctrl+] d detaches)\x07");
}

fn flush(out: &str) {
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(out.as_bytes());
    let _ = stdout.flush();
}

/// Reads the console's input on a thread of its own; reads block.
fn read_keys() -> mpsc::UnboundedReceiver<Vec<u8>> {
    let (keys, receiver) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buffer = [0u8; 4096];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if keys.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    receiver
}

#[cfg(windows)]
mod console {
    use std::io;

    use windows_sys::Win32::System::Console::{
        CONSOLE_MODE, CONSOLE_SCREEN_BUFFER_INFO, DISABLE_NEWLINE_AUTO_RETURN,
        ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        GetConsoleMode, GetConsoleScreenBufferInfo, GetStdHandle, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetConsoleMode,
    };

    /// This process's console: put in raw mode, and restored when dropped.
    pub struct Console {
        modes: Option<(CONSOLE_MODE, CONSOLE_MODE)>,
    }

    fn input() -> windows_sys::Win32::Foundation::HANDLE {
        // SAFETY: a plain call for the process's own standard handle.
        unsafe { GetStdHandle(STD_INPUT_HANDLE) }
    }

    fn output() -> windows_sys::Win32::Foundation::HANDLE {
        // SAFETY: as above.
        unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }
    }

    fn mode(handle: windows_sys::Win32::Foundation::HANDLE) -> io::Result<CONSOLE_MODE> {
        let mut mode = 0;
        // SAFETY: `mode` is a valid out-pointer; an invalid handle just fails.
        if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(mode)
    }

    impl Console {
        /// Checks that stdin and stdout are a console.
        pub fn open() -> io::Result<Self> {
            mode(input())?;
            mode(output())?;
            Ok(Self { modes: None })
        }

        /// Raw input with keys as VT sequences, and VT output.
        pub fn raw(&mut self) -> io::Result<()> {
            let (input_mode, output_mode) = (mode(input())?, mode(output())?);
            // SAFETY: plain calls on the standard handles.
            unsafe {
                if SetConsoleMode(input(), ENABLE_VIRTUAL_TERMINAL_INPUT) == 0
                    || SetConsoleMode(
                        output(),
                        output_mode
                            | ENABLE_PROCESSED_OUTPUT
                            | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                            | DISABLE_NEWLINE_AUTO_RETURN,
                    ) == 0
                {
                    let error = io::Error::last_os_error();
                    SetConsoleMode(input(), input_mode);
                    SetConsoleMode(output(), output_mode);
                    return Err(error);
                }
            }
            self.modes = Some((input_mode, output_mode));
            Ok(())
        }

        /// The console window's size: (columns, rows).
        pub fn size(&self) -> (u16, u16) {
            let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
            // SAFETY: `info` is the struct the call fills.
            if unsafe { GetConsoleScreenBufferInfo(output(), &mut info) } == 0 {
                return (80, 24);
            }
            let window = info.srWindow;
            (
                u16::try_from(window.Right - window.Left + 1).unwrap_or(80),
                u16::try_from(window.Bottom - window.Top + 1).unwrap_or(24),
            )
        }
    }

    impl Drop for Console {
        fn drop(&mut self) {
            if let Some((input_mode, output_mode)) = self.modes {
                // SAFETY: plain calls on the standard handles.
                unsafe {
                    SetConsoleMode(input(), input_mode);
                    SetConsoleMode(output(), output_mode);
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod console {
    use std::io;

    pub struct Console;

    impl Console {
        pub fn open() -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "only on Windows for now",
            ))
        }

        pub fn raw(&mut self) -> io::Result<()> {
            Ok(())
        }

        pub fn size(&self) -> (u16, u16) {
            (80, 24)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_protocol::TerminalCursor;

    fn view() -> View {
        View::new(
            "trm_1",
            TerminalSnapshot {
                terminal_id: "trm_1".into(),
                cols: 10,
                rows: 2,
                cursor: TerminalCursor {
                    row: 0,
                    col: 0,
                    visible: true,
                    shape: CursorShape::Block,
                },
                lines: vec![blank(0), blank(1)],
                modes: TerminalModes::default(),
                title: None,
                scrollback: 0,
                exit: None,
            },
            (80, 24),
        )
    }

    #[test]
    fn ctrl_close_bracket_then_d_detaches() {
        let mut view = view();
        assert_eq!(view.keys(b"ab").as_deref(), Some("ab"));
        assert_eq!(view.keys(&[ESCAPE]).as_deref(), Some(""));
        assert_eq!(view.keys(b"d"), None);
        assert_eq!(view.keys(&[ESCAPE, ESCAPE]).as_deref(), Some("\x1d"));
        assert_eq!(view.keys(&[ESCAPE, b'x']).as_deref(), Some("\x1dx"));
    }

    #[test]
    fn runs_become_sgr_and_control_characters_are_never_drawn() {
        let mut out = String::new();
        style(
            &mut out,
            &TerminalRun {
                text: String::new(),
                fg: Some(Color::Indexed(9)),
                bg: Some(Color::Rgb(1, 2, 3)),
                flags: TerminalRun::BOLD | TerminalRun::INVERSE,
            },
        );
        assert_eq!(out, "\x1b[0;1;7;91;48;2;1;2;3m");

        let mut view = view();
        view.lines[0].runs = vec![TerminalRun::plain("a\x1b[2Jb")];
        let mut out = String::new();
        view.draw_rows(&mut out, 0..1);
        assert!(!out.contains("\x1b[2J"), "{out:?}");
        assert!(out.contains("a\u{fffd}[2Jb"), "{out:?}");
    }

    #[test]
    fn modes_are_mirrored_both_ways() {
        let mut out = String::new();
        let on = TerminalModes {
            app_cursor: true,
            bracketed_paste: true,
            mouse: Some(MouseTracking::Drag),
            mouse_sgr: true,
            ..TerminalModes::default()
        };
        set_modes(&mut out, &on, &TerminalModes::default());
        assert_eq!(out, "\x1b[?1h\x1b[?2004h\x1b[?1006h\x1b[?1002h");
        let mut out = String::new();
        set_modes(&mut out, &TerminalModes::default(), &on);
        assert_eq!(out, "\x1b[?1l\x1b[?2004l\x1b[?1006l\x1b[?1002l");
    }

    #[test]
    fn a_scroll_moves_rows_up() {
        let mut view = view();
        view.lines[0].runs = vec![TerminalRun::plain("one")];
        view.lines[1].runs = vec![TerminalRun::plain("two")];
        view.apply(TerminalUpdate {
            terminal_id: "trm_1".into(),
            reset: false,
            cols: None,
            rows: None,
            scrolled: 1,
            lines: vec![TerminalLine {
                index: 1,
                runs: vec![TerminalRun::plain("three")],
                wrapped: false,
            }],
            cursor: view.cursor.clone(),
            modes: None,
            title: None,
            scrollback: None,
            bell: false,
        });
        let texts: Vec<String> = view.lines.iter().map(TerminalLine::text).collect();
        assert_eq!(texts, ["two", "three"]);
    }
}
