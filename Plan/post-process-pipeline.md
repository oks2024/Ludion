# Plan: The Post-Process Pipeline (the "HD" arc)

> **CLOSED — shipped; the waves delivered through "Wave 11 delivered 2026-07-26" and
> "`PostProcess.ssao_direct` shipped whole 2026-07-29" (wave log below). Owed: the `ssao_direct`
> value gate — the field is whole; only its shipped value rides a human sitting.**
> Status: **ACCEPTED 2026-07-25** (user direction) — direction settled the same day in a grill-me
> session where every decision below was answered explicitly. Multi-wave master plan, executed step
> by step — defects first. Runs under the graphics-engine feature stream as a standalone plan (the
> `Plan/shadow-mapping.md` / `Plan/environment-ibl.md` shape), not a numbered engine phase. The arc
> closes on one human sitting judging the new look against sweep sheets.
>
> **Wave log:** **wave 0 delivered 2026-07-25** (Lore revs 356–358, three commits). Zero pixels —
> full `check.mjs` green, every golden byte-identical. Amendments to the wave as written:
> `dump-graph` was already misreporting any scene with UI or a sun, so scene-derivation fixed a
> standing defect rather than only future-proofing; the plan's "pipelines and BGLs become
> `ShaderId`-indexed arrays" assumed a 1:1 shader↔layout mapping that does not hold (mesh binds
> four), so layouts got their own `LayoutId`-indexed array; `fullscreen_effect` drops the planned
> explicit `inputs` argument and reads `pass.inputs` (one source of truth, cannot mis-order); and
> **no `L11xx` was added** — the reachable misshapes are already covered (`L1014` for a negative
> scalar from the schemars range, `L1009` for a non-finite one from the scene format's scalar
> resolver, and JSON cannot spell one on the wire), so a dedicated code would have been a second way
> to reject the same thing. `PostProcess` ships with the two bloom knobs only; each later field
> arrives with its consumer.
>
> **Wave 1 delivered 2026-07-25** (Lore revs 360–364, five commits). Full `check.mjs` green,
> every golden byte-identical. Amendments to the wave as written: `--set` needed **no new rejection
> path for a bad value** — overrides land between `parse` and `validate`, so an ill-typed one is the
> ordinary `L1014` at the same pointer, and the single new `L4008` covers only what no schema can (a
> malformed spec, an unresolvable pointer, and the three schema-less skeleton fields
> `/name`/`/generated_from`/`/seed`); `render_scene_with_registry` **grew a parameter** rather than
> gaining a third `render_scene_*` sibling; `image-stats --check` spends **two codes, not three**
> (`L4009` invalid file *including* an unknown key, `L4010` per breach) because the metric vocabulary
> is a closed four, unlike a profile budget's dynamic pass/system names; and `--sweep` is capped at
> **16 cells** with an `L4008` and lays out **near-square**, not in a row, because cell area is what a
> taste call needs. The sweep's labels ride `model_sheet`'s `UiText` trick, which means a sweep cell
> is **not** byte-identical to the same `render --set` — the overlay pass runs after all post, so it
> cannot touch the imaging being judged, and a sheet is a judging instrument, never a golden.
> `budgets/image-stats-windows.toml` records the baseline; its ceilings sit just above the measured
> values rather than at 2× a median as a profile budget does, because these numbers are a pure
> function of committed bytes and cannot drift on noise. **Vigil's three districts are recorded but
> not CLI-checkable** — a district scene names vigil's own components, so it renders only through the
> game registry, and only `district-plaza` has a committed golden.
>
> **Wave 2 delivered 2026-07-25** (Lore revs 372–374, three commits). Full `check.mjs` green, every
> golden byte-identical (`night-sky` re-rendered to the same sha256, not merely SSIM 1.0). Amendments
> to the wave as written: **the pyramid is gated on a field, not on `PostProcess` presence** — the
> plan's wording did not survive wave 0, which migrated `night-sky.yaml`'s bloom knobs onto a
> `PostProcess`, so presence-gating would have moved a committed golden this wave; and wave 4's own
> stated test requires a *fully-neutral* `PostProcess` to stay byte-identical, which presence-gating
> makes unimplementable. Settled with the user: `PostProcess.bloom: gaussian | pyramid`, default
> `gaussian`, and **wave 11 deletes the `gaussian` path and the field together**. The bright-pass is
> shared — it writes pyramid level 0 as much as it writes the Gaussian chain's input — so "6-level
> downsample" is that pass plus five halvings, with the Karis average on the *first downsample*
> (past that the energy is already spread and the weighting would only dim the glow). Consequence
> recorded rather than fudged: the pyramid composites the **sum of all six levels**, so at a given
> `bloom_strength` it carries ~6× the Gaussian chain's energy — the reference filter's behaviour,
> which is why reference implementations pair it with strengths near 0.1. **Wave 11's defaults flip
> must re-author `bloom_strength` down by roughly 6×** or every scene blows out; normalising it away
> was rejected as making one field mean two things. Every filter offset is in *target*-texel units,
> so a level is a fixed fraction of the frame and the footprint is already scale-invariant ahead of
> wave 3. **No new L-code, the third wave running**: a misspelled enum value is the closed enum's own
> `L1014` at the field's pointer; `L4004`/`L4005`/`L4006`'s explain text grew the group vocabulary
> instead (and `L4006`'s pass list was found already stale). **Decision 14's `[post_groups]` landed in
> a sibling budget file, not a new section of the existing one**: every section of a budget is checked
> against every run, and `post-full.yaml`'s `scene` pass costs 0.103 ms against the lights.yaml-
> calibrated 0.06 ceiling, so one file cannot serve two scenes without widening a ceiling until it
> stops being a tripwire. `budgets/pinned-rtx3090-post-full.toml` holds the post chain's ceilings
> only; both invocations are in `ci/teamcity/README.md`, and **omitting the second would compare the
> `bloom` group against gaussian's three passes forever** — a disarmed tripwire reached by omission,
> which no diagnostic catches. Bloom group measured 0.060 ms summed (stable to ~1% over three runs;
> individual members sit within a few multiples of the adapter's ~0.001 ms timestamp granularity,
> which is the second argument for the group being the unit), ceiling 0.12. Two findings for later
> waves: **`--set` replaces a leaf and never creates one**, so overriding a field a scene does not
> already author means overriding the whole component map — this will bite every `--sweep` over a
> newly-added `PostProcess` field in waves 4, 5, 6 and 10, and wants a decision before wave 4; and
> `[passes] scene = 0.06` on lights.yaml has crept to 1.2× headroom against the file's own stated ~2×
> policy, one modest regression from firing.
>
> **Wave 3 delivered 2026-07-26** (Lore rev 377). Full `check.mjs` green, every golden
> byte-identical. The wave's central amendment is that **"screen-relative units" turned out to be a
> *resolution* rule, not a per-parameter one**: expressing a filter's footprint in target-texel units
> only makes it screen-relative if the target is a fixed fraction of the **frame**, and a bloom level
> sized off the render buffer is not. So the normative rule landed as **only `scene`, `scene_depth`
> and `ldr` are sized from the render extent; every other target is sized from the output extent** —
> which makes both bloom chains scale-invariant with *zero* shader or uniform edits (`record_blur`
> and `target_texel` were not touched), makes bloom cost the same at any scale, and is byte-identical
> at scale 1 by construction. The plan's "audit every post parameter to screen-relative units" was
> therefore satisfied by sizing, not by rewriting parameters. `(width, height)` collapsed into
> **`FrameSize { output, render }`** — the `PostChain` move repeated, so the arity stops exploding —
> and `RenderScale` is a validated newtype (finite, `[1.0, 4.0]`; below native is rejected, not
> clamped, because upscaling is a different feature) raising the one new code **`L4011`**. The
> **`resolve`** pass is an exact area-weighted box filter over `textureLoad` fetches, not
> `textureSample`: exact integer texels, no sampler state, no half-texel offset to get wrong, and
> correct at a non-integer ratio like 1.5. Reading an `Rgba8UnormSrgb` source, `textureLoad` returns
> linear, so the averaging happens in linear light — which is the point of resolving *after* tonemap.
> The tail is now four shapes (`tonemap → final`; `→ ldr → overlay`; `→ ldr → resolve → final`;
> `→ ldr → resolve → ldr_resolved → overlay`), with `ldr`/`ldr_resolved` declared after `final` so no
> existing resource list moved. Measured on the pinned RTX 3090: `edge_aliasing` **0.8816 → 0.5843**
> on post-full at scale 2, scale-1-vs-scale-2 SSIM **0.9949** (the invariant: a scale change is a
> quality change, never a look change), `resolve` **0.034 ms** at 3840×2160→1920×1080. Two
> consequences recorded rather than fudged: the bright pass still takes a single bilinear tap, so
> above scale 1 it reduces by more than 2:1 in that tap — today's behaviour at a wider ratio, and
> fixing it would move scale-1 bytes; and **`resolve` sits in no `[post_groups]` entry** — it is not
> a post *effect* and no budget file names it, so a tripwire on it would want a `[passes] resolve`
> line in a scale-2 budget invocation that does not exist today. The SSIM and `image-stats` gates
> live in `crates/ludion-testkit/tests/supersampling.rs`, not in the renderer's own test module:
> `ludion-testkit` owns both instruments and sits *above* the renderer in the crate graph.
> `docs/ludion-renderer.md` and `docs/ludion-render-graph.md` now under-describe the graph (pass
> counts, `resolve`, the extent rule) — wave 12's doc sweep must catch that.

> **Wave 3.5 delivered 2026-07-26** (Lore rev 378) — the blocker wave 2 recorded, settled before
> wave 4 as it asked. `--set`/`--sweep` may now **create a missing *final* field inside a component
> map the file already carries**, and nothing else: no component, no entity, no intermediate node.
> The safety argument is the arc's standing posture rather than a new check — a misspelled field name
> is the schema's own **`L1014`** at the same pointer, naming the nearest declared field, exactly as
> the same typo written into the file would be (verified end to end, including through a sweep).
> Refusing creation would have meant that tuning any knob a file left at its default requires
> restating the whole component map, which is what wave 2 had to do and what waves 4/5/6/10 would
> each have repeated. `AppliedOverride.from` became **`Option<Value>`** rather than gaining a
> `created` flag beside a `Null`: `None` = the field did not exist, `Some(Value::Null)` = it existed
> holding null, and the echo prints `unset -> <value>` so a created field is visible at a glance —
> "this scene never mentioned that field" is exactly when an operator wants to re-read the spelling.
> `SweepOutcome.original` took the same shape.

> **Wave 4 delivered 2026-07-26** (Lore rev 379). Full `check.mjs` green; **zero goldens moved** —
> `post-full.yaml` was deliberately left alone, because decision 9's independence-and-bisectability
> rule and the wave's own stated tests ask only for neutrality proofs, and wave 3.5 had just made
> `--set` able to *create* a field a scene does not author, so every effect is provable through the
> override seam instead. Waves 6 and 9 are where the plan actually asks for a `post-full` re-bless.
> **The pass is now `composite`** (decision 2's "today's `tonemap` pass, grown up") — renamed here
> rather than later, since renaming costs the same churn after a longer period of being wrong: the
> label, `ShaderId`, `tonemap.wgsl` → `composite.wgsl`, `KNOWN_PASS_NAMES`, the `frame_graph_64x64`
> snapshot, `L4004`/`L4006` explain text, and the `[passes]` key in all four budget files. Three new
> `PostProcess` fields, each neutral by default and **skipped by a uniform branch, never computed
> with an identity constant**: `exposure` (a *linear* multiplier, not EV stops — its neutral value is
> 1.0 like every other scalar here, and one field means one thing; applied after the bright-pass so
> `bloom_threshold` keeps meaning *scene* luminance), `tonemap: agx | aces | neutral | none`
> (default `aces`; `aces` is the switch's `default` arm so the frozen path takes the fallthrough),
> and `dither`. **Dither is applied in the *encoded* domain**, which is the wave's one genuinely
> subtle decision: the target is `Rgba8UnormSrgb` and the *hardware* encodes on store, so a linear
> ±1-LSB offset is worth wildly different numbers of output steps at different luminances — most
> wrong in the deep shadows, which is exactly where a night-sky gradient bands. So the offset lands
> between an explicit `linear_to_srgb`/`srgb_to_linear` round trip; it costs well under an LSB in f32
> and needs **no format change**, which is what keeps the dither-off path byte-identical. `BAYER4`
> moved out of `mesh.wgsl` into a shared `snippets/bayer.wgsl` so the screen-door `Fade` and the
> output dither use one tile (mesh-heavy goldens re-rendered to the same sha256, proving the move
> bit-neutral). **No new L-code, the fifth wave running**, and **no `composite` post group**: a group
> exists so a budget can name an *effect* rather than an implementation (bloom is 3 passes or 11),
> and composite is one pass that folding guarantees stays one pass — a one-member group beside
> `[passes] composite` would be two ways to budget one number. Operator validation (the plan's stated
> top risk) is a pixel test against references computed offline in f64: AgX (the three.js/Filament
> "minimal AgX"), ACES, Neutral (Khronos PBR Neutral) and none all came back **exact in every channel**
> on the 3090, with the committed tolerance at 2 LSB as M4 headroom. `banding_score` on night-sky
> **0.8979 → 0.2877** at the golden's 960×540 (0.8417 → 0.3365 at 480×270). Also renamed the pass in
> `docs/ludion-renderer.md` + `TESTING.md` — a doc naming a nonexistent pass is worse than a stale
> one — but `docs/engine-guide.md`'s `PostProcess` field list is now four fields behind, for wave 12.

> **Wave 5 delivered 2026-07-26** (Lore rev 380). Full `check.mjs` green, zero goldens moved. The
> grade is **display-referred** — after the tonemap operator, before the dither: decision 12's "AgX
> ships muted and expects a grade on top" *is* the AgX workflow, and it is also the safe place, since
> values are in `[0, 1]` rather than an unbounded HDR range. Dither stays strictly last, because it
> exists to hide the *output* quantization. The seven fields are **flat, not a nested `grade:` map**,
> and wave 3.5 supplied the second, harder reason on top of decision 3's: an override can create a
> missing *final* field but deliberately not an intermediate node, so a nested map would have broken
> the arc's own tuning instrument on the wave that most needs it. The order of operations is pinned
> and documented in both the component and the shader (white balance → lift/gamma/gain → contrast
> about a 0.5 pivot → saturation), because grading has no canonical order and an unwritten one is a
> bug waiting to be argued about. White balance is a **diagonal von Kries channel scale normalized by
> Rec.709 luma** — a CAT would want a Planckian white point and an LMS matrix to earn a difference no
> eye finds in a display-referred look knob; the normalizer is what stops `temperature` doubling as a
> brightness control, and both factors are exactly `(1,1,1)` at 0 so neutrality is exact. **The one
> new L-code of the arc so far, `L1130`**: my brief said "no new code", and that was wrong — schemars
> `range` reaches a scalar field but **not the elements of an authored triple**, the exact wall
> `Environment`'s colours hit and paid `L1128` for, so `gain >= 0` and `gamma > 0` are the component's
> semantic validator's (`PostProcess` moved to `register_validated`, its append-only index unmoved).
> `lift` stays deliberately unbounded — a negative offset legitimately crushes blacks. `MIN_GAMMA`
> survives as defence in depth against a *validating* gamma small enough to overflow its reciprocal,
> not as the primary answer. **No sweep-sheet PNG was committed**, against the wave's written line:
> this wave chooses no defaults (they stay identity until the flip), so a sheet rendered now records
> no decision; wave 1 already ruled a sheet "a judging instrument, never a golden" and nothing would
> ever compare it; and one rendered under this wave's code is stale by the flip. The recipe ships
> instead, as a runnable `--sweep` example in the field rustdoc. The direction tests are the ones
> worth having: `saturation: 0` greys every pixel, contrast widens the spread about mid-grey, each of
> lift/gamma/gain brightens when raised (pinning the reciprocal convention), and temperature/tint move
> the named axes the named ways — a formula that "does something" but not the right thing fails them.

> **Wave 6 delivered 2026-07-26** (Lore rev 381). Full `check.mjs` green, zero goldens moved. Fog is
> the first post effect to consume the depth buffer wave 0 made sampleable, and getting it in cost
> three things beyond the effect itself. **(1) `composite` reads `scene_depth` unconditionally** — the
> shadow-mapping house rule (a sunless frame binds a zeroed uniform and a 1×1 fallback rather than
> compiling a second pipeline) applies: a conditional read would fork one shader into two layouts and
> two pipelines. The depth is fetched with **`textureLoad` on a `texture_depth_2d`**, because
> `Depth32Float` is not filterable and `fullscreen_effect` binds the shared *filtering* sampler —
> `textureLoad` forms no texture/sampler pair, so the two coexist legally, and the shared helper
> carried the third input with **no argument and no second recording path**. **(2) The scene pass's
> depth store op moved `Discard` → `Store`**: a later pass sampling a discarded attachment reads
> undefined contents, so the declaration had to become honest. Zero pixels moved, and the cost was
> *measured* rather than left as a caveat — `scene` on lights.yaml is **0.0502 ms against its 0.06
> ceiling**, unchanged headroom. **(3) A latent `ludion-render-graph` scheduler defect surfaced**:
> `topo_order` pushed a producer into `deps[consumer]` once per *read* but decremented the in-degree
> once per *producer*, so a pass reading two resources from one producer — exactly `composite` reading
> the scene pass's colour *and* its depth — carried an in-degree it could never work off and reported
> itself as an `L2001` cycle. No graph had that shape before wave 6. Fixed by deduplicating `deps`,
> with a regression test. On the effect: **fog is scene-referred and vignette display-referred**, so
> the pinned order is `scene + bloom` → fog → exposure → tonemap → grade → vignette → dither. **Fog
> does not apply where nothing was drawn** (`depth < 1.0` is the test; the sky sits at exactly far
> depth) — otherwise distance fog would replace an authored `Environment` gradient with a slab of fog
> colour, and the corollary for an author is in the field docs: a `fog_color` near `sky_horizon` is
> what makes the horizon read continuous. Height fog is **Wenzel's analytic integral** (Crytek, GDC
> 2006) over the fogged segment, chosen because as the falloff tends to 0 the expression *collapses
> exactly to plain distance fog* — the neutral default is the formula's own limit, not a special case.
> The vignette takes one shape knob, `vignette_radius`: the outer edge is supplied by the frame's own
> corner, so a smoothness knob would be a second spelling of the same number. Its geometry is raw
> normalized UV with no aspect correction, which is exactly why it is resolution- and scale-invariant
> (an inscribed ellipse is the deliberate consequence). **No new L-code**: `fog_color` is
> `Environment`'s colour case, so `L1130` widened from grade-only to any bounded triple
> (`POST_PROCESS_INVALID_GRADE` → `POST_PROCESS_INVALID_TRIPLE`, code string unchanged) rather than
> spending a second code on one invariant. **Zero goldens moved, overriding the wave's written
> "`post-full` golden update"** — the same call as waves 4 and 5, and switching policy here would be
> incoherent: a `post-full` carrying fog and vignette but not exposure, tonemap, grade or dither is
> not the all-on scene its name claims. The single re-bless happens at the flip, when the roster is
> genuinely on. Consequence accepted and recorded: no committed golden renders fog or vignette until
> then, so between here and the flip those two effects are guarded by their own tests alone.

> **Wave 7 delivered 2026-07-26** (Lore rev 382). Full `check.mjs` green, zero goldens moved. **Two
> parts of the written wave were cut, both by the wave's own decisions rather than for expedience.**
> **(1) MRT was not built.** Decision 7's criterion is "MRT lands mid-arc with *exactly one
> consumer*", and it names the depth+normal prepass — but that is not a consumer: `PassDecl` already
> carries a colour attachment *and* a separate depth attachment, so a depth+normal prepass is one
> **colour** target plus the depth slot `add_external_pass_with_depth` has provided since the shadow
> wave. Turning `color` into a list now would be exactly the speculative generality decision 7
> rejected. **(2) The scene pass's `Equal` depth test was not built.** It needs the scene pass to
> render into a depth attachment an *earlier* pass wrote, and the graph has **no write-after-write
> ordering concept at all** — `topo_order` derives edges from *reads* only, so two passes writing one
> resource are unordered in principle and would work here only by accident, through the transitive
> `prepass → ssao → scene` chain. Closing that gap is a data-model addition with no second consumer,
> and the Risks section already flags `Equal`-with-prepass versus `Less`-without as two policies in
> one pass. So the prepass **owns its own depth**, the scene pass is untouched, and an AO-enabled
> frame rasterizes geometry twice — an opt-in cost, and it is what SSAO reads anyway. Both deferrals
> have triggers, recorded below. `ludion-render-graph` is **not in the diff at all**. What shipped:
> a conditional depth+normal prepass, a 16-sample SSAO estimator, a blur, and AO multiplied into the
> **ambient + IBL term only** (decision 8) — proven by a fixture whose emissive and direct-lit halves
> are asserted as *whole-image byte equalities* while an ambient crease darkens, each guarded against
> passing vacuously. The fixture had to be a slot between two slabs rather than a box on a plane: a
> lone box occludes ~5% at a testable size, which is the estimator's real behaviour, not a bug. AO
> reaches the mesh shader through **group 3** — what AO modulates is exactly what that group carries
> — with a 1×1 fallback and a flag, and an AO-off frame **branches rather than multiplying by one**,
> because unlike an arithmetic identity a texture fetch is not free. Two choices worth their
> reasoning: the kernel is **Roberts' R2 sequence, not Hammersley** (no bit reversal, and well
> distributed at *any* prefix length, so changing the sample count cannot quietly unbalance it), and
> the rotation is **`BAYER4`, not interleaved gradient noise** — IGN has no period, while Bayer's is
> 4×4, so a 4×4 box blur averages exactly one full period and the pattern cancels *exactly* instead
> of leaving residue. Extents follow wave 3's rule: the prepass is geometry and takes the **render**
> extent, `ssao`/`ssao_blur` are post and take half the **output** extent. One deviation, on budgets:
> `POST_GROUPS` gains `ssao = [prepass, ssao, ssao_blur]` and **no `prepass` group** — wave 4's rule
> says a one-member group is a second way to budget one number, and the prepass exists for exactly
> one reason, so its cost belongs inside the effect's ceiling; if depth/normal outlines (decision 13)
> ever give it a second consumer it earns its own line. Measured on the 3090 at 1920×1080: prepass
> 0.027, ssao 0.074, ssao_blur 0.023, **group 0.123 ms**, `scene` unchanged. The ceiling is committed
> **dormant** at 0.25 in the post-full budget file with its calibration in a comment, so the flip
> inherits the number instead of rediscovering it — a group with no measured member skips by design
> (verified: exit 0). Noted in passing, unrelated to this wave:
> `ludion-server::delivery events_stay_gapless_while_frames_flow` was seen to drop an event once
> under full-suite load and pass in isolation.

> **Wave 8 delivered 2026-07-26** (Lore rev 383). Full `check.mjs` green, zero goldens moved, no new
> L-code. *(Entry reconstructed at wave 9 from the commit message: the wave shipped without one, and
> the plan is meant to be the record.)* Two passes behind the scene pass — a ray-march of the sun
> shadow map along each eye ray at half the output extent, then the 4×4 box that cancels the march's
> step offset — with `composite` tinting the blurred scalar by the sun's colour and adding it. Shafts
> are the only effect in the roster gated on **two** things, the authored strength *and* a
> `DirectionalLight`, because there is nothing to march without a shadow map; a sunless scene asking
> for shafts derives `false` and simply gets none. **The wave's real architectural product is the
> optional input.** Every conditional input before it reached a pass whose layout could absorb it
> unconditionally, but the shaft term's producer does not exist on most frames, so it is the first
> case where a fullscreen effect's declared read *count* stops equalling its binding count.
> `fullscreen_effect` grew a slot list rather than a second pipeline: `binding_plan(input_count,
> absent)` is the whole mapping, pure arithmetic and GPU-free, pinned by a test proving the property
> "append and pad the tail" would fail — a second optional slot cannot move the first, and neither
> present nor absent can move a required read. The graph's obligation is the mirror image, so
> `composite` declares its optional read **last**. Estimator choices: Henyey–Greenstein (1941) with
> `g` a constant rather than a knob, since the difference it controls is beams versus uniform haze and
> uniform haze is what `fog_density` already spells; the march stops at `3 / shafts_density`, which is
> why there is no `shafts_distance` field; `shafts_density` is deliberately not `fog_density`, since
> shafts add in-scattered light and never extinguish. The shadow map is `textureLoad`-ed rather than
> comparison-sampled — a marched point is in free space, so there is no acne to fight and one exact
> texel is the answer — and `ssao_blur.wgsl` became `box_blur.wgsl`, one filter whose width is the
> Bayer tile's period under two pass names. Order in composite: after fog, before exposure. The
> fixture parks the camera deliberately **off to the side** of the slot's beam, because from inside it
> every ray would be lit near the camera where extinction weights most and a bug adding a constant
> would pass. Banding gate 0.4761 → 0.4258. Budget group `shafts = [shafts, shafts_blur]` committed
> dormant at 0.32 against 0.161 measured.

> **Wave 9 delivered 2026-07-26.** Full `check.mjs` green, **zero goldens moved** — the sixth wave
> running, and again overriding the wave's written "`post-full` golden update" for the reason waves 4
> through 8 gave: a `post-full` carrying depth of field but not exposure, tonemap, grade, fog,
> vignette, AO or shafts is not the all-on scene its name claims, and the single re-bless belongs at
> the flip. **No new L-code, the seventh wave running**: both new fields are scalars, so a bad one is
> the schemars range's own `L1014` at its pointer, and there is no authored triple for `L1130` to
> reach.
>
> **The focus model is a lens, and it is two fields.** `dof` is the aperture — the far field's circle
> of confusion as a fraction of the frame's **height** — and `focus_distance` is the focal plane in
> world units. The CoC is the thin-lens formula written out verbatim, `dof · (1 - focus_distance / d)`,
> which is `A·f²/(s-f)·|1/s - 1/d|` with the constant absorbed into the one authored number. Three
> things come free from that rather than from a band-plus-ramp parameterisation: it is exactly zero at
> the focal plane, so neutral is the formula's own zero (the posture wave 6's Wenzel integral took);
> it **saturates at `dof`** as the distance runs to infinity, so `dof` is literally the background's
> blur radius *and* the background needs no special case at all; and its sign separates near from far,
> which is all the gather needs. **There is deliberately no in-focus range field** — under the same
> relation the sharp band is a *consequence* of the aperture, so spelling it separately would let a
> scene author a lens that cannot exist and would make `dof` mean two things (the call
> `shafts_density` made by setting both a beam's brightness and its reach). **Tilt-shift was rejected
> as a second mechanism, not as a second field:** it makes the circle of confusion a function of screen
> *y* and consumes no depth at all, which is a different effect wearing the same name. It is also not
> needed for the case that motivated it — vigil is top-down-ish, and a distance model with the focal
> plane on the player produces the same miniature look with the occlusion right, since a tower high in
> the frame but close to the camera stays sharp where a screen-height ramp would blur it. **Autofocus
> is rejected with the same argument decision 5 used against TAA:** it would reduce the depth buffer
> and feed the answer into the *next* frame, making the image a function of the camera's history
> rather than of world state. A game that wants it writes `focus_distance` from a system each tick —
> sim state, deterministic, replays, and it needs nothing from the renderer.
>
> **Where it lands: step 1, ahead of the bloom mix and the fog** — and the position is decided by the
> gather's *input*, not by where a lens sits in the optical path. Physically the lens defocuses light
> that has already been fogged and already carries the medium's in-scatter, which argues for step 5;
> but the gather reads the `scene` target, which is pre-fog and pre-shafts, so a blend placed after
> the fog would mix a fogged sharp image against an unfogged blurred one and thick fog would vanish
> exactly where the defocus is strongest. Blending first applies one fog to one image, using each
> pixel's own sharp depth. The shafts sit behind the blend for a stronger reason than ordering:
> in-scattered light is spread along the whole ray, so it has no single depth and therefore no circle
> of confusion, and it is already a low-frequency field — defocusing it would cost a gather to change
> nothing. **Bloom stays computed from the sharp scene** and is added after the blend, because the
> bright pass reads `scene` and moving the defocus in front of it would insert a full-resolution HDR
> round trip between the scene pass and the bright pass on every DoF frame, which is the cost decision
> 2's folding rule exists to refuse. Consequence recorded rather than fudged: a defocused highlight
> blooms with its **focused** shape. **Trigger:** the gate verdict, if a defocused street lamp reads
> wrong — the fix is a full-resolution DoF target ahead of the bright pass.
>
> **Pass shape: two passes, and MRT was not built.** The near/far split the wave was written around
> dissolves under the scatter-as-gather weight — a tap reaches a pixel only if its *own* disc covers
> it, so a sharp foreground (CoC ≈ 0) scatters nowhere and cannot smear onto the background, while a
> blurred one reaches out over it. The sign of the CoC therefore never selects a *pass*, only an
> occlusion rule, and that rule is one line: a tap **behind** this pixel contributes in proportion to
> `|coc(p)| / |coc(q)|`, which is zero under a sharp surface and one between two equally defocused
> ones (the CoC is monotone in depth, so comparing CoCs compares depths and no second fetch is
> needed). So **the MRT deferral's trigger is unchanged — the first pass that genuinely writes two
> *colour* targets** — and the plan's "it may well not be one either" is now answered rather than
> guessed. What the second pass *is* is a **prefilter** (`dof_coc`), and it earns its place three
> times: it turns each of the gather's thirty-two taps into one filterable fetch instead of a colour,
> a depth and a matrix multiply; it area-averages the colour once instead of point-sampling it per
> tap; and it takes the circle of confusion from the **nearest** of the source texels a target texel
> covers, which is what stops a silhouette thinning as it is downsampled. It is the same
> bright-pass-then-blur shape bloom already has, and it makes `dof = [dof_coc, dof]` an honest
> two-member group rather than the one-member group wave 4's rule forbids.
>
> **The kernel is a Vogel golden-angle spiral** (`r = sqrt((i + ½)/N)`, `θ = i · φ`) — `sqrt` makes the
> samples uniform in *area*, and the golden angle is the disc analogue of wave 7's argument for
> Roberts' R2 over Hammersley: well distributed at any prefix length, so changing `DOF_TAPS` cannot
> quietly leave a lobe unsampled. The radius is in fractions of the frame's height, converted to the
> target's own texels, so it is screen-relative by construction (the wave-3 rule) — never source
> texels. **The wave's one genuinely subtle finding is that every tap must be a `textureLoad`, not a
> bilinear sample.** The gather's alpha channel is a circle of confusion, and a bilinear tap near a
> silhouette returns the *average* of a sharp surface's CoC and a defocused one's — a number no
> surface in the scene has. The disc test then admits that tap, and with it a share of the sharp
> surface's colour, which is how an in-focus subject bleeds into the defocused background around it.
> It is not a small effect: measured on the fixture below, the in-focus patch's contrast went
> **263.3 → 222.4** with bilinear taps and **263.3 → 253.8** with exact texels, at an unchanged blur on
> both defocused patches. A filtered colour was worth nothing here anyway — the pass is an average of
> thirty-two of them. **The gather's output is premultiplied by the coverage it claims**, for the
> mirror-image reason: `composite` reads that target bilinearly, and filtering a straight
> (colour, coverage) pair mixes a colour a neighbour never had at a coverage it never claimed. Under
> premultiplication the interpolation *is* the over operator, and the composite's step is one line.
>
> **The composite-back is `composite`'s second optional input, not a pass of its own** — a lerp of
> already-sampled colour is exactly decision 2's folding class, and being the second consumer of wave
> 8's fixed-slot scheme was this wave's stated architectural job. It cost the scheme nothing:
> `dof_tex` took binding 4 behind `shafts_tex` at 3, the graph declares the two optional reads last in
> that order, and `optional_inputs_hold_their_binding` now pins the real pair through
> `shader_gen::DOF_TEX_BINDING` instead of the hypothetical `shafts + 1` wave 8 left it asserting.
> `FrameSize::half_post()` was extracted while doing it, so the bloom, AO, shaft and DoF chains cannot
> disagree about what "half" means.
>
> **Recorded consequence, with a trigger:** blending a half-extent layer back at full resolution
> softens a sharp silhouette against a *fully* defocused background by about one output pixel. It is
> the standard artefact of the structure the wave asked for, it is a soft edge rather than a bright
> halo now that the layer is premultiplied, and it costs the adversarial fixture 3.6% of its in-focus
> contrast (263.3 → 253.8, against 88% lost on the defocused patches). **Trigger:** a silhouette
> verdict at the sitting — the fixes are a full-resolution gather or a depth-aware upsample, both of
> which cost a full-resolution pass or four extra fetches in `composite`.
>
> **No CLI cross-process determinism test**, unlike waves 7 and 8. Both of those needed one because
> they are dithered estimators with no temporal accumulation to hide a non-reproducible one; this is
> the arc's first effect with **no stochastic element at all** — a fixed spiral over exact texels — so
> such a test would prove nothing the existing two do not, at the price of three more subprocess
> renders inside a two-minute check budget.
>
> Tests: a three-patch fixture (identical square waves at 0.4×, 1× and 4× the focal distance, each
> scaled so all three project to the same size and separated sideways by more than their own bokeh)
> asserting both defocused patches lose ≥ half their contrast while the focal one keeps ≥ 90% —
> **283.9 → 30.5 near, 283.9 → 33.5 far, 263.3 → 253.8 sharp**; byte identity at `dof = 0` guarded
> against passing vacuously by a third render with a real aperture; scale invariance as an on/off
> ratio at scale 1 versus 2; the graph's shape, its off-state resource list, the shafts-then-dof read
> order and the extent rule. The defect gate is `banding_score`, deliberately **not** `edge_aliasing`
> — a blur lowers edge aliasing for free, which is precisely the metric-gaming the Risks section warns
> about — and a defocused background is the smoothest thing this pipeline can produce, so it is the
> right thing to watch: **0.4761 off → 0.4845 on** on post-full at 480×270, ceiling 0.60. Budget group
> `dof` measured **0.0932 ms** at 1920×1080 (dof_coc 0.0164 + dof 0.0768, identical to the last digit
> over three runs), committed **dormant** at 0.19. The gather is 4.7× the prefilter, which is the
> prefilter's own justification.
>
> **A `STATUS_ACCESS_VIOLATION` in the concurrent GPU test group was investigated during this wave and
> is NOT this wave's** — it is the **second sighting** of the flake the code audit recorded on
> 2026-07-25 (`Plan/code-audit-2026-07-25.md`, "Observed during the arc, unresolved";
> the private code-audit notebook says in as many words "if it reappears, this is the second sighting"). That
> record predates rev 356, so it predates wave 0 of this arc and every line of depth-of-field code.
> **Sighting 1:** `ludion-renderer --lib`, died after 18/20 tests, `--full` run, passed 20/20 on
> re-run. **Sighting 2:** `ludion-server::phase10`, both of its tests, `0xc0000005` at tests 1104 and
> 1106 of a 1419-test `check.mjs` lane, with a `ludion-cli::phase7` cargo build and two vigil replays
> running alongside. Different binaries, different tests, one signature: a process holding a wgpu
> device dies with an access violation under a saturated box, no Rust panic and no wgpu validation
> message. It is a device-lifecycle fault in the harness's most loaded moment, not an effect's bug.
>
> The attribution was tested rather than assumed, because a stash/restore A/B across a flake with a
> shifting base rate reads as causal on a small sample. On the wave-9 tree the two crashing tests were
> executed **≈170 times**: the coordinator's exact two-binary reproduction **29 times** — at the
> committed `gpu` cap of 4, at 8, at 32, and under a saturating concurrent
> `cargo build --workspace --all-targets` — plus **50** runs of `phase10` alone against a concurrent
> `ludion-renderer` loop, two full-workspace `nextest` runs, and five full `check.mjs` runs. Zero
> crashes, against a reported 2-for-2 on the same command. And wave 9's delta is inert for
> those tests by construction: they author no `PostProcess`, so `PostChain::dof` is false and neither
> pass is in the graph; the `Composite` uniform is **byte-identical in size and in every field offset**
> (`dof: u32` took the exact 4 bytes `pad2: f32` occupied at offset 116 — the generated `offset_of`
> asserts are compile-time and derived from the WGSL, so the two sides cannot disagree); and
> `binding_plan(3, &[3, 4])`, the case a no-post frame actually hits, is `[Some(0), Some(1), Some(2),
> None, None]`, already gated by `optional_inputs_hold_their_binding`. What wave 9 *does* change for
> every device is two more pipelines at `Renderer::new` — a scheduling shift, which is exactly what
> moves a schedule-sensitive flake's probability without being its cause.
>
> **`.config/nextest.toml` was deliberately not touched.** Lowering the `gpu` cap would attribute a
> pre-arc fault to this wave and disarm the harness at the same time, which is the papering-over that
> file's own header forbids. **What the next sighting needs is a dump, not another guess:** this box
> has no admin rights, so WER `LocalDumps` could not be enabled (`HKLM\...\Windows Error
> Reporting\LocalDumps`, `DumpType=2`) — with it set, the faulting module name settles in one run
> whether this is the DX12 driver, wgpu, or engine code. Recorded here because waves 10–12 add more
> shaders and more pipelines, so the next wave to shift GPU-test scheduling will be blamed for it too.

> **Wave 10 delivered 2026-07-26** (Lore revs 385–386, two commits). Full `check.mjs` green ×2 (90.9 s
> and 91.4 s), **zero goldens moved** — the seventh wave running — and **no new L-code, the eighth**:
> all four fields are scalars, so a bad value is the schemars `range`'s own `L1014` at its pointer.
> Decision 2's folding rule took all three effects whole: `ludion-render-graph` is not in the diff,
> `PostChain` is untouched, **no pass was added and therefore no budget group** — the wave's entire
> engine reach is four uniform fields and three branches in one fragment shader. No shader-scheduling
> shift either, so wave 9's flake warning had nothing to fire on (none sighted across five full runs).
>
> **Chromatic aberration lives in the fetch, not in a per-pixel step later** — it is not applied to
> the image, it is how the image was formed. Red is pulled toward the axis and blue pushed out (the
> crown-glass direction) while green stays on the sensor's own grid, which is what makes a neutral
> value *exactly* the fetch it replaces. The authored number is the displacement at an edge midpoint
> in **normalized UV**, so it is resolution- and scale-invariant by construction and follows the
> frame's format the way the vignette's inscribed ellipse does. **Only the scene fetch is displaced:**
> bloom, the shaft term and the defocus layer are low-frequency by construction, so an offset of a few
> pixels returns the same field, and the fog reads each pixel's *own* depth — displacing that would
> fog a surface that is at a different pixel. It is wave 9's call repeated (what an effect *reads*
> decides where it applies), and it saves four texture fetches per pixel.
>
> **Halation reuses `bloom_tex` and its tint is a CONSTANT, not a field.** That is wave 8's
> Henyey–Greenstein `g` argument transplanted exactly: what an authorable `halation_color` would
> control is achromatic glow versus chromatic glow, and achromatic glow is precisely what
> `bloom_strength` already spells — a colour field would be a second way to write bloom. It is also
> the faithful model, since halation's red-orange comes from the film *base* rather than from the
> shot. The tint is normalized by Rec.709 luma (`white_balance`'s trick against the same hazard), so
> `halation` is a strength on exactly bloom's scale instead of doubling as a dimmer. Measured **free**
> — 0.0000 ms delta at 1920×1080, one multiply-add on a texel `composite` had already sampled, which
> is the reuse argument proving itself. **Recorded consequence with a trigger:** it inherits
> `bloom_threshold` and the chain's radius and cannot be tuned apart from them; its own blur chain
> waits on a look verdict at the closing sitting.
>
> **Grain lands after the vignette and before the dither** — light passes the lens, *then* it is
> recorded, and the dither is not a camera at all. It is applied in the **encoded** domain and shares
> **one** encode/decode round trip with the dither, because the sum of two offsets is one offset and a
> second trip would quantize nothing while costing a second `pow` pair; with grain off the arithmetic
> is operation for operation what the dither shipped with, so a dither-on frame survives the refactor
> byte-identical. `grain_size` is a fraction of the frame's **height**, never a pixel count, and here
> wave 3's screen-relative rule is load-bearing rather than tidy: grain authored per *render* pixel is
> averaged straight back out by the `resolve` pass at `--scale 2`, so raising a quality setting would
> change the look. The height comes from `textureDimensions(scene_tex)`, so no uniform field can
> disagree with it. The amplitude is **uniform and monochrome**, not weighted toward the midtones the
> way real emulsion is, because decision 13 puts grain in the roster to break up 8-bit night gradients
> and that is a *deep shadow* problem. The hash is **Wellons' `triple32`** over the cell's integer
> coordinates — integer arithmetic deliberately, **never `fract(sin(dot(…)))`**, whose last bits are
> not guaranteed across backends while this arc rests on byte-identity — with the top 23 bits mapped
> to `[-grain, grain)` at a mean of *exactly* zero (the Bayer half-step trick). **The pattern is
> static, and that is a decision rather than an omission:** `RenderScene` carries no frame counter and
> `extract()` has no access to one, and animating it wants a `frame` on `RenderScene` fed by the live
> loop — deterministic world state, so **not** what decision 5's anti-TAA argument rules out.
> **Trigger:** the first live build that authors grain and reads as fixed-pattern dirt in motion.
>
> The `Composite` uniform grew **208 → 224 bytes with every pre-existing field offset unmoved** (the
> aberration and halation scalars took `pad0`/`pad1` at 120/124; the grain pair opens a new block at
> 144). Three directional fixtures, each guarded against passing vacuously: the aberration splits an
> off-centre bar into **opposed** fringes — red beyond it, blue between it and the centre — which no
> per-pixel channel scale and no constant (longitudinal) offset can pass, with green asserted
> *byte-exactly* unchanged; halation lights a dark ring at `bloom_strength: 0.0`, proving it is not a
> tinted bloom mix; grain takes a byte-exactly flat patch to tens of steps of spread while its mean
> holds, and at an eight-pixel cell every run of eight adjacent pixels is exactly equal while nine of
> twelve boundaries change.
>
> **The three gates.** (1) **Banding: 0.8417 → 0.0115** on night-sky at 480×270 (0.8979 → **0.0143**
> at 960×540), against the dither's 0.3365/0.2877 — the ordered tile repeats every four pixels and the
> hash does not repeat at all. Guarded against the Risks section's metric-gaming hazard by *not*
> shopping for an amplitude: the committed 0.02 is the component's own documented "fine film-ish
> stipple", and 0.005 — finer than anything shippable, ±1.3 output steps — already reads 0.8417 →
> 0.1370. (2) **Cross-process determinism**, the test that would break if the grain ever reached for a
> transcendental: two OS processes render byte-identically with grain on through `--set`, with a third
> grain-free render as the non-vacuity guard (since the step is gated on `grain != 0`, a broken
> override is exactly what a vacuous pass looks like). (3) **Scale invariance as an on/off ratio**
> (wave 8's shape, because the two scales draw different images and only a quantity normalized by the
> same frame's grain-free self compares across them): **1.7313 at 1× against 1.6988 at 2×**, 1.9%
> apart under a 10% tolerance, and the gate is demonstrably failable — at the *shipped* `grain_size`
> on a 270-line frame the cell floors to one render pixel and the same measurement reads 1.7744 vs
> **1.2790**, 28% apart, which is the resolve averaging the grain away.
>
> **A boundary worth knowing, for wave 11's flip:** `MIN_GRAIN_CELL` floors the cell at one render
> pixel, and below the floor grain *is* per-render-pixel grain. At the real 1920×1080 with the shipped
> default 0.002 (2.16 render px at 1×) the ratios are 1.7687 vs 1.6409 — **7.2% apart, inside the
> tolerance but measurably softened** by the 2× resolve straddling the cell boundaries; at 0.004 it
> tightens to 3.7%. The rule works (7% instead of 28%), but the shipped default sits at the fine end
> of where it fully holds.
>
> **Cost** on `post-full.yaml` at 1920×1080 (median of three, identical to the last digit): `composite`
> **0.0256 ms off → 0.03584 ms with all three on**, +0.01024 ms (+40% of a pass that is 1/70th of the
> frame) — CA +0.00307, grain +0.00819, halation +0.00000. `scene` and the bloom group unmoved. **No
> budget file was edited**: the effects are off by default, no pass was added, and wave 4 already ruled
> a one-member `composite` group would be a second way to budget one number.
>
> **One finding recorded as a fact and deliberately NOT built into a gate:** chromatic aberration
> *lowers* `edge_aliasing` — 0.8816 → 0.7861 at 0.001 → **0.6741** at 0.002 → 0.5654 at 0.004 on
> post-full — because splitting a hard luma edge into three displaced channel copies inserts
> intermediate samples the metric reads as a softer edge. It anti-aliases nothing. That is squarely
> the metric-gaming hazard the Risks section names, so the number is written down here and gates
> nothing; wave 9 made the same call in the opposite direction when it refused `edge_aliasing` as
> DoF's defect gate. `docs/engine-guide.md`'s `PostProcess` field list is now four further fields
> behind, for wave 12.

> **Wave 11 delivered 2026-07-26** (Lore revs 387–389, three commits). Full `check.mjs` green on
> each; both `profile --check` invocations and `image-stats --check` exit 0. The engine's neutral
> image is now **pyramid bloom, AgX, dither on, exposure 1.3**, and the legacy 5-tap gaussian chain
> is deleted whole — `blur.wgsl`, its `ShaderId`/`LayoutId`, `record_blur`, the two pass names,
> `BloomTargets`, `PostChain.bloom`, `PostData.bloom`, `PostProcess.bloom` and the `BloomMode` enum
> in both mirrors. The second way wave 2 introduced *on a schedule* is gone on that schedule rather
> than by hope, which is what the Risks section's "standing violation if that wave slips" was
> guarding. Shaders 20→19, layouts 22→21, `KNOWN_PASS_NAMES` 25→23, the base graph 5→13 passes.
>
> **What flipped is the imaging *chain*, not a look.** The grade stays identity and every effect
> gate stays off by default; the arc's framing constraint is style-agnostic machinery, so the engine
> ships operators and content authors the look. Turning the roster *on* is `post-full.yaml`'s job,
> which is why this wave is three commits rather than one.
>
> **`bloom_strength` 0.6 → 0.10, and it could not be derived by dividing by wave 2's nominal six.**
> Matching bloom energy on the *output* image gives 0.014 — a 40×, and completely wrong — because
> the ACES shoulder destroys most of the narrow gaussian's added energy while the pyramid's wide
> halo lands in the curve's linear part. Measured in the **HDR domain** instead (`tonemap: none`,
> `exposure: 0.02`, so the readback is a linear scaling of what the composite mixed) the ratio is
> 6.106× on lights, 5.740× on night-sky, 5.692× on post-full — mean **5.85**, confirming wave 2's
> prediction to within 3%. The trap is worth the sentence: the obvious measurement is the wrong one.
>
> **`exposure` 1.0 → 1.3, on the criterion that swapping the operator must not read as a brightness
> change** — the wave-12 sitting has to be judging the operator, not a darkening. Mid-grey 0.18
> linear reads 0.266899 through the Narkowicz fit and 0.214512 through AgX; the multiplier closing
> that is 1.3333. The suite-wide alternative was measured too and **has no single answer**: the
> argmin moves with the norm — 1.0 by display luma, 1.2 by log ratio, 1.6 by absolute linear — so it
> is a property of which scenes happen to be in the gallery rather than of the operators. **The
> finding behind the disagreement is that the two curves cross at ≈0.09 scene luminance**, so no
> multiplier matches both ends; 1.3 anchors the middle, deep-shadow scenes read slightly brighter
> than under ACES and diffuse ones slightly darker, and that consequence is in the field rustdoc
> where an author meets it. `exposure` and `dither` are therefore the two shipped defaults that are
> deliberately **not** decision 9's skip — they are the chain, not an authored effect.
>
> **Whole Windows suite re-blessed**, 23 goldens, SSIM 0.857–0.959, `goldens/macos/**` untouched and
> queued as a bulk entry in the private macOS queue. Set-mean banding **0.7189 → 0.2374** (night-sky
> 0.8979 → 0.2867, sprites 0.9608 → 0.1830); `text` is the only image the dither made *worse*
> (0.0000 → 0.0090 — a flat background has nothing to contour) and is still the lowest in the set.
> Nothing near a clipping ceiling. Diff reports committed: `post-process-wave11-{night-sky,
> post-full,district-plaza}.diff.json` and, for the roster commit, `-post-full-roster.diff.json`.
>
> **Four fixtures changed for reasons that are findings, not accommodations.** (1) **Wave 8's shafts
> banding gate reversed sign.** Under ACES, in-scattered light lifted the deep-shadow gradient out of
> the flat 8-bit steps it sat in and shafts *removed* banding for free; AgX's gentler toe plus
> exposure 1.3 lift those shadows first, so what the march now adds is a broad shallow ramp that
> `banding_score` reads as flat-run content. The gate became the unchanged absolute ceiling plus a
> bounded rise (0.06, ≈2× measured) rather than being deleted — and undithered the same pair reads
> 0.4821 → 0.5349, which is itself the argument for shipping the dither on. (2) **Halation inherits
> the bloom chain's *gain*, not only its threshold and radius** (wave 10 recorded the latter two), so
> the pyramid made it ~6× stronger at the same authored value. (3) **AgX couples the channels**
> through its Rec.2020 inset/outset matrices, so the chromatic-aberration fixture's byte-exact green
> and halation's red:blue ratio both pin `tonemap: none` — the coupling belongs to the operator, not
> to the effect under test. (4) Dither-on-by-default breaks every "two different pixels are
> byte-equal" fixture, fixed at the source rather than by loosening an assertion.
>
> **`post-full.yaml` now authors the whole roster** — the promise seven consecutive wave entries
> deferred to "the flip". Every value came off a `--sweep` sheet or an `image-stats` measurement at
> the golden's own 960×540, under one governing constraint: nothing may be an order of magnitude
> louder than its neighbours, measured as the on/off frame-mean luma each effect costs (shafts +3.3,
> fog +2.1, halation +2.2, vignette −2.4 of 255). That constraint is what caught **shafts**: the moon
> rakes toward the viewer, so every eye ray is lit near the camera and the march has no dark half to
> draw beams against — a shipped-ish 0.05 lifts the frame mean by 7.8, four times the next loudest
> effect. It ships at 0.02.
>
> **`contrast` is the one field left neutral, and that is a finding.** It pivots about 0.5 and almost
> every pixel in a night frame sits below that pivot, so it cannot deepen shadows without pushing
> them through zero: `clipped_low` reads 0.0013 at 1.0, 0.048 at 1.04, 0.100 at 1.08, 0.162 at 1.12
> against a 0.02 ceiling. A positive `lift` cannot rescue it, because the contrast step runs *after*
> the CDL — at 1.12 anything below 0.054 clips and lifting the black point that far is a visible
> fade. The darkening job went to **`gamma`**, whose reciprocal power pins both ends and therefore
> cannot crush, with blue held above red and green so the darkening itself leans the midtones blue.
> **Decision 12's "AgX ships muted and expects a grade on top" is now demonstrated rather than
> asserted:** ungraded, this scene reads as overcast dusk.
>
> **Four gates had gone vacuous, or would have.** The shafts and DoF banding gates compared two
> frames that both authored the effect — and worse, the authored grain floored `banding_score` to
> ~0.03 at *both* ends (0.0324 → 0.0326), so neither could fail at all; with grain off at both ends
> they read 0.2583 → 0.2578 and 0.2552 → 0.2595 and are sensitive again. The supersampling
> `edge_aliasing` gate had the same disease in the other direction: the defocus, the aberration and
> the grain take the 1× baseline from 0.566 to 0.214 — softer than a supersampled frame — so it would
> have measured them instead of the resolve; both ends now switch all three off (0.8181 → 0.4891
> against the 0.15 demanded). **The general lesson for wave 12 and beyond: the all-on scene is the
> host for several gates, and every one of them must state what it switches off.**
>
> **Budgets.** `lights.yaml`'s bloom moved from three `[passes]` lines to one `[post_groups] bloom`
> (the eleven members read 0.0020–0.0123 ms against ~0.001 ms granularity, so a noise-proof per-pass
> ceiling would sit above the value it guards); `composite` 0.035 → 0.08 against 0.0399, the shipped
> chain's own cost. Both `pinned-apple-m4` files delete their stale keys and **invent nothing** — the
> ten pyramid passes have never been measured on that hardware, so they are explicitly ungated with
> the calibration queued, the open half of a criterion rather than a waived one. On post-full all
> four groups were re-measured live: bloom 0.0563, ssao 0.1065, shafts 0.1403, dof 0.0809,
> `post_total_ms` **0.384** across eighteen passes. Every group came in ~10% *under* its calibration
> — **including `bloom`, which was measured live hours earlier and had no reason to move** — so the
> shift is machine-wide (a heavier frame holding higher sustained clocks is the likeliest mechanism)
> rather than per-effect, and **no ceiling was cut on it**: re-cutting on a uniform 10% would chase
> noise into the gate. `post_total_ms` 0.76 sits deliberately *below* the 0.88 the four ceilings sum
> to, since four independent 2× allowances added together would gate the chain at nothing.
>
> **The metric-gaming hazard showed up in the flesh and was recorded, not tuned away.** post-full's
> `edge_aliasing` fell 0.9000 → 0.2144: 0.34 of that is the defocus replacing real detail with a
> gradient (exactly why wave 9 refused this metric as a gate) and **0.15 is the chromatic aberration,
> which anti-aliases nothing whatsoever**. So the aberration ships at 0.0008 — a quarter of what its
> own docs call a normal value — precisely so it stays a lens rather than a cheap way to buy the
> number. The banding fall (0.2097 → 0.0725) is the opposite case and is genuine: 0.2366 grain-off
> against 0.0725 grain-on on the same frame.
>
> **Docs:** only what the deletion made *false* was fixed (`docs/ludion-renderer.md`'s chain diagram
> and pass counts, `docs/ludion-cli.md`'s `[post_groups]` bullet, both re-mirrored). Wave 12 still
> owns the real sweep. Note `TESTING.md` is ignored by both `.loreignore` and `.gitignore`, so its
> correction is local-only. **No GPU access-violation flake sighted** across ~15 full `check.mjs`
> runs despite this wave deleting a shader and a pipeline; `.config/nextest.toml` untouched.
>
> **`PostProcess.ssao_direct` shipped whole 2026-07-29 (Lore rev 405)** — the fraction of the AO
> factor that also attenuates the direct lobes, default `0.0` byte-identical, riding the
> `EnvParams.ao.w` pad: `extract` carries it, `mesh.wgsl` reads `env.ao.w`, and the editor's
> `chain.ts` row makes the hold-to-compare A/B honest. Its component half (rev 402, 2026-07-26) left
> the workspace test build broken — an exhaustive `PostProcess` literal in `extract.rs` — fixed
> 2026-07-27 from the mac. Its renderer half was M4-blessed 2026-08-01 (rev 446, with `post-full` and
> the first `night-street`). **Open:** only the shipped *value* rides a human gate — vigil authors
> `1.0` through the Art Director's grade; judge off
> `render --sweep /entities/0/components/PostProcess/ssao_direct=0,0.2,0.4,0.8` on `post-full` and a
> vigil district.

## Why (the one-paragraph case)

The engine's post stack is three passes and about forty lines of WGSL: a bright-pass, one 5-tap
separable Gaussian at half resolution, and a Narkowicz ACES fit. There is no anti-aliasing
anywhere (`sample_count: 1` in every pipeline), no exposure control, no colour grading, and no
output dither in front of an 8-bit target that now renders authored night-sky gradients. The
frame costs 1.81 ms of a 16.6 ms budget. The gap between what the renderer *shows* and what it
*could* show for that budget is the largest available quality delta in the engine, and it is
concentrated in the one subsystem that is nearly self-contained. This arc closes that gap and,
in doing so, gives the renderer the pass-composition machinery a modern pipeline needs.

**Framing constraint, stated by the user at plan-authoring:** the post pipeline is
**style-agnostic machinery**. The HD-2D look is what vigil authors on top of it, never what the
engine hard-codes. The engine ships neutral defaults and orthogonal operators.

## Decisions settled in the session (2026-07-25)

1. **De-style the renderer's naming.** `build_hd2d_graph` → `build_frame_graph`,
   `dump_hd2d_graph_json` → `dump_frame_graph_json`, and the crate/module docs and
   `KNOWN_PASS_NAMES` prose stop describing the pipeline as HD-2D. The engine is not limited to
   this look (`Plan/roadmap-voxel-mesh-pbr.md` decision 10, now reflected in the code). Prose
   inside `Plan/` is left alone — those are records of what was decided when.
2. **Hybrid pass granularity, by a stated rule.** An effect needing its own sampling footprint,
   resolution, or ping-pong gets its own **conditional graph pass** (bloom pyramid, DoF, SSAO,
   light shafts). An effect that is a pure per-pixel function of already-sampled colour **folds
   into the composite pass** (exposure, fog, tonemap, grade, vignette, CA, grain, dither).
   "Composite" is today's `tonemap` pass, grown up. Rejected: every-effect-its-own-pass (~8
   needless full-res round trips for one-ALU operations, combinatorial golden churn) and one
   uber-pass (cannot express bloom/DoF/SSAO anyway, and destroys `--dump-timings` attribution).
3. **Authoring surface: an opt-in `PostProcess` singleton component**, inline fields, lowest-
   `EntityId` wins, **absent ⇒ today's pipeline byte-for-byte** (the `Environment` /
   `DirectionalLight` precedent). `Environment` keeps sky/ambient/IBL — *lighting*; `PostProcess`
   owns *imaging*. `bloom_threshold`/`bloom_strength` **migrate off `Environment`**; exactly one
   authored file in the repo sets them (`examples/gallery/night-sky.yaml:28`). The trigger
   `Plan/environment-ibl.md` decision 5 recorded ("no PostProcess component before a second post
   feature exists") has fired. Rejected: extending `Environment` (one component, two unrelated
   jobs), a referenced look-profile doc (indirection with no sharing problem to solve — vigil's
   districts already share `Environment` through the map `fragments:` mechanism), and
   camera-attached post (one camera; breaks the absent-is-byte-identical inversion).
