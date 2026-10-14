//! The document's parts folded into one field, and the corner lattice that field is sampled on.
//!
//! A part's copies (every `repeat` copy and, with `mirror`, its reflection across the model's origin
//! plane) combine by exact `min` into one part distance before the part's op runs. The lattice is
//! snapped to whole multiples of the cell spacing, so a corner's position is `index / resolution`
//! and a model symmetric about an origin plane samples symmetrically.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{SculptAxis, SculptDocument, SculptOp, SculptPart, SculptShape};

use crate::field::{
    MeshField, Modifiers, Noise, PartTransform, PlacedPrimitive, Primitive, Taper, Turn,
    VoxelField, smax, smin,
};
use crate::sources::SculptSources;

/// The most lattice corners one document may sample; a larger grid is refused as `L1157` rather
/// than allocated.
pub(crate) const MAX_GRID_CORNERS: u64 = 16_000_000;

/// Cells of margin beyond the widest blend on every side of the parts' bounds, so every boundary
/// corner lies outside the surface.
const PAD_CELLS: f32 = 2.0;

/// A model-space axis-aligned box `(min, max)`.
pub(crate) type Aabb = ([f32; 3], [f32; 3]);

/// One document part compiled to `f32`: its placed copies and how it folds.
pub(crate) struct FieldPart {
    copies: Vec<PlacedPrimitive>,
    mirror: Option<usize>,
    /// `None` when the part authored neither `deform` nor `noise` — the arithmetic of a plain part
    /// is then exactly what it was before either key existed.
    modifiers: Option<Modifiers>,
    pub(crate) op: SculptOp,
    pub(crate) blend: f32,
}

impl FieldPart {
    /// Compiles one document part. `index` is its position in the document, so a `shape: voxel`
    /// part whose source the caller did not supply reports the pointer the author can read.
    fn new(
        part: &ludion_scene::SculptPart,
        index: usize,
        sources: &SculptSources,
    ) -> Result<Self, Diagnostic> {
        let primitive = primitive(&part.shape, index, sources)?;
        let at = narrow(part.at);
        let rotation = part.rotation.map_or([0.0; 3], narrow);
        let (count, step) = part
            .repeat
            .as_ref()
            .map_or((1, [0.0; 3]), |repeat| (repeat.count, narrow(repeat.step)));
        let copies = (0..count)
            .map(|k| {
                let k = k as f32;
                let at = [
                    at[0] + step[0] * k,
                    at[1] + step[1] * k,
                    at[2] + step[2] * k,
                ];
                PlacedPrimitive {
                    primitive: primitive.clone(),
                    transform: PartTransform::new(at, rotation),
                }
            })
            .collect();
        let modifiers = modifiers(part, &primitive);
        Ok(FieldPart {
            copies,
            mirror: part.mirror.map(axis_index),
            modifiers,
            op: part.op,
            blend: part.blend as f32,
        })
    }

    /// The signed distance of the part, every copy included, at the model-space point `p`.
    pub(crate) fn distance(&self, p: [f32; 3]) -> f32 {
        let reflected = self.mirror.map(|axis| {
            let mut q = p;
            q[axis] = -q[axis];
            q
        });
        let mut distance = f32::INFINITY;
        for copy in &self.copies {
            distance = distance.min(self.copy_distance(copy, p));
            if let Some(q) = reflected {
                distance = distance.min(self.copy_distance(copy, q));
            }
        }
        distance
    }

    /// One copy's distance: the primitive's own, or the modifiers' when the part carries any. The
    /// modifiers work in the copy's local frame, so `deform` and `noise` travel with the part's
    /// `at`, `rotation`, `repeat` and `mirror`.
    fn copy_distance(&self, copy: &PlacedPrimitive, p: [f32; 3]) -> f32 {
        match &self.modifiers {
            Some(modifiers) => modifiers.distance(&copy.primitive, copy.transform.to_local(p)),
            None => copy.distance(p),
        }
    }

    /// A conservative box enclosing every copy.
    fn bounds(&self) -> Aabb {
        let mut bounds = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for copy in &self.copies {
            let (min, max) = match &self.modifiers {
                Some(modifiers) => copy.bounds_of(modifiers.local_bounds()),
                None => copy.bounds(),
            };
            bounds = union(bounds, (min, max));
            if let Some(axis) = self.mirror {
                let (mut rmin, mut rmax) = (min, max);
                rmin[axis] = -max[axis];
                rmax[axis] = -min[axis];
                bounds = union(bounds, (rmin, rmax));
            }
        }
        bounds
    }
}

/// Every part of a document, in fold order.
pub(crate) struct SculptField {
    pub(crate) parts: Vec<FieldPart>,
}

