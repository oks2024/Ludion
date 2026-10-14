//! Implicit-surface meshing for Ludion's sculpt documents: a model authored as a list of primitive
//! parts (sphere, capsule, ellipsoid, rounded box, torus, cone, and a voxel model's own occupancy)
//! folded together by exact or smooth booleans becomes a signed distance field, and the field
//! becomes a triangle mesh. The output is a pure function of its inputs that must hash identically
//! on every machine, so the crate holds to the sim-side determinism rules: `f32` arithmetic, every
//! transcendental through [`ludion_core::math`], no hash-order iteration, fixed loop orders and
//! explicit tie-breaks. [`field`] holds the primitive distances and the polynomial smooth min/max;
//! [`mesh`] runs the whole pipeline.
//!
//! The crate reads no files. A `shape: voxel` or `shape: mesh` part's field is another document, so
//! the caller reads it ([`voxel_paths`], [`mesh_paths`], [`SculptSources`]) and meshes through
//! [`mesh_with`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod connectivity;
pub mod decimate;
pub(crate) mod edt;
pub mod field;
mod grid;
mod material;
mod normals;
mod soup;
mod sources;
mod surface_nets;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{SCULPT_MAX_RESOLUTION, SculptDocument, SculptShape};

use crate::decimate::{TriMesh, decimate};
use crate::grid::{Aabb, Grid, SculptField};

pub use crate::sources::SculptSources;

/// A meshed sculpt in the renderer's flat `MeshData` layout, so conversion moves each list and sets
/// `authored_material: true`.
///
/// Model space is world units with the origin bottom-center: the surface's bounding box has minimum
/// `y` of `0` and is centered on `x` and `z`.
#[derive(Clone, Debug, PartialEq)]
pub struct SculptMesh {
    /// Positions, 3 values per vertex.
    pub positions: Vec<f32>,
    /// Unit normals, 3 values per vertex: the field gradient, faceted across creases on `sharp` parts.
    pub normals: Vec<f32>,
    /// Linear RGBA, 4 values per vertex: the palette colors of the parts blended by `weights`.
    pub colors: Vec<f32>,
    /// `(roughness, metallic, emissive)`, 3 values per vertex, blended like `colors`; an unauthored
    /// palette channel takes the default material `(1, 0, 0)`.
    pub materials: Vec<f32>,
    /// Triangle list, 3 vertex indices per triangle, counter-clockwise seen from outside.
    pub indices: Vec<u32>,
    /// Values per vertex in `weights`: the document's part count.
    pub part_count: usize,
    /// Per-part material weights, `part_count` values per vertex in part order — vertex `v`'s weight
    /// for part `i` at `v * part_count + i`. Only `union` parts carry weight; a vertex's weights sum
    /// to `1` up to rounding.
    pub weights: Vec<f32>,
    /// The part each vertex is attributed to, one entry per vertex: its nearest part surface, the
    /// attribution [`PartStats`] counts and bounds. `None` only when no part has a surface there.
    pub part_of: Vec<Option<u32>>,
    /// What the mesher measured, as `check-scene` prints it.
    pub stats: SculptStats,
}

/// The measurements of one [`mesh`] run. Every box is `(min, max)` in the final model space.
#[derive(Clone, Debug, PartialEq)]
pub struct SculptStats {
    /// The sampling lattice's box, corner to corner.
    pub extent: ([f32; 3], [f32; 3]),
    /// Cells in the lattice.
    pub cells: u64,
    /// Cells the surface crosses — the vertex count before decimation.
    pub surface_cells: u32,
    /// Triangles extracted, before decimation.
    pub triangles_before: u32,
    /// Triangles in the output.
    pub triangles_after: u32,
    /// Vertices in the output, crease duplicates included.
    pub vertices: u32,
    /// Connected components of the output surface, counted before the crease split so a duplicated
    /// vertex never separates one.
    pub components: u32,
    /// One entry per document part, in part order.
    pub parts: Vec<PartStats>,
    /// One entry per document bone, in bone order.
    pub bones: Vec<BoneStats>,
}

/// What one part contributes to the output surface.
#[derive(Clone, Debug, PartialEq)]
pub struct PartStats {
    /// The part's name.
    pub name: String,
    /// The box of the output vertices the part contributes, `None` when it contributes none.
    pub bounds: Option<([f32; 3], [f32; 3])>,
    /// Output vertices the part contributes: those whose nearest part surface (smallest
    /// `|distance|`, every copy included, ties to the lower part index) is this part's. Summed over
    /// the parts this is [`SculptStats::vertices`].
    pub surface_vertices: u32,
}

/// The surface parented to one bone.
#[derive(Clone, Debug, PartialEq)]
pub struct BoneStats {
    /// The bone's name.
    pub name: String,
    /// The union of the [`PartStats::bounds`] of the parts on this bone, `None` when none has any.
    pub bounds: Option<([f32; 3], [f32; 3])>,
    /// The document's `head` for this bone, in the mesh's output space — the same bottom-center
    /// translation the positions get, so a bone end and the surface around it share coordinates.
    /// `None` when the bone is unpositioned.
    pub head: Option<[f32; 3]>,
    /// The document's `tail`, translated like [`BoneStats::head`]; the two are authored together.
    pub tail: Option<[f32; 3]>,
}

/// Meshes a validated sculpt document.
///
/// The pipeline: the field sampled on a lattice covering every part, naive surface nets, per-part
/// material weights at each vertex, quadric decimation to `doc.triangles`, gradient normals, the
/// crease split on `sharp` parts, palette blending, then the bottom-center translation. A document
/// with no parts, or whose field never changes sign, meshes to an empty mesh.
///
/// A document carrying a `shape: voxel` or `shape: mesh` part cannot mesh this way — its field
/// lives in another file, and this crate reads none. Mesh it through [`mesh_with`] instead.
///
/// # Errors
///
/// `L1157` at `/resolution` when the lattice would exceed the mesher's corner cap, or at
/// `/parts/<i>/path` when a part reads another document.
///
/// # Example
///
/// ```
/// let doc = ludion_scene::check_sculpt(
///     "format: 1\nkind: sculpt\nresolution: 16\ntriangles: 200\n\
///      palette:\n  S:\n    color:\n      - 0.5\n      - 0.5\n      - 0.5\n      - 1.0\n\
///      bones:\n  - name: root\n\
///      parts:\n  - name: ball\n    shape: sphere\n    radius: 0.5\n    material: S\n",
/// )
/// .expect("the sculpt is valid");
/// let mesh = ludion_sculpt::mesh(&doc).expect("the grid fits");
/// assert!(mesh.stats.triangles_after <= 200);
/// assert_eq!(mesh.positions.len(), 3 * mesh.stats.vertices as usize);
/// assert!(
///     ludion_sculpt::unresolved_parts(&doc, &ludion_sculpt::SculptSources::new(), &mesh).is_empty()
/// );
/// ```
pub fn mesh(doc: &SculptDocument) -> Result<SculptMesh, Diagnostic> {
    mesh_with(doc, &SculptSources::new())
}

