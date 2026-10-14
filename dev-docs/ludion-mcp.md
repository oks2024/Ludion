---
description: MCP stdio adapter — one tool per protocol method, derived from the schema, driving `ludion serve`
---

# ludion-mcp

**Scope:** The `ludion-mcp` bin crate: a Model Context Protocol (MCP) server over stdio that lets
Claude Code drive a running engine (`ludion serve`) through MCP tools. It is a *thin peer client*
(engine-architecture.md §2: `ludion-mcp → ludion-client`) — a stdio JSON-RPC 2.0 server that
translates MCP `tools/call`s into Ludion `Command`s (`crates/ludion-protocol/src/command.rs`) over the WebSocket
and translates responses back into MCP content. It hosts **no** MCP-private capabilities: every tool
is a protocol method, so the same parity gate the CLI and editor obey holds here. The wire shapes
live in `ludion-protocol`; the blocking transport is `ludion-client` (see `crate-graph` for the
boundary picture, `ludion-client` for the SDK this drives, and `diagnostics` for the `L3###` codes it
surfaces verbatim).

## Layout

- `crates/ludion-mcp/src/main.rs` — the binary: `#![forbid(unsafe_code)]`/`#![warn(missing_docs)]`,
  the crate rustdoc header (with the `claude mcp add` / `.mcp.json` snippets), `--url` parsing
  (default `ws://127.0.0.1:9411`, hand-rolled — one flag does not justify a dep), and the stdio wiring
  (`stdin.lock()` → `McpServer::run` → `stdout.lock()`).
- `crates/ludion-mcp/src/jsonrpc.rs` — line-delimited JSON-RPC 2.0 framing only: `parse` (a line →
  `Incoming { id, method, params }`), the standard `error_code`s, and `success`/`failure` builders.
  Requests carry an `id`; notifications (no `id`, or `id: null`) are never answered.
- `crates/ludion-mcp/src/tools.rs` — the tool catalog. `catalog()` reads
  [`ludion_protocol::schema::protocol_schema`] at runtime and emits one `ToolSpec` per `Command`
  variant plus the synthetic `ludion_status` tool. Names are `sanitize`d (`.` → `_`); each `ToolSpec`
  keeps the original dotted method so `tools/call` never reverse-parses a name.
- `crates/ludion-mcp/src/session.rs` — the wire connection: `Session` holds an `Option<Client>`,
  connects lazily, probes a held connection (`Client::check_open`) and reconnects only if it is
  already closed, never re-sends a command, and exposes a reconnecting `status()` probe.
- `crates/ludion-mcp/src/server.rs` — the MCP method loop: `initialize` / `ping` / `tools/list` /
  `tools/call`, plus the content-block building (text for most bodies, an **image** block for
  `render.screenshot`, `isError` for engine diagnostics).
- `crates/ludion-mcp/tests/integration.rs` — the cross-process proof: spawns real `ludion serve
  --scene <gallery> --paused` and the real `ludion-mcp` bin, speaks `initialize` → `tools/list` →
  `tools/call sim_step` → `tools/call sim_hash` over stdio, and asserts a well-formed WorldHash.
- `crates/ludion-mcp/clippy.toml` — deliberately empty: the *permissive* client-side denylist (this
  crate is not on the deterministic sim path), exactly like `ludion-client`.

## Rules

- **One tool per protocol method — derived from the schema, never hand-listed.** The catalog is built
  from `protocol_schema()` (the same document `schema_hash` hashes), so a method added to `Command`
  becomes a tool automatically and the two cannot drift. A tool's `inputSchema` is the method's
  `params` schema with the schema's `$defs` embedded so internal `$ref`s resolve; a parameterless
  method gets an empty-object schema.
- **The tool name is the dotted method with `.` → `_`.** MCP names must match `^[a-zA-Z0-9_-]+$`. The
  mapping is lossy (`sim_set_speed` could have come from `sim.set.speed`), so the reverse lookup is by
  stored method string, never by un-sanitizing the name.
- **`Command` is built by serde, not by a match.** It is adjacently tagged (`{method, params}`), so a
  tool call becomes `serde_json::from_value::<Command>({"method": …, "params": args})` — no per-command
  arm. Unit variants omit `params` on the wire.
- **Engine errors are tool results, not JSON-RPC errors.** A Ludion `Diagnostic` comes back as a tool
  result with `isError: true`, rendered verbatim via its `Display` (which includes the `L####` code,
  the path, and the `help`). JSON-RPC errors are reserved for malformed MCP requests (parse /
  invalid-request) and unknown tools/methods — the transport's own faults, not the engine's.
- **Connect lazily; probe, then reconnect; never re-send.** Startup and `tools/list` touch no network,
  so the adapter can be registered before `ludion serve` is up. The first `tools/call` connects; each
  later call first probes the held connection, and reconnects only if it is already closed (nothing
  written yet, so safe). A call that fails — even with `L3009` — is returned to the model, never
  retried (the command may have run); the dead client is dropped so the next call reconnects.
