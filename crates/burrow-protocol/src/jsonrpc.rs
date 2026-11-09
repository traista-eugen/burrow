//! The JSON-RPC 2.0 envelope: requests, responses and notifications, one per line.
//!
//! Batches aren't used. [`Message::parse`] classifies one incoming line and reports what to answer
//! when the line is malformed.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::error::ErrorObject;
use crate::method::{Method, NotificationMethod};

/// The `"jsonrpc": "2.0"` member. Deserializing anything else fails.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct JsonRpcVersion;

impl JsonRpcVersion {
    /// The only version used: `"2.0"`.
    pub const VALUE: &'static str = "2.0";
}

impl Serialize for JsonRpcVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(Self::VALUE)
    }
}

impl<'de> Deserialize<'de> for JsonRpcVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let version = <Cow<'de, str>>::deserialize(deserializer)?;
        if version == Self::VALUE {
            Ok(Self)
        } else {
            Err(de::Error::custom(format!(
                "unsupported JSON-RPC version {version:?}, expected \"2.0\""
            )))
        }
    }
}

impl JsonSchema for JsonRpcVersion {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "JsonRpcVersion".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "const": "2.0" })
    }
}

/// A request id: a number or a string, chosen by the sender and unique among its own pending
/// requests. Burrow never uses `null` ids in requests.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RequestId {
    /// An integer id. Burrow's own clients count up from 0.
    Number(i64),
    /// A string id.
    String(String),
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::String(s) => write!(f, "{s:?}"),
        }
    }
}

impl From<i64> for RequestId {
    fn from(n: i64) -> Self {
        Self::Number(n)
    }
}

impl From<i32> for RequestId {
    /// So integer literals work: `Request::new(1, …)`.
    fn from(n: i32) -> Self {
        Self::Number(n.into())
    }
}

impl From<u32> for RequestId {
    fn from(n: u32) -> Self {
        Self::Number(n.into())
    }
}

impl From<String> for RequestId {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

impl From<&str> for RequestId {
    fn from(s: &str) -> Self {
        Self::String(s.to_owned())
    }
}

impl Serialize for RequestId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Number(n) => serializer.serialize_i64(*n),
            Self::String(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct IdVisitor;

        impl Visitor<'_> for IdVisitor {
            type Value = RequestId;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an integer or a string")
            }

            fn visit_i64<E: de::Error>(self, n: i64) -> Result<RequestId, E> {
                Ok(RequestId::Number(n))
            }

            fn visit_u64<E: de::Error>(self, n: u64) -> Result<RequestId, E> {
                i64::try_from(n)
                    .map(RequestId::Number)
                    .map_err(|_| E::custom(format!("request id {n} is too large")))
            }

            fn visit_str<E: de::Error>(self, s: &str) -> Result<RequestId, E> {
                Ok(RequestId::String(s.to_owned()))
            }

            fn visit_string<E: de::Error>(self, s: String) -> Result<RequestId, E> {
                Ok(RequestId::String(s))
            }
        }

        deserializer.deserialize_any(IdVisitor)
    }
}

impl JsonSchema for RequestId {
    fn schema_name() -> Cow<'static, str> {
        "RequestId".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::RequestId").into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A request id: an integer or a string, unique among the sender's pending requests.",
            "type": ["integer", "string"],
        })
    }
}

/// A call that expects a response with the same id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// Always `"2.0"`.
    pub jsonrpc: JsonRpcVersion,
    /// Chosen by the sender; the response carries it back.
    pub id: RequestId,
    /// `area/verb`, for example `events/subscribe`.
    pub method: String,
    /// The method's params. Burrow's methods take an object; absent means `{}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A one-way message. It gets no response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("not" = { "required": ["id"] }))]
pub struct Notification {
    /// Always `"2.0"`.
    pub jsonrpc: JsonRpcVersion,
    /// `area/verb`, for example `events/event`.
    pub method: String,
    /// The notification's params. Burrow's notifications take an object; absent means `{}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// The answer to a request: a result or an error, never both.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("required" = ["jsonrpc", "id"]))]
pub struct Response {
    /// Always `"2.0"`.
    pub jsonrpc: JsonRpcVersion,
    /// The id of the request being answered. `null` only on an error about a line whose id
    /// couldn't be read.
    pub id: Option<RequestId>,
    /// `result` or `error`.
    #[serde(flatten)]
    pub outcome: ResponseOutcome,
}

/// Whether a request succeeded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ResponseOutcome {
    /// The method's result.
    Result(Value),
    /// Why the request failed.
    Error(ErrorObject),
}

