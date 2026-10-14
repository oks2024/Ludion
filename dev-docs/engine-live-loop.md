---
description: The live seam: run_live's loop, the frame-boundary drains, the diagnostics channel, egress
---

# The live seam, the frame-boundary drains, and egress

**Scope:** `live.rs`, `egress.rs` and `diagnostics.rs` — how the `Engine` runs on one thread, what
drains at a frame boundary, and what leaves the sim thread for a transport. The `Engine` itself, its
`dispatch.rs` parity gate and the crate's other rules are `ludion devdocs ludion-engine`; what a
transport does with an egress message — lane capacities, latest-wins staging, encoding — is
`ludion devdocs frame-streaming`; the WS host that supplies the channels is
`ludion devdocs ludion-server`; the recorder the drain feeds is `ludion devdocs engine-replay`.

## Layout

- `crates/ludion-engine/src/live.rs` — `StampedCommand` + reply channel, `run_live` (drain, tick, `emit_egress`, park), `shutdown_channel`.
- `crates/ludion-engine/src/egress.rs` — `Egress`/`EgressFrame`/`EgressEvent`, `event_channel`. Read its module doc first: it explains the fan-out-addressed-vs-fully-addressed split and the latest-wins-at-source contract in full.
- `crates/ludion-engine/src/diagnostics.rs` — the channel a *game system* reports through: it calls `diagnostics::report(world, code, key, message)` (or `report_full(world, key, diagnostic)` with a whole `Diagnostic`) on the `Diagnostics` world resource, whose queue holds `Diagnostic`s — no system pushes an `Event`, and no system prints.

## Dataflow

**`live.rs` is the threading model's engine half.** A transport builds a `StampedCommand::new(cmd)`
pair, pushes the command into the intake channel, and blocks on the returned one-shot receiver.
`run_live(engine, intake, shutdown, egress)` loops:

1. **drain** every pending command through `execute`, replying on each command's capacity-1 channel —
   the send never blocks and a dropped receiver is ignored, so the sim loop can never stall on a slow
   client. A paused engine still drains and answers.
2. **tick** at most once via `advance_live`, when the run-state allows and the wall clock says a
   fixed-rate tick is due (rate scaled by `speed`). A speed change from any source (`sim.set_speed`,
   `engine.restore`, `scene.load`) first re-bases the deadline, ahead of the paused check. If the loop falls more than a period behind it
   *resets the baseline* rather than firing catch-up bursts — live view paces by the clock,
   deterministic verification uses lockstep.
3. **emit** — only when a transport supplied an `egress` sender.
4. **park** on `select!` until the next command, shutdown, or tick deadline. Never a busy-spin.