/// [`mesh`], with the documents its `shape: voxel` and `shape: mesh` parts name already read.
///
/// The crate is pure: a part whose field is another document gets it from `sources`, which the
/// caller fills by reading every [`voxel_paths`] and [`mesh_paths`] entry. A document with no such
/// part meshes exactly as [`mesh`] does, whatever `sources` holds.
///
/// # Errors
///
/// [`mesh`]'s, plus `L1157` at `/parts/<i>/path` for a part `sources` has no entry for.
///
/// # Example
///
/// ```
/// let voxel = ludion_scene::check_voxel(
///     "format: 1\nkind: voxel\nsize:\n  - 2\n  - 2\n  - 2\n\
///      palette:\n  S:\n    color:\n      - 0.5\n      - 0.5\n      - 0.5\n      - 1.0\n\
///      slices:\n  - - SS\n    - SS\n  - - SS\n    - SS\n",
/// )
/// .expect("the voxel model is valid");
/// let doc = ludion_scene::check_sculpt(
///     "format: 1\nkind: sculpt\nresolution: 16\ntriangles: 200\n\
///      palette:\n  S:\n    color:\n      - 0.5\n      - 0.5\n      - 0.5\n      - 1.0\n\
///      bones:\n  - name: root\n\
///      parts:\n  - name: block\n    shape: voxel\n    path: ../voxels/block.voxel.yaml\n    \
///      size: 0.25\n    material: S\n",
/// )
/// .expect("the sculpt is valid");
///
/// assert_eq!(ludion_sculpt::voxel_paths(&doc), ["../voxels/block.voxel.yaml"]);
/// let mut sources = ludion_sculpt::SculptSources::new();
/// sources.insert_voxel("../voxels/block.voxel.yaml", &voxel);
///
/// let mesh = ludion_sculpt::mesh_with(&doc, &sources).expect("the grid fits");
/// assert!(mesh.stats.triangles_after > 0);
/// // Without the source the same document refuses, naming the part that needs one.
/// assert_eq!(
///     ludion_sculpt::mesh(&doc).unwrap_err().path.as_deref(),
///     Some("/parts/0/path"),
/// );
/// ```
pub fn mesh_with(doc: &SculptDocument, sources: &SculptSources) -> Result<SculptMesh, Diagnostic> {
    mesh_with_offset(doc, sources).map(|(mesh, _)| mesh)
}

/// Every distinct `shape: voxel` `path` of `doc`, in first-use order — what a caller reads and
/// hands to [`SculptSources::insert_voxel`] before [`mesh_with`].
pub fn voxel_paths(doc: &SculptDocument) -> Vec<&str> {
    source_paths(doc, |shape| match shape {
        SculptShape::Voxel { path, .. } => Some(path),
        _ => None,
    })
}

/// Every distinct `shape: mesh` `path` of `doc`, in first-use order — what a caller reads and hands
/// to [`SculptSources::insert_mesh`] before [`mesh_with`].
pub fn mesh_paths(doc: &SculptDocument) -> Vec<&str> {
    source_paths(doc, |shape| match shape {
        SculptShape::Mesh { path, .. } => Some(path),
        _ => None,
    })
}

fn source_paths(doc: &SculptDocument, of: impl Fn(&SculptShape) -> Option<&String>) -> Vec<&str> {
    let mut paths: Vec<&str> = Vec::new();
    for part in &doc.parts {
        if let Some(path) = of(&part.shape)
            && !paths.contains(&path.as_str())
        {
            paths.push(path);
        }
    }
    paths
}

/// [`mesh_with`], also returning the translation it applied to the document's coordinates.
fn mesh_with_offset(
    doc: &SculptDocument,
    sources: &SculptSources,
) -> Result<(SculptMesh, [f32; 3]), Diagnostic> {
    let field = SculptField::new(doc, sources)?;
    let part_count = doc.parts.len();
    let Some(grid) = Grid::sample(&field, doc.resolution)? else {
        return Ok((empty_mesh(doc), [0.0; 3]));
    };

    let net = surface_nets::extract(&grid);
    let surface_cells = net.positions.len() as u32;
    let triangles_before = (net.indices.len() / 3) as u32;
    let mut attributes = vec![0.0; net.positions.len() * part_count];
    for (&p, weights) in net
        .positions
        .iter()
        .zip(attributes.chunks_exact_mut(part_count))
    {
        material::weights_at(&field, p, weights);
    }
    let decimated = decimate(
        &TriMesh {
            positions: net.positions,
            attributes,
            attribute_len: part_count,
            indices: net.indices,
        },
        doc.triangles,
    );

    let smooth = normals::gradient_normals(&field, &decimated.positions, grid.spacing());
    let sharp: Vec<bool> = decimated
        .attributes
        .chunks_exact(part_count)
        .map(|weights| material::dominant_part(weights).is_some_and(|i| doc.parts[i].sharp))
        .collect();
    let split = normals::crease_split(&decimated.positions, &smooth, &decimated.indices, &sharp);
    let offset = bottom_center_offset(&decimated.positions);
    let channels = material::part_channels(doc);
    let nearest: Vec<Option<usize>> = decimated
        .positions
        .iter()
        .map(|&p| field.nearest_part(p))
        .collect();

    let vertex_count = split.source.len();
    let mut positions = Vec::with_capacity(3 * vertex_count);
    let mut colors = Vec::with_capacity(4 * vertex_count);
    let mut materials = Vec::with_capacity(3 * vertex_count);
    let mut weights = Vec::with_capacity(part_count * vertex_count);
    let mut part_of = Vec::with_capacity(vertex_count);
    let mut part_bounds: Vec<Option<Aabb>> = vec![None; part_count];
    let mut part_vertices = vec![0u32; part_count];
    for &source in &split.source {
        let source = source as usize;
        let p = decimated.positions[source];
        let moved = [p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]];
        positions.extend_from_slice(&moved);
        let vertex_weights = &decimated.attributes[source * part_count..(source + 1) * part_count];
        material::blend(vertex_weights, &channels, &mut colors, &mut materials);
        weights.extend_from_slice(vertex_weights);
        part_of.push(nearest[source].map(|part| part as u32));
        if let Some(part) = nearest[source] {
            part_vertices[part] += 1;
            part_bounds[part] = Some(grow(part_bounds[part], (moved, moved)));
        }
    }

    let (lo, hi) = grid.extent();
    let stats = SculptStats {
        extent: (shift(lo, offset), shift(hi, offset)),
        cells: grid.cell_count(),
        surface_cells,
        triangles_before,
        triangles_after: (split.indices.len() / 3) as u32,
        vertices: vertex_count as u32,
        components: connectivity::connectivity(&decimated.positions, &decimated.indices).0,
        bones: bone_stats(doc, &part_bounds, offset),
        parts: doc
            .parts
            .iter()
            .zip(part_bounds.iter().zip(&part_vertices))
            .map(|(part, (&bounds, &surface_vertices))| PartStats {
                name: part.name.clone(),
                bounds,
                surface_vertices,
            })
            .collect(),
    };
    let mesh = SculptMesh {
        positions,
        normals: split.normals.into_iter().flatten().collect(),
        colors,
        materials,
        indices: split.indices,
        part_count,
        weights,
        part_of,
        stats,
    };
    Ok((mesh, offset))
}

