//! The rendered preview `ludion texture build --preview` writes: a sphere, a rounded cube and a
//! panel wearing the built map set, lit by a day sky and one key light, rendered as one merged model.
//!
//! The three shapes are merged into one `preview.mesh.yaml` in the feedback folder that references
//! the maps by relative path, so the preview exercises the same loader a game does. One document
//! renders in place; a lineup would stage copies of the mesh without its textures.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};

use ludion_core::atomic::write_atomic;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::f64::{cos, sin};
use ludion_engine::{ModelSheetRequest, ModelView};
use ludion_scene::{MeshDocument, MeshTextures, TextureRef, emit_mesh};

const WIDTH: u32 = 1536;
const HEIGHT: u32 = 640;
const SPHERE_SEGMENTS: (usize, usize) = (48, 24);
const CUBE_GRID: usize = 12;
const CUBE_BEVEL: f64 = 0.15;

/// A day sky and one key light from up-left of the iso camera.
const RIG: &str = "\
format: 1
entities:
  - name: environment
    components:
      Environment:
        sky_zenith:
          - 0.25
          - 0.45
          - 0.85
        sky_horizon:
          - 0.7
          - 0.8
          - 0.9
        ibl_intensity: 1.0
  - name: key
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - -56.0
      DirectionalLight:
        direction:
          - 0.25
          - -0.6
          - -0.75
        color:
          - 1.0
          - 0.96
          - 0.9
        intensity: 2.5
";

/// What the preview needs to know about the built map set.
pub struct MapRefs {
    pub base_color: String,
    pub metallic_roughness: String,
    pub normal: String,
    pub occlusion: String,
    pub emissive: Option<String>,
}

struct Geometry {
    positions: Vec<f64>,
    normals: Vec<f64>,
    uvs: Vec<f64>,
    indices: Vec<u32>,
}

impl Geometry {
    fn new() -> Geometry {
        Geometry {
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
        }
    }

    fn vertex(&mut self, position: [f64; 3], normal: [f64; 3], uv: [f64; 2]) -> u32 {
        self.positions.extend(position);
        self.normals.extend(normal);
        self.uvs.extend(uv);
        (self.positions.len() / 3 - 1) as u32
    }

    /// Pushes a triangle wound so its geometric normal agrees with the vertex normals.
    fn triangle(&mut self, a: u32, b: u32, c: u32) {
        let at = |i: u32| {
            let i = i as usize * 3;
            [
                self.positions[i],
                self.positions[i + 1],
                self.positions[i + 2],
            ]
        };
        let (pa, pb, pc) = (at(a), at(b), at(c));
        let e1 = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let e2 = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let cross = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let n = |i: u32| {
            let i = i as usize * 3;
            [self.normals[i], self.normals[i + 1], self.normals[i + 2]]
        };
        let sum: [f64; 3] = std::array::from_fn(|k| n(a)[k] + n(b)[k] + n(c)[k]);
        let facing: f64 = (0..3).map(|k| cross[k] * sum[k]).sum();
        if facing < 0.0 {
            self.indices.extend([a, c, b]);
        } else {
            self.indices.extend([a, b, c]);
        }
    }

    /// Moves every vertex by `by`.
    fn translated(mut self, by: [f64; 3]) -> Geometry {
        for p in self.positions.chunks_mut(3) {
            for (c, d) in p.iter_mut().zip(by) {
                *c += d;
            }
        }
        self
    }

    /// Appends `other`, offsetting its indices past this geometry's vertices.
    fn append(&mut self, other: Geometry) {
        let offset = (self.positions.len() / 3) as u32;
        self.positions.extend(other.positions);
        self.normals.extend(other.normals);
        self.uvs.extend(other.uvs);
        self.indices
            .extend(other.indices.iter().map(|i| i + offset));
    }

    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.triangle(a, b, c);
        self.triangle(a, c, d);
    }
}

fn sphere() -> Geometry {
    let (columns, rows) = SPHERE_SEGMENTS;
    let mut g = Geometry::new();
    for j in 0..=rows {
        let v = j as f64 / rows as f64;
        let theta = PI * v;
        for i in 0..=columns {
            let u = 2.0 * i as f64 / columns as f64;
            let phi = PI * u;
            let n = [sin(theta) * cos(phi), cos(theta), sin(theta) * sin(phi)];
            g.vertex([n[0] * 0.5, n[1] * 0.5, n[2] * 0.5], n, [u, v]);
        }
    }
    let stride = (columns + 1) as u32;
    for j in 0..rows as u32 {
        for i in 0..columns as u32 {
            let a = j * stride + i;
            g.quad(a, a + stride, a + stride + 1, a + 1);
        }
    }
    g
}

