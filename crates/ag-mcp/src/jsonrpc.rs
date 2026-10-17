//! JSON-RPC 2.0 as MCP uses it: parsing of one message per line, error codes, response construction.

use std::io::{self, BufRead};

use serde_json::{Map, Value, json};

/// Malformed JSON.
pub const PARSE_ERROR: i64 = -32700;
/// Not a valid request object.
pub const INVALID_REQUEST: i64 = -32600;
/// Unknown method.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// Invalid parameters (also: unknown tool, unknown prompt, resource not found, bad cursor).
pub const INVALID_PARAMS: i64 = -32602;
/// Internal error.
pub const INTERNAL_ERROR: i64 = -32603;
/// MCP 2026-07-28: the client's protocol version is not supported.
pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    /// Numeric code.
    pub code: i64,
    /// Short description.
    pub message: String,
    /// Optional structured details.
    pub data: Option<Value>,
}

impl RpcError {
    /// An error without data.
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    /// Adds structured data.
    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// `-32602`.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    /// The `error` member of a response.
    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("code".into(), json!(self.code));
        o.insert("message".into(), json!(self.message));
        if let Some(d) = &self.data {
            o.insert("data".into(), d.clone());
        }
        Value::Object(o)
    }
}

/// A parsed incoming message.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// A request: has an id, expects exactly one response.
    Request {
        /// String or integer id.
        id: Value,
        /// Method name.
        method: String,
        /// Parameters (an object, or `Null` when absent).
        params: Value,
    },
    /// A notification: no id, no response.
    Notification {
        /// Method name.
        method: String,
        /// Parameters.
        params: Value,
    },
    /// A response to a request this server sent. The server sends none, so these are ignored.
    Response,
}

/// Parses one line. On failure returns the error to send back together with the id to put in it
/// (`null` when the id could not be read).
pub fn parse_message(line: &[u8]) -> Result<Message, (Value, RpcError)> {
    let value: Value = serde_json::from_slice(line)
        .map_err(|e| (Value::Null, RpcError::new(PARSE_ERROR, format!("invalid JSON: {e}"))))?;
    let Value::Object(mut obj) = value else {
        let msg = if value.is_array() {
            "JSON-RPC batches are not supported by MCP 2025-06-18 and later; send one message per line"
        } else {
            "a JSON-RPC message must be an object"
        };
        return Err((Value::Null, RpcError::new(INVALID_REQUEST, msg)));
    };
    let id = obj.remove("id");
    let id_ok = |v: &Value| v.is_string() || v.is_i64() || v.is_u64();
    if let Some(v) = &id
        && !id_ok(v)
    {
        return Err((
            Value::Null,
            RpcError::new(INVALID_REQUEST, "`id` must be a string or an integer (null and fractions are not allowed)"),
        ));
    }
    let echo = id.clone().unwrap_or(Value::Null);
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err((echo, RpcError::new(INVALID_REQUEST, "`jsonrpc` must be \"2.0\"")));
    }
    match obj.remove("method") {
        Some(Value::String(method)) => {
            let params = obj.remove("params").unwrap_or(Value::Null);
            if !(params.is_null() || params.is_object()) {
                return Err((echo, RpcError::invalid_params("`params` must be an object")));
            }
            match id {
                Some(id) => Ok(Message::Request { id, method, params }),
                None => Ok(Message::Notification { method, params }),
            }
        }
        Some(_) => Err((echo, RpcError::new(INVALID_REQUEST, "`method` must be a string"))),
        None if obj.contains_key("result") || obj.contains_key("error") => Ok(Message::Response),
        None => Err((echo, RpcError::new(INVALID_REQUEST, "missing `method`"))),
    }
}

/// A success response.
pub fn response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// An error response.
pub fn error_response(id: &Value, err: &RpcError) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": err.to_value()})
}

/// A notification.
pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

/// What [`read_line_bounded`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// A complete line (without the terminator).
    Data(Vec<u8>),
    /// The line exceeded the limit; it was skipped up to its terminator.
    TooLong,
    /// End of input.
    Eof,
}