**Rendering for `frames` subscribers is a pure read of the world**, so an attached viewer cannot
perturb determinism — the property `ludion-server`'s `tests/phase4.rs` replay criterion actually
proves. It happens only when the world may have changed; the dirty-flag and latest-wins staging
rules belong to `ludion devdocs frame-streaming`. One latch is engine-side: a missing GPU adapter
(`L2003`), a lost device (`L2011`) or a dead window surface (`L2007`) **latches** `gpu_unavailable`
until a *new* `frames` subscription opens (via `Subscriptions::frames_opened`'s epoch counter), so
an adapterless host does not re-request the adapter every pass; a new subscription also clears
the logged-error latch, so a reconnecting client is told why the stream is dead. The last two are
also reported once to an armed host (`Engine::arm_render_fault_notify`), which ends a windowed or
headless game; the sim never ends itself (`Engine::queue_render_fault` is the test seam that raises
one). Every failure but a missing adapter becomes a deduplicated error-level `Event::Log` on the
`log` channel — logged once per distinct failure text, reset by a success or a *different* failure —
so a frozen frame stream says why instead of going quiet.

**A panicking game system auto-pauses instead of killing the host** (`L4007`). `ludion-ecs` owns the
unwind boundary that leaves a `SystemPanic` resource in the world; this crate owns the drain, and its
**ordering is the load-bearing part**. `drain_system_panic` is the *first* thing
`advance_live`/`advance_observed` do after a tick: it takes the resource, renders `L4007` (system,
`file:line:column`, frame, payload), pushes an `Error` `log` event, stores it for
`take_system_panic`, and pauses the driver — then **skips the other frame-boundary drains
deliberately**, because those read a world the panic left half-written, `drain_scene_switch` would
rebuild it outright, and the recorder would keyframe it. The world is left exactly as the panic left
it, because that shape is the evidence. Three discovery paths, one report: `sim.step` answers with the
diagnostic rather than an Ack (`take_system_panic` is single-taker, so a later step never re-reports a
stale panic), a `log` subscriber gets the event, and the frame stamp stops advancing. Everything that
does not tick still answers.

**A panicking command handler answers `L4016` and the loop keeps serving.** `reply()` wraps
`engine.execute` in `catch_unwind`; the same policy as `L4007` applies, via `Engine::command_panicked`:
the driver pauses, the world stays as the panic left it, the command is not recorded, and the stashed
`pending_doc_reload` is dropped so it cannot be recorded against the next command. The panic location
is on the host's stderr (the ECS hook only swallows panics inside a system). A panic *outside* both
boundaries ends the sim thread; the hosts report it as `L4017` and exit non-zero
(`ludion-server`'s teardown, `Embedded::stop`).

**Egress** is `live`'s outbound mirror: two `crossbeam_channel`s a transport hands to `run_live`. Each
pass, `emit_egress` drains `Engine::take_events()` into one `EgressEvent` per open subscriber on its
`Channel`, stamped with that subscription's own monotonic `Seq` — `event_channel` is the total,
wildcard-free `Event` → `Channel` mapping, so a new event variant forces a routing decision the same
way a new `Command` forces a dispatch handler — then renders **at most one** `EgressFrame`: raw RGBA
plus the list of `frames` subscribers it should reach, never encoded here (encoding happens in the
server). The two lanes' capacities and what happens to a message next are
`ludion devdocs frame-streaming`.

## Rules

- **The `live` seam is tokio-free by construction:** two `crossbeam_channel`s — a multi-producer
  intake of `StampedCommand`s and a capacity-1 one-shot reply per command. That is the entire surface
  `ludion-server` needs.
- **A diagnostic's latch is `(code, key)`, once per session**, so a check running every tick prints
  one line while one code still prints once per offender (`key` = a doc name, a scene name, a slot
  index).
- **Diagnostics are drained in `drain_frame_boundary`**, after the system-panic drain and before
  `drain_scene_switch`: each report becomes one `Event::Log` built by `LogEvent::from_diagnostic` (the
  severity's level, `message` the `Display` rendering, the whole diagnostic in `LogEvent.diagnostic`)
  **and** one `eprintln!` of the same `Display`, so a host with no client still sees it.
- **A diagnostic is never hashed, never serialized** — outside the component registry, so it reaches
  neither `WorldHash` nor a snapshot; reporting cannot move a replay.
- **The latch travels, the queue does not:** `carry_diagnostics` takes it before the driver swap and
  `install_diagnostics` hands it to the replacing world, so a gate crossing stays quiet.

## Gotchas

- `stream.subscribe`/`unsubscribe` manage only the subscription *table* (ids, channel, mode, seq
  counters) — the engine emits `Egress` messages on an optional channel a transport supplies, but
  turning those into wire bytes and fanning them out to connections is entirely `ludion-server`'s
  job (`delivery.rs`); the engine itself never touches a socket. Calling `run_live` with `egress:
  None` (as `dispatch`-only callers and most tests do) means subscriptions open in the table but
  nothing ever renders or is emitted for them.
- `latest_wins` is the only subscribe mode, so the subscription table never opens a subscription
  nothing could deliver to.

## Verify

```
cargo test -p ludion-engine                      # dispatch, extract, transitions — the drains included
cargo test -p ludion-server --test phase4        # an attached viewer cannot perturb a replay
cargo clippy -p ludion-engine -- -D warnings     # the determinism denylist, `run_live` its one exception
```
