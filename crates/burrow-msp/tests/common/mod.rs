//! Shared test helpers: schema validation of whole transcripts, taps that record the wire, and
//! a fake host that replays a recorded transcript.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use burrow_msp::{Event, Events};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};

pub const SCHEMA: &str = include_str!("../../schema/msp.schema.json");

/// How long any one wait in a test may take.
pub const WAIT: Duration = Duration::from_secs(30);

pub fn schema() -> &'static Value {
    static SCHEMA_VALUE: OnceLock<Value> = OnceLock::new();
    SCHEMA_VALUE.get_or_init(|| serde_json::from_str(SCHEMA).unwrap())
}

/// Validates `value` against the definition a `{"$ref": "#/$defs/X"}` node names.
fn validate_ref(node: Option<&Value>, value: &Value, what: &str) -> Vec<String> {
    let Some(reference) = node.and_then(|node| node.get("$ref")) else {
        return Vec::new();
    };
    static VALIDATORS: OnceLock<Mutex<HashMap<String, Arc<jsonschema::Validator>>>> =
        OnceLock::new();
    let reference = reference.as_str().unwrap().to_owned();
    let validator = VALIDATORS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(reference.clone())
        .or_insert_with(|| {
            let mut root = json!({ "$defs": schema()["$defs"].clone() });
            root["$ref"] = json!(reference);
            Arc::new(jsonschema::validator_for(&root).expect("the schema compiles"))
        })
        .clone();
    validator
        .iter_errors(value)
        .map(|error| format!("{what}: {error} at {}", error.instance_path()))
        .collect()
}

/// Which side sent a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Client,
    Host,
}

/// Validates every frame of a transcript against the schema: request params and results,
/// notification params, Muse's requests and the client's receipts. Returns the problems.
pub fn validate_transcript(frames: &[(Side, Value)]) -> Vec<String> {
    let schema = schema();
    let mut problems = Vec::new();
    // Ids name methods per side: the client's requests, and Muse's requests to the client.
    let mut client_calls = HashMap::new();
    let mut host_calls = HashMap::new();
    for (side, frame) in frames {
        if let Some(method) = frame.get("method").and_then(Value::as_str)
            && let Some(id) = frame.get("id")
        {
            let calls = match side {
                Side::Client => &mut client_calls,
                Side::Host => &mut host_calls,
            };
            calls.insert(id.to_string(), method.to_owned());
        }
    }
    for (side, frame) in frames {
        let method = frame.get("method").and_then(Value::as_str);
        let params = frame.get("params").cloned().unwrap_or(Value::Null);
        match (side, method, frame.get("id")) {
            (Side::Client, Some(method), Some(_)) => match schema["methods"].get(method) {
                Some(entry) => {
                    // Muse accepts `params: {}` for a method without params (the probe that
                    // recorded the fixtures sent that); anything more is a mistake.
                    if entry.get("params").is_none()
                        && frame.get("params").is_some_and(|p| p != &json!({}))
                    {
                        problems.push(format!("{method} takes no params"));
                    }
                    problems.extend(validate_ref(entry.get("params"), &params, method));
                }
                None => problems.push(format!("the client called unknown method {method}")),
            },
            (Side::Client, Some(method), None) => {
                if schema["notifications"].get(method).is_none() {
                    problems.push(format!("the client sent unknown notification {method}"));
                }
            }
            (Side::Host, Some(method), Some(_)) => match schema["requests"].get(method) {
                Some(entry) => problems.extend(validate_ref(entry.get("params"), &params, method)),
                None => problems.push(format!("muse sent unknown request {method}")),
            },
            (Side::Host, Some(method), None) => match schema["notifications"].get(method) {
                Some(entry) => problems.extend(validate_ref(entry.get("params"), &params, method)),
                None => problems.push(format!("muse sent unknown notification {method}")),
            },
            (side, None, Some(id)) => {
                let (calls, table) = match side {
                    Side::Host => (&client_calls, "methods"),
                    Side::Client => (&host_calls, "requests"),
                };
                let Some(method) = calls.get(&id.to_string()) else {
                    problems.push(format!("a response to unknown id {id}"));
                    continue;
                };
                if let Some(result) = frame.get("result") {
                    let what = format!("{method} result");
                    problems.extend(validate_ref(
                        schema[table][method.as_str()].get("result"),
                        result,
                        &what,
                    ));
                }
            }
            (_, None, None) => problems.push(format!("not a JSON-RPC message: {frame}")),
        }
    }
    problems
}

