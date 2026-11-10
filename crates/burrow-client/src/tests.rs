//! The client against a scripted fake server over an in-memory pipe.

use std::time::Duration;

use burrow_protocol::methods::{DebugAppend, EventsSubscribe, EventsUnsubscribe, Ping};
use burrow_protocol::notifications::EventsEvent;
use burrow_protocol::{
    ClientKind, DebugAppendParams, DebugAppendResult, Empty, ErrorKind, ErrorObject,
    InitializeResult, Message, Notification, Request, Response, ServerCapabilities, ServerInfo,
    StreamEvent, SubscribeResult, Timestamp, UnsubscribeParams,
};
use burrow_transport::{LineReader, write_line};
use serde_json::json;
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};
use tokio::time::timeout;

use super::*;

const WAIT: Duration = Duration::from_secs(5);

struct FakeServer {
    lines: LineReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
}

impl FakeServer {
    async fn message(&mut self) -> Message {
        let line = timeout(WAIT, self.lines.next_line())
            .await
            .expect("timed out waiting for the client")
            .unwrap()
            .expect("the client closed the connection");
        Message::parse(&line).unwrap()
    }

    async fn request(&mut self) -> Request {
        match self.message().await {
            Message::Request(request) => request,
            other => panic!("expected a request, got {other:?}"),
        }
    }

    async fn send(&mut self, line: String) {
        write_line(&mut self.writer, &line).await.unwrap();
    }

    async fn event(&mut self, stream: &StreamName, seq: u64) {
        let event = StreamEvent {
            stream: stream.clone(),
            seq,
            at: Timestamp::now(),
            event: serde_json::from_value(json!({"type": "test", "seq": seq})).unwrap(),
        };
        self.send(Notification::typed::<EventsEvent>(&event).to_line())
            .await;
    }
}

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "test".into(),
        version: "0.0.0".into(),
        kind: ClientKind::Cli,
    }
}

/// Connects a client to a fake server, which answers `initialize` with `answer`.
async fn connect_with(
    answer: impl FnOnce(Request) -> Response,
) -> (Result<Client, ConnectError>, FakeServer) {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let (reader, writer) = tokio::io::split(server_end);
    let mut server = FakeServer {
        lines: LineReader::new(reader),
        writer,
    };
    let connect = Client::handshake(
        client_end,
        Handle::current(),
        "fake".into(),
        ConnectOptions::new(client_info()),
    );
    let serve = async {
        let request = server.request().await;
        assert_eq!(request.method, Initialize::NAME);
        let params = request.params_as::<Initialize>().unwrap();
        assert_eq!(params.protocol_version, PROTOCOL_VERSION);
        assert_eq!(params.client, client_info());
        server.send(answer(request).to_line()).await;
    };
    let (client, ()) = tokio::join!(connect, serve);
    (client, server)
}

async fn connect() -> (Client, FakeServer) {
    let (client, server) = connect_with(|request| {
        Response::ok::<Initialize>(
            request.id,
            &InitializeResult {
                protocol_version: PROTOCOL_VERSION,
                server: ServerInfo {
                    name: "fake".into(),
                    version: "9.9.9".into(),
                },
                capabilities: ServerCapabilities::default(),
                connection_id: "con_test".into(),
            },
        )
    })
    .await;
    (client.unwrap(), server)
}

/// Subscribes, with the fake server answering `head`/`reset`.
async fn subscribe(
    client: &Client,
    server: &mut FakeServer,
    stream: StreamName,
    after: Option<u64>,
    head: u64,
    reset: bool,
) -> Subscription {
    let subscribing = client.subscribe(stream.clone(), after);
    let serve = async {
        let request = server.request().await;
        assert_eq!(request.method, EventsSubscribe::NAME);
        let params = request.params_as::<EventsSubscribe>().unwrap();
        assert_eq!(
            (params.stream.clone(), params.after),
            (stream.clone(), after)
        );
        let result = SubscribeResult {
            stream: stream.clone(),
            head,
            reset,
        };
        server
            .send(Response::ok::<EventsSubscribe>(request.id, &result).to_line())
            .await;
    };
    let (subscription, ()) = tokio::join!(subscribing, serve);
    subscription.unwrap()
}

