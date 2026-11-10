//! A fake TUI, run as `burrow-fake-agent --tui`: what agent `fake` runs in a terminal session.
//!
//! It is a small line-oriented program on the console, like an agent's TUI on the main screen,
//! so terminal tests have something real to drive without spending quota. It runs the console
//! in raw mode with VT input and output, as Node and Rust TUIs do.
//!
//! - **Its first screen:** a title line with coloured, bold, inverse and wide text, a prompt
//!   (`> `) with the cursor after it, and an inverse status line on the last row:
//!   `status <cols>x<rows> keys <n> last <key>`. The status line follows every resize.
//! - **Typing** echoes; Enter answers `echo: <line>` on the next line and prompts again. Escape
//!   sequences (arrow keys and the like) aren't echoed; the status line shows the last one as
//!   `last \e[A`. A bracketed paste is typed text.
//! - **Commands**, typed and sent with Enter:
//!
//! | Command | What happens |
//! |---|---|
//! | `/flood [n]` | Prints `line 1` to `line n` (default 100), scrolling the screen |
//! | `/long [n]` | Prints `n` (default 10) lines of 100 columns, `long 1 ---…` and so on |
//! | `/pids` | Prints `pids <tui pid> <child pid>` |
//! | `/exit [code]` | Exits with `code` (default 0) |
//! | `/alt` | The alternate screen, with mouse reporting, SGR mouse, bracketed paste and application cursor keys on |
//! | `/main` | Back to the main screen, with those modes off |
//! | `/title <text>` | Sets the window title |
//! | `/bell` | Rings the bell |
//! | `/clear` | Clears the screen and the scrollback |
//!
//! Like the fake agent, it starts a child of its own (`--sleeper`), so a terminal session has a
//! process tree for the Job Object to clean up.

use std::process::{Child, ExitCode};

/// The fake TUI's `main`.
pub fn main() -> ExitCode {
    let child = super::agent::spawn_sleeper();
    #[cfg(windows)]
    {
        windows::run(child)
    }
    #[cfg(not(windows))]
    {
        let _ = child;
        eprintln!("burrow-fake-agent --tui needs a Windows console");
        ExitCode::FAILURE
    }
}

/// The program's state, shared by the input loop and the resize watcher.
struct Screen {
    cols: u16,
    rows: u16,
    keys: u64,
    last: String,
    line: String,
    child: Option<Child>,
}

impl Screen {
    /// The status line on the last row, drawn without moving the cursor.
    fn status(&self) -> String {
        format!(
            "\x1b7\x1b[{};1H\x1b[2K\x1b[7mstatus {}x{} keys {} last {}\x1b[0m\x1b8",
            self.rows, self.cols, self.rows, self.keys, self.last
        )
    }

    fn first_screen(&self) -> String {
        let mut out = String::from("\x1b[?25h\x1b[2J\x1b[H");
        out.push_str(
            "\x1b[1;32mfake tui\x1b[0m \x1b[31mred\x1b[0m \x1b[38;2;255;128;0morange\x1b[0m \
             \x1b[7minverse\x1b[0m 漢字\r\n> ",
        );
        out.push_str(&self.status());
        out
    }

    /// Handles a piece of input and returns what to print, or the exit code.
    fn input(&mut self, text: &str) -> Result<String, u8> {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            self.keys += 1;
            match c {
                '\x1b' => {
                    let mut sequence = String::from("\\e");
                    while let Some(&next) = chars.peek() {
                        sequence.push(next);
                        chars.next();
                        // CSI and SS3 end at a final byte; anything else is one character.
                        if (sequence.len() > 3 && ('@'..='~').contains(&next))
                            || (sequence.len() == 3 && !matches!(next, '[' | 'O'))
                        {
                            break;
                        }
                    }
                    match sequence.as_str() {
                        // A bracketed paste's markers: the text between them is typed.
                        "\\e[200~" | "\\e[201~" => {}
                        _ => self.last = sequence,
                    }
                }
                '\r' | '\n' => {
                    let line = std::mem::take(&mut self.line);
                    out.push_str("\r\n");
                    out.push_str(&self.command(&line)?);
                    out.push_str("> ");
                }
                '\x7f' | '\x08' => {
                    if self.line.pop().is_some() {
                        out.push_str("\x08 \x08");
                    }
                }
                '\x03' => {
                    self.line.clear();
                    out.push_str("^C\r\n> ");
                }
                c if c < ' ' => self.last = format!("^{}", char::from(c as u8 + b'@')),
                c => {
                    self.line.push(c);
                    out.push(c);
                }
            }
        }
        out.push_str(&self.status());
        Ok(out)
    }

    fn command(&mut self, line: &str) -> Result<String, u8> {
        let mut words = line.split_whitespace();
        let out = match words.next() {
            Some("/flood") => {
                let count: u32 = words.next().and_then(|n| n.parse().ok()).unwrap_or(100);
                // Each line clears what was left on its row, such as the status line.
                (1..=count).map(|n| format!("line {n}\x1b[K\r\n")).collect()
            }
            Some("/long") => {
                let count: u32 = words.next().and_then(|n| n.parse().ok()).unwrap_or(10);
                // 100 columns each: rewrapped when the terminal gets narrower.
                (1..=count)
                    .map(|n| format!("{:-<100}\x1b[K\r\n", format!("long {n} ")))
                    .collect()
            }
            Some("/pids") => format!(
                "pids {} {}\r\n",
                std::process::id(),
                self.child.as_ref().map_or(0, Child::id)
            ),
            Some("/exit") => {
                return Err(words.next().and_then(|code| code.parse().ok()).unwrap_or(0));
            }
            Some("/alt") => "\x1b[?1049h\x1b[2J\x1b[Halternate screen\r\n\
                             \x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[?1h"
                .to_owned(),
            Some("/main") => "\x1b[?1000l\x1b[?1006l\x1b[?2004l\x1b[?1l\x1b[?1049l".to_owned(),
            Some("/title") => {
                let title: Vec<&str> = words.collect();
                format!("\x1b]0;{}\x07", title.join(" "))
            }
            Some("/bell") => "\x07".to_owned(),
            Some("/clear") => "\x1b[2J\x1b[3J\x1b[H".to_owned(),
            _ => format!("echo: {line}\x1b[K\r\n"),
        };
        Ok(out)
    }
}

