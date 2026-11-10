//! A dev-only demo of the terminal view (M4): runs a program locally in ConPTY, emulates it
//! with `alacritty_terminal`, and feeds the view the line format `burrowd` sends, so real shells
//! and TUIs can be checked by eye. Nothing here ships in `burrow.exe`.
//!
//! ```text
//! cargo run -p burrow-app --example terminal_demo [-- [options] [program [args…]]]
//! ```
//!
//! The program defaults to `pwsh -NoLogo`. Options:
//! - `--input <text>`: type `text` once the program has drawn something, then the next `--input`
//!   a moment later; `\r`, `\n`, `\t`, `\e` and `\\` are escapes. Repeatable.
//! - `--select <line>,<col>:<line>,<col>`: select those cells after the input.
//! - `--font-size <px>`: the font size, 14 by default.
//! - `--size <width>x<height>`: the window's size in logical pixels.
//! - `--no-blink`: a steady cursor, for screenshots.
//! - `--unfocused`: start without focus, so the cursor shows hollow.
//! - `--bench`: no program; draws a 200×60 screen of styled text and prints how long the view
//!   takes per frame: every line new each frame, one line new, and nothing new.
//!
//! The demo plays `burrowd`'s part: after each batch of output it sends the lines that changed,
//! and how many scrolled into scrollback; after a resize, a snapshot; and pages of scrollback when
//! the view asks for them.

#[allow(dead_code)]
#[path = "../src/theme.rs"]
mod theme;

#[allow(dead_code)]
#[path = "../src/views/terminal/mod.rs"]
mod terminal;

use std::cell::RefCell;
use std::io::{Read as _, Write as _};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line as TermLine};
use alacritty_terminal::term::cell::{Cell, Flags as CellFlags};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AnsiColor, CursorShape as AnsiCursorShape, NamedColor, Processor, Rgb,
};
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    AnyView, App, AppContext as _, Bounds, Context, Entity, Focusable as _, IntoElement,
    ParentElement as _, Render, Styled as _, TitlebarOptions, Window, WindowBounds, WindowOptions,
    div, px, size,
};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use terminal::grid::{Color, Cursor, Flags, Line, Run};
use terminal::selection::GridPoint;
use terminal::{
    CursorShape, IndexedLine, Modes, MouseMode, Snapshot, TerminalEvent, TerminalOptions,
    TerminalView, Update,
};

/// Scrollback the emulator keeps. Large, so the demo can tell how many lines scrolled by how
/// much the scrollback grew.
const SCROLLBACK: usize = 100_000;
/// How long output is gathered before an update goes out: about a frame, as `burrowd` does.
const BATCH: Duration = Duration::from_millis(8);

struct Args {
    program: Vec<String>,
    inputs: Vec<String>,
    select: Option<(GridPoint, GridPoint)>,
    font_size: f32,
    size: Option<(f32, f32)>,
    blink: bool,
    focus: bool,
    bench: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        program: Vec::new(),
        inputs: Vec::new(),
        select: None,
        font_size: 14.0,
        size: None,
        blink: true,
        focus: true,
        bench: false,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--input" => args.inputs.extend(iter.next().map(|text| unescape(&text))),
            "--select" => args.select = iter.next().and_then(|text| parse_selection(&text)),
            "--font-size" => {
                args.font_size = iter.next().and_then(|n| n.parse().ok()).unwrap_or(14.0)
            }
            "--size" => {
                args.size = iter.next().and_then(|text| {
                    let (w, h) = text.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                })
            }
            "--no-blink" => args.blink = false,
            "--unfocused" => args.focus = false,
            "--bench" => args.bench = true,
            _ => {
                args.program.push(arg);
                args.program.extend(iter.by_ref());
            }
        }
    }
    if args.program.is_empty() {
        args.program = vec!["pwsh".into(), "-NoLogo".into()];
    }
    args
}

fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('e') => out.push('\x1b'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn parse_selection(text: &str) -> Option<(GridPoint, GridPoint)> {
    let point = |text: &str| {
        let (line, col) = text.split_once(',')?;
        Some(GridPoint::new(
            line.trim().parse().ok()?,
            col.trim().parse().ok()?,
        ))
    };
    let (from, to) = text.split_once(':')?;
    Some((point(from)?, point(to)?))
}

fn main() {
    let args = parse_args();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("BURROW_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();

    gpui_platform::application().run(move |cx: &mut App| {
        terminal::bind_keys(cx);
        let (width, height) = args.size.unwrap_or(if args.bench {
            (1760.0, 1120.0)
        } else {
            (1100.0, 700.0)
        });
        let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
        let title = if args.bench {
            "Terminal demo: benchmark".to_owned()
        } else {
            format!("Terminal demo: {}", args.program.join(" "))
        };
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| cx.new(|cx| Demo::new(args, window, cx)),
        );
        if let Err(error) = opened {
            eprintln!("terminal_demo: can't open a window: {error}");
            cx.quit();
        }
        cx.activate(true);
    });
}

/// The window: one terminal view, fed by a local program or the benchmark.
struct Demo {
    terminal: Entity<TerminalView>,
}

impl Demo {
    fn new(args: Args, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let options = TerminalOptions {
            font_size: px(args.font_size),
            blink: args.blink,
            ..TerminalOptions::default()
        };
        let terminal = cx.new(|cx| TerminalView::new(options, window, cx));
        if args.focus {
            terminal.focus_handle(cx).focus(window, cx);
        }
        if args.bench {
            bench::start(terminal.clone(), window, cx);
        } else {
            run_program(args, terminal.clone(), window, cx);
        }
        Self { terminal }
    }
}

impl Render for Demo {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            AnyView::from(self.terminal.clone())
                .cached(gpui::StyleRefinement::default().size_full()),
        )
    }
}

/// What the view asks of the host.
enum ToHost {
    Output(Vec<u8>),
    Closed,
    Input(String),
    Resize(u16, u16),
    History { before: i64, count: usize },
}

/// What the host tells the view.
enum FromHost {
    Snapshot(Snapshot),
    Update(Update),
    Modes(Modes),
    History(Vec<IndexedLine>, bool),
    Title(String),
    Exited,
}

fn run_program(
    args: Args,
    terminal: Entity<TerminalView>,
    window: &mut Window,
    cx: &mut Context<Demo>,
) {
    let (to_host, host_inbox) = mpsc::channel::<ToHost>();
    let (to_view, mut view_inbox) = unbounded::<FromHost>();
    let program = args.program.clone();
    let host_sender = to_host.clone();
    std::thread::spawn(move || host(program, host_inbox, host_sender, to_view));

    // The view's input, sizes and requests go to the host.
    let sender = to_host.clone();
    cx.subscribe(&terminal, move |_, _, event: &TerminalEvent, _| {
        let message = match event {
            TerminalEvent::Input(text) => ToHost::Input(text.clone()),
            TerminalEvent::Resize { cols, rows } => ToHost::Resize(*cols, *rows),
            TerminalEvent::RequestHistory { before, count } => ToHost::History {
                before: *before,
                count: *count,
            },
        };
        let _ = sender.send(message);
    })
    .detach();

    // The host's lines go to the view; scripted input follows the first output.
    let inputs = args.inputs;
    let select = args.select;
    cx.spawn_in(window, async move |_, cx| {
        let mut script = Some((inputs, select));
        while let Some(message) = view_inbox.next().await {
            let first_output = matches!(message, FromHost::Update(_));
            let done = cx.update(|window, cx| {
                terminal.update(cx, |view, cx| match message {
                    FromHost::Snapshot(snapshot) => view.apply_snapshot(snapshot, cx),
                    FromHost::Update(update) => view.apply_update(update, cx),
                    FromHost::Modes(modes) => view.set_modes(modes, cx),
                    FromHost::History(lines, complete) => view.apply_history(lines, complete, cx),
                    FromHost::Title(title) => {
                        window.set_window_title(&format!("Terminal demo: {title}"))
                    }
                    FromHost::Exited => cx.quit(),
                });
            });
            if done.is_err() {
                break;
            }
            if first_output && let Some((inputs, select)) = script.take() {
                let to_host = to_host.clone();
                let terminal = terminal.clone();
                cx.spawn(async move |cx| {
                    for text in inputs {
                        cx.background_executor()
                            .timer(Duration::from_millis(1500))
                            .await;
                        let _ = to_host.send(ToHost::Input(text));
                    }
                    if let Some((from, to)) = select {
                        cx.background_executor()
                            .timer(Duration::from_millis(1500))
                            .await;
                        terminal.update(cx, |view, cx| view.select(from, to, cx));
                    }
                })
                .detach();
            }
        }
    })
    .detach();
}

