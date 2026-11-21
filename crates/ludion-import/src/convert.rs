//! Pure glTF → Ludion conversions: a primitive's geometry into a [`MeshDocument`], a node's
//! quaternion rotation into the engine's Euler XYZ degrees, and the small component values (`Mesh`,
//! `Transform`, `Camera`, `PointLight`, `Parent`) as `serde_json::Value`s.
//!
//! # The f32 round-trip rule
//!
//! Every float that lands in a component value is stored as `f64::from(some_f32)` — the exact `f64`
//! that an `f32` widens to. The engine deserializes these components into `f32`-typed structs and
//! re-serializes them on `scene.save`; storing the exact `f32` value means that round-trip is
//! byte-identical (the writer prints an `f32`-schema field as its shortest f32 decimal both times).
//! glTF vertex/camera/color data is `f32` at the source, so this is loss-free, and it is what makes
//! an imported scene a canonical fixed point (`scene.load` → `scene.save` reproduces it). Mesh and
//! clip documents are `f64`-typed and keep the widened digits.

use ludion_scene::{AlphaMode, MeshDocument, MeshTextures};
use serde_json::{Map, Value, json};

/// A JSON number holding the exact `f64` that `v` widens to (the f32 round-trip rule).
fn f32_num(v: f32) -> Value {
    Value::Number(
        serde_json::Number::from_f64(f64::from(v))
            .expect("the importer rejects non-finite floats before building values"),
    )
}

fn vec3(v: [f32; 3]) -> Value {
    Value::Array(v.into_iter().map(f32_num).collect())
}

fn vec4(v: [f32; 4]) -> Value {
    Value::Array(v.into_iter().map(f32_num).collect())
}

/// The `Transform` component value from a node's decomposed TRS. Emits all three fields (matching
/// the engine's serde output) so the scene is a canonical fixed point.
pub(crate) fn transform_value(
    translation: [f32; 3],
    rotation_deg: [f32; 3],
    scale: [f32; 3],
) -> Value {
    let mut map = Map::new();
    map.insert("translation".into(), vec3(translation));
    map.insert("rotation".into(), vec3(rotation_deg));
    map.insert("scale".into(), vec3(scale));
    Value::Object(map)
}

/// The `Mesh` component value: an asset `shape` (serde external tagging, `{asset: "<path>"}`), a
/// flat `color` from the material's base-color factor, and the three material scalars read off the
/// glTF material (`roughnessFactor`/`metallicFactor`/the proportional-emissive rule — the engine
/// always serializes them, so the scene round-trips). The mesh shader's Cook-Torrance lobes read
/// all three. `wrap` / `scatter_color` come from the material's `extras` and are emitted only when
/// non-default.
pub(crate) fn mesh_value(
    asset_path: &str,
    color: [f32; 4],
    roughness: f32,
    metallic: f32,
    emissive: f32,
    wrap: f32,
    scatter_color: [f32; 3],
) -> Value {
    let mut value = json!({
        "shape": { "asset": asset_path },
        "color": vec4(color),
        "roughness": f32_num(roughness),
        "metallic": f32_num(metallic),
        "emissive": f32_num(emissive),
        // glTF has no hit-flash concept; emit the component's own defaults so the imported scene is a
        // canonical fixed point under `scene.save` (which serializes the full `Mesh`).
        "flash": f32_num(0.0),
        "flash_color": vec3([1.0, 1.0, 1.0]),
    });
    // Written only when authored, as the component skips them at their defaults.
    if let Value::Object(map) = &mut value {
        if wrap != 0.0 {
            map.insert("wrap".into(), f32_num(wrap));
        }
        if scatter_color != [1.0; 3] {
            map.insert("scatter_color".into(), vec3(scatter_color));
        }
    }
    value
}

/// The `PointLight` component value. Emits `color`, `intensity`, and `range`, the fields glTF
/// carries; the shadow fields stay at their omitted defaults (cast off), so the scene round-trips.
pub(crate) fn point_light_value(color: [f32; 3], intensity: f32, range: f32) -> Value {
    json!({ "color": vec3(color), "intensity": f32_num(intensity), "range": f32_num(range) })
}

/// The `SpotLight` component value: the fields glTF carries, the shadow fields at their omitted
/// defaults (cast off).
/// `direction` is world-space; the angles are half-angles in degrees.
pub(crate) fn spot_light_value(
    color: [f32; 3],
    intensity: f32,
    range: f32,
    direction: [f32; 3],
    inner_angle: f32,
    outer_angle: f32,
) -> Value {
    json!({
        "color": vec3(color),
        "intensity": f32_num(intensity),
        "range": f32_num(range),
        "direction": vec3(direction),
        "inner_angle": f32_num(inner_angle),
        "outer_angle": f32_num(outer_angle),
    })
}

/// The `Parent` component value: a reference by document position (`generation` is always 0 in a
/// scene; see `L1017`).
pub(crate) fn parent_value(index: usize) -> Value {
    json!({ "index": index, "generation": 0 })
}

/// Builds a [`MeshDocument`] from a primitive's decoded attributes, already flattened to the mesh
/// format's `f64` storage. `positions` is required; `normals`/`uvs`/`tangents` are carried when
/// present (normals omitted → faceted normals computed at load, tangents absent → no normal map
/// asked for a tangent frame); `indices` is a triangle list, generated trivially
/// (`0..vertex_count`) for a non-indexed primitive by the caller.
pub(crate) fn mesh_document(
    name: String,
    positions: Vec<f64>,
    normals: Option<Vec<f64>>,
    uvs: Option<Vec<f64>>,
    tangents: Option<Vec<f64>>,
    textures: MeshTextures,
    indices: Vec<u32>,
) -> MeshDocument {
    MeshDocument {
        name: Some(name),
        // An imported mesh is content from that moment on, not the product of a generator this
        // repo owns, so it carries no derivation marker.
        generated_from: None,
        origin: None,
        positions,
        normals,
        uvs,
        tangents,
        emissive: None,
        metallic: None,
        roughness: None,
        textures,
        alpha_mode: AlphaMode::Opaque,
        double_sided: false,
        skin: None,
        indices,
        rim: None,
        lods: Vec::new(),
        surfaces: None,
    }
}

/// Flattens `[x, y, z]` triples into a flat `f64` list (the mesh format's storage), applying the f32
/// round-trip rule per component.
pub(crate) fn flatten3(v: &[[f32; 3]]) -> Vec<f64> {
    flatten(v.iter().flat_map(|p| p.iter()))
}

pub(crate) fn flatten2(v: &[[f32; 2]]) -> Vec<f64> {
    flatten(v.iter().flat_map(|p| p.iter()))
}

pub(crate) fn flatten4(v: &[[f32; 4]]) -> Vec<f64> {
    flatten(v.iter().flat_map(|p| p.iter()))
}

fn flatten<'a>(components: impl Iterator<Item = &'a f32>) -> Vec<f64> {
    components.map(|&c| f64::from(c)).collect()
}