#[cfg(windows)]
mod windows {
    use std::io::{Read, Write};
    use std::process::{Child, ExitCode};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread;
    use std::time::Duration;

    use windows_sys::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_INPUT,
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetConsoleScreenBufferInfo,
        GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
    };

    use super::Screen;

    /// How often the size is checked.
    const RESIZE_POLL: Duration = Duration::from_millis(20);

    pub fn run(child: Option<Child>) -> ExitCode {
        // SAFETY: plain calls on the process's own standard handles.
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            // Raw: no line editing, no echo, Ctrl+C as a character, keys as VT sequences.
            SetConsoleMode(input, ENABLE_VIRTUAL_TERMINAL_INPUT);
            let output = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut mode = 0;
            GetConsoleMode(output, &mut mode);
            SetConsoleMode(
                output,
                mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            );
        }
        let (cols, rows) = size().unwrap_or((80, 24));
        let screen = Arc::new(Mutex::new(Screen {
            cols,
            rows,
            keys: 0,
            last: "none".into(),
            line: String::new(),
            child,
        }));
        print(
            &screen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .first_screen(),
        );

        let watched = screen.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(RESIZE_POLL);
                let Some((cols, rows)) = size() else { continue };
                let mut screen = watched.lock().unwrap_or_else(PoisonError::into_inner);
                if (cols, rows) != (screen.cols, screen.rows) {
                    screen.cols = cols;
                    screen.rows = rows;
                    print(&screen.status());
                }
            }
        });

        let mut stdin = std::io::stdin();
        let mut buffer = [0u8; 4096];
        let mut pending = Vec::new();
        loop {
            let read = match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => return ExitCode::SUCCESS,
                Ok(read) => read,
            };
            pending.extend_from_slice(&buffer[..read]);
            // Keep an incomplete UTF-8 character for the next read.
            let valid = match std::str::from_utf8(&pending) {
                Ok(text) => text.len(),
                Err(error) => error.valid_up_to(),
            };
            let text = String::from_utf8_lossy(&pending[..valid]).into_owned();
            pending.drain(..valid);
            let mut screen = screen.lock().unwrap_or_else(PoisonError::into_inner);
            match screen.input(&text) {
                Ok(out) => print(&out),
                Err(code) => {
                    print("\r\nbye\r\n");
                    return ExitCode::from(code);
                }
            }
        }
    }

    fn print(text: &str) {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
    }

    /// The console window's size: (columns, rows).
    fn size() -> Option<(u16, u16)> {
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        // SAFETY: `info` is the struct the call fills.
        let ok =
            unsafe { GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) } != 0;
        let window = info.srWindow;
        let cols = u16::try_from(window.Right - window.Left + 1).ok()?;
        let rows = u16::try_from(window.Bottom - window.Top + 1).ok()?;
        (ok && cols > 0 && rows > 0).then_some((cols, rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Screen {
        Screen {
            cols: 80,
            rows: 24,
            keys: 0,
            last: "none".into(),
            line: String::new(),
            child: None,
        }
    }

    #[test]
    fn typing_echoes_and_enter_answers() {
        let mut screen = screen();
        let out = screen.input("hi").unwrap();
        assert!(out.starts_with("hi\x1b7"), "{out:?}");
        let out = screen.input("\r").unwrap();
        assert!(out.starts_with("\r\necho: hi\x1b[K\r\n> "), "{out:?}");
        assert!(out.contains("status 80x24 keys 3 last none"), "{out:?}");
    }

    #[test]
    fn escape_sequences_show_in_the_status_line() {
        let mut screen = screen();
        let out = screen.input("\x1b[A").unwrap();
        assert!(out.contains("last \\e[A"), "{out:?}");
        screen.input("\x1bOB").unwrap();
        assert_eq!(screen.last, "\\eOB");
        screen.input("\x1b[200~pasted\x1b[201~").unwrap();
        assert_eq!(screen.line, "pasted");
        assert_eq!(screen.last, "\\eOB");
    }

    #[test]
    fn commands() {
        let mut screen = screen();
        let out = screen.input("/flood 3\r").unwrap();
        assert!(
            out.contains("line 1\x1b[K\r\nline 2\x1b[K\r\nline 3\x1b[K\r\n> "),
            "{out:?}"
        );
        assert_eq!(screen.input("/exit 7\r"), Err(7));
    }
}
