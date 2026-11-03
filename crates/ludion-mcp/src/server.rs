//! The MCP method loop: `initialize` / `tools/list` / `tools/call` / `ping`, over stdio.
//!
//! [`McpServer::run`] reads one JSON-RPC message per line from `reader`, dispatches it, and writes
//! at most one response line to `writer`. Notifications (no `id`) are never answered. All logging
//! goes to stderr via [`log`]; **stdout carries protocol frames only**.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::jsonrpc::{self, error_code};
use crate::session::{Session, Status};
use crate::tools::{ToolAction, ToolSpec, catalog};
use ludion_protocol::ResponseBody;

/// MCP protocol versions this adapter understands. The newest is offered by default; a client's
/// offered version is echoed back when it is one of these.
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// The version offered when the client asks for one we do not recognise (task-specified default).
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// The longest input line accepted; the wire's own message cap.
const MAX_LINE_BYTES: usize = ludion_protocol::MAX_MESSAGE_BYTES;

/// What [`read_line_capped`] found.
#[derive(Debug, PartialEq, Eq)]
enum LineRead {
    /// End of input with nothing read.
    Eof,
    /// A line is in the buffer (without its `\n` / `\r\n`).
    Line,
    /// The line exceeded the cap and was discarded; the buffer is empty.
    TooLong,
}

/// Reads one line into `buf` (cleared first), refusing to hold more than `cap` bytes: an over-cap
/// line is consumed chunk by chunk and dropped, never buffered. A final line without `\n` still
/// counts as a line.
fn read_line_capped(
    reader: &mut impl BufRead,
    buf: &mut Vec<u8>,
    cap: usize,
) -> std::io::Result<LineRead> {
    buf.clear();
    let mut too_long = false;
    let mut read_any = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        read_any = true;
        let newline = available.iter().position(|&byte| byte == b'\n');
        let (chunk, consumed) = match newline {
            Some(index) => (&available[..index], index + 1),
            None => (available, available.len()),
        };
        if !too_long && buf.len() + chunk.len() > cap {
            too_long = true;
            buf.clear();
        }
        if !too_long {
            buf.extend_from_slice(chunk);
        }
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }
    if !read_any {
        return Ok(LineRead::Eof);
    }
    if too_long {
        return Ok(LineRead::TooLong);
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Ok(LineRead::Line)
}

/// The stdio MCP server: the tool catalog plus the lazily-connected wire [`Session`].
pub(crate) struct McpServer {
    session: Session,
    tools: Vec<ToolSpec>,
    max_line_bytes: usize,
}

impl McpServer {
    /// Builds the server for the given engine `url`, deriving the tool catalog from the protocol
    /// schema. Does not connect (that happens lazily on the first `tools/call`).
    pub(crate) fn new(url: String) -> Self {
        McpServer {
            session: Session::new(url),
            tools: catalog(),
            max_line_bytes: MAX_LINE_BYTES,
        }
    }

