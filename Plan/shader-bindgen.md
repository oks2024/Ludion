# Plan: WGSL Shader Bindgen — reflection-generated CPU bindings

> Status: **waves 1–6 + the decision-16 snippet dedup landed 2026-07-17** (Lore revs 220–226;
> Graphics Engine feature). Open: the Wave 5 **human
> gate** (live shader-tuning session against material-park).
> Outcomes/amendments recorded in the "Wave N outcomes" sections below. Snippet dedup outcome:
> `shaders/snippets/{camera,lights,fullscreen_vs}.wgsl` composed per the two mirrored registries
> (`ShaderId::snippets` in the renderer — authoritative, compile-time — and bindgen's
> `COMPOSITIONS`), reconciled by a byte-comparing renderer test (the two cannot physically merge:
> the renderer needs compile-time `include_str!` and neither crate may depend on the other without
> dragging wgpu into the drift gate or the generator into the shipped renderer). text.wgsl's lone
> `Camera` copy eliminated; render-graph's `gradient`/`vignette` keep their fullscreen VS copies
> (crate-graph scope, Wave 4 amendment). Snippet hot reload is transactional: every dependent
> pipeline rebuilds before any swap; any failure keeps all last-good.
>
> Originally: authored 2026-07-17, ready to start. Direction settled with the user 2026-07-17: shaders are the
> source of truth and the CPU-side bindings derive from them — the user's preferred workflow
> (precedent: DungeonEngine's ShaderGenerator, which text-parses annotated HLSL into C++), done
> the strictly better way the Rust/WGSL ecosystem allows: **real reflection via naga** (offsets
> computed, not inferred from text; no annotations; no hand-authored padding) with **checked-in
> generated Rust behind a CI drift gate** (the established `editor/protocol` pattern). In-house
> generator over `wgsl_bindgen`/`wgsl_to_wgpu` — user decision, for control of the generated
> shape, error quality, canonical/deterministic output, and freedom from third-party churn.
> The timing argument: today's hand-kept surface is ~11 structs / 9 BGLs in one file — cheap to
> migrate; the PBR arc (textures, IBL, shadows) multiplies it. Put the seam in before the growth.
> **Grilled 2026-07-17** (5 branches): feature goal confirmed (PBR arc, bindgen first as the
> seam); **shader hot reload pulled into scope** as Wave 5 (was excluded — decisions 10–11);
> `check.mjs --fix` regenerates (decision 12); vertex structs stay assertions-only with a
> recorded revisit trigger; wave order = bindgen 1–4, reload 5, docs 6.
> **Amended 2026-07-17 for Lore rev 211** (audit-remediation landed while this plan was
> pre-start): mesh/sprite sources are now *composed* (`lit_shader_src` prepends `LIGHT_COUNT`
> from Rust), the lights BGL already has a real `min_binding_size`, and `MeshInstanceRaw` grew a
> normal matrix (96→160 B, instance locations 10–13). Facts below corrected; design impact in
> decisions 15–16. Line references elsewhere in this doc predate rev 211 — trust names over
> numbers.

## Current-code facts that shape the work

- **11 WGSL files, all `include_str!`, no runtime loading, no hot reload.** Renderer:
  `mesh/sprite/text/threshold/blur/tonemap/blit/ui_rect/ui_text.wgsl`
  (`crates/ludion-renderer/src/shaders/`); render-graph proving pipeline: `gradient/vignette.wgsl`.
  *(rev 211)* Nine ship as raw source; **mesh and sprite are composed**: `lit_shader_src()`
  (`render.rs:1526`) prepends `const LIGHT_COUNT: u32 = {MAX_LIGHTS}u;` at pipeline build, and
  both shaders declare `array<Light, LIGHT_COUNT>`. Any reflection pass must reflect the same
  composed string the pipeline sees, not the raw file.
- **~11 hand-written POD upload structs, manual padding, comments as the only sync mechanism.**
  `#[repr(C)]` + `bytemuck::{Pod, Zeroable}` at `render.rs:39–123` (`CameraRaw`, `LightRaw`,
  `LightsRaw`, `MeshInstanceRaw`, `SpriteInstanceRaw`, `TextVertexRaw`, `BlurParams`,
  `UiUniformRaw`, `UiRectInstanceRaw`, `UiTextVertexRaw`) plus `MeshVertex` (`mesh.rs:15`).
  Padding is hand-spelled on both sides (`LightsRaw.pad: [u32; 3]` ↔ `mesh.wgsl`'s
  `pad0/pad1/pad2`); agreement is comments like `// Matches ui_rect.wgsl's VsIn`.
- **The `Camera`/`Lights` struct layouts are copy-pasted between `mesh.wgsl` and `sprite.wgsl`**
  (Phase 11 plan already flags it: "those structs must not change shape or the sprite path
  re-blesses too"). Nothing enforces the two copies agree — it is convention only.
- **`MAX_LIGHTS` is already single-sourced — but in the Rust→WGSL direction.** *(rev 211)* The
  audit closed the duplicate-16 drift class via the `lit_shader_src` injection above. This plan
  single-sources in the opposite direction (WGSL is the source of truth); decision 15 resolves
  the collision.
- **7 cached BGLs + 9 pipelines in the renderer** (`render.rs:158–174`), built by four thin
  helpers; 2 more BGLs in `execute.rs`. **BGL entries have `min_binding_size: None`, except the
  lights BGL** *(rev 211)*: it now pins `size_of::<LightsRaw>()` (`render.rs:1373`) so an
  array-size disagreement fails pipeline creation loudly. The remaining entries still defer the
  buffer-size cross-check to draw time — Wave 4 finishes what the audit started.
- **No reflection, no codegen, no layout tests.** Zero hits for
  `naga|wgsl_bindgen|encase|crevice|offset_of` in both renderer crates; no `build.rs`. The one
  layout assertion is the mesh-vertex test (`mesh_vertex_layout_is_position_normal_color`,
  render.rs:~1414) — vertex format only, nothing for uniforms. The indirect net is the pinned-GPU
  goldens: a layout mismatch corrupts the image, surfacing as a garbled SSIM diff, not as
  "field `ambient` is at offset 16 on the CPU and 32 in the shader".
- **naga 30.0.0 is already in `Cargo.lock`** (transitive via `wgpu = "30"`). naga and wgpu
  release in lockstep from the same repo — pinning `naga = "30"` as a direct dependency adds no
  new upgrade cadence. Its `front::wgsl` parser + `proc::Layouter` are the exact code wgpu runs
  at `create_shader_module`, so reflected offsets are by construction the ones the GPU sees.
  Per-entry-point resource usage (`valid::ModuleInfo` → `GlobalUse`) gives stage visibility.
- **The drift-gate pattern already exists twice.** `editor/protocol` TS types: regenerate,
  compare, restore committed bytes — `schemaDrift()` in `scripts/check.mjs:253`, mirrored by
  `ci/teamcity/schema-drift.ps1`. Embedded CLI docs: drift-gated against `docs/`. This plan adds
  a third instance of the same shape, not a new mechanism.
- **Vertex-buffer structs are not uniform-layout structs.** `MeshInstanceRaw` & co. are tightly
  packed vertex data; their packing, the two-buffer split (per-vertex locations 0–2,
  per-instance 3–7, `render.rs:1504–1506`), and step modes are CPU-side decisions WGSL does not
  express. Only the `@location` → format mapping is reflectable.

## Design decisions (settled by this plan)

1. **In-house generator on naga; no `wgsl_bindgen`.** New internal workspace crate
   **`crates/ludion-shader-bindgen`** (lib = reflection model + emitter, thin bin), `publish =
   false`, excluded from the Phase 9 packaging gate. Run via
   `cargo run -p ludion-shader-bindgen` — **not** a `ludion` CLI subcommand: the CLI is the
   game-author surface and games have no custom-shader surface. Trigger to promote: the day
   games can author shaders.
2. **Checked-in generated code + drift gate, not `build.rs`.** Generated Rust is canonical
   committed text (pillar: text is the single source of truth; a shader edit produces a
   reviewable diff on both sides). `check.mjs` gains a `shader-drift` step in the Rust lane —
   snapshot committed bytes, regenerate, compare, restore, exactly the `schemaDrift()` shape —
   with a matching `ci/teamcity/` script and `.github/workflows/ci.yml` reference entry.
   Consequence worth naming: because output is checked in, **consumers carry no dependency on
   the generator** — only the drift gate runs it.
3. **Generated surface (what the tool emits into `crates/ludion-renderer/src/shader_gen.rs`):**
   - Uniform/storage struct PODs: `#[repr(C)]` + `Pod`/`Zeroable`, explicit pad fields wherever
     the Layouter reports a gap, `const _:` size/offset assertions baked alongside.
   - Consts hoisted from WGSL: array bounds (`MAX_LIGHTS` becomes generated-from-`array<Light,
     16>`; the Rust const at `render.rs:28` retires), `@workgroup_size` when compute arrives.
   - Binding indices: per-shader `const` group/binding numbers replacing magic `0`/`1`s.
   - BGL descriptors: generated `BindGroupLayoutDescriptor` construction with **visibility
     derived from per-entry-point usage** and **`min_binding_size: Some(reflected size)`** — an
     upgrade from today's `None`, moving buffer-size errors from draw time to bind time.
   - Vertex entry-point assertions: generated tests that each pipeline's `@location`/format list
     matches the reflected entry-point inputs. The structs, buffer split, and step modes stay
     hand-written (fact above: WGSL doesn't express them).
4. **Same-named struct in two shaders must have an identical layout — hard error.** The
   generator unifies structs by name across all reflected files and refuses to emit on
   disagreement. Today's copy-paste convention (`Camera`/`Lights` in mesh + sprite) becomes a
   compile-adjacent gate instead of a re-bless surprise.
5. **Names verbatim from WGSL**, namespaced by the generated module (`shader_gen::Lights`). The
   `*Raw` suffix convention retires with the hand-written structs — the name in the shader is
   the name in Rust, one obvious way.
6. **The reflection model in `ludion-shader-bindgen` is wgpu-free** (plain structs: fields,
   offsets, binding kinds, stages); only the emitted code in the renderer names wgpu types. This
   respects the ash-migration seam the architecture doc keeps open — a backend switch rewrites
   the emitter, not the reflector.
7. **Canonical, deterministic output**: stable ordering (file order = sorted shader paths, decl
   order within), rustfmt-clean as emitted, `// GENERATED by ludion-shader-bindgen — edit the
   WGSL, then regenerate` header. Byte-stable output is what makes the drift gate exact.
8. **Plain error messages, no L-codes.** The generator is engine-internal dev tooling, not a
   shipped diagnostic surface; its errors target the agent editing WGSL ("struct `Lights` is
   defined in mesh.wgsl and sprite.wgsl with different layouts: field `ambient` at offset 16 vs
   32"). Trigger to register L-codes: promotion to a `ludion` subcommand (decision 1).
9. **Render-graph proving shaders (`gradient`/`vignette`) are in scope** — no dep-edge cost
   (decision 2) and it keeps the rule regular: every WGSL file in the workspace is reflected.
10. **Shader hot reload ships in this plan (grilling reversal of the original exclusion), Wave
    5.** Mechanism is protocol-first: shaders become reloadable assets under a dev flag on the
    host; the existing **`asset.reload`** command accepts shader paths (no wire-surface growth —
    stays 26), the renderer recreates module + dependent pipelines, and a naga validation
    failure returns a structured error while **keeping the last good pipeline** (a bad edit
    never kills the session). `ludion dev` gets a fast path: a WGSL-only change skips
    rebuild+restart and sends `asset.reload` instead — sub-second edit→pixels instead of a
    cascading `ludion-renderer` rebuild. Without the dev flag, `asset.reload` on a shader path
    is a structured error (code chosen at implementation, existing asset-reload family).
11. **The shipping and test path stays `include_str!`.** Goldens, `ludion render`, and every
    test compile shaders in; only the dev-flagged host reads disk. No divergence risk: the disk
    files ARE the `include_str!` sources in-repo. Reloaded shader state is render-side only —
    no sim state, no WorldHash interaction, nothing for snapshot/restore to carry.
12. **`check.mjs --fix` regenerates `shader_gen.rs`** (before the Rust lane, so the build sees
    fresh bindings); plain `check.mjs` stays fail-only with a one-line fix hint; CI is
    compare-only and never mutates. Same contract as fmt: the gate proves freshness, `--fix`
    restores it.
13. **Bind group layouts unify across shaders the way structs do (decision 4).** Groups with
    the same name and shape reflected from multiple shaders emit **one** BGL — preserving
    today's sharing (`camera_bgl` serves mesh/sprite/text pipelines from one bind group) rather
    than relying on wgpu's structural bind-group compatibility; same-name-different-shape is
    the same hard error as for structs.
14. **Wave 2's assertions are scaffolding, not a permanent surface.** Once Wave 3's generated
    structs carry their own baked size/offset asserts, the Wave 2 test retires with the
    hand-written structs (second-way rule).
15. **The `lit_shader_src` `LIGHT_COUNT` injection retires in Wave 3** (rev 211 amendment). Two
    single-sourcing mechanisms for the same constant is exactly the second-way rule's target.
    Under this plan's premise the WGSL declares the literal (`array<Light, 16>`) and bindgen
    generates the Rust `MAX_LIGHTS` from it — same loudness (generated consts + Wave 4's
    `min_binding_size` come from the same reflection), one fewer mechanism, and mesh/sprite go
    back to being raw reflectable sources. Until Wave 3 lands, the Wave 1 reflector must accept
    a compose-then-reflect input (a preamble prepend), which is also the seam a future shared
    `common.wgsl` would use.
16. **Inherited from the engine audit §4 (rev 211 handoff, ownership accepted):** dedupe the
    fullscreen-triangle VS (copied in 5+ post shaders) and the `Camera`/`Light` struct copies
    (mesh vs sprite). Decision 4's unification hard error is the immediate guard against
    divergence; whether full dedup lands via shared-source composition (a `common.wgsl`
    prepended through the decision 15 seam) or stays error-guarded copies is a Wave 1 design
    output — composition must not break per-file reflection or the Wave 5 reload unit
    (a shared-snippet edit must invalidate every dependent pipeline).

### Wave 1 outcomes (2026-07-17)

- **Binding unification key = the WGSL global resource variable name** (decision 13 made concrete;
  WGSL groups are anonymous). Same-named resources must agree on group, binding, and kind — hard
  `BindingConflict` — while visibility merges across units (mesh's fragment stage reads
  `camera.cam_pos`, so the shared `camera` BGL is Vertex|Fragment). Not keyed by `(group, binding)`:
  different pipelines legitimately reuse a slot (`lights` and `atlas_tex` both live at group 1
  binding 0). Only **buffer-backed** structs unify — vertex I/O structs (`VsIn`/`VsOut`) differ per
  shader by design and are excluded from unification.
- **Decision 16 mechanism = shared-source composition** (`snippets/` prepended through the
  preamble seam), **staged after Wave 3**: the `Camera`/`Light`/`Lights` copies move into a shared
  snippet once WGSL declares `array<Light, 16>` (they belong with the count); the fullscreen VS is
  a lower-value follow-up (no bindings — divergence is behavioral only, goldens catch it). Deciding
  factor is the Wave 5 reload unit: a snippet edit changes every dependent composed string, so
  content-hash-keyed pipeline recreation invalidates all dependents for free — error-guarded
  copies give no such propagation. Decision 4's hard error stays as the interim + permanent guard.

### Wave 4 outcomes (2026-07-17)

- **BGL identity = the set of member resource-variable names sharing a `@group`.** Nine generated
  layouts; structurally-identical texture+sampler pairs stay separate by name (sprite/atlas/rect),
  while `src_tex`/`src_sampler` unifies across blit/threshold/vignette and blur's
  `{src_tex, src_sampler, params}` triple separates naturally as a different name-set. Generated
  names join member names in binding order (`create_src_tex_src_sampler_bgl`).
- **Render-graph's two BGLs are NOT generated — amendment to this wave's "included" wording,
  structural:** `ludion-render-graph` sits below `ludion-renderer` in the crate graph and cannot
  consume its `shader_gen.rs`; a second generated file would fork the single-reflection model and
  duplicate the shared `src` BGL across crates against decision 13. `gradient`/`vignette` remain
  reflected + drift-gated (decision 9 intact); vignette's bindings unify into the renderer's `src`
  layout provenance; `execute.rs`'s hand BGL stays.
- **Vertex assertions = generated data + hand comparison:** the emitter writes
  `*_VS_MAIN_INPUTS: &[(u32, wgpu::VertexFormat)]` from the WGSL; one hand-written renderer test
  pins each pipeline's `vertex_attr_array!` literals to them. All Wave 2 scaffolding retired
  (decision 14 complete); the renderer's bindgen dev-dep dropped with it.

## Waves

1. **Reflection core.** `ludion-shader-bindgen` crate: naga `wgsl-in` parse + validate,
   Layouter-driven layout extraction, per-entry-point binding usage → the wgpu-free model.
   Input is compose-then-reflect (decision 15): a shader unit = optional preamble + file, so
   the reflector sees exactly what `create_shader_module` sees. Unit tests against fixture WGSL
   covering the layout corners we actually have (trailing scalar + pad, `array<T, N>` stride,
   nested struct, texture/sampler bindings) plus the same-name-different-layout error. Design
   output due this wave: the audit §4 dedup mechanism (decision 16).
2. **Validation before generation.** Emit (or hand-wire) assertions of the *existing*
   hand-written structs against reflected layouts — size, every field offset, `MAX_LIGHTS`,
   binding indices. Land it as a renderer test. Any drift it finds gets fixed here, in
   isolation, before generation moves anything. Exit: suite green, goldens untouched.
3. **Struct generation flips on.** Emit `shader_gen.rs`; `render.rs` switches to the generated
   structs; hand-written PODs, the Rust `MAX_LIGHTS`, and the `lit_shader_src` `LIGHT_COUNT`
   injection are deleted (second-way rule; decision 15 — mesh/sprite return to raw sources
   declaring `array<Light, 16>`, and the generated `MAX_LIGHTS` comes from reflection). Wire the
   `shader-drift` step into `check.mjs` + `ci/teamcity/` + the CI reference yaml. Exit:
   `check.mjs --full` green, **all 13 goldens byte-identical**, drift gate demonstrably fails on
   an un-regenerated WGSL edit.
4. **Binding/BGL generation.** Generated binding-index consts and BGL descriptors (derived
   visibility, `Some(min_binding_size)`) replace the four hand helpers where they fully cover;
   render-graph's two BGLs included. The lights BGL already pins its size (rev 211) — this wave
   generalizes that to every buffer binding and moves the pin into generated code. Exit: suite
   green, goldens byte-identical.
5. **Shader hot reload** (decisions 10–11). Dev flag on the host, shader paths through
   `asset.reload`, module + pipeline recreation keyed off the existing hash-keyed GPU-cache
   pattern, keep-last-good on validation failure, `ludion dev` WGSL fast path. Exit: mechanized
   — a protocol test reloads an edited shader and asserts the new pipeline renders (screenshot
   endpoint), a bad-WGSL reload returns the structured error with the session still live, and
   the dev fast path's edit→pixels latency is measured and recorded (target: well under the
   3.1 s rebuild path). Human gate: one live shader-tuning session against the material park.
6. **Docs + bookkeeping.** Agent-docs topic for the generator crate (`ludion-docs` skill;
   crate doc binds it), a "shader authoring" note in `docs/ludion-renderer.md` stating the new
   rule — *edit WGSL, regenerate, never edit `shader_gen.rs`* — plus the hot-reload dev flow in
   `docs/designer-flow.md`; roadmap/notebook updates.

## Explicitly out of scope (triggers recorded)

- **Shader permutations / defines.** WGSL has no preprocessor and we have no permutation need;
  today's variation is separate files. Trigger: first real permutation demand (likely textured
  PBR's feature combinations). The DungeonEngine bitflag-enum scheme is the reference design.
- **Game-authored shaders.** No such surface exists; decision 1 records the promotion trigger.
- **Pipeline/render-pass generation.** The frame graph owns pass structure; bindgen stops at
  structs + bindings + BGLs.
- **Vertex-buffer struct generation.** Reflectable only partially (fact above); assertions only.
  Trigger to revisit (grilling): the first PBR vertex-format change (UVs, tangents touch the
  four writer sites) — if the hand-edit feels error-prone despite the assertions, add the
  buffer-split manifest then.

## Risks

- **Not everything is derivable from WGSL.** Sampler filterability / texture sample-type edge
  cases (e.g. non-filterable float) and dynamic offsets aren't in the shader text. Default to
  filterable-float/filtering-sampler (matches every current binding); add a per-binding override
  in the generator's input manifest **only when a real case appears** — no preemptive config.
- **Generated code vs the fmt/clippy gates.** Emitter output must hold rustfmt-clean and
  clippy-clean under the workspace lints (including the sim-side denylists if paths ever
  overlap — they shouldn't; renderer is not sim-side). Wave 3's exit gate covers it.
- **wgpu major upgrades now touch the generator** (naga API churn is real pre-/post-major).
  Accepted: naga moves in lockstep with wgpu, and the upgrade PR regenerates + re-gates in the
  same commit; the drift gate makes a mismatch loud, not silent.
- **Hot reload touches pipeline lifetime.** Recreating pipelines mid-session must not disturb
  in-flight frames or the GPU caches keyed by content hash; the dev flag confines the risk to
  dev hosts, and the keep-last-good rule bounds the failure mode to "edit didn't take, error
  says why". Watch for BGL identity assumptions when Wave 4's generated BGLs land under it.
- **Wave 2 may surface live drift** (an offset that disagrees today). That's the point — but it
  could force a golden re-bless if the fix changes rendered output. Budget for one bless event,
  evidence-file discipline as in Phase 11.
