//! Newline-delimited JSON: one message per line, each line ending in `\n`.
//!
//! - A line may end in `\r\n`; the `\r` is dropped.
//! - Blank lines are skipped.
//! - A line longer than [`MAX_LINE_BYTES`] is an error. The peer is broken or hostile, and the
//!   connection should be closed: there's no safe way to find the next message.
//! - A line that isn't UTF-8 is an error too, for the same reason.
//! - A last line without a newline, just before the end of the stream, still counts.

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// The longest line accepted, in bytes, not counting the newline: 16 MiB.
///
/// Large content travels through the artifact store, never inside a message, so real messages
/// stay far below this.
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Reads lines from a byte stream.
#[derive(Debug)]
pub struct LineReader<R> {
    reader: BufReader<R>,
    line: Vec<u8>,
    max: usize,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    /// A reader that accepts lines of up to [`MAX_LINE_BYTES`].
    pub fn new(reader: R) -> Self {
        Self::with_max_line(reader, MAX_LINE_BYTES)
    }

    /// A reader that accepts lines of up to `max` bytes.
    pub fn with_max_line(reader: R, max: usize) -> Self {
        Self {
            reader: BufReader::new(reader),
            line: Vec::new(),
            max,
        }
    }

    /// The next non-blank line, without its line ending. `None` at the end of the stream.
    ///
    /// Fails with [`io::ErrorKind::InvalidData`] if a line is too long or isn't UTF-8.
    ///
    /// # Cancel safety
    ///
    /// Cancel safe: if the future is dropped before it completes, the bytes read so far are kept
    /// and the next call carries on with the same line. So it can be a branch of `tokio::select!`.
    pub async fn next_line(&mut self) -> io::Result<Option<String>> {
        loop {
            // The only await point. `fill_buf` is cancel safe, and everything after it runs to
            // completion, so a cancelled call never loses bytes.
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                if self.line.iter().all(u8::is_ascii_whitespace) {
                    self.line.clear();
                    return Ok(None);
                }
                return self.take_line().map(Some);
            }

            let newline = available.iter().position(|&byte| byte == b'\n');
            let chunk = newline.unwrap_or(available.len());
            if self.line.len() + chunk > self.max {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("line longer than {} bytes", self.max),
                ));
            }
            self.line.extend_from_slice(&available[..chunk]);

            match newline {
                Some(end) => {
                    self.reader.consume(end + 1);
                    if self.line.iter().all(u8::is_ascii_whitespace) {
                        self.line.clear();
                        continue;
                    }
                    return self.take_line().map(Some);
                }
                None => self.reader.consume(chunk),
            }
        }
    }

    fn take_line(&mut self) -> io::Result<String> {
        let mut line = std::mem::take(&mut self.line);
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        String::from_utf8(line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

/// Appends `line` and a newline to `buffer`, so several lines can go out in one write.
///
/// `line` must not contain a newline. Serialized JSON never does.
pub fn push_line(buffer: &mut Vec<u8>, line: &str) {
    debug_assert!(
        !line.contains('\n'),
        "a framed line can't contain a newline"
    );
    buffer.reserve(line.len() + 1);
    buffer.extend_from_slice(line.as_bytes());
    buffer.push(b'\n');
}

/// Writes `line` and a newline in one write, then flushes.
///
/// `line` must not contain a newline. Serialized JSON never does.
pub async fn write_line<W: AsyncWrite + Unpin>(writer: &mut W, line: &str) -> io::Result<()> {
    let mut buffer = Vec::new();
    push_line(&mut buffer, line);
    writer.write_all(&buffer).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn lines(input: &[u8], max: usize) -> Vec<io::Result<Option<String>>> {
        let mut reader = LineReader::with_max_line(input, max);
        let mut out = Vec::new();
        loop {
            let next = reader.next_line().await;
            let done = !matches!(next, Ok(Some(_)));
            out.push(next);
            if done {
                return out;
            }
        }
    }

    fn ok(lines: &[&str]) -> Vec<Option<String>> {
        lines
            .iter()
            .map(|line| Some((*line).to_owned()))
            .chain([None])
            .collect()
    }

    async fn read_all(input: &[u8]) -> Vec<Option<String>> {
        lines(input, MAX_LINE_BYTES)
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect()
    }

    #[tokio::test]
    async fn splits_lines_and_drops_line_endings() {
        assert_eq!(
            read_all(b"{\"a\":1}\n{\"b\":2}\r\n").await,
            ok(&["{\"a\":1}", "{\"b\":2}"])
        );
    }

    #[tokio::test]
    async fn skips_blank_lines() {
        assert_eq!(read_all(b"\n\r\n  \nx\n\n").await, ok(&["x"]));
        assert_eq!(read_all(b"").await, ok(&[]));
        assert_eq!(read_all(b"\n \n").await, ok(&[]));
    }

    #[tokio::test]
    async fn keeps_a_last_line_without_a_newline() {
        assert_eq!(read_all(b"a\nb").await, ok(&["a", "b"]));
    }

    #[tokio::test]
    async fn reassembles_lines_split_across_reads() {
        let (mut tx, rx) = tokio::io::duplex(4);
        let writer = tokio::spawn(async move {
            for piece in [&b"{\"lo"[..], b"ng\":", b"true}", b"\n{}", b"\n"] {
                tx.write_all(piece).await.unwrap();
            }
        });
        let mut reader = LineReader::new(rx);
        assert_eq!(
            reader.next_line().await.unwrap().unwrap(),
            "{\"long\":true}"
        );
        assert_eq!(reader.next_line().await.unwrap().unwrap(), "{}");
        writer.await.unwrap();
        assert_eq!(reader.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn rejects_long_lines() {
        let results = lines(b"12345\n123456\n", 5).await;
        assert_eq!(results[0].as_ref().unwrap().as_deref(), Some("12345"));
        let error = results[1].as_ref().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn rejects_invalid_utf8() {
        let results = lines(b"\xff\xfe\n", 100).await;
        assert_eq!(
            results[0].as_ref().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[tokio::test]
    async fn write_line_frames_one_message() {
        let mut out = Vec::new();
        write_line(&mut out, r#"{"jsonrpc":"2.0"}"#).await.unwrap();
        assert_eq!(out, b"{\"jsonrpc\":\"2.0\"}\n");

        let mut buffer = Vec::new();
        push_line(&mut buffer, "a");
        push_line(&mut buffer, "b");
        assert_eq!(buffer, b"a\nb\n");
    }
}
