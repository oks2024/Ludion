---
description: Speaking the JSON-over-WebSocket protocol directly: handshake, request/response, a worked call
---

# Appendix: attaching over the raw wire

**Scope:** Driving a running host from a bare socket, when there is no MCP adapter to register. Starting the host, and the easy paths onto it, are running. The session levers and the entity/prefab verbs are the spokes `ludion docs wire-render` and `ludion docs wire-hierarchy`.

`ludion-mcp` is the easy path (register it, get tools). But if you have no MCP adapter — a bare socket
in a test harness or another language — you can speak the protocol directly. It is a small JSON
protocol over a **WebSocket** at the `ws://…` address the host prints. Every message is one JSON text
frame. This is exactly what the MCP adapter and the editor do underneath. A message is limited to
256 MiB each way (set your socket's max message size to match); past it the message is refused with
`L3045`.

**1. Handshake.** Open a WebSocket to the address, then send a `Hello` and read the `ServerHello`
(this exchange happens once, before any request):

```jsonc
// → client sends first
{"protocol_version": 1, "client": "my-probe 0.0.0", "features": []}
// ← server replies
{"engine_version": "0.0.0", "protocol_version": 1, "schema_hash": "c3dddcfcd8111902"}
```

(The value shown is the hash at the time of writing, not a contract — the schema grows and the hash
moves with it. Never hardcode it in a client; compare against whatever value the server's own
`ServerHello` sends.)

The `schema_hash` is the digest of this engine's exported protocol schema. A client generated from a
different schema must at least tell its user on a mismatch (`ludion-client` refuses; the editor warns
and stays connected) — the protocol is versioned and breaking by design (a bad `v` on any later
request is `L3003`). You don't have to send it back; just check it. How the schema (and this hash) is
allowed to change is the protocol-evolution policy: `ludion devdocs protocol-evolution`.

A browser page served from another origin is refused at the upgrade with HTTP 403 and `L3039`; native
clients (CLI, MCP, Node, your own probe) send no `Origin` header and connect as above. `scene.load`,
`scene.save` and `scene.check` take a path relative to the content root; an absolute, drive-prefixed
or `..` path is `L3040`. `scene.save` refuses a world or destination bearing `generated_from:` (`L1267`; edit the
map and regenerate) and a destination that is a `kind:` document (`L3059`). It also refuses a
destination whose text differs from what this session loaded or last saved there (`L3058`: `scene.load`
it, or pass `force: true`, which skips only that check), and returns the checker's findings over what
it wrote in `diagnostics`; they never block the write. `engine.restore` forgets the current scene path,
so a later restart's `scene.changed` names none. `scene.check` runs everything `ludion check-scene`
does, and returns warnings beside `valid: true`.

**2. Request / response envelope.** After the handshake, every call is a `Request` and every reply a
`Response`, correlated by a client-chosen `id`:

```jsonc
// → Request: v (protocol version), id, method (dotted name), params
{"v": 1, "id": 1, "method": "query.entities", "params": {"filter": ["Sprite"], "include": ["Sprite"]}}
// ← Response (success): echoes id, carries `ok` + the sim `frame` and gameplay `game_frame` it applied at
{"id": 1, "ok": {"kind": "query_result", "entities": [
  {"entity": {"index": 1, "generation": 0}, "components": {"Sprite": {"texture": "assets/brick.png", "size": [1.2, 0.6], "color": [1.0,1.0,1.0,1.0], "billboard": false}}}
]}, "frame": 0, "game_frame": 0}
```

A failure replaces `ok` with a structured `error` (same rustc-style diagnostic every client gets):

```jsonc
{"id": 1, "error": {"code": "L3002", "message": "`entity.spwn` is not a method this protocol version defines", "path": "/method", "help": "nearest known method is `entity.spawn`"}}
```

**3. One worked example: connect → read a component.** Using the entity id from the query above, read
its `Transform`:

```jsonc
// → component.get takes an entity ({index, generation}) and a registry component name
{"v": 1, "id": 2, "method": "component.get", "params": {"entity": {"index": 1, "generation": 0}, "component": "Transform"}}
// ← the value, under kind "component_value"
{"id": 2, "ok": {"kind": "component_value", "value": {"translation": [-2.4, 0.0, 0.0], "rotation": [0.0,0.0,0.0], "scale": [1.0,1.0,1.0]}}, "frame": 0, "game_frame": 0}
```

(The captured shapes are real — floats come back at full `f32` precision on the wire, e.g. `-2.4`
serializes as `-2.4000000953674316`; shown trimmed here.) That is the whole protocol: handshake once,
then `{v,id,method,params}` → `{id, ok|error, frame, game_frame}`. Stamp a `started` with `game_frame`,
never `frame`, which is late by every frame the game has held this session. `engine.describe` returns
the full component and method surface in one call — plus a `documents` object,
`{"<kind>": <JSON Schema>}` for every registered data-doc kind (`ludion docs data-docs`), the same
schema the host validates a `kind:` file against.

A request is strict: an unknown field in `params` (or an unknown envelope key) is refused with `L3001`,
its `path` naming the field (`/params/hsh`) and its `help` suggesting the nearest valid one, rather
than being ignored. `sim.step` runs synchronously, so `ticks` above 216000 (an hour at 60 Hz) is
refused with `L3057`; step in smaller batches.

A `scene.changed` event's `data` has `kind` (`load`, `save`, `restore`, `switch`, `checkpoint_restore`), the `path` when
known, and `request_id`, the `id` of the request that caused it. Ids are chosen per connection, so
another client's request may share the number; it is absent for a game-driven switch.

Events (frame streams, `scene.changed`, `log`) arrive as unsolicited
`{"event": …, "sub": …, "seq": …, "data": …}` messages once you `stream.subscribe`; a request-only
probe never sees them. A `log` event's `data` is `{"level", "message"}`; when the line reports an
`L####` diagnostic it also carries `"diagnostic"` — the same object an error response carries
(`code`, `message`, optional `path`/`help`/`related`) — and `message` is its rustc-style rendering, so
read the structured field rather than parsing the text. Events and frames stay in order, with visible
`seq` gaps if you stop reading. A subscription's first events or frame can arrive *before* its
`stream.subscribe` reply when the subscribe is pipelined behind a held request (a `profile.capture`,
say), so accept, or buffer by `sub`, stream messages whose id you have not yet seen a reply for.

`query.entities` also takes `"entities"` to read a few entities you already hold: the matches are
those ids that are live with the same generation and pass `filter`. A dead, stale or unknown id is
skipped silently (never an error); results keep the canonical order (ascending index), not the
request order:

```jsonc
{"v": 1, "id": 11, "method": "query.entities", "params": {"filter": [], "include": ["Name"], "entities": [{"index": 4, "generation": 0}]}}
```

## Next

- `ludion docs wire-render` — the host's session levers: `render.gi_rebake`, `render.size`, `render.view`, `render.debug`.
- `ludion docs wire-hierarchy` — `entity.reparent`, world poses, cascading destroy, the `prefab.*` verbs.
- `ludion docs running` — hosting a scene and observing it.
- `ludion docs verify-loop` — the `cargo test` verification loop.