/// Reads one `\n`-terminated line of at most `max` bytes. A longer line is consumed (not buffered)
/// and reported as [`Line::TooLong`], so a hostile client cannot make the server allocate without bound.
pub fn read_line_bounded<R: BufRead>(r: &mut R, max: usize) -> io::Result<Line> {
    let mut buf: Vec<u8> = Vec::new();
    let mut too_long = false;
    let mut saw_any = false;
    loop {
        let chunk = match r.fill_buf() {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            // EOF: an unterminated final line still counts as a line.
            return Ok(if too_long {
                Line::TooLong
            } else if saw_any {
                Line::Data(buf)
            } else {
                Line::Eof
            });
        }
        saw_any = true;
        let (take, found) = match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => (i, true),
            None => (chunk.len(), false),
        };
        if !too_long {
            if buf.len() + take > max {
                too_long = true;
                buf = Vec::new();
            } else {
                buf.extend_from_slice(&chunk[..take]);
            }
        }
        r.consume(take + usize::from(found));
        if found {
            if too_long {
                return Ok(Line::TooLong);
            }
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return Ok(Line::Data(buf));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requests_notifications_and_responses() {
        let m = parse_message(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).unwrap();
        assert_eq!(m, Message::Request { id: json!(1), method: "ping".into(), params: Value::Null });
        let m = parse_message(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        assert!(matches!(m, Message::Notification { .. }));
        let m = parse_message(br#"{"jsonrpc":"2.0","id":"x","result":{}}"#).unwrap();
        assert_eq!(m, Message::Response);
    }

    #[test]
    fn rejects_malformed_messages_with_the_right_codes() {
        let (id, e) = parse_message(b"{not json").unwrap_err();
        assert_eq!((id, e.code), (Value::Null, PARSE_ERROR));
        let (_, e) = parse_message(b"[1,2]").unwrap_err();
        assert_eq!(e.code, INVALID_REQUEST);
        assert!(e.message.contains("batch"));
        let (id, e) = parse_message(br#"{"jsonrpc":"1.0","id":4,"method":"x"}"#).unwrap_err();
        assert_eq!((id, e.code), (json!(4), INVALID_REQUEST));
        // MCP forbids null and fractional ids.
        for bad in [r#"{"jsonrpc":"2.0","id":null,"method":"x"}"#, r#"{"jsonrpc":"2.0","id":1.5,"method":"x"}"#] {
            let (id, e) = parse_message(bad.as_bytes()).unwrap_err();
            assert_eq!((id, e.code), (Value::Null, INVALID_REQUEST), "{bad}");
        }
        let (_, e) = parse_message(br#"{"jsonrpc":"2.0","id":1,"method":"x","params":[1]}"#).unwrap_err();
        assert_eq!(e.code, INVALID_PARAMS);
        let (_, e) = parse_message(br#"{"jsonrpc":"2.0","id":1}"#).unwrap_err();
        assert_eq!(e.code, INVALID_REQUEST);
    }

    #[test]
    fn bounded_line_reader() {
        let mut r = io::Cursor::new(b"abc\r\nde\nfghijkl\nlast".to_vec());
        assert_eq!(read_line_bounded(&mut r, 5).unwrap(), Line::Data(b"abc".to_vec()));
        assert_eq!(read_line_bounded(&mut r, 5).unwrap(), Line::Data(b"de".to_vec()));
        // longer than the limit: skipped, and the next line is intact
        assert_eq!(read_line_bounded(&mut r, 5).unwrap(), Line::TooLong);
        assert_eq!(read_line_bounded(&mut r, 5).unwrap(), Line::Data(b"last".to_vec()));
        assert_eq!(read_line_bounded(&mut r, 5).unwrap(), Line::Eof);
    }

    #[test]
    fn a_line_exactly_at_the_limit_is_accepted() {
        let mut r = io::Cursor::new(b"12345\n123456\n".to_vec());
        assert_eq!(read_line_bounded(&mut r, 5).unwrap(), Line::Data(b"12345".to_vec()));
        assert_eq!(read_line_bounded(&mut r, 5).unwrap(), Line::TooLong);
    }
}
