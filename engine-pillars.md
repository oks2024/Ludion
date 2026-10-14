# LLM-First Game Engine — Project Pillars

> A 3D engine written in Rust, designed so that an agentic AI (Claude Code) can do **everything**, through an interface optimized for it **first** — and humans (level designers, game designers) are first-class users of the very same core. HD-2D/stylized content on a modern renderer at native resolution.

---

## North Star

**Metric that everything is scored against:** how many autonomous iterations can an agent perform — change, verify, correct — before a human needs to look at the screen?

Human tooling (editor, viewer, designer workflows) is a supported, welcome use case — but it is never the optimization target. When agent ergonomics and human ergonomics conflict, agent ergonomics win. The shared command core (Pillar 2) is what lets human tools ride along without ever competing with the agent for design priority.

**MVP forcing function:** Claude Code, given a one-line prompt ("make a Breakout clone with dynamic lighting"), goes from `engine new` to a passing visual test suite with zero human intervention — **and** a human can attach the live viewer to the running game and watch, over the same protocol.

---

## Pillar 1 — The Agent Iteration Loop Is the Product

- The renderer is (almost) a commodity; the moat is the feedback loop between "agent makes a change" and "agent knows whether it worked."
- Compile-time feedback > runtime feedback > visual feedback. Push invariants as far left as possible.
- Every vague panic, flaky test, or silent failure costs agent iterations. Treat them as P0 bugs.
- `cargo check` is the primary feedback signal — keep it fast (workspace splits, stable engine core, consider dynamic linking for the dev loop).

## Pillar 2 — One Command Core, Many Clients

- Every engine capability exists as a typed, serializable command/query on a single canonical operation layer. The CLI, the MCP server, and the visual editor are all thin **peer clients** of that layer — they literally call the same functions.
- The engine always hosts a wire protocol (local server, day one): a native, versioned, JSON-RPC-style protocol with typed commands, queries, event subscriptions, and streaming channels. An MCP adapter translates it for Claude Code; the editor speaks it directly.
- **Hard parity gate:** a capability may not ship in the editor unless it already exists as a protocol command. No editor-private endpoints, ever. Corollary: any editor session is replayable as a command script.
- Core CLI commands remain: `ludion new`, `ludion check-scene`, `ludion render --scene x.yaml --out frame.png`, `ludion test`, `ludion profile --json`.
- Renderer introspection as structured output: `--dump-graph` (frame-graph DAG, resource lifetimes, barriers as JSON/Graphviz), `--dump-timings` (per-pass GPU profiling as JSON).

## Pillar 3 — Verifiable Output as a First-Class Primitive

- Headless, deterministic rendering to an image file is a core engine feature, not a test utility.
- **Tiered verification:**
  - CPU-side is bit-exact and non-negotiable: fixed timestep, seeded RNG, reproducible sim state, state dumps on failure.
  - Images gate on perceptual comparison (SSIM + per-pixel tolerance) with a structured diff report — *where* and *how much* — as the default; bit-exact goldens only against pinned GPUs (dev box + one CI GPU).
  - Test mode disables temporal accumulation and any nondeterministic post effects.
- Headless gameplay simulation: scripted inputs ("press jump at frame 12"), assertions on state ("player.y > 3.0 by frame 30").
- Flaky tests are poisonous — agents will chase phantom failures.
- **Humans and agents see through the same eyes:** the engine streams rendered frames over the protocol. The viewer's viewport and the agent's screenshot query are the *same endpoint*. Rendering logic exists in exactly one process.

## Pillar 4 — Text-First Everything

- Scenes, materials, pipelines, palettes, tilemaps: authored in structured text (YAML/JSON/TOML), never binary blobs or GUI-only formats.
- **Text files are the single source of truth — for every client.** When the editor moves an entity, the command mutates an in-memory document that serializes back to canonical text: stable field order, stable entity ordering, fixed float precision. A gizmo drag produces the same minimal git diff a hand edit would.
- Assets can be generated programmatically — an agent can write a tilemap, define a palette in hex, emit a quad mesh in code. Defer glTF import until the engine works.
- Everything the engine *knows* is exportable as text; everything the engine *does* is invokable as a command.

## Pillar 5 — Errors Are the UX Budget

- Target rustc's bar: what's wrong, where, why, and the likely fix.
- Asset/scene validation with suggestions: *"Material references 'brick_d.png' which doesn't exist; nearest match is 'brick_diff.png'."*
- Validation errors as readable messages, not crashes — a key reason to stay on wgpu.
- Protocol errors follow the same bar: every failed command returns a structured, actionable error to whichever client sent it — agent and editor alike.

## Pillar 6 — Regularity Over Cleverness

- One obvious way per task, uniformly applied. Agents thrive on predictable patterns.
- ECS regularity: define component → write system → register. Everything the same shape.
- **Rust is the only Turing-complete layer.** Game designers "script" by describing behavior to the agent, which writes hot-reloadable Rust (script crates / dynamic linking); tunables live in declarative text assets. No embedded scripting language (Rhai/Lua) — it would double the API surface, weaken error feedback, and split every feature into two idioms. Revisit only if compile latency proves fatal to designer flow.
- Avoid: macro-heavy DSLs, deeply generic builder chains, "four ways to do the same thing."
- Encode invariants in the type system (typestate: "render pass must end before submit"). Every invariant moved from docs into types is an instant compile-time message to the agent.