4. **Anti-aliasing is supersampling (SSAA), as a client-side render scale.** The whole chain
   renders at 1.5–2× and resolves down after tonemap. Fixes geometric edges, sprite alpha edges,
   specular shimmer and in-world text in one move, with no new shader cleverness and no
   determinism risk. `--scale` is a **client setting** (CLI flag, serve/host option, a shipped
   game's settings item) — supersampling is a quality level, not a look. Rejected: FXAA (blurs;
   no help for specular shimmer — recorded as a trigger-gated cheap path), MSAA (needs
   multisample support in the graph and fixes only geometric edges — the minority of the
   content), and scene-authored scale (content must not dictate the client's GPU budget).
5. **Temporal AA is rejected, with a recorded trigger.** Not for determinism reasons — TAA's
   jitter is a Halton sequence indexed by frame, and it is bit-reproducible. It is rejected
   because (a) it needs motion vectors, i.e. MRT *plus* previous-frame transforms threaded
   through `extract()` for meshes, sprites, billboards and text; (b) a single-frame `ludion
   render` has no history, so goldening TAA means rendering N frames and blessing frame N —
   a change to the whole golden harness; and (c) decisively, **reprojection makes the image a
   function of the camera's history, not of world state**. Two replays reaching the same world
   state by different routes would produce different pixels, and the agent's screenshot query
   would stop being an observation of the world. What is genuinely given up: TAA's amortisation
   of noise, which would let cheap stochastic SSAO, dithered transparency and low-sample shadows
   resolve clean. **Trigger:** the first effect whose quality is gated on stochastic sampling.
6. **Deferred shading is rejected as a direction; clustered forward (Forward+) is the recorded
   answer if light count ever bites.** Deferred is bandwidth-bound and SSAA multiplies exactly
   that; the majority of the content is alpha-blended and cannot pass through a G-buffer, which
   would put the GGX BRDF in two shaders under two shading models; and the 16-light cap plus
   nearest-16 selection is documented as *dormant* — the problem does not exist. **Trigger for
   Forward+:** content that provably exceeds what the cap plus nearest-16 selection can express,
   **measured, not assumed** (vigil is a night city; per-window lighting is the plausible driver).
7. **MRT lands mid-arc with exactly one consumer.** `PassDecl.color` becomes a list, the executor
   builds N attachments, `writes()` keeps its canonical ordering, `dump-graph`'s shape grows, and
   `ludion-shader-bindgen` learns to reflect fragment `@location` outputs so a pipeline's colour-
   target array cannot drift from its WGSL. Its first consumer is the **depth+normal prepass**,
   which is the proto-G-buffer. Rejected: MRT as wave-0 infrastructure with no consumer
   (speculative generality) and shipping velocity in the same wave (engine reach with no defect
   behind it).
8. **SSAO applies through a conditional depth prepass, not after shading.** In a forward
   renderer, by the time the scene pass has written HDR, direct light / ambient / IBL / emissive
   are one number — multiplying that by AO darkens torch highlights and glowing props, which is
   the classic "AO looks like dirt" failure and is the worst case for a night game lit by point
   lights. So: a depth-only-plus-normal prepass runs first, SSAO computes from it, and the scene
   pass multiplies AO **into the ambient + IBL term only**. The prepass exists only when AO is
   enabled, so the frozen byte-identical path is untouched by construction. It also lets the
   scene pass depth-test `Equal`, cutting overdraw on the expensive lit shader.
9. **Golden policy: byte-identical per wave, one defaults flip at the end.** Every wave asserts
   the existing goldens *unchanged*, so each wave is independently provable and bisectable. The
   enforcement technique is normative: **neutral means the code path is SKIPPED by a uniform
   branch, never computed with identity constants** — `x * 1.0` is exact in IEEE, `pow(x, 1.0)`
   is not guaranteed across backends. One final wave flips the engine defaults and re-blesses the
   whole suite once, on both OSes, with a committed SSIM diff report (the G7 theme-wave ritual).
   Rejected: freezing defaults forever (examples are the primary documentation format for agents;
   they would all keep teaching the old look) and re-blessing as you go (a golden re-blessed every
   wave is not a regression detector).
10. **The arc builds its own instrument, before the defect waves that need it.** Two tools:
    a `--sweep` contact sheet for taste calls, and `ludion image-stats` for defects
    (`banding_score`, `edge_aliasing`, `clipped_low`, `clipped_high`), gateable via `--check`.
    **Taste is not measurable; defects are** — the AA, dither and exposure waves must prove
    themselves by moving those numbers, with no human in the loop. No attempt is made to score
    "looks good".
11. **Tuning never touches content: a `--set` render-time override primitive.**
    `--set <json-pointer>=<value>` applies an override to the loaded scene **in memory**;
    `--sweep` is `--set` with a list. This matters because `WorldHash` hashes every registered
    component's data (`hash.rs` rule 4 — there is no render-only exclusion), so an on-disk look
    change invalidates committed replays. With `--set`, the entire tuning loop costs zero replay
    churn and committing a chosen value costs one re-record, exactly as the Environment arc paid.
    Rejected: excluding render-only components from `WorldHash` — `Fade` **is written by game
    systems** (the camera occlusion policy), so excluding it would hide a genuine sim divergence,
    which is the one thing the hash exists to catch. An applied override must be echoed in output
    (an override silently disagreeing with the file is a footgun).
12. **Tonemapping becomes selectable, defaulting to AgX at the flip.**
    `PostProcess.tonemap: agx | aces | neutral | none`. The Narkowicz ACES fit hue-shifts bright
    saturated colours toward yellow then white as they clip — and the content is a night city lit
    by saturated torch light and emissive props, the exact input it handles worst. AgX
    desaturates gracefully toward white the way film does. AgX ships muted and expects a grade on
    top, which wave 5 provides; the pairing is the standard modern one. An enum *value* is not a
    second mechanism.
13. **Roster: light shafts, SSAO and film emulation are in; stylized outlines are out.**
    Volumetric shafts ray-march the shadow map that already ships — the highest drama per
    millisecond available to a night game, needing no new inputs. Film emulation (grain, halation,
    chromatic aberration) folds into composite at near-zero architectural cost, and grain
    genuinely helps 8-bit night gradients. Depth/normal outlines are **deferred as an
    art-direction decision, not a rejection** — the normal target exists after wave 7, so the
    build is cheap whenever the direction is chosen. With a cascaded sun the shafts march the cascades,
    which cover the `3/fog_density` reach (Clockout 0011, `Plan/shadow-mapping.md` decisions 15 and 17).
14. **Budget by effect group, calibrated on a new all-on scene.** `[post_groups]` ceilings
    (bloom, ssao, dof, shafts, composite) summing member passes, plus a `[post] total`; a budget
    entry for an **absent** pass skips rather than fails (conditional passes require it). A mip
    pyramid is one number, not twelve. `examples/gallery/post-full.yaml` is added and serves
    triple duty: budget calibration (nothing else enables these passes under frozen defaults),
    per-effect golden host, and the `--sweep` target.

## Decisions resolved at plan-authoring (stated, objectable)

- **All post parameters are expressed in screen-relative units, never texels.** The current blur
  steps by texel size (`render.rs:1580`), so at 2× render scale it would be half as wide in
  screen terms — raising quality would silently change the look. Same class of bug for grain
  size, DoF radius and CA offset. Enforced by an invariant test: `render@1x` versus
  `downsample(render@2x)` must SSIM high. **A scale change is a quality change, never a look
  change.**
- **In-world `Text` is scene content and is affected by post like any other geometry;
  screen-space `UiText` renders in the overlay pass after all post and is never touched.** This
  is already the architecture. Vigil uses `UiText` throughout, so DoF blurring in-world text is
  a non-issue for the current game and remains correct for the gallery's `text.yaml`.
- **The bloom pyramid is carried alongside the legacy 5-tap path until the defaults flip.** Bloom
  is unconditionally on today, so replacing the algorithm would move every scene's pixels. Under
  decision 9 the pyramid is therefore gated on `PostProcess` presence until wave 11, which flips
  the default **and deletes the legacy path in the same wave**. The second way is temporary and
  its removal is scheduled, not hoped for.
- **`dump-graph` becomes scene-derived.** It today validates the scene and then dumps
  `(width, height, ui=false, sun=false)` (`dump_graph.rs:26`) — it already misreports any scene
  with UI or a sun. With a variable post chain that becomes useless, so it must reflect the graph
  the scene actually renders. The committed snapshot test moves with it.
- **The `(ui, sun)` bools become one `PostChain` value.** The property that makes
  `build_frame_graph` valuable — a pure function of its arguments, no GPU, byte-stable dump — is
  preserved; only the arity stops exploding.
- **Per-shader boilerplate collapses to a registry.** Adding one shader today touches ~12 sites:
  the `ShaderId` enum, the `[ShaderId; 11]` array, four parallel matches (`file_name`,
  `body_source`, `snippets`, `set_pipeline`), two `Renderer` fields, two `Bgls` sites, the
  `new()` line and `build_pipeline`. At twelve effects that is ~144 mechanical edits and a
  `Renderer` with ~50 flat fields. Pipelines and BGLs become `ShaderId`-indexed arrays, deleting
  three of the five matches outright. **No post-process "manager" object is introduced** — the
  frame graph already owns scheduling, aliasing, lifetimes, timings and the dump; a second
  orchestrator above it would be two ways to do one thing.
- **One `fullscreen_effect(pass, id, inputs, uniform_bytes)` helper** replaces
  `threshold_bind_group`, `tonemap_bind_group` and `record_blur`'s bind-group construction —
  three copies of one shape (N texture views + a sampler + one uniform buffer).
- **`scene_depth` becomes `sampled_depth`** in wave 0; fog, DoF, SSAO and any future depth
  consumer are then one descriptor away rather than blocked.
- **All three occlusion modes (`O`: off / dither / top-fade) stay live through the entire arc.**
  The G7 sitting happened; its verdicts are recorded pending the user's closer look at occlusion.
  No wave may assume the cycle has been cut down.
- **Wire surface stays 26.** `PostProcess` rides the registry (append-only) and `component.set`;
  the inspector picks it up from schemars doc-comments. No `post.*` commands — dedicated tooling
  waits for editor demand.
- **L-codes** under the next free `L11xx` at implementation time for `PostProcess` semantic
  validation (finite, non-negative, in-range scalars; enum spelling with a suggestion), with JSON
  Pointer paths as everywhere. `--set` gets its own code for an unresolvable pointer or an
  ill-typed value.

## Waves

### Wave 0 — architecture (zero pixels)
- Rename `build_hd2d_graph` → `build_frame_graph` and `dump_hd2d_graph_json` →
  `dump_frame_graph_json` (20 identifier sites across 5 crates, including the public re-export
  `ludion_engine::dump_hd2d_graph_json`); rename the committed snapshot
  `hd2d_graph_64x64.json` → `frame_graph_64x64.json`; de-style the crate/module docs and
  `KNOWN_PASS_NAMES`.
- `PostChain` value replaces the `(ui, sun)` bools; `dump-graph` becomes scene-derived.
- `ShaderId`-indexed pipeline/BGL registry; `fullscreen_effect()` helper.
- `scene_depth` → `TextureDesc::sampled_depth`.
- `PostProcess` component: fields, schemars ranges, semantic validation, registry append,
  `extract()` → `RenderScene.post`; renderer reads it only for the bloom knobs.
- `bloom_threshold`/`bloom_strength` migrate off `Environment`; fix
  `examples/gallery/night-sky.yaml`.
- **Tests:** all goldens byte-identical; `dump-graph` snapshot updated once for the rename only;
  validation misshapes with JSON Pointer assertions; full `check.mjs`.

### Wave 1 — the instrument
- `--set <json-pointer>=<value>` render-time override (echoed in output, `L####` on a bad
  pointer or value); `--sweep <pointer>=<v,v,…>` rendering a labelled contact sheet PNG.
- `ludion image-stats <png>` → `banding_score`, `edge_aliasing`, `clipped_low`, `clipped_high`;
  `--check <toml>` for CI gating.
- Baseline numbers recorded for the gallery suite and vigil's three districts — the
  before-figures every defect wave must beat.
- **Tests:** stats are deterministic and resolution-stable; synthetic fixtures with known
  banding/aliasing pin each metric; `--set` never writes to disk.

### Wave 2 — bloom pyramid
- 6-level progressive downsample (Karis average on the first level) + tent-filter upsample,
  additive. Gated on `PostProcess` presence; the legacy 5-tap path stays for the frozen default.
- `examples/gallery/post-full.yaml` created here (bloom only) and grown by each later wave.
- **Tests:** goldens byte-identical without `PostProcess`; one new `post-full` golden; budget
  group `bloom` calibrated.

### Wave 3 — SSAA render scale
- `--scale` on `render` / `serve` / `profile`; the resolve pass after tonemap, before overlay,
  so UI text never softens; every post parameter audited to screen-relative units.
- **Tests:** the scale-invariance SSIM test (`render@1x` vs `downsample(render@2x)`);
  `image-stats edge_aliasing` must fall measurably; goldens at scale 1 byte-identical.

### Wave 4 — composite grows up
- Exposure, tonemap operator enum (`agx | aces | neutral | none`), output dither. Uniform-branch
  skip for every neutral path.
- **Tests:** byte-identity with `PostProcess` absent *and* with a fully-neutral `PostProcess`;
  `image-stats banding_score` must fall with dither on.

### Wave 5 — grade
- Contrast, saturation, temperature/tint, lift/gamma/gain, folded into composite.
- **Tests:** identity grade is a uniform-branch skip; a sweep sheet committed as the wave's
  artifact.

### Wave 6 — fog and vignette
- Depth fog + height fog (consuming wave 0's sampled depth) and vignette, folded.
- **Tests:** fog off is a skip; a `post-full` golden update.

### Wave 7 — MRT, depth+normal prepass, SSAO
- `PassDecl.color` → list; executor attachments; `writes()` canonical ordering; `dump-graph`
  shape; bindgen fragment `@location` reflection plus its drift gate.
- Conditional prepass writing `[depth, normal]`; scene pass depth-tests `Equal` when the prepass
  is present. SSAO + blur; the mesh shader multiplies AO into the ambient + IBL term only.
- **Tests:** goldens byte-identical with AO absent (the prepass must not exist); AO affects
  neither emissive nor direct-light pixels (a fixture proving it); budget groups `prepass` and
  `ssao`.

### Wave 8 — volumetric light shafts
- Half-resolution ray-march of the existing sun shadow map with dithered step offsets, blurred,
  added in composite.
- **Tests:** determinism across processes; banding bounded by `image-stats`; budget group
  `shafts`.

### Wave 9 — depth of field
- Circle-of-confusion from depth, half-res near/far gather, composited back. Focus model
  authored as one field (distance-based or tilt-shift by screen height).
- **Tests:** in-focus configuration is a skip; `post-full` golden; budget group `dof`.

### Wave 10 — film emulation
- Grain (deterministic from frame index + pixel hash), halation, chromatic aberration — folded.
- **Tests:** determinism of grain across processes; each off by default and skipped.

### Wave 11 — the defaults flip
- Flip the shipped defaults: AgX, dither on, pyramid bloom, a sane exposure. **Delete the legacy
  5-tap bloom path.** One whole-suite re-bless on both OSes with the SSIM diff report committed;
  budget files re-calibrated in both `budgets/` and `games/vigil/budgets/`.

### Wave 12 — vigil adopts, docs, close
- Vigil authors its `PostProcess` in the shared fragment (plaza via the map fragment +
  regenerate + `map-drift` green; market and cathedral by direct edit); one `run.lrp` re-record;
  `district-plaza` golden re-bless; the macOS re-bless queue entry in the private macOS queue
  updated in place.
- Docs: `ludion-renderer`, `ludion-render-graph` and `determinism` topics; a "Post-processing"
  chapter in the engine guide; `embedded-docs/` re-mirrored via `check.mjs --fix`.
- **Exit gate:** one human sitting on the dressed build, judging the new look against committed
  sweep sheets.

## Deferred, with triggers

- **Temporal AA** — trigger: the first effect whose quality is gated on stochastic sampling
  (SSR, stochastic SSAO or transparency). Requires the velocity target and a golden-harness
  change; see decision 5.
- **Motion blur / velocity target** — the MRT seam exists after wave 7. Note it is *not* ruled
  out by determinism the way TAA is: with previous transforms held as world state, velocity is a
  pure function of world state and needs no history image. Trigger: on demand.
- **Clustered forward (Forward+)** — trigger: measured light-count pressure past the 16-light cap
  plus nearest-16 selection. Deferred shading is rejected as a direction, not merely deferred.
  **Answered by `Plan/clustered-forward.md`**: the trigger fired (torch-lit districts plus
  light-emitting projectiles), the cap and the selection are deleted rather than raised, and decision
  6's rejection of deferred was re-examined and re-settled there — one lighting architecture.
- **MRT (`PassDecl.color` as a list)** — deferred at wave 7, which was supposed to build it.
  Decision 7's rule is "exactly one consumer", and the depth+normal prepass turned out not to be one:
  the graph already carries a colour attachment *and* a depth attachment separately, so the prepass
  is one colour target plus the existing depth slot. **Trigger:** the first pass that genuinely writes
  two *colour* targets. Wave 9's near/far DoF gather was the next candidate and turned out not to be
  one: the scatter-as-gather weight decides "may this tap reach me" from the tap's own circle of
  confusion, so a sharp foreground scatters nowhere and a blurred one reaches out over what is behind
  it — the sign of the CoC selects an occlusion rule, not a pass. One gather, one target. The trigger
  stands unchanged and now has no named candidate behind it.
- **The scene pass's `Equal` depth test over the prepass** — deferred at wave 7. It needs the scene
  pass to render into a depth attachment an earlier pass wrote, and the graph derives ordering edges
  from *reads* only: it has no write-after-write concept, so two passes writing one resource are
  unordered in principle. That is a data-model addition with no second consumer, and the Risks
  section already flags the two-depth-policies hazard. **Trigger:** measured overdraw pressure on the
  lit shader. Until then an AO-enabled frame rasterizes geometry twice, which is opt-in.
- **Stylized depth/normal outlines** — an art-direction decision. The normal target exists after
  wave 7; the build is cheap whenever the direction is chosen. It would also be the second consumer
  that earns the prepass its own budget group.
- **FXAA as a cheap AA path** — trigger: the first platform that cannot afford supersampling
  (see `Plan/console-strategy.md`).
- **`.cube` LUT colour grading** — trigger: a look analytic grading cannot express. Note a
  `.cube` file is ASCII, so it does not fire the binary-asset objection; the objection is that an
  agent cannot author a 32³ LUT by hand.
- **ID target / editor picking without a CPU raycast** — trigger: editor selection ergonomics or
  perf. Rides the MRT seam.
- **Screen-space reflections** — trigger: content with wet or reflective surfaces. **Answered by
  `Plan/reflections.md`** (2026-10-19): the trigger fired. SSR ships cone-traced over a Hi-Z and a
  pre-convolved colour pyramid, with no history, so it does not wait on TAA. Temporal SSR carries
  that plan's own trigger.

## Risks

- **AgX correctness and cost.** A compact fit must be validated against reference on both pinned
  GPUs before it becomes the default; a wrong fit is a whole-suite re-bless spent badly.
- **The conditional scene-pass depth policy.** `Equal` with a prepass versus `Less` without is
  two policies in one pass; coplanar geometry is where that bites. The absent-AO path must be
  proven byte-identical, not assumed.
- **Two bloom implementations from wave 2 to wave 11.** Scheduled for deletion in wave 11; if
  that wave slips, the duplication is a standing violation of the working conventions.
- **The macOS re-bless queue is already backed up**, and wave 11 adds a
  whole-suite entry to it. The TeamCity mac-agent bring-up remains deferred and UI-side.
- **Metric gaming.** `image-stats` numbers are a gate on defects, not a target to optimise; a
  wave that moves the number without moving the image is a failure that the sweep sheet, not the
  metric, must catch.