/// Passes the emulator's requests to the thread that writes to the PTY.
#[derive(Clone, Default)]
struct Listener(Rc<RefCell<Vec<TermEvent>>>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        self.0.borrow_mut().push(event);
    }
}

struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// The host: starts the program at the view's first size, emulates its output, and sends the
/// view what changed. Runs on its own thread.
fn host(
    program: Vec<String>,
    inbox: mpsc::Receiver<ToHost>,
    to_self: mpsc::Sender<ToHost>,
    to_view: UnboundedSender<FromHost>,
) {
    // Wait for the view's first size, so the program starts at the size it will be drawn at.
    let (cols, rows) = loop {
        match inbox.recv() {
            Ok(ToHost::Resize(cols, rows)) => break (cols, rows),
            Ok(_) => continue,
            Err(_) => return,
        }
    };
    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("terminal_demo: can't open a pseudo-terminal: {error}");
            let _ = to_view.unbounded_send(FromHost::Exited);
            return;
        }
    };
    let mut command = CommandBuilder::new(&program[0]);
    command.args(&program[1..]);
    if let Ok(cwd) = std::env::current_dir() {
        command.cwd(cwd);
    }
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    // A terminal draws colour, whatever the environment the demo started from says.
    command.env_remove("NO_COLOR");
    let child = match pair.slave.spawn_command(command) {
        Ok(child) => child,
        Err(error) => {
            eprintln!("terminal_demo: can't start {}: {error}", program.join(" "));
            let _ = to_view.unbounded_send(FromHost::Exited);
            return;
        }
    };
    drop(pair.slave);
    let master: Box<dyn MasterPty + Send> = pair.master;
    let mut reader = master.try_clone_reader().expect("PTY reader");
    let mut writer = master.take_writer().expect("PTY writer");

    std::thread::spawn(move || {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if to_self.send(ToHost::Output(buffer[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = to_self.send(ToHost::Closed);
    });

    let listener = Listener::default();
    let config = Config {
        scrolling_history: SCROLLBACK,
        ..Config::default()
    };
    let mut term = Term::new(
        config,
        &Size {
            cols: usize::from(cols),
            rows: usize::from(rows),
        },
        listener.clone(),
    );
    let mut parser: Processor = Processor::new();
    let mut mirror = Mirror::new(&term);
    let _ = to_view.unbounded_send(FromHost::Snapshot(mirror.snapshot(&term)));
    let _ = to_view.unbounded_send(FromHost::Modes(mirror.modes));

    let theme = terminal::palette::TerminalTheme::burrow();
    let mut dirty = false;
    loop {
        // Block for the next message, then gather whatever else arrives within a frame.
        let first = if dirty {
            match inbox.recv_timeout(BATCH) {
                Ok(message) => Some(message),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match inbox.recv() {
                Ok(message) => Some(message),
                Err(_) => break,
            }
        };
        let deadline = Instant::now() + BATCH;
        let mut next = first;
        while let Some(message) = next.take() {
            match message {
                ToHost::Output(bytes) => {
                    parser.advance(&mut term, &bytes);
                    dirty = true;
                }
                ToHost::Closed => {
                    let _ = to_view.unbounded_send(FromHost::Exited);
                    drop(child);
                    return;
                }
                ToHost::Input(text) => {
                    let _ = writer.write_all(text.as_bytes());
                    let _ = writer.flush();
                }
                ToHost::Resize(cols, rows) => {
                    let size = Size {
                        cols: usize::from(cols),
                        rows: usize::from(rows),
                    };
                    term.resize(size);
                    let _ = master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                    mirror = Mirror::new(&term);
                    let _ = to_view.unbounded_send(FromHost::Snapshot(mirror.snapshot(&term)));
                    dirty = false;
                }
                ToHost::History { before, count } => {
                    let (lines, complete) = history_page(&term, before, count);
                    let _ = to_view.unbounded_send(FromHost::History(lines, complete));
                }
            }
            // The emulator's answers (cursor position reports and the like) go back to the
            // program at once; ConPTY waits for some of them.
            for event in listener.0.borrow_mut().drain(..) {
                match event {
                    TermEvent::PtyWrite(text) => {
                        let _ = writer.write_all(text.as_bytes());
                        let _ = writer.flush();
                    }
                    TermEvent::Title(title) => {
                        let _ = to_view.unbounded_send(FromHost::Title(title));
                    }
                    TermEvent::TextAreaSizeRequest(format) => {
                        let text = format(WindowSize {
                            num_lines: term.screen_lines() as u16,
                            num_cols: term.columns() as u16,
                            cell_width: 8,
                            cell_height: 17,
                        });
                        let _ = writer.write_all(text.as_bytes());
                    }
                    TermEvent::ColorRequest(index, format) => {
                        let hex = match index {
                            0..=255 => terminal::palette::indexed(index as u8, &theme.ansi),
                            257 => theme.background,
                            258 => theme.cursor,
                            _ => theme.foreground,
                        };
                        let rgb = Rgb {
                            r: (hex >> 16) as u8,
                            g: (hex >> 8) as u8,
                            b: hex as u8,
                        };
                        let _ = writer.write_all(format(rgb).as_bytes());
                    }
                    _ => {}
                }
            }
            if Instant::now() < deadline {
                next = inbox.recv_timeout(deadline - Instant::now()).ok();
            }
        }
        if dirty {
            let (update, modes) = mirror.update(&term);
            if let Some(modes) = modes {
                let _ = to_view.unbounded_send(FromHost::Modes(modes));
            }
            if let Some(update) = update {
                let _ = to_view.unbounded_send(FromHost::Update(update));
            }
            dirty = false;
        }
    }
}

/// What the view has, so updates send only what changed.
struct Mirror {
    lines: Vec<Line>,
    cursor: Cursor,
    modes: Modes,
    history: usize,
    alt: bool,
}

impl Mirror {
    fn new(term: &Term<Listener>) -> Self {
        Self {
            lines: (0..term.screen_lines())
                .map(|row| convert_row(&term.grid()[TermLine(row as i32)]))
                .collect(),
            cursor: cursor(term),
            modes: modes(term),
            history: term.grid().history_size(),
            alt: term.mode().contains(TermMode::ALT_SCREEN),
        }
    }

    fn snapshot(&self, term: &Term<Listener>) -> Snapshot {
        Snapshot {
            cols: term.columns() as u16,
            rows: term.screen_lines() as u16,
            cursor: self.cursor,
            lines: self
                .lines
                .iter()
                .enumerate()
                .map(|(row, line)| IndexedLine::new(row as i64, line.clone()))
                .collect(),
        }
    }

    /// The lines that changed since the last update, and the modes if they changed.
    fn update(&mut self, term: &Term<Listener>) -> (Option<Update>, Option<Modes>) {
        let rows = self.lines.len();
        let alt = term.mode().contains(TermMode::ALT_SCREEN);
        let history = term.grid().history_size();
        // Lines that scrolled into scrollback on the same screen. Switching screens moves none.
        let scrolled = if alt == self.alt && history > self.history {
            history - self.history
        } else {
            0
        };
        self.alt = alt;
        self.history = history;

        let mut lines = Vec::new();
        if scrolled > 0 {
            let moved = scrolled.min(rows);
            self.lines.drain(..moved);
            self.lines.extend((0..moved).map(|_| Line::default()));
            // Lines that went past the screen before the view saw them.
            for back in 1..=scrolled.saturating_sub(rows) {
                let row = convert_row(&term.grid()[TermLine(-(back as i32))]);
                lines.push(IndexedLine::new(-(back as i64), row));
            }
        }
        for row in 0..rows {
            let line = convert_row(&term.grid()[TermLine(row as i32)]);
            if line != self.lines[row] {
                self.lines[row] = line.clone();
                lines.push(IndexedLine::new(row as i64, line));
            }
        }
        let cursor = cursor(term);
        let cursor_changed = cursor != self.cursor;
        self.cursor = cursor;
        let modes = modes(term);
        let modes_changed = (modes != self.modes).then_some(modes);
        self.modes = modes;

        let update = (scrolled > 0 || !lines.is_empty() || cursor_changed).then_some(Update {
            lines,
            cursor: Some(cursor),
            scrolled,
            size: None,
        });
        (update, modes_changed)
    }
}

/// Up to `count` scrollback lines above line `before`, and whether they reach the oldest.
fn history_page(term: &Term<Listener>, before: i64, count: usize) -> (Vec<IndexedLine>, bool) {
    let oldest = -(term.grid().history_size() as i64);
    let top = (before - count as i64).max(oldest);
    let lines = (top..before.min(0))
        .map(|index| IndexedLine::new(index, convert_row(&term.grid()[TermLine(index as i32)])))
        .collect();
    (lines, top <= oldest)
}

fn cursor(term: &Term<Listener>) -> Cursor {
    let style = term.cursor_style();
    let point = term.grid().cursor.point;
    Cursor {
        row: point.line.0.max(0) as u16,
        col: point.column.0 as u16,
        visible: term.mode().contains(TermMode::SHOW_CURSOR)
            && style.shape != AnsiCursorShape::Hidden,
        shape: match style.shape {
            AnsiCursorShape::Underline => CursorShape::Underline,
            AnsiCursorShape::Beam => CursorShape::Bar,
            _ => CursorShape::Block,
        },
    }
}

fn modes(term: &Term<Listener>) -> Modes {
    let mode = term.mode();
    Modes {
        app_cursor: mode.contains(TermMode::APP_CURSOR),
        bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
        mouse: if mode.contains(TermMode::MOUSE_MOTION) {
            MouseMode::Motion
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            MouseMode::Drag
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseMode::Click
        } else {
            MouseMode::Off
        },
        mouse_sgr: mode.contains(TermMode::SGR_MOUSE),
        alt_screen: mode.contains(TermMode::ALT_SCREEN),
        alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
        focus_reporting: mode.contains(TermMode::FOCUS_IN_OUT),
    }
}

/// One of alacritty's rows in the line format: runs of one style, the second half of each wide
/// character left out, and trailing blank cells dropped.
fn convert_row(row: &Row<Cell>) -> Line {
    let cells: Vec<&Cell> = (0..row.len()).map(|col| &row[Column(col)]).collect();
    let blank = |cell: &Cell| {
        cell.c == ' '
            && cell.bg == AnsiColor::Named(NamedColor::Background)
            && !cell
                .flags
                .intersects(CellFlags::INVERSE | CellFlags::ALL_UNDERLINES | CellFlags::STRIKEOUT)
    };
    let end = cells
        .iter()
        .rposition(|cell| !blank(cell))
        .map_or(0, |i| i + 1);
    let mut runs: Vec<Run> = Vec::new();
    for cell in &cells[..end] {
        if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
            continue;
        }
        let mut text = String::new();
        text.push(
            if cell.flags.contains(CellFlags::LEADING_WIDE_CHAR_SPACER) {
                ' '
            } else {
                cell.c
            },
        );
        if let Some(zero_width) = cell.zerowidth() {
            text.extend(zero_width);
        }
        let run = Run {
            text: String::new(),
            fg: color(cell.fg),
            bg: color(cell.bg),
            flags: flags(cell.flags),
        };
        match runs.last_mut() {
            Some(last) if last.style() == run.style() => last.text.push_str(&text),
            _ => runs.push(Run { text, ..run }),
        }
    }
    Line::new(runs)
}

fn color(color: AnsiColor) -> Option<Color> {
    match color {
        AnsiColor::Spec(rgb) => Some(Color::Rgb(rgb.r, rgb.g, rgb.b)),
        AnsiColor::Indexed(index) => Some(Color::Indexed(index)),
        AnsiColor::Named(named) => {
            let index = named as usize;
            let dim_black = NamedColor::DimBlack as usize;
            match index {
                0..=15 => Some(Color::Indexed(index as u8)),
                _ if (dim_black..dim_black + 8).contains(&index) => {
                    Some(Color::Indexed((index - dim_black) as u8))
                }
                // The default foreground and background, and the cursor's colour.
                _ => None,
            }
        }
    }
}

fn flags(cell: CellFlags) -> Flags {
    let mut flags = Flags::empty();
    let pairs = [
        (CellFlags::BOLD, Flags::BOLD),
        (CellFlags::ITALIC, Flags::ITALIC),
        (
            CellFlags::UNDERLINE
                | CellFlags::DOUBLE_UNDERLINE
                | CellFlags::DOTTED_UNDERLINE
                | CellFlags::DASHED_UNDERLINE,
            Flags::UNDERLINE,
        ),
        (CellFlags::INVERSE, Flags::INVERSE),
        (CellFlags::DIM, Flags::DIM),
        (CellFlags::STRIKEOUT, Flags::STRIKETHROUGH),
        (CellFlags::HIDDEN, Flags::HIDDEN),
        (CellFlags::UNDERCURL, Flags::UNDERCURL),
    ];
    for (cell_flag, flag) in pairs {
        if cell.intersects(cell_flag) {
            flags |= flag;
        }
    }
    flags
}

/// Measures the view drawing a full 200×60 screen of styled text.
mod bench {
    use super::*;

    const COLS: u16 = 200;
    const ROWS: u16 = 60;
    const FRAMES: usize = 240;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Phase {
        /// Every line replaced each frame, with new text: everything shaped again.
        AllLines,
        /// One line replaced each frame, as when typing.
        OneLine,
        /// Nothing replaced; the view redrawn from its cached layouts.
        Unchanged,
    }

    /// A 200-column line of 20 styled runs whose text and colours move with `frame`.
    fn line(row: usize, frame: usize) -> Line {
        const TEXT: &[u8] =
            b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789{}[]()<>=+-*/";
        let runs = (0..20)
            .map(|chunk| {
                let text: String = (0..10)
                    .map(|i| TEXT[(row * 7 + chunk * 10 + i + frame) % TEXT.len()] as char)
                    .collect();
                let flags = match (row + chunk + frame) % 6 {
                    0 => Flags::BOLD,
                    1 => Flags::ITALIC,
                    2 => Flags::UNDERLINE,
                    3 => Flags::BOLD | Flags::ITALIC,
                    4 => Flags::STRIKETHROUGH,
                    _ => Flags::empty(),
                };
                let mut run = Run::new(text)
                    .fg(Color::Indexed(((row * 13 + chunk * 7 + frame) % 256) as u8))
                    .flags(flags);
                if chunk % 2 == 1 {
                    run = run.bg(Color::Rgb(
                        (row * 4) as u8,
                        (chunk * 12) as u8,
                        (frame % 128) as u8,
                    ));
                }
                run
            })
            .collect();
        Line::new(runs)
    }

    pub fn start(terminal: Entity<TerminalView>, window: &mut Window, cx: &mut Context<Demo>) {
        terminal.update(cx, |view, cx| {
            view.apply_snapshot(
                Snapshot {
                    cols: COLS,
                    rows: ROWS,
                    cursor: Cursor::default(),
                    lines: (0..ROWS as usize)
                        .map(|row| IndexedLine::new(row as i64, line(row, 0)))
                        .collect(),
                },
                cx,
            );
        });
        // Let the window settle before measuring.
        cx.spawn_in(window, async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            let _ = cx.update(|window, cx| {
                terminal.update(cx, |view, _| view.reset_frame_stats());
                step(terminal, Phase::AllLines, 1, Instant::now(), window, cx);
            });
        })
        .detach();
    }

    /// Changes the screen for one frame of `phase`, and schedules the next after it is drawn.
    fn step(
        terminal: Entity<TerminalView>,
        phase: Phase,
        frame: usize,
        started: Instant,
        window: &mut Window,
        cx: &mut App,
    ) {
        if frame > FRAMES {
            let summary = terminal.read(cx).frame_stats();
            let elapsed = started.elapsed();
            println!(
                "{phase:?}: {} frames, terminal prepaint+paint mean {:.2} ms, p95 {:.2} ms, max {:.2} ms, {:.1} lines laid out per frame; {:.1} frames/s",
                summary.frames,
                summary.mean.as_secs_f64() * 1000.0,
                summary.p95.as_secs_f64() * 1000.0,
                summary.max.as_secs_f64() * 1000.0,
                summary.laid_out,
                FRAMES as f64 / elapsed.as_secs_f64(),
            );
            terminal.update(cx, |view, _| view.reset_frame_stats());
            let next = match phase {
                Phase::AllLines => Phase::OneLine,
                Phase::OneLine => Phase::Unchanged,
                Phase::Unchanged => {
                    cx.quit();
                    return;
                }
            };
            step(terminal, next, 1, Instant::now(), window, cx);
            return;
        }
        terminal.update(cx, |view, cx| match phase {
            Phase::AllLines => view.apply_update(
                Update {
                    lines: (0..ROWS as usize)
                        .map(|row| IndexedLine::new(row as i64, line(row, frame)))
                        .collect(),
                    ..Update::default()
                },
                cx,
            ),
            Phase::OneLine => {
                let row = frame % ROWS as usize;
                view.apply_update(
                    Update {
                        lines: vec![IndexedLine::new(row as i64, line(row, frame + 1000))],
                        ..Update::default()
                    },
                    cx,
                );
            }
            Phase::Unchanged => cx.notify(),
        });
        window
            .on_next_frame(move |window, cx| step(terminal, phase, frame + 1, started, window, cx));
    }
}