/// `L1158` warnings for every part the mesh does not resolve: one thinner than two grid cells
/// (checked first, as the likely cause) or one contributing no output vertex. `mesh` is the result
/// of [`mesh`] or [`mesh_with`] on the same `doc`, with the same `sources`.
///
/// Attribution is on the decimated surface, so a part reduced to nothing by a low `triangles`
/// target is reported as well. Naming the part that hides a surfaceless one costs an extra lattice
/// pass per such part, paid only here, never by the mesher.
pub fn unresolved_parts(
    doc: &SculptDocument,
    sources: &SculptSources,
    mesh: &SculptMesh,
) -> Vec<Diagnostic> {
    let cell = 1.0 / doc.resolution as f32;
    let mut diagnostics = Vec::new();
    let mut frame: Option<Option<(SculptField, Grid)>> = None;
    for (index, part) in doc.parts.iter().enumerate() {
        let path = format!("/parts/{index}");
        let vertices = mesh
            .stats
            .parts
            .get(index)
            .map_or(0, |stats| stats.surface_vertices);
        let thinnest = thinnest_dimension(&part.shape);
        if thinnest < 2.0 * cell {
            let needed = (2.0 / thinnest).ceil() as u32;
            let help = if needed <= SCULPT_MAX_RESOLUTION {
                format!(
                    "raise `resolution` to at least {needed}, or thicken the part to at least {} \
                     world units",
                    2.0 * cell
                )
            } else {
                format!(
                    "thicken the part to at least {} world units; resolving it as authored needs \
                     `resolution: {needed}`, above the maximum {SCULPT_MAX_RESOLUTION}",
                    2.0 * cell
                )
            };
            diagnostics.push(
                Diagnostic::new(
                    code::SCULPT_PART_UNRESOLVED,
                    format!(
                        "sculpt part `{}` is thinner than two grid cells: its thinnest dimension \
                         is {thinnest} world units against a cell of {cell} (`resolution: {}`); it \
                         contributes {vertices} surface vertices",
                        part.name, doc.resolution
                    ),
                )
                .with_path(path)
                .with_help(help),
            );
        } else if vertices == 0 {
            let frame = frame.get_or_insert_with(|| {
                let field = SculptField::new(doc, sources).ok()?;
                let grid = Grid::frame(&field, doc.resolution).ok()??;
                Some((field, grid))
            });
            let culprit = frame
                .as_ref()
                .and_then(|(field, grid)| hider(field, grid, index))
                .and_then(|hider| doc.parts.get(hider as usize))
                .map(|hider| format!("hidden by part `{}`", hider.name));
            let cause = culprit.unwrap_or_else(|| "hidden inside another part".to_string());
            diagnostics.push(
                Diagnostic::new(
                    code::SCULPT_PART_UNRESOLVED,
                    format!(
                        "sculpt part `{}` contributes no surface — it is {cause} or carved away",
                        part.name
                    ),
                )
                .with_path(path)
                .with_help(
                    "move the part out of the body that hides it, enlarge it, or delete it if it \
                     is not meant to show",
                ),
            );
        }
    }
    diagnostics
}

/// The part that owns the field where `victim` sits inside, or `None` when nothing does.
///
/// The mesher already knows which part won where — `L1158` used to make the author find it by
/// bisection. Over the lattice in its own `z, y, x` order, every corner where `victim`'s own
/// distance is negative contributes the folded material weights; the largest total that is not the
/// victim's own wins, ties to the lower part index. The accumulation is sequential and the order
/// fixed, so the answer hashes like the rest of the mesh.
///
/// Only called from [`unresolved_parts`], for a part with no surface at all, so the extra lattice
/// pass is a warning's cost. It reads the lattice frame only, never its samples.
fn hider(field: &SculptField, grid: &Grid, victim: usize) -> Option<u32> {
    #[cfg(test)]
    HIDER_PASSES.with(|passes| passes.set(passes.get() + 1));
    let part_count = field.parts.len();
    let mut totals = vec![0.0f32; part_count];
    let mut weights = vec![0.0f32; part_count];
    for z in 0..=grid.cells[2] {
        for y in 0..=grid.cells[1] {
            for x in 0..=grid.cells[0] {
                let p = grid.corner_position([x, y, z]);
                if field.parts[victim].distance(p) >= 0.0 {
                    continue;
                }
                material::weights_at(field, p, &mut weights);
                for (total, &weight) in totals.iter_mut().zip(weights.iter()) {
                    *total += weight;
                }
            }
        }
    }
    let mut best: Option<(usize, f32)> = None;
    for (index, &total) in totals.iter().enumerate() {
        if index != victim && total > 0.0 && best.is_none_or(|(_, most)| total > most) {
            best = Some((index, total));
        }
    }
    best.map(|(index, _)| index as u32)
}

/// A shape's smallest full dimension, what must span two cells to resolve. For a voxel model that
/// is one voxel: a single-voxel wall is the thinnest feature its occupancy can hold. A mesh's
/// thinnest feature is a property of its triangles, which the document alone does not carry, so a
/// mesh part is never reported thin — only genuinely surfaceless.
fn thinnest_dimension(shape: &SculptShape) -> f32 {
    let thinnest = match *shape {
        SculptShape::Mesh { .. } => return f32::INFINITY,
        SculptShape::Voxel { size, .. } => size,
        SculptShape::Sphere { radius } | SculptShape::Capsule { radius, .. } => 2.0 * radius,
        SculptShape::Ellipsoid { radii } => 2.0 * radii[0].min(radii[1]).min(radii[2]),
        SculptShape::Box { size, .. } => size[0].min(size[1]).min(size[2]),
        SculptShape::Torus { tube, .. } => 2.0 * tube,
        SculptShape::Cone { radius, height } => (2.0 * radius).min(height),
    };
    thinnest as f32
}

