//! MSP over stdio is newline-delimited JSON: one JSON-RPC message per line.
//!
//! `muse serve` writes `\n` line endings; a `\r\n` is accepted too. Blank lines are skipped. A
//! line longer than [`MAX_LINE_BYTES`], or one that isn't UTF-8, ends the connection: there's no
//! safe way to find the next message.

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

/// The longest line accepted, in bytes: 32 MiB.
///
/// MSP bounds what it inlines (64 KiB per streamed text surface, 6 MiB per `item/readOutput`
/// page, which grows by a third as base64), so real messages stay well below this.
pub const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;

/// Reads lines from a byte stream.
#[derive(Debug)]
pub(crate) struct LineReader<R> {
    reader: BufReader<R>,
    line: Vec<u8>,
    max: usize,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    pub fn new(reader: R) -> Self {
        Self::with_max_line(reader, MAX_LINE_BYTES)
    }

    pub fn with_max_line(reader: R, max: usize) -> Self {
        Self {
            reader: BufReader::new(reader),
            line: Vec::new(),
            max,
        }
    }

    /// The next non-blank line, without its line ending. `None` at the end of the stream.
    ///
    /// Cancel safe: bytes read before a cancelled call are kept for the next one.
    pub async fn next_line(&mut self) -> io::Result<Option<String>> {
        loop {
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

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::*;

    async fn read_all(input: &[u8], max: usize) -> Vec<io::Result<Option<String>>> {
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

    fn lines(results: Vec<io::Result<Option<String>>>) -> Vec<Option<String>> {
        results.into_iter().map(Result::unwrap).collect()
    }

    #[tokio::test]
    async fn splits_lines_and_skips_blank_ones() {
        let read = lines(read_all(b"{\"a\":1}\n\r\n{\"b\":2}\r\n\nlast", MAX_LINE_BYTES).await);
        let expected: Vec<Option<String>> = ["{\"a\":1}", "{\"b\":2}", "last"]
            .iter()
            .map(|line| Some((*line).to_owned()))
            .chain([None])
            .collect();
        assert_eq!(read, expected);
    }

    #[tokio::test]
    async fn reassembles_lines_split_across_reads() {
        let (mut tx, rx) = tokio::io::duplex(4);
        let writer = tokio::spawn(async move {
            for piece in [&b"{\"lo"[..], b"ng\":", b"true}", b"\n{}\n"] {
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
    async fn rejects_long_lines_and_invalid_utf8() {
        let results = read_all(b"12345\n123456\n", 5).await;
        assert_eq!(results[0].as_ref().unwrap().as_deref(), Some("12345"));
        assert_eq!(
            results[1].as_ref().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let results = read_all(b"\xff\xfe\n", 100).await;
        assert_eq!(
            results[0].as_ref().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