fn rounded_cube() -> Geometry {
    let mut g = Geometry::new();
    let inner = 0.5 - CUBE_BEVEL;
    let stride = (CUBE_GRID + 1) as u32;
    for axis in 0..3 {
        for sign in [-1.0, 1.0] {
            let base = g.positions.len() as u32 / 3;
            for gv in 0..=CUBE_GRID {
                for gu in 0..=CUBE_GRID {
                    let (u, v) = (gu as f64 / CUBE_GRID as f64, gv as f64 / CUBE_GRID as f64);
                    let mut p = [0.0; 3];
                    p[axis] = 0.5 * sign;
                    p[(axis + 1) % 3] = u - 0.5;
                    p[(axis + 2) % 3] = v - 0.5;
                    let q = p.map(|c| c.clamp(-inner, inner));
                    let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    let n = d.map(|c| c / len);
                    let pos = [0, 1, 2].map(|k| q[k] + CUBE_BEVEL * n[k]);
                    g.vertex(pos, n, [u, v]);
                }
            }
            for gv in 0..CUBE_GRID as u32 {
                for gu in 0..CUBE_GRID as u32 {
                    let a = base + gv * stride + gu;
                    g.quad(a, a + 1, a + stride + 1, a + stride);
                }
            }
        }
    }
    g
}

fn panel() -> Geometry {
    let mut g = Geometry::new();
    let n = [0.0, 0.0, 1.0];
    let a = g.vertex([-1.0, 0.0, 0.0], n, [0.0, 2.0]);
    let b = g.vertex([1.0, 0.0, 0.0], n, [2.0, 2.0]);
    let c = g.vertex([1.0, 2.0, 0.0], n, [2.0, 0.0]);
    let d = g.vertex([-1.0, 2.0, 0.0], n, [0.0, 0.0]);
    g.quad(a, b, c, d);
    g
}

fn document(name: &str, g: Geometry, refs: &MapRefs, recipe: &str) -> MeshDocument {
    let tangents = ludion_import::compute_tangents(&g.positions, &g.normals, &g.uvs, &g.indices);
    MeshDocument {
        name: Some(name.to_string()),
        generated_from: Some(recipe.to_string()),
        positions: g.positions,
        normals: Some(g.normals),
        uvs: Some(g.uvs),
        tangents: Some(tangents),
        textures: MeshTextures {
            base_color: Some(TextureRef::new(&refs.base_color)),
            metallic_roughness: Some(TextureRef::new(&refs.metallic_roughness)),
            normal: Some(TextureRef::new(&refs.normal)),
            occlusion: Some(TextureRef::new(&refs.occlusion)),
            emissive: refs.emissive.as_deref().map(TextureRef::new),
        },
        indices: g.indices,
        ..MeshDocument::default()
    }
}

/// What a render attempt produced.
pub enum Outcome {
    Rendered(PathBuf),
    /// No GPU adapter on this machine; the reason is reportable and not a failure.
    NoAdapter(String),
}

/// Writes `preview.mesh.yaml` (sphere, rounded cube and panel side by side on y = 0) and the rig
/// into `dir` and renders `preview.png` there.
///
/// `recipe` is the recipe path relative to `dir`; `refs` spell the maps relative to `dir`.
///
/// # Errors
/// A write failure, or the renderer's diagnostics other than a missing adapter.
pub fn render(dir: &Path, recipe: &str, refs: &MapRefs) -> Result<Outcome, Vec<Diagnostic>> {
    let io = |what: &Path, e: &dyn std::fmt::Display| {
        vec![Diagnostic::new(
            ludion_core::codes::code::SCENE_IO,
            format!("could not write `{}`: {e}", what.display()),
        )]
    };
    let mut shapes = sphere().translated([-1.4, 0.5, 0.0]);
    shapes.append(rounded_cube().translated([0.0, 0.5, 0.0]));
    shapes.append(panel().translated([1.6, 0.0, 0.0]));
    let model = dir.join("preview.mesh.yaml");
    let text = emit_mesh(&document("preview", shapes, refs, recipe));
    write_atomic(&model, text.as_bytes()).map_err(|e| io(&model, &e))?;
    let rig = dir.join("rig.yaml");
    write_atomic(&rig, RIG.as_bytes()).map_err(|e| io(&rig, &e))?;

    let models = [model.as_path()];
    let sheet = ludion_engine::render_model_sheet(&ModelSheetRequest {
        views: &[ModelView::Iso],
        rig: Some(&rig),
        ..ModelSheetRequest::new(&models, WIDTH, HEIGHT)
    });
    match sheet {
        Ok(outcome) => {
            let out = dir.join("preview.png");
            outcome.image.write_png(&out).map_err(|d| vec![d])?;
            Ok(Outcome::Rendered(out))
        }
        Err(diagnostics) if ludion_core::require_gpu::is_no_adapter(&diagnostics) => {
            Ok(Outcome::NoAdapter("no GPU adapter".to_string()))
        }
        Err(diagnostics) => Err(diagnostics),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_has_consistent_buffers_and_outward_winding() {
        for g in [sphere(), rounded_cube(), panel()] {
            let n = g.positions.len() / 3;
            assert_eq!(g.normals.len(), n * 3);
            assert_eq!(g.uvs.len(), n * 2);
            assert!(g.indices.iter().all(|i| (*i as usize) < n));
            assert_eq!(g.indices.len() % 3, 0);
        }
        // The panel faces +Z: its single front-facing quad is counter-clockwise seen from +Z.
        let p = panel();
        let t = &p.indices[..3];
        let at = |i: u32| (p.positions[i as usize * 3], p.positions[i as usize * 3 + 1]);
        let (a, b, c) = (at(t[0]), at(t[1]), at(t[2]));
        assert!((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0) > 0.0);
    }
}