fn empty_mesh(doc: &SculptDocument) -> SculptMesh {
    SculptMesh {
        positions: Vec::new(),
        normals: Vec::new(),
        colors: Vec::new(),
        materials: Vec::new(),
        indices: Vec::new(),
        part_count: doc.parts.len(),
        weights: Vec::new(),
        part_of: Vec::new(),
        stats: SculptStats {
            extent: ([0.0; 3], [0.0; 3]),
            cells: 0,
            surface_cells: 0,
            triangles_before: 0,
            triangles_after: 0,
            vertices: 0,
            components: 0,
            parts: doc
                .parts
                .iter()
                .map(|part| PartStats {
                    name: part.name.clone(),
                    bounds: None,
                    surface_vertices: 0,
                })
                .collect(),
            bones: bone_stats(doc, &vec![None; doc.parts.len()], [0.0; 3]),
        },
    }
}

fn bone_stats(
    doc: &SculptDocument,
    part_bounds: &[Option<Aabb>],
    offset: [f32; 3],
) -> Vec<BoneStats> {
    (0..doc.bones.len())
        .map(|bone| BoneStats {
            name: doc.bones[bone].name.clone(),
            bounds: doc
                .parts
                .iter()
                .zip(part_bounds)
                .filter(|(part, _)| doc.bone_index(part) == Some(bone))
                .filter_map(|(_, bounds)| *bounds)
                .fold(None, |acc, bounds| Some(grow(acc, bounds))),
            head: doc.bones[bone].head.map(|head| place(head, offset)),
            tail: doc.bones[bone].tail.map(|tail| place(tail, offset)),
        })
        .collect()
}

/// A document position in the mesh's output space: narrowed to `f32` and moved by the mesh's own
/// bottom-center translation.
fn place(position: [f64; 3], offset: [f32; 3]) -> [f32; 3] {
    shift(position.map(|c| c as f32), offset)
}

/// The translation putting the box of `positions` on `y = 0`, centered on `x` and `z`.
fn bottom_center_offset(positions: &[[f32; 3]]) -> [f32; 3] {
    let Some((min, max)) = positions.iter().map(|&p| (p, p)).reduce(grid::union) else {
        return [0.0; 3];
    };
    [-(min[0] + max[0]) * 0.5, -min[1], -(min[2] + max[2]) * 0.5]
}

fn grow(bounds: Option<Aabb>, more: Aabb) -> Aabb {
    bounds.map_or(more, |bounds| grid::union(bounds, more))
}

fn shift(p: [f32; 3], offset: [f32; 3]) -> [f32; 3] {
    [p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]]
}