impl Request {
    /// A request with untyped params.
    pub fn new(id: impl Into<RequestId>, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JsonRpcVersion,
            id: id.into(),
            method: method.into(),
            params,
        }
    }

    /// A request for method `M`.
    pub fn typed<M: Method>(id: impl Into<RequestId>, params: &M::Params) -> Self {
        Self::new(id, M::NAME, Some(to_value(params)))
    }

    /// Decodes the params as method `M`'s. Absent params decode as `{}`.
    ///
    /// On failure, returns the `invalidParams` error to send back.
    pub fn params_as<M: Method>(&self) -> Result<M::Params, ErrorObject> {
        debug_assert_eq!(self.method, M::NAME, "decoding params of another method");
        decode_params(self.params.as_ref())
            .map_err(|error| ErrorObject::invalid_params(format!("{}: {error}", M::NAME)))
    }
}

impl Notification {
    /// A notification with untyped params.
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JsonRpcVersion,
            method: method.into(),
            params,
        }
    }

    /// A notification `N`.
    pub fn typed<N: NotificationMethod>(params: &N::Params) -> Self {
        Self::new(N::NAME, Some(to_value(params)))
    }

    /// Decodes the params as notification `N`'s. Absent params decode as `{}`.
    pub fn params_as<N: NotificationMethod>(&self) -> Result<N::Params, serde_json::Error> {
        debug_assert_eq!(
            self.method,
            N::NAME,
            "decoding params of another notification"
        );
        decode_params(self.params.as_ref())
    }
}

impl Response {
    /// A successful response with an untyped result.
    pub fn success(id: RequestId, result: Value) -> Self {
        Self {
            jsonrpc: JsonRpcVersion,
            id: Some(id),
            outcome: ResponseOutcome::Result(result),
        }
    }

    /// A successful response to method `M`.
    pub fn ok<M: Method>(id: RequestId, result: &M::Result) -> Self {
        Self::success(id, to_value(result))
    }

    /// An error response. Pass `None` only when the request's id couldn't be read.
    pub fn error(id: Option<RequestId>, error: ErrorObject) -> Self {
        Self {
            jsonrpc: JsonRpcVersion,
            id,
            outcome: ResponseOutcome::Error(error),
        }
    }

    /// The result, or the error the other side sent.
    pub fn into_result(self) -> Result<Value, ErrorObject> {
        match self.outcome {
            ResponseOutcome::Result(value) => Ok(value),
            ResponseOutcome::Error(error) => Err(error),
        }
    }

    /// Decodes the result as method `M`'s.
    ///
    /// Returns the error the other side sent. A result that doesn't match `M` is reported as an
    /// internal error (`-32603`) whose message says so.
    pub fn result_as<M: Method>(&self) -> Result<M::Result, ErrorObject> {
        match &self.outcome {
            ResponseOutcome::Result(value) => M::Result::deserialize(value).map_err(|error| {
                ErrorObject::internal(format!("malformed result for {}: {error}", M::NAME))
            }),
            ResponseOutcome::Error(error) => Err(error.clone()),
        }
    }

    /// One line of JSON, without the trailing newline.
    pub fn to_line(&self) -> String {
        to_line(self)
    }
}

/// Any one JSON-RPC message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// Has `method` and `id`.
    Request(Request),
    /// Has `result` or `error`, and no `method`.
    Response(Response),
    /// Has `method` and no `id`.
    Notification(Notification),
}

/// Why an incoming line isn't a valid [`Message`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageError {
    /// The line isn't JSON. Answer `parseError` with a `null` id.
    Parse(String),
    /// The line is JSON but not a valid message. Answer `invalidRequest`, with the id if it could
    /// be read.
    Invalid {
        /// The message's id, if it had a readable one.
        id: Option<RequestId>,
        /// What's wrong.
        reason: String,
    },
}

impl MessageError {
    /// The id to answer with, if one could be read.
    pub fn id(&self) -> Option<&RequestId> {
        match self {
            Self::Parse(_) => None,
            Self::Invalid { id, .. } => id.as_ref(),
        }
    }

    /// The error to send back.
    pub fn to_error(&self) -> ErrorObject {
        match self {
            Self::Parse(reason) => ErrorObject::parse_error(reason.clone()),
            Self::Invalid { reason, .. } => ErrorObject::invalid_request(reason.clone()),
        }
    }

    /// The response to send back.
    pub fn to_response(&self) -> Response {
        Response::error(self.id().cloned(), self.to_error())
    }
}

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(reason) => write!(f, "not JSON: {reason}"),
            Self::Invalid { reason, .. } => write!(f, "invalid JSON-RPC message: {reason}"),
        }
    }
}

impl std::error::Error for MessageError {}

