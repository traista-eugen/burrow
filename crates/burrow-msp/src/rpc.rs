//! The JSON-RPC 2.0 envelope as `muse serve` speaks it.
//!
//! Two things go beyond plain JSON-RPC:
//! - Notifications carry a top-level `emittedAtMs` next to `params`. It's read and kept, but
//!   nothing depends on it.
//! - Errors put a machine-readable `kind` in `error.data`, plus `retryable` and kind-specific
//!   members such as `reason` (see [`RpcError`]).

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::ErrorKind;

/// A JSON-RPC request id. This crate numbers its own requests; Muse numbers its requests to the
/// client (`approval/request`, `userInput/request`) independently.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// A numeric id.
    Number(i64),
    /// A string id.
    String(String),
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(id) => write!(f, "{id}"),
            Self::String(id) => write!(f, "{id:?}"),
        }
    }
}

/// A JSON-RPC error object from Muse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    /// The numeric code. The schema's `errors` table maps codes to kinds.
    pub code: i64,
    /// Human-readable text. Display only; branch on [`RpcError::kind`].
    pub message: String,
    /// Structured detail: `kind`, `retryable`, and members that depend on the kind, such as
    /// `reason` for `commandRejected` or `sessionId` for `sessionNotFound`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// The error kind: `data.kind` when present, else the kind the code maps to.
    pub fn kind(&self) -> ErrorKind {
        match self.data_str("kind") {
            Some(kind) => ErrorKind::from(kind),
            None => ErrorKind::from_code(self.code),
        }
    }

    /// Whether Muse says retrying may succeed: `data.retryable`, else the error table's default.
    pub fn retryable(&self) -> bool {
        self.data
            .as_ref()
            .and_then(|data| data.get("retryable"))
            .and_then(Value::as_bool)
            .unwrap_or_else(|| self.kind().retryable_by_default())
    }

    /// `data.reason`: why a command was rejected, as a snake_case word such as `missing_run`.
    pub fn reason(&self) -> Option<&str> {
        self.data_str("reason")
    }

    fn data_str(&self, key: &str) -> Option<&str> {
        self.data.as_ref()?.get(key)?.as_str()
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}): {}", self.kind(), self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

/// One message read from Muse.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Incoming {
    /// The answer to one of our requests. `id` is `None` only when Muse couldn't read a request.
    Response {
        id: Option<RequestId>,
        outcome: Result<Value, RpcError>,
    },
    /// A request Muse sends us, which must be answered.
    Request {
        id: RequestId,
        method: String,
        params: Option<Value>,
    },
    /// A notification.
    Notification {
        method: String,
        params: Option<Value>,
    },
}

/// Why a line isn't a JSON-RPC message.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ParseError {
    #[error("not JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not a JSON-RPC message: {0}")]
    Shape(&'static str),
}

impl Incoming {
    pub fn parse(line: &str) -> Result<Self, ParseError> {
        let mut object: Map<String, Value> = serde_json::from_str(line)?;
        let id = match object.remove("id") {
            None | Some(Value::Null) => None,
            Some(id) => Some(serde_json::from_value::<RequestId>(id)?),
        };
        let params = object.remove("params");
        if let Some(method) = object.remove("method") {
            let Value::String(method) = method else {
                return Err(ParseError::Shape("`method` isn't a string"));
            };
            return Ok(match id {
                Some(id) => Self::Request { id, method, params },
                None => Self::Notification { method, params },
            });
        }
        if let Some(error) = object.remove("error") {
            let error: RpcError = serde_json::from_value(error)?;
            return Ok(Self::Response {
                id,
                outcome: Err(error),
            });
        }
        match object.remove("result") {
            Some(result) if id.is_some() => Ok(Self::Response {
                id,
                outcome: Ok(result),
            }),
            Some(_) => Err(ParseError::Shape("a result without an id")),
            None => Err(ParseError::Shape("neither method, result nor error")),
        }
    }
}

/// Encodes a request. `params: None` leaves the member out, for methods that take none.
pub(crate) fn request_line(id: &RequestId, method: &str, params: Option<&Value>) -> String {
    let mut object = Map::new();
    object.insert("jsonrpc".into(), "2.0".into());
    object.insert("id".into(), serde_json::to_value(id).expect("ids encode"));
    object.insert("method".into(), method.into());
    if let Some(params) = params {
        object.insert("params".into(), params.clone());
    }
    Value::Object(object).to_string()
}

