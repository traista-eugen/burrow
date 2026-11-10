//! Input a terminal sends by itself, not the user: focus reports and the mouse moving with no
//! button held. Such input doesn't take the terminal's size under "the latest wins" (D-040,
//! D-045).
//!
//! ConPTY turns focus reporting (mode 1004) on for every program, so a client whose window
//! gains or loses focus sends `ESC [ I` or `ESC [ O` to every terminal it shows. ConPTY consumes
//! them; the program never reads them. A program that asks for every mouse movement (mode 1003,
//! as Codex does) gets a report whenever the pointer crosses a cell. Neither is the user
//! typing or clicking in that client.

/// Whether `data` is nothing but reports a terminal sends by itself:
/// - focus changes: `ESC [ I` and `ESC [ O`;
/// - the mouse moving with no button held, in SGR (`ESC [ < 35 ; x ; y M`) or X10
///   (`ESC [ M` and three bytes) encoding, with or without Shift, Alt or Ctrl.
///
/// Empty input isn't a report. A press, a release, a drag, the wheel or any other byte makes
/// the whole input typing.
pub fn only_reports(data: &str) -> bool {
    let mut rest = data.as_bytes();
    if rest.is_empty() {
        return false;
    }
    while !rest.is_empty() {
        rest = if let Some(after) = rest
            .strip_prefix(b"\x1b[I")
            .or_else(|| rest.strip_prefix(b"\x1b[O"))
        {
            after
        } else if let Some(after) = rest.strip_prefix(b"\x1b[<") {
            match sgr_motion(after) {
                Some(after) => after,
                None => return false,
            }
        } else if let Some(after) = rest.strip_prefix(b"\x1b[M") {
            match after {
                [button, _, _, after @ ..] if is_motion(u16::from(button.wrapping_sub(32))) => {
                    after
                }
                _ => return false,
            }
        } else {
            return false;
        };
    }
    true
}

/// An SGR mouse report's body after `ESC [ <`, if it is a movement with no button held:
/// returns what follows it.
fn sgr_motion(body: &[u8]) -> Option<&[u8]> {
    let end = body.iter().position(|&b| b == b'M' || b == b'm')?;
    let mut fields = body[..end].split(|&b| b == b';');
    let button = number(fields.next()?)?;
    let (_col, _row) = (number(fields.next()?)?, number(fields.next()?)?);
    if fields.next().is_some() || body[end] != b'M' || !is_motion(button) {
        return None;
    }
    Some(&body[end + 1..])
}

/// A mouse report's button code says "moved, no button": the motion bit (32) and button 3,
/// with any of Shift (4), Alt (8) and Ctrl (16).
fn is_motion(button: u16) -> bool {
    button & !(4 | 8 | 16) == 32 + 3
}

fn number(digits: &[u8]) -> Option<u16> {
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::only_reports;

    #[test]
    fn focus_and_motion_reports_are_not_typing() {
        for data in [
            "\x1b[I",
            "\x1b[O",
            "\x1b[O\x1b[I",
            "\x1b[<35;10;5M",
            // Shift, Alt and Ctrl held.
            "\x1b[<39;1;1M",
            "\x1b[<43;1;1M",
            "\x1b[<51;120;40M",
            "\x1b[<35;10;5M\x1b[<35;11;5M\x1b[I",
            // X10: 32 + 35, then column and row + 33.
            "\x1b[MC!!",
            "\x1b[MC!!\x1b[MC\"!",
        ] {
            assert!(only_reports(data), "{data:?}");
        }
    }

    #[test]
    fn anything_else_is_typing() {
        for data in [
            "",
            "x",
            "\r",
            "\x1b",
            "\x1b[A",
            "\x1bOA",
            "\x1b[200~text\x1b[201~",
            // A focus report followed by a key.
            "\x1b[Ix",
            "x\x1b[I",
            // Presses, releases, drags and the wheel.
            "\x1b[<0;10;5M",
            "\x1b[<0;10;5m",
            "\x1b[<32;10;5M",
            "\x1b[<64;10;5M",
            "\x1b[<65;10;5M",
            "\x1b[M !!",
            "\x1b[M@!!",
            // A movement with a release's final byte, or malformed ones.
            "\x1b[<35;10;5m",
            "\x1b[<35;10M",
            "\x1b[<35;10;5;1M",
            "\x1b[<35;a;5M",
            "\x1b[<35;10;5",
            "\x1b[MC!",
        ] {
            assert!(!only_reports(data), "{data:?}");
        }
    }
}
