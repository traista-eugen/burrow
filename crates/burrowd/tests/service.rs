//! End-to-end tests: the real `burrowd` binary, driven through `burrow-client` or a raw pipe.
//!
//! The first test is M1's "Done when" (docs/plan.md).

mod common;

use std::collections::BTreeMap;

use burrow_client::{CallError, Client};
use burrow_protocol::methods::{DebugAppend, Ping};
use burrow_protocol::{
    DebugAppendParams, Empty, ErrorKind, Message, PROTOCOL_VERSION, Response, StreamName,
};
use burrow_transport::{LineReader, pipe_path, write_line};
use common::*;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::time::timeout;

/// M1, "Done when": start burrowd, subscribe to `index`, append events; kill and restart
/// burrowd; resubscribe with the last seq seen: every event arrives exactly once, in order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_survive_a_hard_kill_and_resume_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let pipe = unique_pipe("restart");
    let index = StreamName::index();

    // 1. Start burrowd, subscribe to index, append events.
    let mut daemon = Daemon::start(dir.path(), &pipe, true).await;
    let client = connect(&pipe).await;
    let mut subscription = client.subscribe(index.clone(), Some(0)).await.unwrap();
    assert_eq!((subscription.head(), subscription.reset()), (0, false));
    for n in 1..=20 {
        assert_eq!(append(&client, &index, n).await, n);
    }
    // Process the first 12. The rest are in flight, or buffered but unprocessed, when burrowd dies.
    let mut processed = take(&mut subscription, 12).await;

    // 2. Kill burrowd the hard way, and start it again on the same data dir and pipe.
    daemon.kill();
    timeout(WAIT, client.closed())
        .await
        .expect("the client notices that burrowd is gone");
    let resume = subscription.resume_params();
    assert_eq!(resume.after, Some(12));
    drop(subscription);
    drop(client);
    let _daemon = Daemon::start(dir.path(), &pipe, true).await;

    // 3. Resubscribe from the last seq processed, and append more.
    let client = connect(&pipe).await;
    let mut subscription = client.subscribe_with(resume).await.unwrap();
    assert_eq!((subscription.head(), subscription.reset()), (20, false));
    for n in 21..=30 {
        assert_eq!(append(&client, &index, n).await, n);
    }
    processed.extend(take(&mut subscription, 18).await);
    assert_quiet(&mut subscription).await;

    // Every event exactly once, in order, without gaps, and each is the one appended at its seq.
    let expected: Vec<u64> = (1..=30).collect();
    assert_eq!(seqs(&processed), expected);
    assert_eq!(
        processed.iter().map(event_number).collect::<Vec<_>>(),
        expected
    );
    assert!(processed.iter().all(|event| event.stream == index));
}