- **`render_screenshot` is the imaging path.** Its base64 PNG becomes an MCP **image** content block
  (`type: "image"`, `mimeType: "image/png"`) plus a short text block naming the dimensions — the
  agent's eyes. Subscription tools (`stream_subscribe`/`stream_unsubscribe`) are exposed for parity but
  say in their description that streamed events/frames are not deliverable over request/response MCP.
- **stdout is the protocol channel; logging goes to stderr.** One structured `[ludion-mcp] …` line per
  request on stderr; nothing but JSON-RPC frames on stdout.
- `#![forbid(unsafe_code)]`, `#![warn(missing_docs)]` — like every sibling crate.

## How it works

`main` parses `--url`, logs a startup line to stderr, and hands `stdin.lock()`/`stdout.lock()` to
`McpServer::run`, which reads one JSON-RPC message per line. `handle_line` parses the line and
dispatches: `initialize` returns `{protocolVersion, capabilities: {tools: {}}, serverInfo}` (echoing
the client's offered version if it is one this build supports, else `"2025-03-26"`);
`notifications/initialized` and any other notification are ignored (no response); `ping` returns `{}`;
`tools/list` returns every tool's `{name, description, inputSchema}`; `tools/call` runs the tool.

`tools/call` looks the tool up by name (unknown → JSON-RPC `-32602`). For a protocol tool it builds
the `Command` value from the stored method plus the caller's `arguments`, deserializes it (a shape
mismatch is an `isError` tool result the model can correct, not a protocol failure), and sends it
through `Session::call`. On success, `render.screenshot` becomes an image + text block and every other
`ResponseBody` becomes pretty-printed JSON in a text block. On an engine `Diagnostic`, an `isError`
tool result carries `diagnostic.to_string()`. The synthetic `ludion_status` tool reconnects if needed
and reports the server hello (engine version, protocol version, schema hash) plus the current sim and gameplay
frames and WorldHash from a `sim.hash` probe.

`Session` owns the lazy `Option<Client>`. `ensure_connected` calls `Client::connect` (which performs
the handshake and the connect-time schema-drift check) on first use; `call` probes with
`Client::check_open` and reconnects before sending if the connection is closed. A call that fails
with `PROTOCOL_ENGINE_UNAVAILABLE` (`L3009`) is returned and the client dropped — one `Client::call`
per `Session::call`, so a command is never sent twice.

## Verify

```
cargo test -p ludion-mcp                          # 22 unit tests (framing, catalog shape, dispatch) + the integration test
cargo test -p ludion-mcp --test integration       # cross-process: real serve + real mcp over stdio, asserts a WorldHash
cargo clippy -p ludion-mcp --all-targets -- -D warnings
cargo run -p ludion-mcp -- --help                 # usage
```

Register it with Claude Code (default endpoint `ws://127.0.0.1:9411`):

```
claude mcp add ludion -- ludion-mcp --url ws://127.0.0.1:9411
```

or as an `.mcp.json` entry:

```json
{
  "mcpServers": {
    "ludion": { "command": "ludion-mcp", "args": ["--url", "ws://127.0.0.1:9411"] }
  }
}
```

Then start the engine (`ludion serve --scene level.yaml`) and the tools drive it.

## Gotchas

- **The adapter does not launch the engine.** It attaches to a `ludion serve` you start separately;
  if the engine is down, the first `tools/call` returns an `isError` result whose text is the `L3009`
  diagnostic ("could not connect …"). `ludion_status` is the quick reachability check.
- **The schema-drift gate is the engine's, inherited.** `Client::connect` fails with `L3003` if the
  served schema hash differs from this build's — so a `ludion-mcp` built against a stale protocol can't
  silently drive a newer engine. That surfaces as an `isError` tool result on the first call.
- **Tool-name collisions are avoided by construction, not checked.** No two protocol methods sanitize
  to the same name today (the only underscores in method names are already underscores, e.g.
  `sim.set_speed` → `sim_set_speed`); `ludion_status` is the one synthetic name and no method maps to
  it. If a future method would collide, the catalog would emit two same-named tools — a `tools/list`
  count test guards the count, not uniqueness.
- **The integration test builds and locates `ludion` itself.** `ludion` lives in `ludion-cli`, so
  `CARGO_BIN_EXE_ludion` is not set for this crate's tests. The test runs `cargo build -p ludion-cli
  --bin ludion` (matching its own profile) and finds the binary next to its test executable, then
  spawns it directly so `Child::kill` reaps the actual server rather than orphaning it behind a
  `cargo run` wrapper.
- **`sim.step`/`sim.hash` need no GPU;** the integration test uses a paused engine (a deterministic
  attach point) and a system-free world (frame-invariant hash), so it is green on adapterless CI too.
  Only `render_screenshot` touches the GPU, and the engine returns `L2003` there when no adapter
  exists — which the adapter relays as an ordinary `isError` tool result.
