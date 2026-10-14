# Finding: glTF importer — panics inside the `gltf` crate on structurally incomplete documents

**Target:** `gltf_import` (`ludion_import::import`, `ExternalFiles::Refused`)
**Severity:** panic on untrusted input (`ludion import` on a hostile or truncated `.gltf`/`.glb`)
**Status:** FIXED — `ludion-import`'s `document.rs::check_structure` runs on the parsed JSON before the crate's validator and wrappers see it; each reproducer is an `L1201`, pinned by the regression tests in `crates/ludion-import/tests/import.rs` (`an_image_without_a_source_is_l1201`, `a_missing_accessors_array_is_l1201`). The same class turned up two more panics in the crate — an image naming a `bufferView` without a `mimeType`, a camera missing the projection object its `type` names — and `check_structure` covers them, with tests.
**Discovered by:** `mutation_driver 20000` over `corpus/gltf_import/scene.gltf`; each reproducer is
that seed (`examples/import/scene.gltf`, 5650 bytes) with one byte deleted.

## Reproducers

| file | mutation | panic |
|------|----------|-------|
| `image-without-source.gltf` | `"uri"` → `"ur"` on the image entry, so it has no source | `gltf-1.4.1/src/image.rs:128:46` — `Option::unwrap()` on `None` |
| `accessors-key-missing.gltf` | `"accessors"` → `"accesors"`, so the array is absent and index 0 is out of range | `gltf-json-1.4.1/src/mesh.rs:151:43` — index out of bounds (len 0, index 0) |

Repro (either file):
```rust
let bytes = std::fs::read("image-without-source.gltf").unwrap();
let _ = ludion_import::import(&bytes, ludion_import::ExternalFiles::Refused, false);
```