/// A recorded transcript fixture: one `{"dir": "client"|"host", "msg": …}` per line.
pub fn fixture(name: &str) -> Vec<(Side, Value)> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("can't read {path}: {error}"))
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let entry: Value = serde_json::from_str(line).unwrap();
            let side = match entry["dir"].as_str() {
                Some("client") => Side::Client,
                Some("host") => Side::Host,
                other => panic!("bad dir {other:?}"),
            };
            (side, entry["msg"].clone())
        })
        .collect()
}

/// Bytes seen on one side of the wire.
#[derive(Clone, Debug, Default)]
pub struct Recording(Arc<Mutex<Vec<u8>>>);

impl Recording {
    pub fn frames(&self, side: Side) -> Vec<(Side, Value)> {
        let bytes = self.0.lock().unwrap().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| (side, serde_json::from_str(line).unwrap()))
            .collect()
    }
}

/// Records what passes through a reader or writer.
pub struct Tap<T> {
    inner: T,
    recording: Recording,
}

impl<T> Tap<T> {
    pub fn new(inner: T) -> (Self, Recording) {
        let recording = Recording::default();
        (
            Self {
                inner,
                recording: recording.clone(),
            },
            recording,
        )
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Tap<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        ready!(Pin::new(&mut self.inner).poll_read(cx, buf))?;
        let read = buf.filled()[before..].to_vec();
        self.recording.0.lock().unwrap().extend_from_slice(&read);
        Poll::Ready(Ok(()))
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Tap<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let written = ready!(Pin::new(&mut self.inner).poll_write(cx, data))?;
        self.recording
            .0
            .lock()
            .unwrap()
            .extend_from_slice(&data[..written]);
        Poll::Ready(Ok(written))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// What the replaying host saw the client send.
pub type ClientLines = Arc<Mutex<Vec<Value>>>;

/// Plays Muse's side of a recorded transcript on `host`: writes each host frame, and reads one
/// client message for each client frame, which must be the same kind (method, or response id).
/// Ends when the client closes, or with a panic message on a mismatch.
pub fn replay<S>(
    transcript: Vec<(Side, Value)>,
    host: S,
) -> (tokio::task::JoinHandle<()>, ClientLines)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let seen: ClientLines = Arc::default();
    let record = seen.clone();
    let task = tokio::spawn(async move {
        let (reader, mut writer) = tokio::io::split(host);
        let mut lines = BufReader::new(reader).lines();
        for (side, frame) in transcript {
            match side {
                Side::Host => {
                    let mut line = frame.to_string();
                    line.push('\n');
                    writer.write_all(line.as_bytes()).await.unwrap();
                }
                Side::Client => {
                    let line = tokio::time::timeout(WAIT, lines.next_line())
                        .await
                        .unwrap_or_else(|_| panic!("the client never sent {frame}"))
                        .unwrap()
                        .unwrap_or_else(|| panic!("the client closed before sending {frame}"));
                    let actual: Value = serde_json::from_str(&line).unwrap();
                    assert_eq!(
                        shape(&actual),
                        shape(&frame),
                        "the client sent {actual}, the recording has {frame}"
                    );
                    record.lock().unwrap().push(actual);
                }
            }
        }
        // Drain until the client closes its side, as Muse does.
        while let Ok(Some(line)) = lines.next_line().await {
            record
                .lock()
                .unwrap()
                .push(serde_json::from_str(&line).unwrap());
        }
    });
    (task, seen)
}

/// What identifies a message for matching: its method, or the id it answers.
fn shape(message: &Value) -> Value {
    match message.get("method") {
        Some(method) => json!({ "method": method, "request": message.get("id").is_some() }),
        None => json!({ "response": message.get("id") }),
    }
}

/// Reads events until `stop` matches one, returning them all. Panics on a timeout or if the
/// stream ends first.
pub async fn events_until(events: &mut Events, mut stop: impl FnMut(&Event) -> bool) -> Vec<Event> {
    let mut seen = Vec::new();
    loop {
        let event = tokio::time::timeout(WAIT, events.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out; events so far: {seen:#?}"))
            .unwrap_or_else(|| panic!("the event stream ended; events so far: {seen:#?}"));
        let done = stop(&event);
        seen.push(event);
        if done {
            return seen;
        }
    }
}

/// Every `Malformed` event: they mean a type here doesn't match Muse.
pub fn malformed(events: &[Event]) -> Vec<&Event> {
    events
        .iter()
        .filter(|event| matches!(event, Event::Malformed { .. }))
        .collect()
}