impl SculptField {
    /// Compiles every part of `doc`, reading each `shape: voxel` part's field out of `sources`.
    pub(crate) fn new(doc: &SculptDocument, sources: &SculptSources) -> Result<Self, Diagnostic> {
        let parts = doc
            .parts
            .iter()
            .enumerate()
            .map(|(index, part)| FieldPart::new(part, index, sources))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SculptField { parts })
    }

    /// The folded field at `p`: `+inf` before the first part, then each part's op in order.
    pub(crate) fn value(&self, p: [f32; 3]) -> f32 {
        let mut field = f32::INFINITY;
        for part in &self.parts {
            field = fold(field, part, part.distance(p)).0;
        }
        field
    }

    /// The index of the part whose surface is nearest `p` (smallest `|distance|`, ties to the lower
    /// index), or `None` with no parts.
    pub(crate) fn nearest_part(&self, p: [f32; 3]) -> Option<usize> {
        let mut nearest: Option<(usize, f32)> = None;
        for (index, part) in self.parts.iter().enumerate() {
            let distance = part.distance(p).abs();
            if nearest.is_none_or(|(_, best)| distance < best) {
                nearest = Some((index, distance));
            }
        }
        nearest.map(|(index, _)| index)
    }

    fn bounds(&self) -> Option<Aabb> {
        self.parts.iter().map(FieldPart::bounds).reduce(union)
    }

    fn max_blend(&self) -> f32 {
        self.parts
            .iter()
            .fold(0.0, |widest: f32, part| widest.max(part.blend))
    }
}

/// Folds `distance` of `part` into the running `field` by the part's op, returning the new field
/// and the smooth-combine weight (the incoming part's share).
pub(crate) fn fold(field: f32, part: &FieldPart, distance: f32) -> (f32, f32) {
    match part.op {
        SculptOp::Union => smin(field, distance, part.blend),
        SculptOp::Subtract => smax(field, -distance, part.blend),
        SculptOp::Intersect => smax(field, distance, part.blend),
    }
}

/// The field sampled once per corner of a lattice covering every part.
pub(crate) struct Grid {
    /// Lattice index of corner `(0, 0, 0)`; corner `c` sits at `(origin + c) / resolution`.
    origin: [i64; 3],
    /// Cells along x, y, z; corners are one more on each axis.
    pub(crate) cells: [usize; 3],
    resolution: f32,
    /// Corner values in `z, y, x` order.
    samples: Vec<f32>,
}

impl Grid {
    /// Samples `field` at `resolution` cells per unit, or `None` when the document has no parts.
    /// A lattice above [`MAX_GRID_CORNERS`] is an `L1157` at `/resolution`.
    pub(crate) fn sample(field: &SculptField, resolution: u32) -> Result<Option<Grid>, Diagnostic> {
        let Some(mut grid) = Grid::frame(field, resolution)? else {
            return Ok(None);
        };
        let [nx, ny, nz] = grid.cells;
        grid.samples.reserve_exact((nx + 1) * (ny + 1) * (nz + 1));
        for z in 0..=nz {
            for y in 0..=ny {
                for x in 0..=nx {
                    let value = field.value(grid.corner_position([x, y, z]));
                    grid.samples.push(value);
                }
            }
        }
        Ok(Some(grid))
    }

    /// The lattice `sample` fills, with no samples: for passes that evaluate the field themselves.
    /// `value` must not be called on it.
    pub(crate) fn frame(field: &SculptField, resolution: u32) -> Result<Option<Grid>, Diagnostic> {
        let Some((min, max)) = field.bounds() else {
            return Ok(None);
        };
        let res = resolution as f32;
        let pad = field.max_blend() + PAD_CELLS / res;
        let lo = min.map(|v| ((v - pad) * res).floor() as i64);
        let hi = max.map(|v| ((v + pad) * res).ceil() as i64);
        let too_large = |summary: String| {
            Diagnostic::new(code::SCULPT_DOCUMENT_DATA, summary)
                .with_path("/resolution")
                .with_help(format!(
                    "lower `resolution` (the model spans {:.3}×{:.3}×{:.3} world units, each cell \
                     1/{resolution}) or shrink the parts' extent",
                    max[0] - min[0],
                    max[1] - min[1],
                    max[2] - min[2]
                ))
        };
        let extent = |axis: usize| hi[axis].checked_sub(lo[axis]);
        let (Some(x), Some(y), Some(z)) = (extent(0), extent(1), extent(2)) else {
            return Err(too_large(format!(
                "the sculpt's grid extent overflows the lattice index range, far above the \
                 mesher's cap of {MAX_GRID_CORNERS} corners"
            )));
        };
        let cells = [x, y, z].map(|n| n.max(1) as u64);
        let corners = (cells[0] + 1)
            .saturating_mul(cells[1] + 1)
            .saturating_mul(cells[2] + 1);
        if corners > MAX_GRID_CORNERS {
            return Err(too_large(format!(
                "the sculpt's grid is {}×{}×{} cells ({corners} corners), above the mesher's cap \
                 of {MAX_GRID_CORNERS} corners",
                cells[0], cells[1], cells[2]
            )));
        }
        let cells = cells.map(|n| n as usize);
        Ok(Some(Grid {
            origin: lo,
            cells,
            resolution: res,
            samples: Vec::new(),
        }))
    }

    /// The model-space position of corner `c`.
    pub(crate) fn corner_position(&self, c: [usize; 3]) -> [f32; 3] {
        [0, 1, 2].map(|axis| (self.origin[axis] + c[axis] as i64) as f32 / self.resolution)
    }

