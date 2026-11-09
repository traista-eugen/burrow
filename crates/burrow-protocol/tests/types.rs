//! Serde behaviour of the protocol types: round trips, forward compatibility, message
//! classification, errors, typed methods and timestamps.

mod common;

use burrow_protocol::methods::{DebugAppend, EventsSubscribe, Initialize, Ping};
use burrow_protocol::notifications::{Cancel, EventsEvent};
use burrow_protocol::*;
use serde_json::{Value, json};

use common::{AT, samples};

#[test]
fn every_sample_round_trips() {
    for sample in samples() {
        (sample.check)(&sample.json);
    }
}

// ---- Forward compatibility -------------------------------------------------------------------

#[test]
fn unknown_fields_are_ignored() {
    let params: InitializeParams = serde_json::from_value(json!({
        "protocolVersion": 0,
        "client": {"name": "burrow-desktop", "version": "0.1.0", "kind": "desktop", "build": "abc"},
        "capabilities": {"terminal": true, "mediaTypes": [], "holograms": true},
        "somethingNew": {"nested": [1, 2]},
    }))
    .unwrap();
    assert_eq!(params.client.kind, ClientKind::Desktop);
    assert!(params.capabilities.terminal);

    let result: SubscribeResult =
        serde_json::from_value(json!({"stream": "index", "head": 3, "tail": 1})).unwrap();
    assert_eq!(result.head, 3);

    // In a known event, too: the event still decodes as its own type.
    let event: Event = serde_json::from_value(json!({
        "type": "entityStateChanged", "entity": "session", "id": "ses_1", "state": "idle", "reason": "new",
    }))
    .unwrap();
    assert!(matches!(event, Event::EntityStateChanged(ref e) if e.state == "idle"));

    // And in the envelope.
    let message =
        Message::parse(r#"{"jsonrpc":"2.0","id":1,"method":"$/ping","meta":{"trace":"x"}}"#)
            .unwrap();
    assert!(matches!(message, Message::Request(_)));
}

#[test]
fn absent_optional_fields_take_defaults() {
    let params: InitializeParams = serde_json::from_value(json!({
        "protocolVersion": 0,
        "client": {"name": "burrowctl", "version": "0.1.0", "kind": "cli"},
    }))
    .unwrap();
    assert_eq!(params.capabilities, ClientCapabilities::default());

    let result: InitializeResult = serde_json::from_value(json!({
        "protocolVersion": 0,
        "server": {"name": "burrowd", "version": "0.1.0"},
        "connectionId": "con_1",
    }))
    .unwrap();
    assert_eq!(result.capabilities, ServerCapabilities::default());

    let result: SubscribeResult =
        serde_json::from_value(json!({"stream": "index", "head": 0})).unwrap();
    assert!(!result.reset);
}

#[test]
fn unknown_event_types_are_kept() {
    let raw = json!({"type": "stepProgress", "stepPath": "plan", "message": "Reading the issue…", "n": [1, {"x": null}]});
    let event: Event = serde_json::from_value(raw.clone()).unwrap();
    let Event::Unknown(unknown) = &event else {
        panic!("expected an unknown event, got {event:?}");
    };
    assert_eq!(unknown.type_name(), "stepProgress");
    assert_eq!(event.type_name(), "stepProgress");
    assert!(!event.is_known());
    assert_eq!(Value::Object(unknown.as_object().clone()), raw);
    assert_eq!(serde_json::to_value(&event).unwrap(), raw);

    // Inside a notification too.
    let line = format!(
        r#"{{"jsonrpc":"2.0","method":"events/event","params":{{"stream":"session:ses_1","seq":121,"at":"{AT}","event":{raw}}}}}"#
    );
    let Message::Notification(notification) = Message::parse(&line).unwrap() else {
        panic!("expected a notification");
    };
    let params = notification.params_as::<EventsEvent>().unwrap();
    assert_eq!(params.event, event);
    assert_eq!(params.seq, 121);
}

#[test]
fn malformed_events_are_rejected() {
    // A known type with the wrong fields is a bug, not a newer server.
    let error = serde_json::from_value::<Event>(json!({"type": "entityRenamed", "entity": "run"}))
        .unwrap_err();
    assert!(error.to_string().contains("entityRenamed"), "{error}");
    assert!(serde_json::from_value::<Event>(json!({"entity": "run"})).is_err());
    assert!(serde_json::from_value::<Event>(json!({"type": 3})).is_err());
    assert!(serde_json::from_value::<Event>(json!("entityCreated")).is_err());
}

#[test]
fn known_events_serialize_with_type_first() {
    let event = Event::from(EntityRenamed {
        entity: EntityKind::Run,
        id: "run_1".into(),
        title: "Deliver #12".into(),
    });
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"type":"entityRenamed","entity":"run","id":"run_1","title":"Deliver #12"}"#
    );
    assert_eq!(event.type_name(), "entityRenamed");
    assert!(Event::KNOWN_TYPES.contains(&"inboxItemResolved"));
}