    /// Runs the read-dispatch-write loop until `reader` reaches EOF.
    ///
    /// A line over [`MAX_LINE_BYTES`] and a line that is not UTF-8 are each answered with a
    /// `PARSE_ERROR` carrying a null id (none is recoverable from either), and the adapter keeps
    /// running.
    ///
    /// # Errors
    /// Only a hard I/O failure reading from `reader` or writing to `writer` (a closed stdout) stops
    /// the loop early.
    pub(crate) fn run(
        &mut self,
        mut reader: impl BufRead,
        mut writer: impl Write,
    ) -> std::io::Result<()> {
        let mut buf = Vec::new();
        loop {
            match read_line_capped(&mut reader, &mut buf, self.max_line_bytes)? {
                LineRead::Eof => return Ok(()),
                LineRead::TooLong => {
                    let message = format!("message is longer than {} bytes", self.max_line_bytes);
                    log(&message);
                    write_response(&mut writer, &parse_failure(message))?;
                    continue;
                }
                LineRead::Line => {}
            }
            let line = match std::str::from_utf8(&buf) {
                Ok(line) => line,
                Err(error) => {
                    let message = format!("message is not valid UTF-8: {error}");
                    log(&message);
                    write_response(&mut writer, &parse_failure(message))?;
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Some(response) = self.handle_line(line) {
                write_response(&mut writer, &response)?;
            }
        }
    }

    /// Dispatches one line, returning the response value to write, or `None` for a notification.
    fn handle_line(&mut self, line: &str) -> Option<Value> {
        let incoming = match jsonrpc::parse(line) {
            Ok(incoming) => incoming,
            Err(err) => {
                log(&format!("parse error: {}", err.message));
                // A line that cannot be parsed is always answered, with a null id when none is known.
                return Some(jsonrpc::failure(err.id, err.code, err.message));
            }
        };

        // Notifications carry no id and must never be answered.
        let Some(id) = incoming.id.clone() else {
            log(&format!("notification {}", incoming.method));
            return None;
        };

        Some(match incoming.method.as_str() {
            "initialize" => {
                log("initialize");
                jsonrpc::success(id, self.initialize_result(&incoming.params))
            }
            "ping" => jsonrpc::success(id, json!({})),
            "tools/list" => {
                log("tools/list");
                jsonrpc::success(id, self.tools_list_result())
            }
            "tools/call" => match self.tools_call(&incoming.params) {
                Ok(result) => jsonrpc::success(id, result),
                Err(err) => jsonrpc::failure(Some(id), err.0, err.1),
            },
            other => {
                log(&format!("unknown method {other}"));
                jsonrpc::failure(
                    Some(id),
                    error_code::METHOD_NOT_FOUND,
                    format!("unknown method `{other}`"),
                )
            }
        })
    }

    /// The `initialize` result: negotiated protocol version, tool capability, server identity.
    fn initialize_result(&self, params: &Value) -> Value {
        let offered = params.get("protocolVersion").and_then(Value::as_str);
        let version = match offered {
            Some(v) if SUPPORTED_PROTOCOL_VERSIONS.contains(&v) => v,
            _ => DEFAULT_PROTOCOL_VERSION,
        };
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "ludion-mcp", "version": env!("CARGO_PKG_VERSION") },
        })
    }

    /// The `tools/list` result: every tool's `{name, description, inputSchema}`.
    fn tools_list_result(&self) -> Value {
        let tools: Vec<Value> = self.tools.iter().map(ToolSpec::to_list_entry).collect();
        json!({ "tools": tools })
    }

    /// Handles `tools/call`.
    ///
    /// Returns the tool result value on success (including in-band engine errors, which are tool
    /// results with `isError: true`), or a `(code, message)` JSON-RPC error for a malformed call —
    /// a missing/unknown tool name (the only cases the spec routes to JSON-RPC, not tool results).
    fn tools_call(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                (
                    error_code::INVALID_PARAMS,
                    "`tools/call` requires a string `name`".to_string(),
                )
            })?
            .to_string();
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(arguments) => arguments.clone(),
        };

        let tool = self
            .tools
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| (error_code::INVALID_PARAMS, format!("unknown tool `{name}`")))?;

        // Match on the action by value we already borrowed; clone the small bits so we drop the
        // borrow of `self.tools` before touching `self.session`.
        let result = match &tool.action {
            ToolAction::Status => {
                log(&format!("tools/call {name} (status)"));
                self.status_result()
            }
            ToolAction::Protocol { method, has_params } => {
                let method = method.clone();
                let has_params = *has_params;
                self.call_protocol(&name, &method, has_params, arguments)
            }
        };
        Ok(result)
    }

    /// Builds and sends a protocol command, turning the response into MCP tool content.
    fn call_protocol(
        &mut self,
        name: &str,
        method: &str,
        has_params: bool,
        arguments: Value,
    ) -> Value {
        // Unit methods take no `params`; every other method's arguments must match its params
        // exactly (a stray field is refused with the field named).
        let params = if has_params {
            Some(arguments)
        } else {
            if arguments
                .as_object()
                .is_none_or(|object| !object.is_empty())
            {
                log(&format!(
                    "tools/call {name} -> arguments for a no-argument tool"
                ));
                return tool_error(format!(
                    "tool `{name}` takes no arguments; call it with none (or `{{}}`)"
                ));
            }
            None
        };
        let command = match ludion_protocol::envelope::command_from_parts(method, params) {
            Ok(command) => command,
            Err(diagnostic) => {
                // A well-formed MCP call with arguments that don't fit the tool schema: an in-band
                // tool error the model can correct, not a JSON-RPC protocol failure.
                log(&format!("tools/call {name} -> bad arguments: {diagnostic}"));
                return tool_error(format!(
                    "arguments do not match tool `{name}`: {diagnostic}\n\nsee the tool's inputSchema \
                     for the expected shape"
                ));
            }
        };

        match self.session.call(command) {
            Ok(ok) => {
                let frame = ok
                    .frame
                    .map(|f| f.as_u64().to_string())
                    .unwrap_or_else(|| "n/a".to_string());
                log(&format!("tools/call {name} -> ok (frame {frame})"));
                success_content(method, ok.body)
            }
            Err(diagnostic) => {
                // Engine diagnostics come back as tool errors, rendered verbatim — L#### code and
                // all — so the model reads exactly what the engine said (and its `help`).
                log(&format!(
                    "tools/call {name} -> {} {}",
                    diagnostic.code, diagnostic.message
                ));
                tool_error(diagnostic.to_string())
            }
        }
    }

    /// The `ludion_status` tool result (a text block), or a tool error if the engine is unreachable.
    fn status_result(&mut self) -> Value {
        match self.session.status() {
            Ok(status) => tool_text(render_status(&status)),
            Err(diagnostic) => tool_error(format!(
                "not connected to the engine at {}\n\n{}",
                self.session.url(),
                diagnostic
            )),
        }
    }
}

