#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-mcp` — a Model Context Protocol (MCP) server that lets Claude Code drive a running
//! Ludion engine over the wire protocol.
//!
//! It is a thin peer client (engine-architecture.md §2: `ludion-mcp → ludion-client`): a stdio
//! JSON-RPC 2.0 server that translates MCP tool calls into Ludion [`Command`](ludion_protocol::Command)s
//! sent over `ludion serve`'s WebSocket, and translates the responses back into MCP content. There
//! are no MCP-private capabilities — every tool is a protocol method, so the same parity gate the
//! CLI and editor obey holds here too.
//!
//! # Tool surface
//!
//! One MCP tool per protocol method, derived at runtime from
//! [`ludion_protocol::schema::protocol_schema`] so the tools can never drift from the wire surface —
//! except `stream.subscribe`/`stream.unsubscribe`, which are omitted: MCP is request/response, so a
//! subscription's events have nowhere to arrive, and exposing the tool is a trap rather than a
//! convenience. MCP names must match `^[a-zA-Z0-9_-]+$`, so the dotted method is sanitized with
//! `.` → `_` (`sim.step` → `sim_step`). One extra tool, `ludion_status`, reports connection health.
//! The `render_screenshot` tool returns its image as an MCP image content block — the agent's eyes.
//!
//! # Connection lifecycle
//!
//! The adapter connects **lazily** — startup and `tools/list` need no engine, so it can be
//! registered before `ludion serve` is up. The first `tools/call` connects; a call probes the
//! connection before sending and reconnects a closed one; a call that fails mid-flight is returned,
//! never re-sent. Engine errors come back as tool results
//! with `isError: true`, rendered verbatim (their `L####` code included); JSON-RPC errors are
//! reserved for malformed MCP requests and unknown tools.
//!
//! # Usage
//!
//! ```text
//! ludion-mcp [--url ws://127.0.0.1:9411]
//! ```
//!
//! Register it with Claude Code (the engine is reached at `--url`, default `ws://127.0.0.1:9411`):
//!
//! ```text
//! claude mcp add ludion -- ludion-mcp --url ws://127.0.0.1:9411
//! ```
//!
//! or as an `.mcp.json` entry:
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "ludion": {
//!       "command": "ludion-mcp",
//!       "args": ["--url", "ws://127.0.0.1:9411"]
//!     }
//!   }
//! }
//! ```
//!
//! Then start the engine (`ludion serve --scene level.yaml`) and the tools drive it.

mod jsonrpc;
mod server;
mod session;
mod tools;

use std::io::{BufReader, Write};
use std::process::ExitCode;

use server::McpServer;

/// The default engine endpoint when `--url` is not given.
const DEFAULT_URL: &str = "ws://127.0.0.1:9411";

fn main() -> ExitCode {
    let url = match parse_args(std::env::args().skip(1)) {
        Ok(Some(url)) => url,
        Ok(None) => return ExitCode::SUCCESS, // --help printed usage
        Err(message) => {
            eprintln!("[ludion-mcp] {message}");
            eprintln!("usage: ludion-mcp [--url ws://HOST:PORT]");
            return ExitCode::FAILURE;
        }
    };

    eprintln!(
        "[ludion-mcp] {} starting; engine endpoint {url} (connecting lazily on first tools/call)",
        env!("CARGO_PKG_VERSION"),
    );

    // stdout is the protocol channel and nothing else; stdin is the request stream.
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut server = McpServer::new(url);
    match server.run(BufReader::new(stdin.lock()), stdout.lock()) {
        Ok(()) => {
            eprintln!("[ludion-mcp] stdin closed; shutting down");
            ExitCode::SUCCESS
        }
        Err(e) => {
            // A broken stdout means the MCP client went away mid-write — nothing left to do.
            let _ =
                std::io::stderr().write_all(format!("[ludion-mcp] I/O error: {e}\n").as_bytes());
            ExitCode::FAILURE
        }
    }
}

/// Parses the command line, returning the resolved URL, `None` if `--help` was handled, or an error
/// message.
///
/// Accepts `--url <value>`, `--url=<value>`, and `-h`/`--help`. Deliberately hand-rolled: one flag
/// does not justify a dependency (the dep policy is conservative).
fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<String>, String> {
    let mut url = DEFAULT_URL.to_string();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("usage: ludion-mcp [--url ws://HOST:PORT]");
                println!();
                println!(
                    "An MCP (Model Context Protocol) stdio server adapting the Ludion wire protocol."
                );
                println!(
                    "  --url <ws://HOST:PORT>   engine endpoint to drive (default {DEFAULT_URL})"
                );
                return Ok(None);
            }
            "--url" => {
                url = args
                    .next()
                    .ok_or_else(|| "`--url` needs a value".to_string())?;
            }
            other if other.starts_with("--url=") => {
                url = other["--url=".len()..].to_string();
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(Some(url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<String>, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn defaults_the_url() {
        assert_eq!(parse(&[]).unwrap(), Some(DEFAULT_URL.to_string()));
    }

    #[test]
    fn accepts_url_as_two_tokens_or_joined() {
        assert_eq!(
            parse(&["--url", "ws://host:1"]).unwrap(),
            Some("ws://host:1".to_string())
        );
        assert_eq!(
            parse(&["--url=ws://host:2"]).unwrap(),
            Some("ws://host:2".to_string())
        );
    }

    #[test]
    fn help_returns_none() {
        assert_eq!(parse(&["--help"]).unwrap(), None);
    }

    #[test]
    fn rejects_unknown_flags_and_missing_url_value() {
        assert!(parse(&["--nope"]).is_err());
        assert!(parse(&["--url"]).is_err());
    }
}