#[test]
fn open_enums_keep_unknown_values() {
    assert_eq!(
        EntityKind::from("device"),
        EntityKind::Other("device".into())
    );
    assert_eq!(EntityKind::from("run"), EntityKind::Run);
    assert_eq!(
        serde_json::to_value(EntityKind::from("device")).unwrap(),
        json!("device")
    );
    assert!(!InboxItemKind::from("poll").is_known());
    assert_eq!(InboxItemKind::ManualTask.as_str(), "manualTask");
    assert_eq!(
        ErrorKind::from("somethingNewer").to_string(),
        "somethingNewer"
    );
}

#[test]
fn closed_enums_reject_unknown_values() {
    assert!(serde_json::from_value::<ClientKind>(json!("watch")).is_err());
    assert!(serde_json::from_value::<InboxItemStatus>(json!("snoozed")).is_err());
}

// ---- Message classification ------------------------------------------------------------------

#[test]
fn classifies_requests_responses_and_notifications() {
    let request = Message::parse(
        r#"{"jsonrpc":"2.0","id":3,"method":"events/subscribe","params":{"stream":"index"}}"#,
    )
    .unwrap();
    assert!(matches!(&request, Message::Request(r) if r.id == RequestId::Number(3)));
    assert_eq!(request.method(), Some("events/subscribe"));

    let notification =
        Message::parse(r#"{"jsonrpc":"2.0","method":"$/cancel","params":{"id":3}}"#).unwrap();
    assert!(matches!(notification, Message::Notification(_)));

    let ok = Message::parse(r#"{"jsonrpc":"2.0","id":"x","result":null}"#).unwrap();
    let Message::Response(ok) = ok else { panic!() };
    assert_eq!(ok.id, Some(RequestId::from("x")));
    assert_eq!(ok.outcome, ResponseOutcome::Result(Value::Null));

    let failed =
        Message::parse(r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"bad"}}"#)
            .unwrap();
    let Message::Response(failed) = failed else {
        panic!()
    };
    assert_eq!(failed.id, None);
    assert_eq!(failed.into_result().unwrap_err().code, code::PARSE_ERROR);
}

#[test]
fn messages_serialize_as_one_line() {
    let request = Request::typed::<EventsSubscribe>(
        5,
        &SubscribeParams {
            stream: StreamName::session("ses_1"),
            after: Some(120),
        },
    );
    // Params go through `serde_json::Value`, whose key order depends on serde_json's
    // `preserve_order` feature (on in workspace builds, via GPUI), so compare as JSON.
    let line = request.to_line();
    assert!(!line.contains('\n'));
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({"jsonrpc": "2.0", "id": 5, "method": "events/subscribe",
               "params": {"stream": "session:ses_1", "after": 120}})
    );
    assert!(line.starts_with(r#"{"jsonrpc":"2.0","id":5,"method":"events/subscribe","params":{"#));
    let response = Response::error(None, ErrorObject::parse_error("not JSON"));
    assert_eq!(
        response.to_line(),
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"not JSON"}}"#
    );
    let notification = Notification::typed::<Cancel>(&CancelParams { id: 5.into() });
    assert_eq!(
        notification.to_line(),
        r#"{"jsonrpc":"2.0","method":"$/cancel","params":{"id":5}}"#
    );
    let message = Message::Notification(notification);
    assert!(!message.to_line().contains('\n'));
    assert_eq!(Message::parse(&message.to_line()).unwrap(), message);
}