/// Appends `count` events numbered `first + 1 ..` to `stream` from `writers` connections at
/// once, and returns which number got which seq.
async fn fill(
    pipe: String,
    stream: StreamName,
    first: u64,
    count: u64,
    writers: u64,
) -> BTreeMap<u64, u64> {
    let tasks: Vec<_> = (0..writers)
        .map(|writer| {
            let (pipe, stream) = (pipe.clone(), stream.clone());
            tokio::spawn(async move {
                let client = connect(&pipe).await;
                let mut seqs = Vec::new();
                for n in (first + 1..=first + count).filter(|n| n % writers == writer) {
                    seqs.push((append(&client, &stream, n).await, n));
                }
                seqs
            })
        })
        .collect();
    let mut by_seq = BTreeMap::new();
    for task in tasks {
        for (seq, n) in task.await.unwrap() {
            assert!(by_seq.insert(seq, n).is_none(), "seq {seq} given twice");
        }
    }
    by_seq
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replays_racing_with_appends_stay_in_order_without_gaps() {
    const BEFORE: u64 = 600;
    const DURING: u64 = 600;
    const TOTAL: u64 = BEFORE + DURING;

    let dir = tempfile::tempdir().unwrap();
    let pipe = unique_pipe("race");
    let _daemon = Daemon::start(dir.path(), &pipe, true).await;
    let stream = StreamName::new("race");

    // More history than one replay page, then a second wave that runs while clients subscribe.
    let mut numbers = fill(pipe.clone(), stream.clone(), 0, BEFORE, 6).await;
    let wave = tokio::spawn(fill(pipe.clone(), stream.clone(), BEFORE, DURING, 6));
    let from_start = connect(&pipe)
        .await
        .subscribe(stream.clone(), Some(0))
        .await
        .unwrap();
    let from_middle = connect(&pipe)
        .await
        .subscribe(stream.clone(), Some(250))
        .await
        .unwrap();
    let live = connect(&pipe)
        .await
        .subscribe(stream.clone(), None)
        .await
        .unwrap();
    numbers.extend(wave.await.unwrap());
    assert_eq!(
        numbers.keys().copied().collect::<Vec<_>>(),
        (1..=TOTAL).collect::<Vec<_>>()
    );

    let live_from = live.head() + 1;
    for (mut subscription, first) in [(from_start, 1), (from_middle, 251), (live, live_from)] {
        let events = take(&mut subscription, (TOTAL - first + 1) as usize).await;
        assert_eq!(seqs(&events), (first..=TOTAL).collect::<Vec<_>>());
        for event in &events {
            assert_eq!(
                event_number(event),
                numbers[&event.seq],
                "seq {}",
                event.seq
            );
        }
        assert_quiet(&mut subscription).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_subscribers_see_the_same_events() {
    let dir = tempfile::tempdir().unwrap();
    let pipe = unique_pipe("two");
    let _daemon = Daemon::start(dir.path(), &pipe, true).await;
    let index = StreamName::index();
    let writer = connect(&pipe).await;
    for n in 1..=3 {
        append(&writer, &index, n).await;
    }

    let mut history = connect(&pipe)
        .await
        .subscribe(index.clone(), Some(0))
        .await
        .unwrap();
    let mut live = connect(&pipe)
        .await
        .subscribe(index.clone(), None)
        .await
        .unwrap();
    assert_eq!(live.head(), 3);
    for n in 4..=6 {
        append(&writer, &index, n).await;
    }

    let all = take(&mut history, 6).await;
    let new = take(&mut live, 3).await;
    assert_eq!(seqs(&all), [1, 2, 3, 4, 5, 6]);
    assert_eq!(seqs(&new), [4, 5, 6]);
    assert_eq!(all[3..], new[..], "the same events, timestamps included");
    assert_quiet(&mut history).await;
    assert_quiet(&mut live).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscriptions_answer_reset_and_end_on_unsubscribe() {
    let dir = tempfile::tempdir().unwrap();
    let pipe = unique_pipe("unsubscribe");
    let _daemon = Daemon::start(dir.path(), &pipe, true).await;
    let client = connect(&pipe).await;
    let stream = StreamName::session("ses_x");
    append(&client, &stream, 1).await;
    append(&client, &stream, 2).await;

    // An `after` beyond the head: nothing replayed, live events still follow.
    let mut subscription = client.subscribe(stream.clone(), Some(10)).await.unwrap();
    assert_eq!((subscription.head(), subscription.reset()), (2, true));
    assert_eq!(subscription.position(), 2);

    // One subscription per stream per connection.
    let error = client.subscribe(stream.clone(), None).await.unwrap_err();
    assert_eq!(error.rpc().unwrap().kind(), Some(ErrorKind::InvalidState));

    append(&client, &stream, 3).await;
    assert_eq!(seqs(&take(&mut subscription, 1).await), [3]);
    subscription.unsubscribe().await.unwrap();

    // Once unsubscribed, the stream can be subscribed again.
    append(&client, &stream, 4).await;
    let mut again = client.subscribe(stream.clone(), Some(2)).await.unwrap();
    assert_eq!(seqs(&take(&mut again, 2).await), [3, 4]);
    assert_quiet(&mut again).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debug_append_is_refused_without_dev() {
    let dir = tempfile::tempdir().unwrap();
    let pipe = unique_pipe("nodev");
    let _daemon = Daemon::start(dir.path(), &pipe, false).await;
    let client = connect(&pipe).await;
    client.call::<Ping>(&Empty {}).await.unwrap();
    let error = client
        .call::<DebugAppend>(&DebugAppendParams {
            stream: StreamName::index(),
            event: event(1),
        })
        .await
        .unwrap_err();
    let CallError::Rpc(error) = error else {
        panic!("expected an error answer, got {error:?}");
    };
    assert_eq!(error.code, burrow_protocol::code::METHOD_NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_burrowd_on_the_same_pipe_exits() {
    let first_dir = tempfile::tempdir().unwrap();
    let second_dir = tempfile::tempdir().unwrap();
    let pipe = unique_pipe("single");
    let mut first = Daemon::start(first_dir.path(), &pipe, false).await;

    let args = [
        "--pipe".to_owned(),
        pipe.clone(),
        "--data-dir".to_owned(),
        second_dir.path().display().to_string(),
    ];
    let output = tokio::task::spawn_blocking(move || {
        run_to_exit(&args.iter().map(String::as_str).collect::<Vec<_>>())
    })
    .await
    .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("already running"), "{stderr}");
    assert!(
        !second_dir.path().join("burrow.db").exists(),
        "it exits before opening a database"
    );

    // The first one carries on.
    assert!(first.exited().is_none());
    connect(&pipe).await.call::<Ping>(&Empty {}).await.unwrap();
}

/// A client speaking raw lines over the pipe, for what `burrow-client` never sends.
struct RawClient {
    lines: LineReader<ReadHalf<NamedPipeClient>>,
    writer: WriteHalf<NamedPipeClient>,
}

impl RawClient {
    fn connect(pipe: &str) -> Self {
        let pipe = ClientOptions::new().open(pipe_path(pipe)).unwrap();
        let (reader, writer) = tokio::io::split(pipe);
        Self {
            lines: LineReader::new(reader),
            writer,
        }
    }

    async fn send(&mut self, line: &str) {
        write_line(&mut self.writer, line).await.unwrap();
    }

    async fn response(&mut self) -> Response {
        let line = timeout(WAIT, self.lines.next_line())
            .await
            .expect("timed out waiting for burrowd")
            .unwrap()
            .expect("burrowd closed the connection");
        match Message::parse(&line).unwrap() {
            Message::Response(response) => response,
            other => panic!("expected a response, got {other:?}"),
        }
    }

    async fn initialize(&mut self, id: u32, version: u32) -> Response {
        self.send(&format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"initialize","params":{{"protocolVersion":{version},"client":{{"name":"raw","version":"0","kind":"cli"}}}}}}"#
        ))
        .await;
        self.response().await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_handshake_comes_first() {
    let dir = tempfile::tempdir().unwrap();
    let pipe = unique_pipe("handshake");
    let _daemon = Daemon::start(dir.path(), &pipe, true).await;
    let mut raw = RawClient::connect(&pipe);

    // Nothing but initialize before initialize.
    raw.send(r#"{"jsonrpc":"2.0","id":1,"method":"$/ping"}"#)
        .await;
    let response = raw.response().await;
    assert_eq!(response.id, Some(1.into()));
    assert_eq!(
        response.into_result().unwrap_err().kind(),
        Some(ErrorKind::InvalidState)
    );

    // A line that isn't JSON: parse error, null id.
    raw.send("this is not json").await;
    let response = raw.response().await;
    assert_eq!(response.id, None);
    assert_eq!(response.into_result().unwrap_err().code, -32700);

    // Another protocol version: versionMismatch, listing what burrowd supports.
    let error = raw.initialize(2, 99).await.into_result().unwrap_err();
    assert_eq!(error.kind(), Some(ErrorKind::VersionMismatch));
    assert_eq!(error.data.unwrap().supported, Some(vec![PROTOCOL_VERSION]));

    // The right version: a connection id, `con_` plus 26 base32 characters.
    let result = raw
        .initialize(3, PROTOCOL_VERSION)
        .await
        .into_result()
        .unwrap();
    let connection_id = result["connectionId"].as_str().unwrap();
    assert!(connection_id.starts_with("con_"), "{connection_id}");
    assert_eq!(connection_id.len(), 30, "{connection_id}");
    assert_eq!(result["server"]["name"], "burrowd");

    // Only once.
    let error = raw
        .initialize(4, PROTOCOL_VERSION)
        .await
        .into_result()
        .unwrap_err();
    assert_eq!(error.kind(), Some(ErrorKind::InvalidState));

    // $/cancel is accepted and gets no answer: the next answer is the ping's.
    raw.send(r#"{"jsonrpc":"2.0","method":"$/cancel","params":{"id":5}}"#)
        .await;
    raw.send(r#"{"jsonrpc":"2.0","id":5,"method":"$/ping"}"#)
        .await;
    let response = raw.response().await;
    assert_eq!(response.id, Some(5.into()));
    assert_eq!(response.into_result().unwrap(), serde_json::json!({}));

    // Unknown methods.
    raw.send(r#"{"jsonrpc":"2.0","id":"x","method":"nope/nope"}"#)
        .await;
    let response = raw.response().await;
    assert_eq!(response.id, Some("x".into()));
    assert_eq!(response.into_result().unwrap_err().code, -32601);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_stops_reading_is_disconnected() {
    const FLOOD: u64 = 5000;

    let dir = tempfile::tempdir().unwrap();
    let pipe = unique_pipe("slow");
    let daemon = Daemon::start(dir.path(), &pipe, true).await;
    let stream = StreamName::new("flood");

    let mut slow = RawClient::connect(&pipe);
    slow.initialize(1, PROTOCOL_VERSION)
        .await
        .into_result()
        .unwrap();
    slow.send(
        r#"{"jsonrpc":"2.0","id":2,"method":"events/subscribe","params":{"stream":"flood"}}"#,
    )
    .await;
    slow.response().await.into_result().unwrap();

    // The slow client reads nothing while far more events than its buffers hold are appended.
    fill(pipe.clone(), stream, 0, FLOOD, 8).await;

    // burrowd has closed the connection: reading reaches the end instead of waiting forever.
    let mut received = 0;
    let ended = timeout(WAIT, async {
        while let Ok(Some(_)) = slow.lines.next_line().await {
            received += 1;
        }
    })
    .await;
    assert!(ended.is_ok(), "burrowd kept a stuck client connected");
    assert!(received < FLOOD, "received all {received} events");
    assert!(daemon.logs().contains("fell behind"), "{}", daemon.logs());

    // Everyone else is unaffected.
    connect(&pipe).await.call::<Ping>(&Empty {}).await.unwrap();
}

/// The desktop app runs on GPUI's executor, not tokio (D-016). The client's futures must work
/// there, with the I/O on a runtime the client is given.
#[test]
fn the_client_works_from_another_executor() {
    let io = burrow_client::io_runtime().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pipe = unique_pipe("executor");
    let _daemon = io.block_on(Daemon::start(dir.path(), &pipe, true));

    // No tokio context on this thread from here on.
    futures::executor::block_on(async {
        assert!(tokio::runtime::Handle::try_current().is_err());
        let client = connect_on(&pipe, io.handle().clone()).await;
        client.call::<Ping>(&Empty {}).await.unwrap();
        let mut subscription = client
            .subscribe(StreamName::index(), Some(0))
            .await
            .unwrap();
        let seq = append(&client, &StreamName::index(), 1).await;
        assert_eq!(subscription.next().await.unwrap().seq, seq);

        // Without a runtime, connecting says so instead of panicking.
        let error = Client::connect(options(&pipe)).await.unwrap_err();
        assert!(
            matches!(error, burrow_client::ConnectError::NoRuntime),
            "{error:?}"
        );
    });
}