    /// The field value at corner `c`.
    pub(crate) fn value(&self, c: [usize; 3]) -> f32 {
        let (nx, ny) = (self.cells[0] + 1, self.cells[1] + 1);
        self.samples[(c[2] * ny + c[1]) * nx + c[0]]
    }

    /// World units per cell.
    pub(crate) fn spacing(&self) -> f32 {
        1.0 / self.resolution
    }

    /// The lattice's model-space box, corner to corner.
    pub(crate) fn extent(&self) -> Aabb {
        (
            self.corner_position([0; 3]),
            self.corner_position(self.cells),
        )
    }

    /// The number of cells.
    pub(crate) fn cell_count(&self) -> u64 {
        self.cells.iter().map(|&n| n as u64).product()
    }
}

/// The smallest box enclosing both.
pub(crate) fn union(a: Aabb, b: Aabb) -> Aabb {
    (
        [0, 1, 2].map(|axis| a.0[axis].min(b.0[axis])),
        [0, 1, 2].map(|axis| a.1[axis].max(b.1[axis])),
    )
}

fn narrow(v: [f64; 3]) -> [f32; 3] {
    v.map(|c| c as f32)
}

fn axis_index(axis: SculptAxis) -> usize {
    match axis {
        SculptAxis::X => 0,
        SculptAxis::Y => 1,
        SculptAxis::Z => 2,
    }
}

/// The part's `deform` and `noise` compiled against the primitive's own local box, or `None` when
/// it authored neither.
fn modifiers(part: &SculptPart, primitive: &Primitive) -> Option<Modifiers> {
    let deform = part.deform.as_ref();
    let turn = |turn: &ludion_scene::SculptTurn| Turn {
        axis: axis_index(turn.axis),
        degrees: turn.degrees as f32,
    };
    let bend = deform.and_then(|d| d.bend.as_ref()).map(turn);
    let twist = deform.and_then(|d| d.twist.as_ref()).map(turn);
    let taper = deform.and_then(|d| d.taper.as_ref()).map(|taper| Taper {
        axis: axis_index(taper.axis),
        from: taper.from as f32,
        to: taper.to as f32,
    });
    let noise = part.noise.as_ref().map(|noise| Noise {
        amplitude: noise.amplitude as f32,
        scale: noise.scale as f32,
        seed: noise.seed,
    });
    if bend.is_none() && twist.is_none() && taper.is_none() && noise.is_none() {
        return None;
    }
    Some(Modifiers::new(
        primitive.local_bounds(),
        bend,
        twist,
        taper,
        noise,
    ))
}

fn primitive(
    shape: &SculptShape,
    index: usize,
    sources: &SculptSources,
) -> Result<Primitive, Diagnostic> {
    Ok(match shape {
        SculptShape::Voxel { path, size } => {
            let Some(source) = sources.voxel(path) else {
                return Err(missing_source("voxel", path, index));
            };
            Primitive::Voxel(VoxelField::new(
                source.grid.clone(),
                source.dims,
                *size as f32,
            ))
        }
        SculptShape::Mesh { path, size } => {
            let Some(source) = sources.mesh(path) else {
                return Err(missing_source("mesh", path, index));
            };
            Primitive::Mesh(MeshField::new(
                source.grid.clone(),
                source.cell,
                source.origin,
                source.bounds,
                size.unwrap_or(1.0) as f32,
            ))
        }
        &SculptShape::Sphere { radius } => Primitive::Sphere {
            radius: radius as f32,
        },
        &SculptShape::Capsule { radius, from, to } => Primitive::Capsule {
            radius: radius as f32,
            from: narrow(from),
            to: narrow(to),
        },
        &SculptShape::Ellipsoid { radii } => Primitive::Ellipsoid {
            radii: narrow(radii),
        },
        &SculptShape::Box { size, round } => Primitive::Box {
            size: narrow(size),
            round: round as f32,
        },
        &SculptShape::Torus { radius, tube } => Primitive::Torus {
            radius: radius as f32,
            tube: tube as f32,
        },
        &SculptShape::Cone { radius, height } => Primitive::Cone {
            radius: radius as f32,
            height: height as f32,
        },
    })
}

/// `L1157` for a part whose source document the caller never supplied — the whole answer a pure
/// crate can give, since reading the file is the caller's half of the contract.
fn missing_source(shape: &str, path: &str, index: usize) -> Diagnostic {
    let (extension, paths) = match shape {
        "voxel" => (".voxel.yaml", "voxel_paths"),
        _ => (".mesh.yaml", "mesh_paths"),
    };
    Diagnostic::new(
        code::SCULPT_DOCUMENT_DATA,
        format!(
            "part {index} is a `shape: {shape}` reading `{path}`, but no source was supplied for \
             that path"
        ),
    )
    .with_path(format!("/parts/{index}/path"))
    .with_help(format!(
        "a {shape}-shaped part needs its `{extension}` read and passed in: mesh through \
         `mesh_with(doc, &sources)` with every `{paths}(doc)` entry inserted, not through \
         `mesh(doc)`"
    ))
}
