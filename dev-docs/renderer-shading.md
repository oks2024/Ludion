---
description: The renderer's Cook-Torrance mesh shading model: the BRDF lobes, light units, and the sun term
---
# Mesh shading (Cook-Torrance)

**Scope:** The shading math `mesh.wgsl` evaluates per fragment. The image-based lighting that supersedes its ambient branch is `ludion devdocs renderer-ibl`; editing the WGSL itself is `ludion devdocs renderer-shaders`.

`mesh.wgsl` is the one shading-model change site; `sprite.wgsl`'s `shade_two_sided()` is untouched
Lambert and reads none of these uniforms. Pinned math, not to be casually retuned — a change here
re-blesses every mesh-bearing golden:

- **D** — GGX/Trowbridge-Reitz (`distribution_ggx`), `α = roughness²`; perceptual roughness is
  clamped to `>= 0.045` in-shader (the GGX singularity at 0).
- **V** — Smith height-correlated visibility (`visibility_smith`), the `1 / (4·ndl·ndv)` denominator
  folded in.
- **F** — Schlick (`fresnel_schlick`), `F0 = mix(vec3(0.04), albedo, metallic)`.
- **Diffuse** — `(1 − metallic) × albedo`, no `1/π` (the light-unit convention below).
- **Light units** — a `PointLight`'s premultiplied color is the Lambert response at `ndl = 1` (no
  `1/π` on the diffuse lobe, `atten²`-ranged; a `SpotLight` is the same times its cone factor,
  `ludion devdocs renderer-clustering`); the specular lobe is scaled by `π` (`π·D·V·F`) so the
  two lobes stay physically proportioned against that convention. Break the convention on one lobe
  and every lit scene changes brightness rather than gaining highlights.
- **Multi-scatter energy** — every specular site (point/spot loop, sun, sun disc, and the sky, probe
  and SSR fills) is scaled by `dfg_energy = 1 + F0·(1/(a+b) − 1)` from the split-sum LUT
  (`snippets/dfg.wgsl`), with or without an `Environment`: it is a BRDF property. With an
  `Environment`, the IBL diffuse and the ambient floor are scaled by `1 − (F0·a + b)`, the energy the
  specular took.
- **Ambient** — `ambient.rgb × albedo` when the scene carries no `Environment`. An `Environment`
  supersedes this branch entirely with split-sum image-based lighting
  (`ludion devdocs renderer-ibl`).
- **Emissive** — `albedo × emissive` added to the fragment before tonemapping; values above `1.0`
  bloom through the untouched threshold/blur/composite passes, with no new pass or uniform.
- **View vector** — `normalize(camera.cam_pos.xyz − world_pos)`.
- **Sun** — a `DirectionalLight`, exposed to `mesh.wgsl` as a mesh-only `@group(2)` (`struct Sun`,
  `shadow_tex: texture_depth_2d`, `shadow_samp: sampler_comparison`) so the shared `@group(1)`
  clustered-light payload — and therefore `sprite.wgsl` and its goldens — stays untouched by
  construction. The sun term is the *same* D/V/F lobes and light-unit convention as a point light,
  with no distance attenuation, multiplied by a 3×3 PCF shadow factor
  (`textureSampleCompareLevel`, hardware comparison sampling, 9 taps). A sunless frame binds a
  zeroed `Sun` and a persistent 1×1 fallback depth texture at group 2 — no shader permutation, the
  term simply evaluates to zero. Only the mesh path reads group 2: sprites, billboards and text
  neither cast into the shadow map nor sample it (`Plan/shadow-mapping.md` decision 1).
  With cascades the `Sun` uniform also carries per cascade `cascade_space` (world→light), `cascade_rect`
  (the tile's UV rect in the atlas) and `cascade_params` (near, far, bias in that cascade's light-NDC),
  plus `view_depth` and `cascade_count`. `sun_shadow_cascaded` picks the cascade by view depth, blends
  across a band of 10% of its depth at each seam, fades to lit across the last band and clamps PCF
  inside the tile; the bias is held at the same texel count per cascade. `sun_shadow` calls
  `sun_shadow_anchored` verbatim at count 0, which is the bit-identity rule for a sun without cascades
  (`ludion devdocs renderer-quality-axes`, `Plan/shadow-mapping.md` decisions 12–17).