## Pillar 7 — Docs Are the Prompt

- A new engine has zero representation in model training data — agent-legibility is the survival condition, not a nice-to-have.
- Large gallery of small, complete, runnable examples — one feature each. Highest-leverage documentation format for agents.
- Excellent rustdoc on the public API.
- Machine-oriented reference: structured schema of components, systems, asset formats, and **protocol commands** an agent can load into context.
- Ship a `CLAUDE.md`-style engine guide with the engine itself.

---

## Architecture Decisions (settled)

| Decision | Choice | Rationale |
|---|---|---|
| Language | Rust, all the way down | The compiler feedback loop is the whole point; a C++ FFI renderer would put the most agent-hostile code in the most agent-critical subsystem |
| Graphics API | wgpu — settled, but held loosely: the frame graph's declarative interface is the seam for a later switch to ash if experiments outgrow it | Safe, validation-as-messages (agents read errors as text, not device-lost), well documented, covers everything the content style needs; native Metal backend on macOS (raw Vulkan would mean MoltenVK translation) |
| Platforms | Windows + macOS, both first-class from day one | Consequences accepted: two pinned GPU targets for the bit-exact golden tier, CI matrix on both OSes, Tauri builds for both |
| Renderer spine | Frame graph from day one, introspectable | Pass scheduling, transient aliasing, barrier placement — the interesting 3D work — validated with cheap content |
| Content style | HD-2D / stylized: sprites, billboards, low-poly on a full 3D pipeline, rendered at native resolution (≥1080p) with a modern post stack | Exercises the renderer without an asset pipeline; the style constraint keeps scope finite, the resolution does not carry it. A stylistic default, not an engine ceiling — the renderer grows toward full PBR by priority (`Plan/roadmap-voxel-mesh-pbr.md`) |
| Command core | All capability as typed serializable commands; engine hosts a native wire protocol (local server) from day one; MCP adapter for Claude Code | One layer, many peer clients; parity between agent and editor by construction |
| Editor | Out-of-process client of the protocol. Read-only live viewer (scene view, inspector, hot reload) first; editing tools after the command core proves itself | Level designers see agent work early; editing never outruns the command layer |
| Viewport | Frame streaming over the protocol (raw/lightly compressed on localhost) | Human viewport = agent screenshot endpoint; one renderer, one truth. Revisit shared-texture handles only if editing latency hurts |
| Scripting | Rust-only, hot-reloadable script crates; agent acts as the designer's compiler; declarative text for tunables | Preserves compile-time feedback as the moat; avoids a second, worse-documented API surface |
| Structure | Workspace of small, orthogonal crates | Agents work best when the relevant API surface fits in context; also protects compile times |
| Scene/data model | ECS + canonical text scene format (stable ordering/formatting, minimal diffs from any client) | Regularity + agent-writable, git-friendly content |

---

## Scoping Guardrails

- **Vertical slice before general systems.** Classic failure mode: six months on a beautiful render graph with no game on top.
- First milestone: *agent-built Breakout/Sokoban with dynamic lighting, end-to-end, verified by golden tests — plus a human attaching the live viewer to the running game over the wire protocol.* Touches windowing, input, ECS, frame graph, post stack, text rendering, test harness, protocol, and frame streaming — brutally finite, and the command core gets a second client from day one.
- Every renderer feature must justify itself by appearing in the current slice.
- Defer: editing tools (gizmos, in-viewer manipulation — viewer itself is *not* deferred), glTF/asset import pipeline, audio (beyond minimal), networking, embedded scripting language (deferred indefinitely, see Pillar 6), exotic rendering features (mesh shaders, RT).

## Anti-Goals

- Not a general-purpose human-first engine competing with Bevy/Godot on features.
- Not a frontier renderer showcase — rendering experiments serve the agent story, not the reverse.
- **No GUI-exclusive capabilities.** A feature that exists only in the editor does not exist. Anything a human can do through the editor, an agent can do through the protocol — by construction, not by policy review.
- No second-class human tools either: the editor is a real product for level and game designers, but it is a *client*, never the source of truth.
- No flexibility for its own sake.

---

## Success Checks (revisit regularly)

1. Can the agent detect its own rendering mistakes without a human looking at the screen?
2. Is every failure mode expressible as text the agent can act on?
3. Could Claude Code build the current milestone game from a one-line prompt?
4. Is `cargo check` still fast enough for a 20-iteration fix loop?
5. Would a fresh agent, given only the docs + examples in context, use the API correctly on the first try?
6. Is every editor capability reachable as a protocol command — could yesterday's editor session be replayed as a command script?
7. Can a level designer attach the viewer and watch an agent's change live?
8. Did the last editor session produce clean, minimal git diffs in the scene files?