/// Encodes a notification.
pub(crate) fn notification_line(method: &str, params: Option<&Value>) -> String {
    let mut object = Map::new();
    object.insert("jsonrpc".into(), "2.0".into());
    object.insert("method".into(), method.into());
    if let Some(params) = params {
        object.insert("params".into(), params.clone());
    }
    Value::Object(object).to_string()
}

/// Encodes the answer to one of Muse's requests.
pub(crate) fn response_line(id: &RequestId, outcome: Result<Value, RpcError>) -> String {
    let mut object = Map::new();
    object.insert("jsonrpc".into(), "2.0".into());
    object.insert("id".into(), serde_json::to_value(id).expect("ids encode"));
    match outcome {
        Ok(result) => object.insert("result".into(), result),
        Err(error) => object.insert(
            "error".into(),
            serde_json::to_value(error).expect("errors encode"),
        ),
    };
    Value::Object(object).to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_the_four_shapes() {
        let response = Incoming::parse(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#).unwrap();
        assert_eq!(
            response,
            Incoming::Response {
                id: Some(RequestId::Number(3)),
                outcome: Ok(json!({})),
            }
        );
        let error = Incoming::parse(
            r#"{"jsonrpc":"2.0","id":8,"error":{"code":-32030,"message":"rejected: missing_run",
                "data":{"kind":"commandRejected","retryable":false,"reason":"missing_run"}}}"#,
        )
        .unwrap();
        let Incoming::Response {
            outcome: Err(error),
            ..
        } = error
        else {
            panic!("expected an error response");
        };
        assert_eq!(error.kind(), ErrorKind::CommandRejected);
        assert_eq!(error.reason(), Some("missing_run"));
        assert!(!error.retryable());

        let request = Incoming::parse(
            r#"{"jsonrpc":"2.0","id":1,"method":"approval/request","params":{"a":1}}"#,
        )
        .unwrap();
        assert!(
            matches!(request, Incoming::Request { method, .. } if method == "approval/request")
        );

        let notification = Incoming::parse(
            r#"{"jsonrpc":"2.0","method":"turn/started","params":{},"emittedAtMs":1794210188397}"#,
        )
        .unwrap();
        assert!(
            matches!(notification, Incoming::Notification { method, .. } if method == "turn/started")
        );
    }

    #[test]
    fn a_result_of_null_is_still_a_result() {
        let response = Incoming::parse(r#"{"jsonrpc":"2.0","id":"a","result":null}"#).unwrap();
        assert_eq!(
            response,
            Incoming::Response {
                id: Some(RequestId::String("a".into())),
                outcome: Ok(Value::Null),
            }
        );
    }

    #[test]
    fn rejects_lines_that_are_not_messages() {
        assert!(Incoming::parse("not json").is_err());
        assert!(Incoming::parse(r#"{"jsonrpc":"2.0"}"#).is_err());
        assert!(Incoming::parse(r#"{"jsonrpc":"2.0","result":{}}"#).is_err());
    }

    #[test]
    fn an_error_without_data_falls_back_to_the_code_table() {
        let error = RpcError {
            code: -32001,
            message: "busy".into(),
            data: None,
        };
        assert_eq!(error.kind(), ErrorKind::Overloaded);
        assert!(error.retryable());
    }

    #[test]
    fn encodes_requests_with_and_without_params() {
        let with: Value = serde_json::from_str(&request_line(
            &RequestId::Number(1),
            "turn/start",
            Some(&json!({"x": 1})),
        ))
        .unwrap();
        assert_eq!(
            with,
            json!({"jsonrpc":"2.0","id":1,"method":"turn/start","params":{"x":1}})
        );
        let without: Value =
            serde_json::from_str(&request_line(&RequestId::Number(2), "usage/read", None)).unwrap();
        assert_eq!(
            without,
            json!({"jsonrpc":"2.0","id":2,"method":"usage/read"})
        );
        let receipt: Value =
            serde_json::from_str(&response_line(&RequestId::Number(1), Ok(json!({})))).unwrap();
        assert_eq!(receipt, json!({"jsonrpc":"2.0","id":1,"result":{}}));
    }
}