- **Local lights' shadows** — opt-in (`cast_shadows`), also mesh-only group 2, beside the sun:
  `3 local_shadow_tex: texture_depth_2d`, `4 local_views: array<LocalView>`, `5 light_shadow: array<u32>`
  (the light's first view index, `0xFFFFFFFF` unshadowed). The 48-byte `Light` record, the GI shaders and
  the sprite layouts are unchanged. Inside the clustered loop each light's radiance is multiplied by
  `local_shadow`, which returns an exact `1.0` without sampling for an unshadowed or ungranted light, so
  those lights shade bit-identically. A granted spot samples its tile; a point light picks its cube face
  by major axis. PCF reuses the sun's kernel with taps clamped inside the tile, and the bias is applied in
  world space before projection (`Plan/shadow-mapping.md` decisions 9–11).
- **Material selection** — per-instance, not per-mesh-kind: `MeshInstanceRaw`'s material vec4 is
  `(roughness, metallic, emissive, use_vertex_material)`, and `vs_main` does
  `mix(instance_material, vertex_material, flag)`. So any mesh that authors its own material (a
  voxel palette entry, a mesh-doc per-vertex list) uses it entirely, and the `Mesh` component's
  fields apply only to meshes that author none — the full precedence rule and the three author
  surfaces are `ludion docs materials`.
- **Texture maps** — an asset mesh's up-to-five maps ride a per-draw `@group(4)`: five
  `texture_2d<f32>` (`base_color_tex`/`metallic_roughness_tex`/`normal_tex`/`occlusion_tex`/
  `emissive_tex`), one shared sampler, and a 16-byte `MeshMaterial { flags, normal_scale,
  occlusion_strength }` uniform. `MeshVertex` grows two lanes over the untextured layout — `uv` and
  `tangent` — so a mesh's full vertex struct is six lanes (position, normal, color, material, uv,
  tangent; locations 0–5), with the per-instance lanes following at 6–17.
- **Cutout and double-sided ride the same uniform.** `MATERIAL_ALPHA_MASK` and `MATERIAL_DOUBLE_SIDED` are two more `material.flags` bits and `alpha_cutoff` is the lane that was `pad0`, so `MeshMaterial` stays 16 bytes. A `mask` fragment whose coverage (`Mesh.color.a` × base-map alpha) is under the cutoff is discarded; on a double-sided back face the whole frame flips (the final normal is negated after the normal map, so relief matches). Clear bits keep the historical arithmetic, which is why opaque culled content is byte-identical.
- **The flag rule is what keeps an untextured draw byte-identical.** Every map sample in `fs_main`
  sits behind its own `material.flags` bit (`MATERIAL_BASE_COLOR`/`MATERIAL_METALLIC_ROUGHNESS`/
  `MATERIAL_NORMAL`/`MATERIAL_OCCLUSION`/`MATERIAL_EMISSIVE`); an untextured mesh binds a flags-zero
  material and takes the exact pre-texture arithmetic, with no texture read at all — byte-identical
  by construction, not by a separate code path that happens to agree.
- **Base color and metallic-roughness multiply** the vertex/instance albedo and `(roughness,
  metallic)` respectively (glTF packing: `G` = roughness, `B` = metallic). **Normal replaces, not
  nudges**: the fragment shader builds a TBN frame from the interpolated world normal and tangent
  (Gram-Schmidt re-orthogonalized against the normal, bitangent from the tangent's handedness `w`, which `vs_main` multiplies by the instance's `fade.w` sign so a reflected model keeps a correct frame),
  samples the map, applies `normal_scale` to its XY, and the result *replaces* `world_normal`
  outright. **Occlusion multiplies the ambient/IBL term at exactly the site SSAO does, and nowhere
  else** — direct light and emissive stay unattenuated by it, by the same "AO looks like dirt"
  reasoning that governs SSAO — scaled by `occlusion_strength` (`0` is a no-op). **Emissive is
  independent of base color**, per glTF: `emissive_texture` multiplies the per-vertex emissive
  intensity whether or not a base-color map is bound.
- **Colour spaces**: `base_color_tex` and `emissive_tex` decode sRGB (authored colour);
  `metallic_roughness_tex`, `normal_tex` and `occlusion_tex` decode linear (measurements). The GPU
  texture format carries this (`Rgba8UnormSrgb` vs. `Rgba8Unorm`), and the upload cache keys on
  `(content hash, sRGB, encoding)` so one PNG bound sRGB by one material and linear by another cannot alias.
- **A cooked map carries its own mips** (BC7/ASTC, uploaded level by level, no mipgen).
- **Mips are GPU-generated**, a blit chain (`mipgen.rs`) reusing `blit.wgsl`, trilinear filtering
  with anisotropy 16 on every material sampler (repeat/clamp/mirror, one sampler per wrap mode).

## Gotchas
- Metals go dark under ambient-only light in an `Environment`-less scene — accepted physics of the
  scope, not a bug: `F0 = albedo` for a metal and the diffuse lobe is zero, so a flat ambient term
  has nothing to reflect. Author lighting (more or closer point lights, or an `Environment`) rather
  than fighting the BRDF.
- **Anisotropic filtering is more sensitive to driver updates than plain trilinear** — a driver bump
  may force a per-OS golden re-bless at a few grazing-angle texels on a textured mesh. Re-blessable,
  not a signal of a regression; the untextured suite is unaffected (it binds no material texture).

The frame graph and this crate's layout are `ludion devdocs ludion-renderer`; the per-pass walk is `ludion devdocs renderer-passes`; which lights a fragment's loop actually visits is `ludion devdocs renderer-clustering`.
