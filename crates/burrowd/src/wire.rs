//! How events are stored, and how stored records become `events/event` notifications (D-014).
//!
//! A store record has a `type` column and a JSON payload. `burrowd` writes:
//!
//! - **payload:** the complete wire `event` object, `type` included, exactly as it is sent to
//!   clients. For example `{"type":"entityCreated","entity":"session","id":"ses_…"}`.
//! - **type:** the same `type`, repeated so SQL can filter by type without parsing JSON.
//!
//! Sending an event then needs no parsing: the stored payload is spliced into the notification
//! as raw JSON. Fields this build doesn't know (written by a newer `burrowd` on the same
//! database) survive untouched. The record's `at_ms` becomes the wire `at`, a [`Timestamp`].

use burrow_protocol::methods::DebugAppend;
use burrow_protocol::notifications::EventsEvent;
use burrow_protocol::{Event, JsonRpcVersion, Method, NotificationMethod, StreamName, Timestamp};
use burrow_store::NewEvent;
use serde::Serialize;
use serde_json::value::RawValue;

/// The store record for `event`.
pub fn record(event: &Event) -> NewEvent {
    let payload = serde_json::value::to_raw_value(event)
        .expect("an Event always serializes to a JSON object");
    NewEvent::new(event.type_name(), payload)
}

/// Why an event can't be stored. Reported to the client as invalid params.
pub fn check_appendable(stream: &StreamName, event: &Event) -> Result<(), String> {
    if stream.as_str().is_empty() {
        return Err(format!("{}: stream name is empty", DebugAppend::NAME));
    }
    if event.type_name().is_empty() {
        return Err(format!("{}: event type is empty", DebugAppend::NAME));
    }
    Ok(())
}

/// The `events/event` notification for one stored event, as one line of JSON.
pub fn event_line(stream: &StreamName, seq: u64, at_ms: i64, payload: &RawValue) -> String {
    #[derive(Serialize)]
    struct Notification<'a> {
        jsonrpc: JsonRpcVersion,
        method: &'static str,
        params: Params<'a>,
    }

    /// Serializes exactly like `burrow_protocol::StreamEvent`, with the event as raw JSON.
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Params<'a> {
        stream: &'a StreamName,
        seq: u64,
        at: Timestamp,
        event: &'a RawValue,
    }

    let notification = Notification {
        jsonrpc: JsonRpcVersion,
        method: EventsEvent::NAME,
        params: Params {
            stream,
            seq,
            at: timestamp(at_ms),
            event: payload,
        },
    };
    serde_json::to_string(&notification).expect("an event notification always serializes")
}

/// `at_ms` as a wire timestamp. The store writes wall-clock milliseconds, which are always in
/// range; a corrupted value shows as the Unix epoch rather than failing the stream.
fn timestamp(at_ms: i64) -> Timestamp {
    Timestamp::from_unix_millis(at_ms)
        .or_else(|_| Timestamp::from_unix_millis(0))
        .expect("the Unix epoch is a valid timestamp")
}

#[cfg(test)]
mod tests {
    use burrow_protocol::{
        EntityCreated, EntityKind, Message, StreamEvent, UnknownEvent, notifications::EventsEvent,
    };
    use serde_json::json;

    use super::*;

    fn decode(line: &str) -> StreamEvent {
        let Message::Notification(notification) = Message::parse(line).unwrap() else {
            panic!("not a notification: {line}");
        };
        assert_eq!(notification.method, EventsEvent::NAME);
        notification.params_as::<EventsEvent>().unwrap()
    }

    #[test]
    fn a_stored_event_becomes_the_same_stream_event() {
        let event = Event::EntityCreated(EntityCreated {
            entity: EntityKind::Session,
            id: "ses_1".into(),
            title: Some("Fix login".into()),
            project_id: None,
            state: Some("idle".into()),
            agent: None,
            mode: None,
        });
        let record = record(&event);
        assert_eq!(record.event_type, "entityCreated");

        let stream = StreamName::index();
        let line = event_line(&stream, 7, 1_762_437_802_418, &record.payload);
        assert_eq!(
            decode(&line),
            StreamEvent {
                stream,
                seq: 7,
                at: "2025-11-06T14:03:22.418Z".parse().unwrap(),
                event,
            }
        );
    }

    #[test]
    fn unknown_events_and_fields_pass_through_unchanged() {
        let raw = json!({"type": "fromTheFuture", "nested": {"a": [1, 2]}, "x": null});
        let event: Event = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(event, Event::Unknown(_)));
        let record = record(&event);
        assert_eq!(record.event_type, "fromTheFuture");

        // A known type with a field this build doesn't know, as a newer burrowd would store it.
        let newer = RawValue::from_string(
            r#"{"type":"entityRemoved","entity":"run","id":"run_1","reason":"archived"}"#.into(),
        )
        .unwrap();
        let line = event_line(&StreamName::run("run_1"), 1, 0, &newer);
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["params"]["event"]["reason"], "archived");
        assert_eq!(value["params"]["at"], "1970-01-01T00:00:00.000Z");

        let line = event_line(&StreamName::index(), 2, 0, &record.payload);
        assert_eq!(
            decode(&line).event,
            Event::Unknown(UnknownEvent::from_object(raw.as_object().unwrap().clone()).unwrap())
        );
    }

    #[test]
    fn empty_streams_and_types_are_refused() {
        let event: Event = serde_json::from_value(json!({"type": ""})).unwrap();
        assert!(check_appendable(&StreamName::index(), &event).is_err());
        let event: Event = serde_json::from_value(json!({"type": "x"})).unwrap();
        assert!(check_appendable(&StreamName::new(""), &event).is_err());
        assert!(check_appendable(&StreamName::index(), &event).is_ok());
    }
}