#[test]
fn rejects_invalid_messages() {
    fn invalid(line: &str) -> MessageError {
        let error = Message::parse(line).expect_err(line);
        assert!(
            matches!(error, MessageError::Invalid { .. }),
            "{line}: {error:?}"
        );
        assert_eq!(error.to_error().code, code::INVALID_REQUEST);
        error
    }

    // Not JSON at all: a parse error, answered with a null id.
    let error = Message::parse("{not json").unwrap_err();
    assert!(matches!(error, MessageError::Parse(_)));
    assert_eq!(error.to_response().id, None);
    assert_eq!(error.to_error().code, code::PARSE_ERROR);

    invalid(r#"[{"jsonrpc":"2.0","id":1,"method":"$/ping"}]"#); // batches
    invalid(r#""just a string""#);
    invalid(r#"{"id":1,"method":"$/ping"}"#); // no jsonrpc
    invalid(r#"{"jsonrpc":"1.0","id":1,"method":"$/ping"}"#);
    invalid(r#"{"jsonrpc":"2.0","id":1,"method":7}"#);
    invalid(r#"{"jsonrpc":"2.0","id":null,"method":"$/ping"}"#); // null request id
    invalid(r#"{"jsonrpc":"2.0","id":1.5,"method":"$/ping"}"#);
    invalid(r#"{"jsonrpc":"2.0","id":1,"method":"$/ping","params":3}"#);
    invalid(r#"{"jsonrpc":"2.0","id":1,"method":"$/ping","result":{}}"#);
    invalid(r#"{"jsonrpc":"2.0","id":1}"#);
    invalid(r#"{"jsonrpc":"2.0","result":{}}"#); // response without id
    invalid(r#"{"jsonrpc":"2.0","id":null,"result":{}}"#);
    invalid(r#"{"jsonrpc":"2.0","id":1,"result":{},"error":{"code":1,"message":"x"}}"#);
    invalid(r#"{"jsonrpc":"2.0","id":1,"error":{"code":"x"}}"#);

    // The id is reported whenever it could be read, so the error can be answered.
    let error = invalid(r#"{"jsonrpc":"1.0","id":"abc","method":"$/ping"}"#);
    assert_eq!(error.id(), Some(&RequestId::from("abc")));
    assert_eq!(error.to_response().id, Some(RequestId::from("abc")));
}

#[test]
fn request_ids_are_integers_or_strings() {
    assert_eq!(
        serde_json::from_str::<RequestId>("7").unwrap(),
        RequestId::Number(7)
    );
    assert_eq!(
        serde_json::from_str::<RequestId>("\"7\"").unwrap(),
        RequestId::String("7".into())
    );
    assert!(serde_json::from_str::<RequestId>("7.5").is_err());
    assert!(serde_json::from_str::<RequestId>("null").is_err());
    assert!(serde_json::from_str::<RequestId>("18446744073709551615").is_err());
}

// ---- Typed methods ---------------------------------------------------------------------------

#[test]
fn typed_methods_share_one_table() {
    struct Names(Vec<&'static str>, Vec<&'static str>);
    impl MethodVisitor for Names {
        fn method<M: Method>(&mut self) {
            self.0.push(M::NAME);
        }
        fn notification<N: NotificationMethod>(&mut self) {
            self.1.push(N::NAME);
        }
    }
    let mut names = Names(Vec::new(), Vec::new());
    visit_methods(&mut names);
    assert_eq!(
        names.0,
        [
            "initialize",
            "$/ping",
            "events/subscribe",
            "events/unsubscribe",
            "debug/append",
            "project/list",
            "project/add",
            "project/get",
            "project/remove",
            "agent/list",
            "agent/refresh",
            "usage/get",
            "session/list",
            "session/create",
            "session/get",
            "session/prompt",
            "session/cancel",
            "session/setModel",
            "session/stop",
            "session/archive",
            "inbox/list",
            "inbox/get",
            "inbox/respond",
        ]
    );
    assert_eq!(names.1, ["$/cancel", "events/event"]);
}

#[test]
fn typed_params_and_results() {
    // Absent params decode as `{}`.
    let ping = Request::new(1, Ping::NAME, None);
    assert_eq!(ping.params_as::<Ping>().unwrap(), Empty {});

    // Wrong params give the error to send back.
    let bad = Request::new(2, DebugAppend::NAME, Some(json!({"stream": "index"})));
    let error = bad.params_as::<DebugAppend>().unwrap_err();
    assert_eq!(error.code, code::INVALID_PARAMS);
    assert!(
        error.message.starts_with("debug/append"),
        "{}",
        error.message
    );

    let response = Response::ok::<DebugAppend>(2.into(), &DebugAppendResult { seq: 9 });
    assert_eq!(response.result_as::<DebugAppend>().unwrap().seq, 9);

    let malformed = Response::success(3.into(), json!({"seq": "nine"}));
    assert_eq!(
        malformed.result_as::<DebugAppend>().unwrap_err().code,
        code::INTERNAL_ERROR
    );

    let failed = Response::error(
        Some(4.into()),
        ErrorObject::new(ErrorKind::NotFound, "gone"),
    );
    assert_eq!(
        failed.result_as::<Initialize>().unwrap_err().kind(),
        Some(ErrorKind::NotFound)
    );
}

// ---- Errors ----------------------------------------------------------------------------------

#[test]
fn error_kinds_map_to_codes() {
    let table = [
        (ErrorKind::NotFound, -32001),
        (ErrorKind::InvalidState, -32002),
        (ErrorKind::Conflict, -32003),
        (ErrorKind::Unauthorized, -32004),
        (ErrorKind::LimitReached, -32005),
        (ErrorKind::VersionMismatch, -32006),
        (ErrorKind::Cancelled, -32007),
        (ErrorKind::Unavailable, -32008),
    ];
    assert_eq!(table.len(), ErrorKind::KNOWN.len());
    for (kind, code) in table {
        assert_eq!(kind.code(), code);
        assert_eq!(ErrorKind::from_code(code), Some(kind.clone()));
        let error = ErrorObject::new(kind.clone(), "x");
        assert_eq!(error.code, code);
        assert_eq!(error.kind(), Some(kind));
    }
    for standard in [
        code::PARSE_ERROR,
        code::INVALID_REQUEST,
        code::METHOD_NOT_FOUND,
        code::INVALID_PARAMS,
        code::INTERNAL_ERROR,
    ] {
        assert_eq!(ErrorKind::from_code(standard), None);
        assert_eq!(ErrorObject::standard(standard, "x").kind(), None);
    }
}

#[test]
fn error_kind_comes_from_data() {
    // A kind this version doesn't know is kept.
    let error: ErrorObject = serde_json::from_value(json!({
        "code": -32099, "message": "new", "data": {"kind": "quotaFrozen", "extra": 1},
    }))
    .unwrap();
    assert_eq!(error.kind(), Some(ErrorKind::Other("quotaFrozen".into())));

    // Without data.kind, a Burrow code still names its kind.
    let error: ErrorObject =
        serde_json::from_value(json!({"code": -32003, "message": "taken"})).unwrap();
    assert_eq!(error.kind(), Some(ErrorKind::Conflict));
}

#[test]
fn version_mismatch_and_limit_errors_carry_details() {
    let error = ErrorObject::version_mismatch(3, vec![0]);
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        json!({
            "code": -32006,
            "message": "protocol version 3 is not supported; supported: [0]",
            "data": {"kind": "versionMismatch", "supported": [0]},
        })
    );

    let resets_at: Timestamp = AT.parse().unwrap();
    let error = ErrorObject::limit_reached("weekly limit", Some(resets_at));
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        json!({"code": -32005, "message": "weekly limit", "data": {"kind": "limitReached", "resetsAt": AT}})
    );
}

// ---- Timestamps ------------------------------------------------------------------------------

#[test]
fn timestamps_always_have_three_fractional_digits() {
    let cases = [
        (AT, AT),
        ("2026-11-06T14:03:22Z", "2026-11-06T14:03:22.000Z"),
        ("2026-11-06T14:03:22.4Z", "2026-11-06T14:03:22.400Z"),
        ("2026-11-06T14:03:22.418999999Z", "2026-11-06T14:03:22.418Z"),
        ("2026-11-06T16:03:22.418+02:00", "2026-11-06T14:03:22.418Z"),
        ("1969-12-31T23:59:59.9995Z", "1969-12-31T23:59:59.999Z"),
    ];
    for (input, wire) in cases {
        let timestamp: Timestamp = input.parse().unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_eq!(timestamp.to_string(), wire, "{input}");
        assert_eq!(serde_json::to_value(timestamp).unwrap(), json!(wire));
        let decoded: Timestamp = serde_json::from_value(json!(input)).unwrap();
        assert_eq!(decoded, timestamp);
    }
}

#[test]
fn timestamp_now_is_in_wire_format() {
    let now = Timestamp::now().to_string();
    let bytes = now.as_bytes();
    assert_eq!(bytes.len(), 24, "{now}");
    assert_eq!(
        (bytes[4], bytes[10], bytes[19], bytes[23]),
        (b'-', b'T', b'.', b'Z'),
        "{now}"
    );
    assert_eq!(now.parse::<Timestamp>().unwrap().to_string(), now);
}

#[test]
fn timestamps_convert_to_unix_millis() {
    let epoch = Timestamp::from_unix_millis(0).unwrap();
    assert_eq!(epoch.to_string(), "1970-01-01T00:00:00.000Z");
    let at: Timestamp = AT.parse().unwrap();
    assert_eq!(
        Timestamp::from_unix_millis(at.as_unix_millis()).unwrap(),
        at
    );
    assert_eq!(
        Timestamp::from_unix_millis(-1).unwrap().to_string(),
        "1969-12-31T23:59:59.999Z"
    );
    let min = Timestamp::from_unix_millis(-62_167_219_200_000).unwrap();
    assert_eq!(min.to_string(), "0000-01-01T00:00:00.000Z");
    assert!(Timestamp::from_unix_millis(-62_167_219_200_001).is_err());
    assert!(Timestamp::from_unix_millis(i64::MAX).is_err());
}

#[test]
fn invalid_timestamps_are_rejected() {
    for input in [
        "",
        "2026-11-06",
        "2026-11-06T14:03:22",
        "yesterday",
        "-000001-01-01T00:00:00Z",
    ] {
        assert!(input.parse::<Timestamp>().is_err(), "{input:?} parsed");
        assert!(serde_json::from_value::<Timestamp>(json!(input)).is_err());
    }
    assert!(serde_json::from_value::<Timestamp>(json!(1_700_000_000_000_i64)).is_err());
}

#[test]
fn stream_names() {
    assert_eq!(StreamName::index().as_str(), "index");
    assert!(StreamName::index().is_index());
    assert_eq!(StreamName::session("ses_1").to_string(), "session:ses_1");
    assert_eq!(StreamName::run("run_1").as_str(), "run:run_1");
    assert!(!StreamName::run("run_1").is_index());
}

#[test]
fn protocol_version_is_zero() {
    assert_eq!(PROTOCOL_VERSION, 0);
}