impl Message {
    /// Classifies one line (or any one JSON text) as a request, response or notification.
    ///
    /// - `method` and a non-null `id`: a request.
    /// - `method` and no `id`: a notification.
    /// - no `method`, and exactly one of `result` or `error`: a response.
    ///
    /// Anything else, including a batch (a JSON array), is [`MessageError::Invalid`]. Unknown
    /// members are ignored.
    pub fn parse(line: &str) -> Result<Self, MessageError> {
        let value: Value =
            serde_json::from_str(line).map_err(|error| MessageError::Parse(error.to_string()))?;
        Self::from_value(value)
    }

    /// Like [`Message::parse`], for JSON that has already been parsed.
    pub fn from_value(value: Value) -> Result<Self, MessageError> {
        let object = match value {
            Value::Object(object) => object,
            Value::Array(_) => return Err(invalid(None, "batches aren't supported")),
            _ => return Err(invalid(None, "a message must be a JSON object")),
        };

        // Read the id first, so every later error can carry it.
        let id = match object.get("id") {
            None | Some(Value::Null) => None,
            Some(raw) => Some(
                RequestId::deserialize(raw)
                    .map_err(|_| invalid(None, "id must be an integer or a string"))?,
            ),
        };

        match object.get("jsonrpc") {
            Some(Value::String(version)) if version == JsonRpcVersion::VALUE => {}
            Some(_) => return Err(invalid(id, "jsonrpc must be \"2.0\"")),
            None => return Err(invalid(id, "missing \"jsonrpc\": \"2.0\"")),
        }

        let has_result = object.contains_key("result");
        let has_error = object.contains_key("error");

        match object.get("method") {
            Some(Value::String(_)) => {
                if has_result || has_error {
                    return Err(invalid(
                        id,
                        "a message can't have both method and result/error",
                    ));
                }
                if let Some(params) = object.get("params")
                    && !matches!(params, Value::Object(_) | Value::Array(_) | Value::Null)
                {
                    return Err(invalid(id, "params must be an object or an array"));
                }
                match object.get("id") {
                    None => decode(object, None).map(Message::Notification),
                    Some(Value::Null) => Err(invalid(None, "a request id can't be null")),
                    Some(_) => decode(object, id).map(Message::Request),
                }
            }
            Some(_) => Err(invalid(id, "method must be a string")),
            None => match (has_result, has_error) {
                (true, true) => Err(invalid(id, "a response can't have both result and error")),
                (false, false) => Err(invalid(id, "a message needs method, result or error")),
                _ if !object.contains_key("id") => Err(invalid(None, "a response needs an id")),
                (true, false) if id.is_none() => {
                    Err(invalid(None, "a successful response needs a non-null id"))
                }
                _ => decode(object, id).map(Message::Response),
            },
        }
    }

    /// The method name, for requests and notifications.
    pub fn method(&self) -> Option<&str> {
        match self {
            Self::Request(request) => Some(&request.method),
            Self::Notification(notification) => Some(&notification.method),
            Self::Response(_) => None,
        }
    }

    /// One line of JSON, without the trailing newline.
    pub fn to_line(&self) -> String {
        to_line(self)
    }
}

impl Serialize for Message {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Request(request) => request.serialize(serializer),
            Self::Response(response) => response.serialize(serializer),
            Self::Notification(notification) => notification.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_value(Value::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

impl JsonSchema for Message {
    fn schema_name() -> Cow<'static, str> {
        "Message".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Message").into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "Any one JSON-RPC 2.0 message: one line on the named pipe or stdin/stdout, or one WebSocket text frame.",
            "oneOf": [
                generator.subschema_for::<Request>(),
                generator.subschema_for::<Response>(),
                generator.subschema_for::<Notification>(),
            ],
        })
    }
}

macro_rules! impl_to_line {
    ($($ty:ty),*) => {$(
        impl $ty {
            /// One line of JSON, without the trailing newline.
            pub fn to_line(&self) -> String {
                to_line(self)
            }
        }
    )*};
}

impl_to_line!(Request, Notification);

fn invalid(id: Option<RequestId>, reason: &str) -> MessageError {
    MessageError::Invalid {
        id,
        reason: reason.to_owned(),
    }
}

fn decode<T: for<'de> Deserialize<'de>>(
    object: Map<String, Value>,
    id: Option<RequestId>,
) -> Result<T, MessageError> {
    serde_json::from_value(Value::Object(object)).map_err(|error| MessageError::Invalid {
        id,
        reason: error.to_string(),
    })
}

/// Absent params decode as `{}`, so methods whose params are all optional accept no params.
fn decode_params<T: for<'de> Deserialize<'de>>(
    params: Option<&Value>,
) -> Result<T, serde_json::Error> {
    match params {
        Some(value) if !value.is_null() => T::deserialize(value),
        _ => T::deserialize(&Value::Object(Map::new())),
    }
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("protocol types always serialize to JSON")
}

fn to_line<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("protocol types always serialize to JSON")
}
