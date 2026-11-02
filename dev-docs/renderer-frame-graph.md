---
description: The renderer frame graph: spine, conditional chains, tail variants, render-vs-output extents
---
# Renderer frame graph

**Scope:** The shape `build_frame_graph(FrameSize, PostChain)` produces and why. The crate, its rules and entry points are `ludion devdocs ludion-renderer`; what each pass does is `ludion devdocs renderer-passes`.

## Dataflow
The graph is a pure function of `(FrameSize, PostChain)`, which is what makes `ludion dump-graph` CPU-only and byte-stable: it derives the chain from the scene it names, so it reports the graph that scene actually renders.

```
                                                                     [sun == true only]
shadow (2048² depth-only) ──────────────────────────────────────────────────┐
                                                                              ▼ (sampled)
scene (HDR, full res) ─► bloom_threshold ─► bloom_down_1 ─► … ─► bloom_down_5     (level L is width >> (L+1))
   │                            │                 │                    │
   │                            ▼                 ▼                    ▼
   │                      bloom_up_0  ◄──── bloom_up_1  ◄─ … ◄─ bloom_up_4        (tent-filtered coarse + own level)
   │                            │
   └────────────────────────────┴─────────────────────────────────► composite ─► final (Rgba8UnormSrgb)   [ui == false]
                                                                          ▼
                                                                         ldr (Rgba8UnormSrgb; Rgba16Float under a resample) ─► overlay ─► final    [ui == true]
```

The three content-conditional chains hang off that spine, each one ordered purely by the read its consumer declares:

```
prepass ─► ssao ─► ssao_blur ─► scene ─► …          [ssao]    the scene pass consumes it, so it runs in front
scene ─► shafts ─► shafts_blur ─► composite         [shafts]  the composite consumes it, so it runs behind
scene ─► dof_coc ─► dof ─► composite                [dof]     likewise
```

and the tail is at most three passes, each writing the output directly when it is the last:

```
composite ─► final                                                [native, no UI]
composite ─► ldr ─► overlay ─► final                              [native, UI]
composite ─► ldr ─► resample ─► final                             [scaled, no UI]
composite ─► ldr ─► resample ─► ldr_resolved ─► overlay ─► final  [scaled, UI]
```

**Which extent sizes which target** is normative: only the scene pass's colour and depth targets, the AO prepass's two targets, and `ldr` take the **render** extent; every other target takes the **output** extent. That is what makes a render scale a pure quality change, in both directions — below native the half-extent post targets are *larger* than the scene the chain drew, which is the same statement read the other way round. A filter whose footprint is in *target*-texel units — the bloom pyramid, `box_blur`, the DoF disc — is screen-relative only while its target stays a fixed fraction of the *frame*; sizing those off the render buffer would keep the tap offsets identical while halving the fraction of the image they cover, so the glow would narrow and the beams sharpen every time quality went up. The prepass is the exception that proves the rule: it is *geometry*, not post, so it matches the scene pass it substitutes for. One known consequence: the bright pass takes a single bilinear tap, so above scale 1 it reduces the scene by more than 2:1 in that tap.

`resample` above is one pass, either `resolve` (an area-weighted box down, above native) or `upscale` (bilinear up, below it). The box filter degenerates to nearest-neighbour under magnification, which is why the second direction needs its own shader rather than a wider ratio through the first.

Each pass writes a distinct virtual resource — ping-ponging into a read resource would be a graph cycle. `final` excludes `TextureBinding` usage, so `overlay` cannot read it; the `ldr` transient is the seam that lets `overlay` read what `composite` wrote. Under a resample `ldr` is half-float (`PostChain::resample_owns_output`), because `resolve`/`upscale` then own the grain and dither and their sub-LSB offsets need bits below 8; a debug view keeps it 8-bit.

What each pass then does per frame, and the composite imaging chain: `ludion devdocs renderer-passes`.