#[cfg(test)]
thread_local! {
    /// Counts `hider` passes on this thread, so a test can prove the mesher never runs one.
    static HIDER_PASSES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const TAU: f32 = core::f32::consts::TAU;

    /// One part of a `sculpt` document: `key: value` pairs separated by `; `, a `key: [a, b, c]`
    /// spelling out a vector. A nested mapping (`deform`, `noise`) has no shorthand — those parts
    /// are written out and passed to [`sculpt_source`].
    fn part_block(part: &str) -> String {
        let mut block = String::new();
        for (i, field) in part.split("; ").enumerate() {
            block.push_str(if i == 0 { "  - " } else { "    " });
            match field.split_once(": [") {
                Some((key, list)) => {
                    block.push_str(&format!("{key}:\n"));
                    for value in list.trim_end_matches(']').split(", ") {
                        block.push_str(&format!("      - {value}\n"));
                    }
                }
                None => block.push_str(&format!("{field}\n")),
            }
        }
        block
    }

    /// A one-glyph, one-bone document; each part is `key: value` pairs separated by `; `.
    fn sculpt(resolution: u32, triangles: u32, parts: &[&str]) -> SculptDocument {
        let block: String = parts.iter().copied().map(part_block).collect();
        sculpt_source(resolution, triangles, &block)
    }

    /// A one-glyph, one-bone document whose `parts:` block is given verbatim.
    fn sculpt_source(resolution: u32, triangles: u32, parts: &str) -> SculptDocument {
        let source = format!(
            "format: 1\nkind: sculpt\nresolution: {resolution}\ntriangles: {triangles}\n\
             palette:\n  A:\n    color:\n      - 1.0\n      - 0.5\n      - 0.25\n      - 1.0\n    \
             roughness: 0.5\nbones:\n  - name: root\nparts:\n{parts}"
        );
        ludion_scene::check_sculpt(&source).unwrap_or_else(|d| panic!("{d:#?}"))
    }

    /// Asserts every undirected edge is used by exactly two triangles, once in each direction.
    fn assert_closed(indices: &[u32]) {
        assert!(!indices.is_empty(), "the mesh is empty");
        let mut directed: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        for triangle in indices.chunks_exact(3) {
            for k in 0..3 {
                *directed
                    .entry((triangle[k], triangle[(k + 1) % 3]))
                    .or_default() += 1;
            }
        }
        for (&(a, b), &count) in &directed {
            assert_eq!(count, 1, "directed edge {a}->{b} used {count} times");
            assert_eq!(
                directed.get(&(b, a)),
                Some(&1),
                "edge {a}-{b} has no opposite half: the mesh is open"
            );
        }
    }

    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    fn length(v: [f32; 3]) -> f32 {
        ludion_core::math::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
    }

    fn vertex(positions: &[f32], v: usize, offset: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|axis| positions[3 * v + axis] - offset[axis])
    }

    #[test]
    fn unit_sphere_meshes_closed_accurate_and_outward() {
        let doc = sculpt(
            32,
            1_000_000,
            &["name: ball; shape: sphere; radius: 1.0; material: A"],
        );
        let field = SculptField::new(&doc, &SculptSources::new()).unwrap();
        let grid = Grid::sample(&field, 32).unwrap().unwrap();
        let net = surface_nets::extract(&grid);
        assert_closed(&net.indices);

        let cell = 1.0 / 32.0;
        for p in &net.positions {
            assert!((length(*p) - 1.0).abs() <= cell, "{p:?} is off the surface");
        }
        let mut area = 0.0f64;
        for triangle in net.indices.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|k| net.positions[triangle[k] as usize]);
            let normal = cross(
                [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
            );
            let centroid = [0, 1, 2].map(|axis| a[axis] + b[axis] + c[axis]);
            let facing =
                normal[0] * centroid[0] + normal[1] * centroid[1] + normal[2] * centroid[2];
            assert!(facing > 0.0, "triangle {triangle:?} faces inward");
            area += 0.5 * length(normal) as f64;
        }
        let sphere = 2.0 * TAU as f64;
        assert!(
            ((area - sphere) / sphere).abs() < 0.02,
            "area {area} is not within 2% of 4π"
        );
    }

    #[test]
    fn subtract_removes_the_carved_volume() {
        let doc = sculpt(
            32,
            1_000_000,
            &[
                "name: ball; shape: sphere; radius: 1.0; material: A",
                "name: bite; shape: sphere; radius: 0.6; at: [0, 1, 0]; op: subtract",
            ],
        );
        let (mesh, offset) = mesh_with_offset(&doc, &SculptSources::new()).unwrap();
        let cell = 1.0 / 32.0;
        let mut on_the_cut = 0;
        for v in 0..mesh.stats.vertices as usize {
            let p = vertex(&mesh.positions, v, offset);
            let to_bite = length([p[0], p[1] - 1.0, p[2]]);
            assert!(to_bite >= 0.6 - cell, "{p:?} lies inside the carved volume");
            assert!(p[1] < 0.82 + cell, "{p:?} rises above the cut's rim");
            if (to_bite - 0.6).abs() <= cell && length(p) < 1.0 - cell {
                on_the_cut += 1;
            }
        }
        assert!(on_the_cut > 100, "only {on_the_cut} vertices on the cut");
        assert!(mesh.stats.parts[1].surface_vertices > 0);
        let color = &mesh.colors[..4];
        assert_eq!(color, &[1.0, 0.5, 0.25, 1.0]);
    }

    #[test]
    fn mirror_meshes_symmetric_to_the_cell() {
        let doc = sculpt(
            32,
            1_000_000,
            &[
                "name: ear; shape: ellipsoid; radii: [0.3, 0.2, 0.25]; at: [-0.5, 0.2, 0.1]; \
               rotation: [0, 0, 30]; mirror: x; material: A",
            ],
        );
        let (mesh, offset) = mesh_with_offset(&doc, &SculptSources::new()).unwrap();
        assert_closed(&mesh.indices);
        let cell = 1.0 / 32.0;
        let count = mesh.stats.vertices as usize;
        let points: Vec<[f32; 3]> = (0..count)
            .map(|v| vertex(&mesh.positions, v, offset))
            .collect();
        let left = points.iter().filter(|p| p[0] < 0.0).count();
        assert_eq!(left * 2, count, "the halves differ in vertex count");
        for p in &points {
            let mirrored = [-p[0], p[1], p[2]];
            let nearest = points
                .iter()
                .map(|q| length([q[0] - mirrored[0], q[1] - mirrored[1], q[2] - mirrored[2]]))
                .fold(f32::INFINITY, f32::min);
            assert!(nearest <= cell, "{p:?} has no mirror image ({nearest})");
        }
    }

    #[test]
    fn decimation_reaches_the_target_and_stays_closed() {
        let doc = sculpt(
            32,
            2000,
            &["name: ball; shape: sphere; radius: 1.0; material: A"],
        );
        let mesh = mesh(&doc).unwrap();
        assert!(mesh.stats.triangles_before > 2000);
        assert!(mesh.stats.triangles_after <= 2000);
        assert_eq!(mesh.indices.len(), 3 * mesh.stats.triangles_after as usize);
        assert_closed(&mesh.indices);
        assert_eq!(mesh.stats.parts[0].surface_vertices, mesh.stats.vertices);
    }

    #[test]
    fn sharp_parts_split_creases_and_nothing_else_grows() {
        // Surface nets bevels a box edge over about a cell in faces under the crease angle apart;
        // only a decimation down to the flat faces leaves creases to split.
        let part = "name: slab; shape: box; size: [1, 0.5, 0.8]; material: A";
        let smooth = mesh(&sculpt(32, 12, &[part])).unwrap();
        let sharp = mesh(&sculpt(32, 12, &[&format!("{part}; sharp: true")])).unwrap();
        assert_eq!(smooth.stats.triangles_after, sharp.stats.triangles_after);
        assert!(sharp.stats.vertices > smooth.stats.vertices);
        assert_eq!(sharp.weights.len(), sharp.stats.vertices as usize);
        for normal in sharp.normals.chunks_exact(3) {
            assert!((length([normal[0], normal[1], normal[2]]) - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn output_sits_bottom_center() {
        let doc = sculpt(
            16,
            300,
            &["name: ball; shape: sphere; radius: 0.5; at: [3, -2, 1]; material: A"],
        );
        let mesh = mesh(&doc).unwrap();
        let axis = |a: usize| mesh.positions.iter().skip(a).step_by(3).copied();
        let min = [0, 1, 2].map(|a| axis(a).fold(f32::INFINITY, f32::min));
        let max = [0, 1, 2].map(|a| axis(a).fold(f32::NEG_INFINITY, f32::max));
        assert_eq!(min[1], 0.0);
        assert!((min[0] + max[0]).abs() < 1e-5 && (min[2] + max[2]).abs() < 1e-5);
        assert_eq!(mesh.stats.bones[0].bounds, mesh.stats.parts[0].bounds);
    }

    /// `part_of` is the attribution the stats summarize: per part, its vertices count to
    /// `surface_vertices` and their box is the part's bounds.
    #[test]
    fn part_of_agrees_with_the_part_stats() {
        let doc = sculpt(
            32,
            3000,
            &[
                "name: body; shape: sphere; radius: 0.5; material: A",
                "name: nose; shape: sphere; radius: 0.15; at: [0, 0, 0.5]; blend: 0.05; material: A",
            ],
        );
        let mesh = mesh(&doc).unwrap();
        assert_eq!(mesh.part_of.len(), mesh.stats.vertices as usize);
        for (index, stats) in mesh.stats.parts.iter().enumerate() {
            let vertices: Vec<usize> = (0..mesh.part_of.len())
                .filter(|&v| mesh.part_of[v] == Some(index as u32))
                .collect();
            assert_eq!(vertices.len(), stats.surface_vertices as usize);
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for v in vertices {
                for axis in 0..3 {
                    min[axis] = min[axis].min(mesh.positions[3 * v + axis]);
                    max[axis] = max[axis].max(mesh.positions[3 * v + axis]);
                }
            }
            assert_eq!(stats.bounds, Some((min, max)), "part {index}");
        }
    }

    /// A content hash of everything a caller sees, for the "this key changed the mesh" assertions.
    fn digest(mesh: &SculptMesh) -> u64 {
        let mut state = 0xcbf2_9ce4_8422_2325u64;
        let mut mix = |bits: u64| {
            state = (state ^ bits).wrapping_mul(0x1000_0000_01b3);
        };
        for value in mesh
            .positions
            .iter()
            .chain(&mesh.normals)
            .chain(&mesh.colors)
        {
            mix(value.to_bits() as u64);
        }
        for index in &mesh.indices {
            mix(*index as u64);
        }
        state
    }

    /// An upright slab, the body every deformer test starts from.
    const SHAFT: &str = "  - name: shaft\n    shape: box\n    size:\n      - 0.4\n      \
                         - 1.2\n      - 0.4\n    material: A\n";

    /// A smooth upright body, for the assertions that need a closed surface: naive surface nets can
    /// leave a non-manifold cell on a box's hard edge, which is a documented limit of the extractor
    /// and has nothing to do with the deformers. Its x and z radii differ, so a twist about `y` is
    /// visible on it.
    const BLOB: &str = "  - name: blob\n    shape: ellipsoid\n    radii:\n      - 0.25\n      \
                        - 0.6\n      - 0.15\n    material: A\n";

    const BEND: &str = "    deform:\n      bend:\n        axis: y\n        degrees: 40.0\n";
    const TWIST: &str = "    deform:\n      twist:\n        axis: y\n        degrees: 90.0\n";
    const TAPER: &str = "    deform:\n      taper:\n        axis: y\n        from: 1.0\n        \
                         to: 0.3\n";
    const NOISE: &str = "    noise:\n      amplitude: 0.03\n      scale: 0.25\n      seed: 11\n";

    /// Each deformer and the noise: the mesh stays a closed surface, it is not the undeformed mesh,
    /// and two runs agree bit for bit.
    #[test]
    fn deformers_and_noise_change_the_mesh_and_repeat_exactly() {
        let base = mesh(&sculpt_source(24, 2000, BLOB)).unwrap();
        for (name, key) in [
            ("bend", BEND),
            ("twist", TWIST),
            ("taper", TAPER),
            ("noise", NOISE),
        ] {
            let doc = sculpt_source(24, 2000, &format!("{BLOB}{key}"));
            let modified = mesh(&doc).unwrap();
            assert_closed(&modified.indices);
            assert!(modified.stats.vertices > 0, "{name} meshed to nothing");
            assert_ne!(digest(&modified), digest(&base), "{name} changed nothing");
            assert_eq!(digest(&mesh(&doc).unwrap()), digest(&modified), "{name}");
        }
    }

    /// The deformers do what they are named: a taper narrows the far end and leaves the near one,
    /// a twist widens the cross-section a box presents, a bend leans the column off its own axis.
    #[test]
    fn each_deformer_moves_the_surface_the_way_it_says() {
        let half_width = |mesh: &SculptMesh, low: f32, high: f32| {
            (0..mesh.stats.vertices as usize)
                .filter(|v| {
                    let y = mesh.positions[3 * v + 1];
                    y >= low && y <= high
                })
                .map(|v| mesh.positions[3 * v].abs())
                .fold(0.0f32, f32::max)
        };
        let reach = |mesh: &SculptMesh, axis: usize| {
            (0..mesh.stats.vertices as usize)
                .map(|v| mesh.positions[3 * v + axis].abs())
                .fold(0.0f32, f32::max)
        };
        let shaped = |key: &str| mesh(&sculpt_source(24, 4000, &format!("{SHAFT}{key}"))).unwrap();

        let base = mesh(&sculpt_source(24, 4000, SHAFT)).unwrap();
        let tapered = shaped(TAPER);
        assert!(
            half_width(&tapered, 1.0, 1.2) < 0.5 * half_width(&base, 1.0, 1.2),
            "the taper did not narrow the top"
        );
        assert!(
            half_width(&tapered, 0.0, 0.2) > 0.9 * half_width(&base, 0.0, 0.2),
            "the taper narrowed the bottom, where its scale is 1"
        );

        // A 90-degree-per-unit twist turns the top's corners off the axes the untwisted box keeps
        // them on.
        let twisted = shaped(TWIST);
        assert!(
            half_width(&twisted, 1.1, 1.2) > 1.2 * half_width(&base, 1.1, 1.2),
            "the twist did not turn the top cross-section"
        );

        // The bend turns in the (y, z) plane — y's next axis — so the column leans in z.
        let bent = shaped(BEND);
        assert!(
            reach(&bent, 2) > 1.3 * reach(&base, 2),
            "the bend did not lean the column in z"
        );
    }

    /// The noise displaces the surface by about its amplitude, follows its seed, and is inert at
    /// amplitude `0`.
    #[test]
    fn noise_displaces_by_its_amplitude_and_follows_its_seed() {
        const BALL: &str = "  - name: ball\n    shape: sphere\n    radius: 0.5\n    material: A\n";
        let noised = |seed: u32, amplitude: &str| {
            let source = format!(
                "{BALL}    noise:\n      amplitude: {amplitude}\n      scale: 0.2\n      \
                 seed: {seed}\n"
            );
            mesh_with_offset(&sculpt_source(32, 4000, &source), &SculptSources::new()).unwrap()
        };
        let (rough, offset) = noised(3, "0.04");
        let spread = (0..rough.stats.vertices as usize)
            .map(|v| (length(vertex(&rough.positions, v, offset)) - 0.5).abs())
            .fold(0.0f32, f32::max);
        assert!(
            (0.01..0.12).contains(&spread),
            "the noise moved the surface by {spread}, not about its 0.04 amplitude"
        );
        assert_ne!(
            digest(&noised(4, "0.04").0),
            digest(&rough),
            "the seed changes nothing"
        );
        let smooth = mesh(&sculpt_source(32, 4000, BALL)).unwrap();
        assert_eq!(digest(&noised(3, "0.0").0), digest(&smooth));
    }

    /// A deformed or displaced part stays inside the sampled lattice — the grown box is the guard,
    /// and a surface that reached the boundary would be clipped open. The angles here are past what
    /// the closure test uses, on purpose: the bound has to hold at the extremes too.
    #[test]
    fn a_deformed_part_stays_inside_the_lattice() {
        for key in [
            "    deform:\n      bend:\n        axis: x\n        degrees: 60.0\n",
            "    deform:\n      twist:\n        axis: z\n        degrees: 120.0\n",
            "    deform:\n      taper:\n        axis: y\n        from: 0.4\n        to: 2.5\n",
            "    noise:\n      amplitude: 0.06\n      scale: 0.15\n      seed: 5\n",
        ] {
            let meshed = mesh(&sculpt_source(20, 3000, &format!("{BLOB}{key}"))).unwrap();
            assert!(meshed.stats.vertices > 0, "{key} meshed to nothing");
            let (lo, hi) = meshed.stats.extent;
            for v in 0..meshed.stats.vertices as usize {
                for axis in 0..3 {
                    let value = meshed.positions[3 * v + axis];
                    assert!(
                        lo[axis] <= value && value <= hi[axis],
                        "{key}: vertex {v} axis {axis} at {value} escapes {lo:?}..{hi:?}"
                    );
                }
            }
        }
    }

    /// `head`/`tail` reach the stats in the mesh's output space, moved by the same translation the
    /// positions get.
    #[test]
    fn bone_positions_reach_the_stats_in_output_space() {
        let source = "format: 1\nkind: sculpt\nresolution: 16\ntriangles: 300\n\
             palette:\n  A:\n    color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\n\
             bones:\n  - name: root\n    head:\n      - 0.0\n      - 0.0\n      - 0.0\n    \
             tail:\n      - 0.0\n      - 1.0\n      - 0.0\n  - name: loose\n    parent: root\n\
             parts:\n  - name: ball\n    shape: sphere\n    radius: 0.5\n    at:\n      - 0.0\n  \
             \x20   - 2.0\n      - 0.0\n    material: A\n";
        let doc = ludion_scene::check_sculpt(source).unwrap_or_else(|d| panic!("{d:#?}"));
        let (meshed, offset) = mesh_with_offset(&doc, &SculptSources::new()).unwrap();
        let root = &meshed.stats.bones[0];
        assert_eq!(root.head, Some([0.0, offset[1], 0.0]));
        assert_eq!(root.tail, Some([0.0, 1.0 + offset[1], 0.0]));
        assert!(meshed.stats.bones[1].head.is_none());
    }

    #[test]
    fn no_parts_is_an_empty_mesh() {
        let doc = SculptDocument::default();
        let mesh = mesh(&doc).unwrap();
        assert!(mesh.positions.is_empty() && mesh.indices.is_empty());
        assert_eq!(mesh.stats.vertices, 0);
    }

    #[test]
    fn an_oversized_grid_is_refused_at_resolution() {
        let doc = sculpt(
            128,
            1000,
            &["name: hall; shape: box; size: [20, 20, 20]; material: A"],
        );
        let error = mesh(&doc).unwrap_err();
        assert_eq!(error.code, code::SCULPT_DOCUMENT_DATA);
        assert_eq!(error.path.as_deref(), Some("/resolution"));
        assert!(error.message.contains("cells"), "{}", error.message);
    }

    #[test]
    fn a_grid_extent_past_the_index_range_is_refused_not_a_panic() {
        let doc = sculpt(
            128,
            1000,
            &["name: void; shape: sphere; radius: 1e17; material: A"],
        );
        let error = mesh(&doc).unwrap_err();
        assert_eq!(error.code, code::SCULPT_DOCUMENT_DATA);
        assert_eq!(error.path.as_deref(), Some("/resolution"));
    }

    /// The culprit is whichever part owns the field there, including one written after the victim —
    /// the case that cost the creature's author a render, since the warning read like the part
    /// before it.
    #[test]
    fn a_hidden_part_names_a_later_culprit() {
        let doc = sculpt(
            32,
            4000,
            &[
                "name: spine; shape: box; size: [0.4, 0.4, 2.4]; at: [2, 0, 0]; material: A",
                "name: pearl; shape: sphere; radius: 0.2; material: A",
                "name: belly; shape: sphere; radius: 1.0; material: A",
            ],
        );
        let mesh = mesh(&doc).unwrap();
        let diagnostics = unresolved_parts(&doc, &SculptSources::new(), &mesh);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0].message.contains("hidden by part `belly`"),
            "{}",
            diagnostics[0].message
        );
    }

    /// The mesher never runs the culprit search: `unresolved_parts` runs one pass per surfaceless
    /// part, and none when every part shows.
    #[test]
    fn mesh_with_never_runs_the_hider_pass() {
        let hidden = sculpt(
            32,
            4000,
            &[
                "name: spine; shape: box; size: [0.4, 0.4, 2.4]; at: [2, 0, 0]; material: A",
                "name: pearl; shape: sphere; radius: 0.2; material: A",
                "name: belly; shape: sphere; radius: 1.0; material: A",
            ],
        );
        HIDER_PASSES.with(|passes| passes.set(0));
        let hidden_mesh = mesh(&hidden).unwrap();
        assert_eq!(HIDER_PASSES.with(|passes| passes.get()), 0);
        assert_eq!(hidden_mesh.stats.parts[1].surface_vertices, 0);
        let diagnostics = unresolved_parts(&hidden, &SculptSources::new(), &hidden_mesh);
        assert_eq!(HIDER_PASSES.with(|passes| passes.get()), 1);
        assert!(diagnostics[0].message.contains("hidden by part `belly`"));

        let shown = sculpt(
            32,
            4000,
            &["name: ball; shape: sphere; radius: 0.5; material: A"],
        );
        HIDER_PASSES.with(|passes| passes.set(0));
        let shown_mesh = mesh(&shown).unwrap();
        assert!(unresolved_parts(&shown, &SculptSources::new(), &shown_mesh).is_empty());
        assert_eq!(HIDER_PASSES.with(|passes| passes.get()), 0);
    }

    /// A part hidden under a union adds nothing to the folded field, so deleting it leaves the
    /// geometry as it was.
    #[test]
    fn a_hidden_part_leaves_the_geometry_unchanged() {
        let belly = "name: belly; shape: sphere; radius: 1.0; material: A";
        let with = mesh(&sculpt(
            32,
            4000,
            &[
                "name: pearl; shape: sphere; radius: 0.2; material: A",
                belly,
            ],
        ))
        .unwrap();
        let without = mesh(&sculpt(32, 4000, &[belly])).unwrap();
        assert_eq!(with.positions, without.positions);
        assert_eq!(with.normals, without.normals);
        assert_eq!(with.indices, without.indices);
    }

    #[test]
    fn hidden_and_thin_parts_are_unresolved() {
        let doc = sculpt(
            32,
            4000,
            &[
                "name: body; shape: sphere; radius: 1.0; material: A",
                "name: pearl; shape: sphere; radius: 0.2; material: A",
                "name: fin; shape: box; size: [0.8, 0.04, 0.8]; at: [0, 1.2, 0]; material: A",
            ],
        );
        let mesh = mesh(&doc).unwrap();
        let diagnostics = unresolved_parts(&doc, &SculptSources::new(), &mesh);
        let paths: Vec<_> = diagnostics.iter().map(|d| d.path.as_deref()).collect();
        assert_eq!(paths, vec![Some("/parts/1"), Some("/parts/2")]);
        assert!(
            diagnostics
                .iter()
                .all(|d| d.code == code::SCULPT_PART_UNRESOLVED)
        );
        assert!(diagnostics[0].message.contains("contributes no surface"));
        // The warning names the part that owns the field there, not just the victim.
        assert!(
            diagnostics[0].message.contains("hidden by part `body`"),
            "{}",
            diagnostics[0].message
        );
        assert!(!mesh.part_of.contains(&Some(1)));
        assert!(
            diagnostics[1]
                .message
                .contains("thinner than two grid cells")
        );
        assert!(
            diagnostics[1]
                .help
                .as_deref()
                .unwrap()
                .contains("resolution")
        );
    }

    /// A solid `n x n x n` voxel model, one glyph, filled to its own grid boundary — the shape the
    /// distance transform's padding ring exists for.
    fn solid_block(n: u32) -> ludion_scene::VoxelDocument {
        let row = "S".repeat(n as usize);
        let mut slices = String::new();
        for _ in 0..n {
            for z in 0..n {
                slices.push_str(if z == 0 { "  - - " } else { "    - " });
                slices.push_str(&row);
                slices.push('\n');
            }
        }
        let source = format!(
            "format: 1\nkind: voxel\nsize:\n  - {n}\n  - {n}\n  - {n}\n\
             palette:\n  S:\n    color:\n      - 1.0\n      - 0.5\n      - 0.25\n      - 1.0\n\
             slices:\n{slices}"
        );
        ludion_scene::check_voxel(&source).unwrap_or_else(|d| panic!("{d:#?}"))
    }

    /// The sources one `block.voxel.yaml` of `n` voxels a side makes.
    fn block_sources(n: u32) -> SculptSources {
        let mut sources = SculptSources::new();
        sources.insert_voxel("block.voxel.yaml", &solid_block(n));
        sources
    }

    /// A `shape: voxel` part meshes the cube its occupancy describes, at the extent its `size`
    /// buys, bottom-centred like every other model — and twice over, bit for bit.
    #[test]
    fn a_voxel_part_meshes_its_model_box() {
        let doc = sculpt(
            16,
            4000,
            &["name: block; shape: voxel; path: block.voxel.yaml; size: 0.25; material: A"],
        );
        let sources = block_sources(4);
        let mesh = mesh_with(&doc, &sources).unwrap();
        assert_closed(&mesh.indices);
        assert!(mesh.stats.vertices > 0);
        assert_eq!(mesh_with(&doc, &sources).unwrap(), mesh);

        // Four voxels at 0.25 units each is a unit cube: x and z centred, y on the ground.
        let cell = 1.0 / 16.0;
        let (lo, hi) = mesh.stats.parts[0].bounds.expect("the block has a surface");
        for (value, expected) in lo.into_iter().zip([-0.5, 0.0, -0.5]) {
            assert!(
                (value - expected).abs() <= cell,
                "{lo:?} against the model box"
            );
        }
        for (value, expected) in hi.into_iter().zip([0.5, 1.0, 0.5]) {
            assert!(
                (value - expected).abs() <= cell,
                "{hi:?} against the model box"
            );
        }
    }

    /// `blend`, `mirror` and `deform` read a voxel part exactly as they read a primitive: the
    /// modifiers work on the part's local frame, which a voxel field has like any other.
    #[test]
    fn a_voxel_part_blends_mirrors_and_deforms() {
        let sources = block_sources(4);
        let blended = sculpt(
            16,
            4000,
            &[
                "name: block; shape: voxel; path: block.voxel.yaml; size: 0.25; material: A",
                "name: knob; shape: sphere; radius: 0.3; at: [0, 1.0, 0]; blend: 0.2; material: A",
            ],
        );
        let blended = mesh_with(&blended, &sources).unwrap();
        assert_closed(&blended.indices);
        assert!(
            blended.stats.parts[1].surface_vertices > 0,
            "the knob shows"
        );

        let mirrored = sculpt(
            16,
            4000,
            &[
                "name: block; shape: voxel; path: block.voxel.yaml; size: 0.25; \
                 at: [0.75, 0, 0]; mirror: x; material: A",
            ],
        );
        let mirrored = mesh_with(&mirrored, &sources).unwrap();
        assert_closed(&mirrored.indices);
        let (lo, hi) = mirrored.stats.parts[0]
            .bounds
            .expect("the block has a surface");
        // Two copies about `x = 0`: the pair is symmetric, so the meshed box is too.
        assert!(
            (lo[0] + hi[0]).abs() <= 1.0 / 16.0,
            "{lo:?}..{hi:?} is not symmetric"
        );

        let tapered = sculpt_source(
            16,
            4000,
            "  - name: block\n    shape: voxel\n    path: block.voxel.yaml\n    size: 0.25\n    \
             material: A\n    deform:\n      taper:\n        axis: y\n        from: 1.0\n        \
             to: 0.4\n",
        );
        let tapered = mesh_with(&tapered, &sources).unwrap();
        assert_closed(&tapered.indices);
        let (_, hi) = tapered.stats.parts[0]
            .bounds
            .expect("the block has a surface");
        let top = tapered
            .positions
            .chunks_exact(3)
            .filter(|v| v[1] > hi[1] - 1.0 / 16.0)
            .fold(0.0f32, |widest, v| widest.max(v[0].abs()));
        assert!(top < 0.35, "the taper does not narrow the top: {top}");
    }

    /// The document alone cannot mesh a voxel part: this crate reads no files, so the refusal names
    /// the part whose source the caller still owes.
    #[test]
    fn a_voxel_part_without_its_source_refuses() {
        let doc = sculpt(
            16,
            4000,
            &[
                "name: pad; shape: sphere; radius: 0.2; material: A",
                "name: block; shape: voxel; path: block.voxel.yaml; size: 0.25; material: A",
            ],
        );
        assert_eq!(voxel_paths(&doc), ["block.voxel.yaml"]);
        let error = mesh(&doc).unwrap_err();
        assert_eq!(error.code, code::SCULPT_DOCUMENT_DATA);
        assert_eq!(error.path.as_deref(), Some("/parts/1/path"));
        assert!(
            error.message.contains("block.voxel.yaml"),
            "{}",
            error.message
        );
        assert!(mesh_with(&doc, &block_sources(4)).is_ok());
    }
}
