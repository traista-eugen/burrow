//! What the terminal sends for keys, pastes, the mouse and focus: the bytes for `terminal/input`,
//! as xterm sends them (D-041).
//!
//! Keys that type text (letters, digits, punctuation, space, with or without Shift, and AltGr
//! characters) aren't encoded here: they arrive as text through the window's input handler, which
//! also carries IME composition. [`encode_key`] covers what text input doesn't: Enter, Backspace,
//! Tab, Esc, the cursor and editing keys, F1–F12, Ctrl and Alt combinations.

use gpui::{Keystroke, Modifiers};

use super::grid::{Modes, MouseMode};

/// The xterm modifier parameter: 1 plus Shift 1, Alt 2, Ctrl 4. `None` without modifiers.
fn modifier_param(modifiers: &Modifiers) -> Option<u8> {
    let param = 1
        + u8::from(modifiers.shift)
        + 2 * u8::from(modifiers.alt)
        + 4 * u8::from(modifiers.control);
    (param > 1).then_some(param)
}

/// The bytes a key sends, or `None` when it sends none or types text, which comes through the
/// input handler instead.
pub fn encode_key(keystroke: &Keystroke, modes: &Modes) -> Option<String> {
    let modifiers = &keystroke.modifiers;
    // The Windows key belongs to the system and the app.
    if modifiers.platform || modifiers.function {
        return None;
    }
    let key = keystroke.key.as_str();
    let param = modifier_param(modifiers);

    // Cursor keys: `ESC [ x`, `ESC O x` in application mode, `ESC [ 1 ; m x` with modifiers.
    let cursor = match key {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(letter) = cursor {
        return Some(match param {
            Some(param) => format!("\x1b[1;{param}{letter}"),
            None if modes.app_cursor => format!("\x1bO{letter}"),
            None => format!("\x1b[{letter}"),
        });
    }

    // F1–F4: `ESC O P`…`ESC O S`, `ESC [ 1 ; m P` with modifiers.
    let ss3 = match key {
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    };
    if let Some(letter) = ss3 {
        return Some(match param {
            Some(param) => format!("\x1b[1;{param}{letter}"),
            None => format!("\x1bO{letter}"),
        });
    }

    // Editing keys and F5–F12: `ESC [ n ~`, `ESC [ n ; m ~` with modifiers.
    let tilde = match key {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(number) = tilde {
        return Some(match param {
            Some(param) => format!("\x1b[{number};{param}~"),
            None => format!("\x1b[{number}~"),
        });
    }

    let alt = modifiers.alt;
    let with_alt = |bytes: &str| {
        if alt {
            format!("\x1b{bytes}")
        } else {
            bytes.to_owned()
        }
    };
    match key {
        "enter" => return Some(with_alt("\r")),
        // Ctrl+Backspace sends ^H, which shells take as deleting a word.
        "backspace" if modifiers.control => return Some(with_alt("\x08")),
        "backspace" => return Some(with_alt("\x7f")),
        "tab" if modifiers.shift => return Some("\x1b[Z".into()),
        "tab" => return Some(with_alt("\t")),
        "escape" => return Some(with_alt("\x1b")),
        _ => {}
    }

    if modifiers.control {
        let control = control_byte(key, modifiers.shift)?;
        return Some(with_alt(&char::from(control).to_string()));
    }

    if alt {
        // Alt+key: ESC, then what the key types. "space" types " ".
        let typed = match keystroke.key_char.as_deref() {
            Some(text) => text.to_owned(),
            None if key == "space" => " ".to_owned(),
            None if key.chars().count() == 1 => key.to_owned(),
            None => return None,
        };
        return Some(format!("\x1b{typed}"));
    }

    // Plain keys that type text come through the input handler.
    None
}

/// The control character Ctrl+`key` sends: letters to ^A–^Z, and the xterm conventions for the
/// keys around them (Ctrl+[ is ESC, Ctrl+Space and Ctrl+2 are NUL, Ctrl+/ is ^_). `None` for keys
/// with no control character, which then send nothing. Ctrl+- alone is left alone: apps zoom
/// with it; Ctrl+Shift+- (Ctrl+_) sends ^_.
fn control_byte(key: &str, shift: bool) -> Option<u8> {
    let mut chars = key.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return (key == "space").then_some(0);
    };
    Some(match ch.to_ascii_lowercase() {
        letter @ 'a'..='z' => letter as u8 - b'a' + 1,
        '@' | '2' | ' ' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '-' if shift => 0x1f,
        '_' | '/' | '7' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

/// The bytes for pasting `text`: newlines become carriage returns, as typed Enter would send, and
/// with bracketed paste on the text is wrapped in its markers, minus any end marker inside it.
pub fn encode_paste(text: &str, modes: &Modes) -> String {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if modes.bracketed_paste {
        let text = text.replace("\x1b[201~", "");
        format!("\x1b[200~{text}\x1b[201~")
    } else {
        text
    }
}

/// A mouse button, for reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// A mouse event to report to the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseReport {
    Press(MouseButton),
    Release(MouseButton),
    /// A move with a button held.
    Drag(MouseButton),
    /// A move with no button held.
    Move,
    WheelUp,
    WheelDown,
}

/// The bytes reporting a mouse event at a cell (0-based), or `None` if the mode doesn't report it.
/// SGR encoding when the program asked for it, otherwise X10. X10 puts each coordinate in one
/// byte; input is UTF-8 text, so only ASCII bytes are sent, and nothing past column or row 95.
pub fn encode_mouse(
    report: MouseReport,
    col: u16,
    row: u16,
    modifiers: &Modifiers,
    modes: &Modes,
) -> Option<String> {
    let wanted = match report {
        MouseReport::Press(_)
        | MouseReport::Release(_)
        | MouseReport::WheelUp
        | MouseReport::WheelDown => modes.mouse != MouseMode::Off,
        MouseReport::Drag(_) => matches!(modes.mouse, MouseMode::Drag | MouseMode::Motion),
        MouseReport::Move => modes.mouse == MouseMode::Motion,
    };
    if !wanted {
        return None;
    }
    let button_code = |button: MouseButton| match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let mut code: u32 = match report {
        MouseReport::Press(button) | MouseReport::Release(button) => button_code(button),
        MouseReport::Drag(button) => button_code(button) + 32,
        MouseReport::Move => 3 + 32,
        MouseReport::WheelUp => 64,
        MouseReport::WheelDown => 65,
    };
    code += 4 * u32::from(modifiers.shift)
        + 8 * u32::from(modifiers.alt)
        + 16 * u32::from(modifiers.control);
    let (x, y) = (u32::from(col) + 1, u32::from(row) + 1);

    if modes.mouse_sgr {
        let end = if matches!(report, MouseReport::Release(_)) {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{x};{y}{end}"));
    }
    // X10 can't say which button was released.
    if matches!(report, MouseReport::Release(_)) {
        code = (code & !3) | 3;
    }
    let byte = |value: u32| char::from_u32(32 + value).filter(|_| 32 + value <= 127);
    Some(format!("\x1b[M{}{}{}", byte(code)?, byte(x)?, byte(y)?))
}

/// What a focus change sends with focus reporting on: `ESC [ I` or `ESC [ O`.
pub fn encode_focus(focused: bool, modes: &Modes) -> Option<&'static str> {
    modes
        .focus_reporting
        .then_some(if focused { "\x1b[I" } else { "\x1b[O" })
}

/// What a wheel notch sends on the alternate screen without mouse reporting: arrow keys, so
/// pagers and editors scroll (xterm's alternate scroll mode).
pub fn encode_alternate_scroll(up: bool, lines: usize, modes: &Modes) -> Option<String> {
    if !modes.alt_screen || !modes.alternate_scroll || modes.mouse != MouseMode::Off {
        return None;
    }
    let letter = if up { 'A' } else { 'B' };
    let one = if modes.app_cursor {
        format!("\x1bO{letter}")
    } else {
        format!("\x1b[{letter}")
    };
    Some(one.repeat(lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A keystroke from GPUI's notation, `ctrl-shift-a`, plus the text it would type.
    fn key(notation: &'static str, typed: Option<&str>) -> Keystroke {
        let mut keystroke =
            Keystroke::parse(notation).unwrap_or_else(|_| panic!("bad keystroke {notation}"));
        keystroke.key_char = typed.map(str::to_owned);
        keystroke
    }

    fn app_cursor() -> Modes {
        Modes {
            app_cursor: true,
            ..Modes::default()
        }
    }

    #[test]
    fn keys_encode_as_xterm_does() {
        let normal = Modes::default();
        let app = app_cursor();
        #[rustfmt::skip]
        let table: &[(&'static str, Option<&str>, &Modes, Option<&str>)] = &[
            // Text is typed through the input handler, not encoded.
            ("a", Some("a"), &normal, None),
            ("shift-a", Some("A"), &normal, None),
            ("space", Some(" "), &normal, None),
            ("shift-1", Some("!"), &normal, None),
            ("enter", None, &normal, Some("\r")),
            ("shift-enter", None, &normal, Some("\r")),
            ("alt-enter", None, &normal, Some("\x1b\r")),
            ("backspace", None, &normal, Some("\x7f")),
            ("ctrl-backspace", None, &normal, Some("\x08")),
            ("alt-backspace", None, &normal, Some("\x1b\x7f")),
            ("tab", None, &normal, Some("\t")),
            ("shift-tab", None, &normal, Some("\x1b[Z")),
            ("escape", None, &normal, Some("\x1b")),
            ("alt-escape", None, &normal, Some("\x1b\x1b")),
            // Arrows, normal and in application mode (DECCKM).
            ("up", None, &normal, Some("\x1b[A")),
            ("down", None, &normal, Some("\x1b[B")),
            ("right", None, &normal, Some("\x1b[C")),
            ("left", None, &normal, Some("\x1b[D")),
            ("up", None, &app, Some("\x1bOA")),
            ("down", None, &app, Some("\x1bOB")),
            ("right", None, &app, Some("\x1bOC")),
            ("left", None, &app, Some("\x1bOD")),
            // Modified arrows, xterm style, in either mode.
            ("ctrl-right", None, &normal, Some("\x1b[1;5C")),
            ("ctrl-right", None, &app, Some("\x1b[1;5C")),
            ("shift-left", None, &normal, Some("\x1b[1;2D")),
            ("alt-up", None, &normal, Some("\x1b[1;3A")),
            ("ctrl-shift-down", None, &normal, Some("\x1b[1;6B")),
            ("ctrl-alt-shift-up", None, &normal, Some("\x1b[1;8A")),
            // Home and End.
            ("home", None, &normal, Some("\x1b[H")),
            ("end", None, &normal, Some("\x1b[F")),
            ("home", None, &app, Some("\x1bOH")),
            ("end", None, &app, Some("\x1bOF")),
            ("ctrl-home", None, &normal, Some("\x1b[1;5H")),
            ("shift-end", None, &app, Some("\x1b[1;2F")),
            // Editing keys.
            ("insert", None, &normal, Some("\x1b[2~")),
            ("delete", None, &normal, Some("\x1b[3~")),
            ("pageup", None, &normal, Some("\x1b[5~")),
            ("pagedown", None, &normal, Some("\x1b[6~")),
            ("ctrl-delete", None, &normal, Some("\x1b[3;5~")),
            ("shift-pageup", None, &normal, Some("\x1b[5;2~")),
            // Function keys.
            ("f1", None, &normal, Some("\x1bOP")),
            ("f2", None, &normal, Some("\x1bOQ")),
            ("f3", None, &normal, Some("\x1bOR")),
            ("f4", None, &normal, Some("\x1bOS")),
            ("f5", None, &normal, Some("\x1b[15~")),
            ("f6", None, &normal, Some("\x1b[17~")),
            ("f7", None, &normal, Some("\x1b[18~")),
            ("f8", None, &normal, Some("\x1b[19~")),
            ("f9", None, &normal, Some("\x1b[20~")),
            ("f10", None, &normal, Some("\x1b[21~")),
            ("f11", None, &normal, Some("\x1b[23~")),
            ("f12", None, &normal, Some("\x1b[24~")),
            ("shift-f1", None, &normal, Some("\x1b[1;2P")),
            ("ctrl-f5", None, &normal, Some("\x1b[15;5~")),
            // Ctrl+letter, and Ctrl+Shift+letter, which no binding took.
            ("ctrl-a", None, &normal, Some("\x01")),
            ("ctrl-c", None, &normal, Some("\x03")),
            ("ctrl-z", None, &normal, Some("\x1a")),
            ("ctrl-shift-a", None, &normal, Some("\x01")),
            // Ctrl with the keys around the letters.
            ("ctrl-[", None, &normal, Some("\x1b")),
            ("ctrl-\\", None, &normal, Some("\x1c")),
            ("ctrl-]", None, &normal, Some("\x1d")),
            ("ctrl-^", None, &normal, Some("\x1e")),
            ("ctrl-_", None, &normal, Some("\x1f")),
            ("ctrl-shift-6", None, &normal, Some("\x1e")),
            ("ctrl-shift--", None, &normal, Some("\x1f")),
            ("ctrl-/", None, &normal, Some("\x1f")),
            ("ctrl-space", None, &normal, Some("\x00")),
            ("ctrl-2", None, &normal, Some("\x00")),
            ("ctrl-8", None, &normal, Some("\x7f")),
            ("ctrl--", None, &normal, None),
            ("ctrl-.", None, &normal, None),
            // Alt as an ESC prefix.
            ("alt-a", Some("a"), &normal, Some("\x1ba")),
            ("alt-shift-a", Some("A"), &normal, Some("\x1bA")),
            ("alt-.", Some("."), &normal, Some("\x1b.")),
            ("alt-space", Some(" "), &normal, Some("\x1b ")),
            ("ctrl-alt-a", None, &normal, Some("\x1b\x01")),
            // The Windows key is the system's.
            ("win-a", None, &normal, None),
        ];
        for (notation, typed, modes, expected) in table {
            let keystroke = key(notation, *typed);
            assert_eq!(
                encode_key(&keystroke, modes).as_deref(),
                *expected,
                "{notation}"
            );
        }
    }

    #[test]
    fn pastes_send_carriage_returns_and_brackets() {
        let normal = Modes::default();
        let bracketed = Modes {
            bracketed_paste: true,
            ..Modes::default()
        };
        assert_eq!(encode_paste("ls\r\ndir\n", &normal), "ls\rdir\r");
        assert_eq!(encode_paste("é界", &normal), "é界");
        assert_eq!(
            encode_paste("one\ntwo", &bracketed),
            "\x1b[200~one\rtwo\x1b[201~"
        );
        // A pasted end marker can't end the paste early.
        assert_eq!(
            encode_paste("a\x1b[201~rm -rf", &bracketed),
            "\x1b[200~arm -rf\x1b[201~"
        );
    }

    #[test]
    fn mouse_reports_follow_the_mode() {
        let none = Modifiers::default();
        let ctrl = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        let modes = |mouse, sgr| Modes {
            mouse,
            mouse_sgr: sgr,
            ..Modes::default()
        };
        let left = MouseButton::Left;
        /// A report at a cell, with modifiers, in modes, and the bytes it sends.
        type Case<'a> = (MouseReport, u16, u16, &'a Modifiers, Modes, Option<&'a str>);
        #[rustfmt::skip]
        let table: &[Case] = &[
            (MouseReport::Press(left), 0, 0, &none, modes(MouseMode::Off, true), None),
            (MouseReport::Press(left), 0, 0, &none, modes(MouseMode::Click, true), Some("\x1b[<0;1;1M")),
            (MouseReport::Release(left), 4, 9, &none, modes(MouseMode::Click, true), Some("\x1b[<0;5;10m")),
            (MouseReport::Press(MouseButton::Right), 2, 3, &ctrl, modes(MouseMode::Click, true), Some("\x1b[<18;3;4M")),
            (MouseReport::Press(MouseButton::Middle), 2, 3, &none, modes(MouseMode::Click, true), Some("\x1b[<1;3;4M")),
            (MouseReport::WheelUp, 1, 1, &none, modes(MouseMode::Click, true), Some("\x1b[<64;2;2M")),
            (MouseReport::WheelDown, 1, 1, &none, modes(MouseMode::Click, true), Some("\x1b[<65;2;2M")),
            // Moves only in the modes that asked for them.
            (MouseReport::Drag(left), 1, 1, &none, modes(MouseMode::Click, true), None),
            (MouseReport::Drag(left), 1, 1, &none, modes(MouseMode::Drag, true), Some("\x1b[<32;2;2M")),
            (MouseReport::Move, 1, 1, &none, modes(MouseMode::Drag, true), None),
            (MouseReport::Move, 1, 1, &none, modes(MouseMode::Motion, true), Some("\x1b[<35;2;2M")),
            // X10: bytes offset by 32, releases as button 3, nothing past column 95.
            (MouseReport::Press(left), 0, 0, &none, modes(MouseMode::Click, false), Some("\x1b[M !!")),
            (MouseReport::Release(MouseButton::Right), 0, 0, &none, modes(MouseMode::Click, false), Some("\x1b[M#!!")),
            (MouseReport::Press(left), 94, 0, &none, modes(MouseMode::Click, false), Some("\x1b[M \x7f!")),
            (MouseReport::Press(left), 95, 0, &none, modes(MouseMode::Click, false), None),
        ];
        for (report, col, row, modifiers, modes, expected) in table {
            assert_eq!(
                encode_mouse(*report, *col, *row, modifiers, modes).as_deref(),
                *expected,
                "{report:?} at {col},{row}"
            );
        }
    }

    #[test]
    fn focus_and_alternate_scroll() {
        let mut modes = Modes::default();
        assert_eq!(encode_focus(true, &modes), None);
        modes.focus_reporting = true;
        assert_eq!(encode_focus(true, &modes), Some("\x1b[I"));
        assert_eq!(encode_focus(false, &modes), Some("\x1b[O"));

        let mut modes = Modes::default();
        assert_eq!(encode_alternate_scroll(true, 3, &modes), None);
        modes.alt_screen = true;
        modes.alternate_scroll = true;
        assert_eq!(
            encode_alternate_scroll(true, 2, &modes).as_deref(),
            Some("\x1b[A\x1b[A")
        );
        modes.app_cursor = true;
        assert_eq!(
            encode_alternate_scroll(false, 1, &modes).as_deref(),
            Some("\x1bOB")
        );
        modes.mouse = MouseMode::Click;
        assert_eq!(encode_alternate_scroll(false, 1, &modes), None);
    }
}
