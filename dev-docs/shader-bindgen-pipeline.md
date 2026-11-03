---
description: shader-bindgen internals: reflect then emit, the drift gate, what is deliberately not generated
---
# shader-bindgen pipeline

**Scope:** The flow from WGSL to `shader_gen.rs`. Layout, unification rules and gotchas are `ludion devdocs ludion-shader-bindgen`.

## Dataflow
`main.rs` reads the fixed, sorted list of renderer + render-graph `.wgsl` files into `ShaderUnit`s —
composing each shader's snippets (per `snippets_for`) ahead of its body through `with_preamble` —
calls `reflect(&units)`, and passes the resulting `Reflection` to `emit`. `reflect` parses and
naga-validates each unit's composed source, runs `naga::proc::Layouter` to get real memory offsets
(the exact code `wgpu::Device::create_shader_module` runs, so a reflected offset is by
construction the one the GPU sees), walks each entry point's `naga::valid::ModuleInfo` to derive
per-resource stage visibility (`GlobalUse`), then folds every unit's structs/bindings/consts into
one `Reflection`, erroring on the first disagreement (Rules in `ludion devdocs ludion-shader-bindgen`). `emit` walks that `Reflection`
in sorted order and writes: hoisted scalar consts (e.g. the cluster grid's `CLUSTER_TILES_X`, taken
from WGSL's own `const` — a shader constant's source of truth is the WGSL, never a Rust constant
injected into the shader text, per decision 15), `#[repr(C)]` `Pod`/`Zeroable` structs with a
`const _: () = assert!(...)` per size and per field offset (a layout slip becomes a compile error
at the use site, never a corrupted pixel), per-resource `@group`/`@binding` consts, `create_*_bgl`
functions (derived visibility, `min_binding_size: Some(reflected size)` — an upgrade from the old
hand-written `None`), and `*_VS_MAIN_INPUTS: &[(u32, wgpu::VertexFormat)]` per vertex entry point
(compared against each pipeline's hand-written `vertex_attr_array!` by one renderer test — vertex
*buffer* structs, the two-buffer per-vertex/per-instance split, and step modes stay hand-written;
see "What is NOT generated" below).

`main.rs` in default mode writes `crates/ludion-renderer/src/shader_gen.rs` only when the emitted
bytes differ from the committed file; `--check` never writes, only compares and exits nonzero on a
mismatch. `scripts/check.mjs`'s `shader-drift` step (Rust lane) runs the `--check` form; `--fix`
runs the regenerating form first so the Rust lane then builds against fresh bindings — the same
contract as `cargo fmt`/`cargo fmt --check`. `ci/scripts/shader-drift.mjs` (the one copy; `check.mjs`, the
`.github/workflows/ci.yml` entry and TeamCity all call it) runs the same `--check` invocation; CI never mutates
(decision 12).

**What is NOT generated**, and why: vertex-buffer structs (`MeshInstanceRaw` and siblings) are
tightly packed CPU-side vertex data whose two-buffer split (per-vertex vs. per-instance locations)
and step modes WGSL does not express — only the `@location`→format mapping is reflectable, which
is why those become assertions, not generated structs. Textured PBR fired this trigger: `MeshVertex`
grew `uv` and `tangent` lanes (six lanes, locations 0–5, instance lanes following at 6–17) and stayed
hand-written, exactly as before; the `*_VS_MAIN_INPUTS` assertions caught the widened layout at the
four writer sites rather than any structural change to this crate. `ludion-render-graph`'s three
shaders (`gradient`, `vignette`, `window_blit`) and their `src` BGL are reflected and drift-gated but
**not** generated into `shader_gen.rs`: `ludion-render-graph` sits below `ludion-renderer` in the crate graph and cannot
consume the renderer's generated file, and forking a second generated file would duplicate the
shared `src` BGL against the unification rule in `ludion devdocs ludion-shader-bindgen` (Wave 4 amendment); `execute.rs`'s hand-written
BGL for that crate stays hand-written, and so does `Blit::new`'s for `window_blit.wgsl` (its binding
names match `vignette.wgsl`'s, so the unified `src` group is checked against its shape). The GPU test
`needs_gpu_the_shared_blit_shows_the_frame_bytes_unchanged` builds `Blit::new` and draws through it, so
a disagreement between the WGSL and the hand BGL fails there; only the window surface itself is checked
by running a window. Pipeline/render-pass structure is out of scope entirely —
the frame graph owns pass structure; this crate stops at structs, bindings, and BGLs.