/// Writes one response line and flushes it.
fn write_response(writer: &mut impl Write, response: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string(response).expect("a response value always serializes to JSON");
    writer.write_all(text.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// A `PARSE_ERROR` response with a null id, for a line no id can be read from.
fn parse_failure(message: String) -> Value {
    jsonrpc::failure(None, error_code::PARSE_ERROR, message)
}

/// Turns a successful [`ResponseBody`] into tool content.
///
/// `render.screenshot` is special: its base64 PNG becomes an MCP **image** content block (the
/// agent's eyes), with a short text block naming the dimensions. Every other body is pretty-printed
/// JSON in a single text block.
fn success_content(method: &str, body: ResponseBody) -> Value {
    if method == "render.screenshot"
        && let ResponseBody::Screenshot {
            width,
            height,
            format,
            data,
            stale,
        } = &body
    {
        // Screenshots are always PNG (`ResponseBody::Screenshot.format` is `"png"`).
        // The staleness note is in the *text* block, not dropped: an agent looking at held global
        // illumination cannot see that from the pixels, and would otherwise reason about light the
        // world no longer implies.
        let held = if *stale {
            " (GI held — this light is stale; `render.gi_rebake` with `rebake: true` refreshes it)"
        } else {
            ""
        };
        return json!({
            "content": [
                { "type": "image", "data": data, "mimeType": "image/png" },
                { "type": "text", "text": format!("rendered {width}x{height} {format} screenshot{held}") },
            ],
            "isError": false,
        });
    }

    let text = serde_json::to_string_pretty(&body)
        .unwrap_or_else(|e| format!("<the response body could not be serialized: {e}>"));
    tool_text(text)
}

/// A single-text-block tool result (not an error).
fn tool_text(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": false })
}

/// A single-text-block tool result flagged `isError: true`.
fn tool_error(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

/// Renders a [`Status`] as the human-readable text block the status tool returns.
fn render_status(status: &Status) -> String {
    let frame = status
        .frame
        .map(|f| f.to_string())
        .unwrap_or_else(|| "n/a".to_string());
    let game_frame = status
        .game_frame
        .map(|f| f.to_string())
        .unwrap_or_else(|| "n/a".to_string());
    format!(
        "connected to {url}\n  engine version:   {engine}\n  protocol version: {proto}\n  \
         schema hash:      {schema}\n  current frame:    {frame}\n  game frame:       {game_frame}\n  \
         world hash:       {hash}",
        url = status.url,
        engine = status.engine_version,
        proto = status.protocol_version,
        schema = status.schema_hash,
        hash = status.world_hash,
    )
}

/// Writes one structured line to stderr. **Never stdout** — that is the protocol channel.
fn log(message: &str) {
    eprintln!("[ludion-mcp] {message}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Drives the server over an in-memory transport with the given input lines, returning each
    /// response line parsed as JSON. No engine is contacted (only `initialize`/`tools/list`/`ping`).
    fn drive(lines: &[&str]) -> Vec<Value> {
        drive_bytes(lines.join("\n").as_bytes())
    }

    fn drive_bytes(input: &[u8]) -> Vec<Value> {
        let mut output = Vec::new();
        let mut server = McpServer::new("ws://127.0.0.1:9411".to_string());
        server
            .run(Cursor::new(input), &mut output)
            .expect("in-memory run never fails to write");
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).expect("each response line is JSON"))
            .collect()
    }

    #[test]
    fn a_non_utf8_line_is_answered_with_a_null_id_and_the_loop_keeps_running() {
        let out = drive_bytes(b"\xff\xfe\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["id"], Value::Null);
        assert_eq!(out[0]["error"]["code"], json!(error_code::PARSE_ERROR));
        assert_eq!(out[1]["id"], json!(1));
    }

    #[test]
    fn invalid_json_is_answered_with_a_null_id() {
        let out = drive(&["{not json", r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["id"], Value::Null);
        assert_eq!(out[0]["error"]["code"], json!(error_code::PARSE_ERROR));
        assert_eq!(out[1]["id"], json!(2));
    }

    #[test]
    fn a_batch_is_refused_as_an_invalid_request() {
        let out = drive(&[r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], Value::Null);
        assert_eq!(out[0]["error"]["code"], json!(error_code::INVALID_REQUEST));
        assert!(
            out[0]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("batches")
        );
    }

    #[test]
    fn an_over_cap_line_is_answered_and_the_loop_keeps_running() {
        let mut input = vec![b'x'; 65];
        input.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
        let mut output = Vec::new();
        let mut server = McpServer::new("ws://127.0.0.1:9411".to_string());
        server.max_line_bytes = 64;
        server.run(Cursor::new(input), &mut output).unwrap();
        let out: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["id"], Value::Null);
        assert_eq!(out[0]["error"]["code"], json!(error_code::PARSE_ERROR));
        assert_eq!(out[1]["id"], json!(1));
    }

    fn tool_call(name: &str, arguments: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
        )
    }

    #[test]
    fn a_no_argument_tool_refuses_arguments_without_touching_the_engine() {
        let out = drive(&[&tool_call("sim_hash", r#"{"x":1}"#)]);
        assert_eq!(out[0]["result"]["isError"], json!(true));
        let text = out[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("sim_hash") && text.contains("no arguments"),
            "{text}"
        );
    }

    #[test]
    fn a_stray_argument_is_an_in_band_error_naming_the_field() {
        // No engine is reachable here, so a call that got past decoding would say "not connected".
        let out = drive(&[&tool_call("sim_step", r#"{"ticks":1,"hsh":true}"#)]);
        assert_eq!(out[0]["result"]["isError"], json!(true));
        let text = out[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("hsh") && text.contains("`hash`") && text.contains("/params/hsh"),
            "{text}"
        );
    }

    #[test]
    fn null_arguments_are_read_as_absent() {
        let out = drive(&[&tool_call("sim_step", "null")]);
        assert_eq!(out[0]["result"]["isError"], json!(true));
        let text = out[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("invalid type: null"), "{text}");
        assert!(text.contains("missing field"), "{text}");
    }

    #[test]
    fn an_over_cap_line_is_discarded_without_buffering_and_reading_resumes() {
        let mut reader = Cursor::new(&b"0123456789abc\nok\n"[..]);
        let mut buf = Vec::new();
        assert_eq!(
            read_line_capped(&mut reader, &mut buf, 8).unwrap(),
            LineRead::TooLong
        );
        assert!(buf.is_empty());
        assert_eq!(
            read_line_capped(&mut reader, &mut buf, 8).unwrap(),
            LineRead::Line
        );
        assert_eq!(buf, b"ok");
        assert_eq!(
            read_line_capped(&mut reader, &mut buf, 8).unwrap(),
            LineRead::Eof
        );
    }

    #[test]
    fn a_last_line_without_a_newline_is_still_a_line() {
        let mut reader = Cursor::new(&b"tail\r"[..]);
        let mut buf = Vec::new();
        assert_eq!(
            read_line_capped(&mut reader, &mut buf, 64).unwrap(),
            LineRead::Line
        );
        assert_eq!(buf, b"tail");
    }

    #[test]
    fn initialize_reports_capabilities_and_server_info() {
        let out = drive(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        ]);
        assert_eq!(out.len(), 1);
        let result = &out[0]["result"];
        assert_eq!(result["protocolVersion"], json!("2025-06-18"));
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], json!("ludion-mcp"));
    }

    #[test]
    fn initialize_falls_back_for_an_unknown_offered_version() {
        let out = drive(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        ]);
        assert_eq!(out[0]["result"]["protocolVersion"], json!("2025-06-18"));
    }

    #[test]
    fn notifications_get_no_response() {
        let out = drive(&[
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        ]);
        // Only the ping (which has an id) is answered.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], json!(2));
        assert_eq!(out[0]["result"], json!({}));
    }

    #[test]
    fn tools_list_returns_every_tool_with_an_input_schema() {
        let out = drive(&[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#]);
        let tools = out[0]["result"]["tools"].as_array().expect("tools array");
        assert_eq!(
            tools.len(),
            ludion_protocol::Command::METHODS.len() + 1 - crate::tools::EXCLUDED_METHODS.len(),
        );
        for tool in tools {
            assert!(tool["name"].is_string());
            assert!(tool["description"].is_string());
            assert_eq!(tool["inputSchema"]["type"], json!("object"), "{tool}");
        }
    }

    #[test]
    fn unknown_method_is_a_jsonrpc_error() {
        let out = drive(&[r#"{"jsonrpc":"2.0","id":9,"method":"tools/frobnicate"}"#]);
        assert_eq!(out[0]["error"]["code"], json!(error_code::METHOD_NOT_FOUND));
    }

    #[test]
    fn unknown_tool_is_a_jsonrpc_invalid_params_error() {
        let out = drive(&[
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"no_such_tool"}}"#,
        ]);
        assert_eq!(out[0]["error"]["code"], json!(error_code::INVALID_PARAMS));
    }
}
