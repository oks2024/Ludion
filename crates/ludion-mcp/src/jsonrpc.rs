//! Line-delimited JSON-RPC 2.0 framing — the MCP stdio transport.
//!
//! Every message is one JSON object on its own line (newline-terminated). This module owns the
//! wire-shape concerns only: parsing an incoming line into an [`Incoming`], the standard error
//! codes, and building `result`/`error` response values. The MCP method semantics live in
//! [`crate::server`].
//!
//! A message with an `id` is a *request* (it must be answered); a message with no `id` is a
//! *notification* (it must never be answered, even on error). [`Incoming::id`] carries that
//! distinction.

use serde_json::{Value, json};

/// The standard JSON-RPC 2.0 error codes (spec §5.1) this adapter uses.
pub(crate) mod error_code {
    /// Invalid JSON was received (the line did not parse).
    pub(crate) const PARSE_ERROR: i64 = -32700;
    /// The JSON was not a valid request object (not an object, or missing `method`).
    pub(crate) const INVALID_REQUEST: i64 = -32600;
    /// The method does not exist.
    pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
    /// Invalid method parameters (e.g. `tools/call` naming an unknown tool).
    pub(crate) const INVALID_PARAMS: i64 = -32602;
}

/// One parsed inbound message.
#[derive(Debug)]
pub(crate) struct Incoming {
    /// The correlation id, or `None` for a notification (which must never be answered).
    pub(crate) id: Option<Value>,
    /// The JSON-RPC method name (e.g. `"tools/call"`).
    pub(crate) method: String,
    /// The `params` object, defaulted to `null` when absent.
    pub(crate) params: Value,
}

/// A framing-level failure that still needs a response written (only when `id` is present).
#[derive(Debug)]
pub(crate) struct RpcError {
    /// The id to echo, or `None` if the offending message was a notification / unparseable.
    pub(crate) id: Option<Value>,
    /// A JSON-RPC error code from [`error_code`].
    pub(crate) code: i64,
    /// The human-readable error message.
    pub(crate) message: String,
}

/// Parses one line as a JSON-RPC request/notification.
///
/// # Errors
/// - [`error_code::PARSE_ERROR`] (id `None`) if the line is not valid JSON.
/// - [`error_code::INVALID_REQUEST`] if the JSON is not an object or has no string `method`. The id
///   is preserved when present so a malformed *request* can still be answered.
pub(crate) fn parse(line: &str) -> Result<Incoming, RpcError> {
    let value: Value = serde_json::from_str(line).map_err(|e| RpcError {
        id: None,
        code: error_code::PARSE_ERROR,
        message: format!("message is not valid JSON: {e}"),
    })?;

    let obj = value.as_object().ok_or_else(|| RpcError {
        id: None,
        code: error_code::INVALID_REQUEST,
        message: "a JSON-RPC message must be an object".to_string(),
    })?;

    // `id` may be a number, a string, or absent (a notification). `null` is treated as absent.
    let id = match obj.get("id") {
        Some(Value::Null) | None => None,
        Some(other) => Some(other.clone()),
    };

    let method = obj
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError {
            id: id.clone(),
            code: error_code::INVALID_REQUEST,
            message: "a JSON-RPC message must carry a string `method`".to_string(),
        })?
        .to_string();

    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    Ok(Incoming { id, method, params })
}

/// Builds a JSON-RPC success response echoing `id` and carrying `result`.
pub(crate) fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Builds a JSON-RPC error response. A `None` id serializes as JSON `null`, per the spec's rule for
/// errors where the id could not be determined.
pub(crate) fn failure(id: Option<Value>, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": { "code": code, "message": message.into() },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_request_with_params() {
        let inc = parse(r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"x"}}"#)
            .expect("valid request");
        assert_eq!(inc.id, Some(json!(7)));
        assert_eq!(inc.method, "tools/call");
        assert_eq!(inc.params["name"], json!("x"));
    }

    #[test]
    fn a_notification_has_no_id() {
        let inc = parse(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .expect("valid notification");
        assert!(inc.id.is_none());
        assert_eq!(inc.params, Value::Null);
    }

    #[test]
    fn null_id_is_treated_as_a_notification() {
        let inc = parse(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#).expect("valid");
        assert!(inc.id.is_none());
    }

    #[test]
    fn invalid_json_is_a_parse_error_with_no_id() {
        let err = parse("{not json").unwrap_err();
        assert_eq!(err.code, error_code::PARSE_ERROR);
        assert!(err.id.is_none());
    }

    #[test]
    fn a_request_missing_method_keeps_its_id() {
        let err = parse(r#"{"jsonrpc":"2.0","id":3}"#).unwrap_err();
        assert_eq!(err.code, error_code::INVALID_REQUEST);
        assert_eq!(err.id, Some(json!(3)));
    }

    #[test]
    fn success_and_failure_have_the_2_0_envelope() {
        let ok = success(json!(1), json!({ "a": 1 }));
        assert_eq!(ok["jsonrpc"], json!("2.0"));
        assert_eq!(ok["id"], json!(1));
        assert_eq!(ok["result"]["a"], json!(1));

        let err = failure(None, error_code::METHOD_NOT_FOUND, "nope");
        assert_eq!(err["id"], Value::Null);
        assert_eq!(err["error"]["code"], json!(error_code::METHOD_NOT_FOUND));
    }
}