async fn next(subscription: &mut Subscription) -> Option<StreamEvent> {
    timeout(WAIT, subscription.next())
        .await
        .expect("timed out waiting for an event")
}

#[tokio::test]
async fn the_handshake_reports_the_server() {
    let (client, _server) = connect().await;
    assert_eq!(client.connection_id(), "con_test");
    assert_eq!(client.server().server.version, "9.9.9");
    assert_eq!(client.pipe(), "fake");
    assert!(!client.is_closed());
}

#[tokio::test]
async fn a_version_mismatch_is_rejected() {
    let (client, _server) = connect_with(|request| {
        Response::error(
            Some(request.id),
            ErrorObject::version_mismatch(PROTOCOL_VERSION, vec![7]),
        )
    })
    .await;
    match client.unwrap_err() {
        ConnectError::Rejected(error) => assert_eq!(error.kind(), Some(ErrorKind::VersionMismatch)),
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[tokio::test]
async fn responses_are_matched_to_calls_by_id() {
    let (client, mut server) = connect().await;
    let params = DebugAppendParams {
        stream: StreamName::index(),
        event: serde_json::from_value(json!({"type": "test"})).unwrap(),
    };
    let first = client.call::<DebugAppend>(&params);
    let second = client.call::<DebugAppend>(&params);
    let serve = async {
        let a = server.request().await;
        let b = server.request().await;
        // Answered in reverse order.
        server
            .send(Response::ok::<DebugAppend>(b.id, &DebugAppendResult { seq: 2 }).to_line())
            .await;
        server
            .send(Response::ok::<DebugAppend>(a.id, &DebugAppendResult { seq: 1 }).to_line())
            .await;
    };
    let (first, second, ()) = tokio::join!(first, second, serve);
    assert_eq!(first.unwrap().seq, 1);
    assert_eq!(second.unwrap().seq, 2);
}

#[tokio::test]
async fn errors_come_back_as_rpc_errors() {
    let (client, mut server) = connect().await;
    let call = client.call::<Ping>(&Empty {});
    let serve = async {
        let request = server.request().await;
        let error = ErrorObject::method_not_found(&request.method);
        server
            .send(Response::error(Some(request.id), error).to_line())
            .await;
    };
    let (result, ()) = tokio::join!(call, serve);
    assert_eq!(result.unwrap_err().rpc().unwrap().code, -32601);
}

#[tokio::test]
async fn a_subscription_yields_events_and_tracks_its_position() {
    let (client, mut server) = connect().await;
    let stream = StreamName::index();
    let mut subscription = subscribe(&client, &mut server, stream.clone(), Some(3), 5, false).await;
    assert_eq!((subscription.head(), subscription.reset()), (5, false));
    assert_eq!(subscription.position(), 3);

    server.event(&stream, 4).await;
    server.event(&StreamName::session("other"), 1).await;
    server.event(&stream, 5).await;
    assert_eq!(next(&mut subscription).await.unwrap().seq, 4);
    assert_eq!(subscription.position(), 4);
    assert_eq!(next(&mut subscription).await.unwrap().seq, 5);
    assert_eq!(
        subscription.resume_params(),
        SubscribeParams {
            stream,
            after: Some(5)
        }
    );
}

#[tokio::test]
async fn live_only_and_reset_subscriptions_start_at_the_head() {
    let (client, mut server) = connect().await;
    let live = subscribe(&client, &mut server, StreamName::run("a"), None, 7, false).await;
    assert_eq!(live.position(), 7);
    let reset = subscribe(
        &client,
        &mut server,
        StreamName::run("b"),
        Some(10),
        7,
        true,
    )
    .await;
    assert!(reset.reset());
    assert_eq!(reset.position(), 7);
    assert_eq!(reset.resume_params().after, Some(7));
}

#[tokio::test]
async fn events_that_arrive_before_the_subscribe_answer_are_dropped() {
    let (client, mut server) = connect().await;
    let stream = StreamName::index();
    let subscribing = client.subscribe(stream.clone(), Some(0));
    let serve = async {
        let request = server.request().await;
        // Left over from an earlier subscription to the same stream.
        server.event(&stream, 99).await;
        let result = SubscribeResult {
            stream: stream.clone(),
            head: 0,
            reset: false,
        };
        server
            .send(Response::ok::<EventsSubscribe>(request.id, &result).to_line())
            .await;
        server.event(&stream, 1).await;
    };
    let (subscription, ()) = tokio::join!(subscribing, serve);
    let mut subscription = subscription.unwrap();
    assert_eq!(next(&mut subscription).await.unwrap().seq, 1);
}

#[tokio::test]
async fn dropping_a_subscription_unsubscribes() {
    let (client, mut server) = connect().await;
    let stream = StreamName::session("ses_1");
    let subscription = subscribe(&client, &mut server, stream.clone(), None, 0, false).await;
    drop(subscription);
    let request = server.request().await;
    assert_eq!(request.method, EventsUnsubscribe::NAME);
    assert_eq!(
        request.params_as::<EventsUnsubscribe>().unwrap(),
        UnsubscribeParams {
            stream: stream.clone()
        }
    );
    // Its events no longer go anywhere, and nothing breaks.
    server.event(&stream, 1).await;
    let ping = client.call::<Ping>(&Empty {});
    let serve = async {
        let request = server.request().await;
        server
            .send(Response::ok::<Ping>(request.id, &Empty {}).to_line())
            .await;
    };
    let (pong, ()) = tokio::join!(ping, serve);
    pong.unwrap();
}

#[tokio::test]
async fn unsubscribe_waits_for_the_answer() {
    let (client, mut server) = connect().await;
    let stream = StreamName::index();
    let subscription = subscribe(&client, &mut server, stream.clone(), None, 0, false).await;
    let unsubscribing = subscription.unsubscribe();
    let serve = async {
        let request = server.request().await;
        assert_eq!(request.method, EventsUnsubscribe::NAME);
        server
            .send(Response::ok::<EventsUnsubscribe>(request.id, &Empty {}).to_line())
            .await;
    };
    let (result, ()) = tokio::join!(unsubscribing, serve);
    result.unwrap();
}

#[tokio::test]
async fn a_disconnect_fails_calls_and_ends_subscriptions() {
    let (client, mut server) = connect().await;
    let mut subscription =
        subscribe(&client, &mut server, StreamName::index(), None, 0, false).await;
    server.event(&StreamName::index(), 1).await;

    let ping = client.call::<Ping>(&Empty {});
    let hang_up = async {
        server.request().await;
        drop(server);
    };
    let (result, ()) = tokio::join!(ping, hang_up);
    assert!(matches!(result, Err(CallError::Disconnected)), "{result:?}");

    // Events already received are still yielded, then the subscription ends.
    assert_eq!(next(&mut subscription).await.unwrap().seq, 1);
    assert!(next(&mut subscription).await.is_none());
    timeout(WAIT, client.closed()).await.unwrap();
    assert!(client.is_closed());
    assert!(matches!(
        client.call::<Ping>(&Empty {}).await,
        Err(CallError::Disconnected)
    ));
}

#[tokio::test]
async fn close_ends_the_connection() {
    let (client, mut server) = connect().await;
    client.close();
    assert!(client.is_closed());
    let end = timeout(WAIT, server.lines.next_line()).await.unwrap();
    assert!(matches!(end, Ok(None)), "{end:?}");
}

#[tokio::test]
async fn other_notifications_reach_the_notifications_stream() {
    let (client, mut server) = connect().await;
    let mut notifications = client.notifications();
    server
        .send(Notification::new("test/hello", Some(json!({"n": 1}))).to_line())
        .await;
    let notification = timeout(WAIT, notifications.next()).await.unwrap().unwrap();
    assert_eq!(notification.method, "test/hello");
}

#[tokio::test]
async fn answers_sent_in_order_arrive_among_the_notifications() {
    let (client, mut server) = connect().await;
    let mut notifications = client.notifications();
    let id = client.send_in_order::<Ping>(&Empty {}).unwrap();
    let request = server.request().await;
    assert_eq!(request.id, id);
    // A notification before the answer, then the answer, then one after it.
    let hello = |n: u32| Notification::new("test/hello", Some(json!({"n": n})));
    server.send(hello(1).to_line()).await;
    server
        .send(Response::ok::<Ping>(request.id, &Empty {}).to_line())
        .await;
    server.send(hello(2).to_line()).await;
    let mut seen = Vec::new();
    for _ in 0..3 {
        seen.push(
            timeout(WAIT, notifications.next_incoming())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert_eq!(seen[0], Incoming::Notification(hello(1)));
    match &seen[1] {
        Incoming::Answer(response) => {
            assert_eq!(response.id.as_ref(), Some(&id));
            response.result_as::<Ping>().unwrap();
        }
        other => panic!("expected the answer, got {other:?}"),
    }
    assert_eq!(seen[2], Incoming::Notification(hello(2)));

    // `next` skips such answers.
    let id = client.send_in_order::<Ping>(&Empty {}).unwrap();
    let request = server.request().await;
    assert_eq!(request.id, id);
    server
        .send(Response::ok::<Ping>(request.id, &Empty {}).to_line())
        .await;
    server.send(hello(3).to_line()).await;
    let notification = timeout(WAIT, notifications.next()).await.unwrap().unwrap();
    assert_eq!(notification, hello(3));
    assert!(notifications.try_next_incoming().is_none());

    // Closed: no request goes out.
    client.close();
    assert!(matches!(
        client.send_in_order::<Ping>(&Empty {}),
        Err(CallError::Disconnected)
    ));
}

#[tokio::test]
async fn requests_from_the_server_get_method_not_found() {
    let (_client, mut server) = connect().await;
    server
        .send(Request::new(41, "client/doSomething", None).to_line())
        .await;
    let Message::Response(response) = server.message().await else {
        panic!("expected a response");
    };
    assert_eq!(response.id, Some(41.into()));
    assert_eq!(response.into_result().unwrap_err().code, -32601);
}

#[tokio::test]
async fn an_undecodable_event_ends_its_subscription() {
    let (client, mut server) = connect().await;
    let stream = StreamName::index();
    let mut subscription = subscribe(&client, &mut server, stream.clone(), None, 0, false).await;
    // `entityCreated` without its required fields.
    let bad = json!({"stream": "index", "seq": 1, "at": "2026-11-09T00:00:00.000Z",
                     "event": {"type": "entityCreated"}});
    server
        .send(Notification::new(EventsEvent::NAME, Some(bad)).to_line())
        .await;
    assert!(next(&mut subscription).await.is_none());
    let request = server.request().await;
    assert_eq!(request.method, EventsUnsubscribe::NAME);
}

#[tokio::test]
async fn a_pending_call_can_be_cancelled() {
    let (client, mut server) = connect().await;
    let call = client.start_call::<Ping>(&Empty {}).unwrap();
    let request = server.request().await;
    assert_eq!(&request.id, call.id());

    call.cancel().unwrap();
    let Message::Notification(cancel) = server.message().await else {
        panic!("expected $/cancel");
    };
    let params = cancel
        .params_as::<burrow_protocol::notifications::Cancel>()
        .unwrap();
    assert_eq!(params.id, request.id);

    let error = ErrorObject::new(ErrorKind::Cancelled, "the request was cancelled");
    server
        .send(Response::error(Some(request.id), error).to_line())
        .await;
    let result = timeout(WAIT, call.result()).await.unwrap();
    assert_eq!(
        result.unwrap_err().rpc().unwrap().kind(),
        Some(ErrorKind::Cancelled)
    );
}
