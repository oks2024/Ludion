//! The derived tree: one bake shared by writing and checking.
//!
//! A `kind: mesh-edit` document, the `kind: mesh` document it names and that mesh's committed maps
//! are the three inputs; the output is a directory the derive owns every byte of — `mesh.mesh.yaml`
//! carrying `generated_from:` plus one content-hash-named PNG per edited map. Maps no op touched
//! are **referenced** from the derived document, not copied, so one new emissive map costs one new
//! file rather than a second set of five.
//!
//! [`derive()`] produces the tree in memory; [`Derived::write`] commits it and [`Derived::check`]
//! byte-compares it, the shape `generate-atlas --check` established. The derived tree is never
//! hand-edited: it is regenerated from its three inputs, and a difference is drift.
//!
//! Every path the derived document carries is computed **lexically**, never through the filesystem,
//! so the same edit document derives byte-identical text on Windows and macOS — where resolving a
//! real path folds `..` differently.

use std::path::{Path, PathBuf};

use ludion_core::atomic::{is_temp_name_for, write_atomic};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::f64::sqrt;
use ludion_core::paths::{normalize, relative_refusal, sibling, slash};
use ludion_scene::{
    MeshDocument, MeshEditDocument, MeshEditKey, MeshEditMap, MeshEditOp, MeshEditRegion,
    MeshEditRegionShape, MeshTextures, TextureRef,
};
use serde::Serialize;

use crate::fill::{self, FillRefusal, FillSpec, FillWeights, Surface};
use crate::paint::{self, DILATE_PAD, MapSampler, WorkingMap};
use crate::raster::{self, Coverage};
use crate::region::{self, Aabb};

/// The name the derived mesh document always takes inside the derived directory.
pub const DERIVED_DOCUMENT: &str = "mesh.mesh.yaml";

/// The folder derived maps are written into, relative to the derived directory.
pub const DERIVED_TEXTURE_DIR: &str = "textures";

/// The five texture slots in canonical order — the order maps are repointed and reported in.
const ALL_MAPS: [MeshEditMap; 5] = [
    MeshEditMap::BaseColor,
    MeshEditMap::MetallicRoughness,
    MeshEditMap::Normal,
    MeshEditMap::Occlusion,
    MeshEditMap::Emissive,
];

/// The reminder every report ends on: a map alone never glows.
const NEXT_HINT: &str = "a map carries the glow's shape and colour, never its intensity: the \
                         entity pointing at this mesh needs a non-zero `Mesh.emissive`, and its \
                         `Mesh.color` tints the result";

/// One file of the derived tree: where it goes and the exact bytes that belong there.
#[derive(Clone, Debug, PartialEq)]
pub struct DerivedFile {
    /// The path, always inside the derived directory.
    pub path: PathBuf,
    /// The file's bytes.
    pub bytes: Vec<u8>,
}

/// A derived tree held in memory: the document, the maps it owns, and the report of how it got
/// there.
#[derive(Clone, Debug)]
pub struct Derived {
    out_dir: PathBuf,
    document_path: PathBuf,
    document: String,
    generated_from: String,
    textures: Vec<DerivedFile>,
    report: Report,
}

impl Derived {
    /// The directory this tree owns every byte of.
    pub fn out_dir(&self) -> &Path {
        &self.out_dir
    }

    /// The derived mesh document's canonical text.
    pub fn document(&self) -> &str {
        &self.document
    }

    /// Every file of the tree, document first, each with the bytes that belong at its path.
    pub fn files(&self) -> Vec<(&Path, &[u8])> {
        let mut files: Vec<(&Path, &[u8])> =
            vec![(self.document_path.as_path(), self.document.as_bytes())];
        files.extend(
            self.textures
                .iter()
                .map(|texture| (texture.path.as_path(), texture.bytes.as_slice())),
        );
        files
    }

    /// What each op painted and what each written map holds — the instruments that answer "did this
    /// edit land where I meant it to" without rendering anything.
    pub fn report(&self) -> &Report {
        &self.report
    }

    /// Writes the tree, creating the derived directory and replacing what an earlier derive left
    /// there.
    ///
    /// Files matching the derive's own naming (`mesh.mesh.yaml`, `textures/<16 hex>.png`) are
    /// rewritten, and one the derive no longer produces is removed. Anything else under the derived
    /// directory is a hand edit of generated content or the wrong directory, and so is a tree whose
    /// document names another edit: both are refused before a single byte is written. The maps go
    /// first (their names are content hashes, so a reader of the old tree is undisturbed), the
    /// document lands by temp-then-rename, and stale files go last — a failure partway leaves the
    /// previous tree whole.
    ///
    /// # Errors
    /// `L1176` when the derived directory holds a file the derive did not write or another edit's
    /// tree; `L1015` when the tree cannot be listed or its document read; `L1038` when a file cannot
    /// be written, encoded or removed.
    pub fn write(&self) -> Result<Vec<PathBuf>, Vec<Diagnostic>> {
        let existing = list_existing(&self.out_dir)?;
        let foreign: Vec<Diagnostic> = existing
            .iter()
            .filter(|(relative, _)| !is_derive_owned(relative))
            .map(|(_, path)| {
                inapplicable(format!(
                    "the derived directory `{}` holds `{}`, which this edit does not derive",
                    slash(&self.out_dir),
                    slash(path)
                ))
                .with_help(
                    "the derive owns every byte under `out`: move the file elsewhere, or point \
                     `out` at a directory of this edit's own",
                )
            })
            .collect();
        if !foreign.is_empty() {
            return Err(foreign);
        }
        self.check_owner()?;

        let files = self.files();
        for (path, bytes) in files.iter().skip(1) {
            write_file(path, bytes)?;
        }
        write_file(&self.document_path, self.document.as_bytes())?;

        // A map an earlier edit produced and this one does not would otherwise sit in the tree
        // forever, and the drift gate has no way to tell it from a file someone added.
        for (_, path) in &existing {
            if files.iter().any(|(written, _)| written == path) {
                continue;
            }
            remove_stale(path)?;
        }
        Ok(files.iter().map(|(path, _)| path.to_path_buf()).collect())
    }

    /// Refuses a derived directory whose document names another edit. A line scan, not a parse: a
    /// drifted or hand-broken copy of this edit's own document must still be regenerable.
    fn check_owner(&self) -> Result<(), Vec<Diagnostic>> {
        let text = match std::fs::read_to_string(&self.document_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(vec![read_io(format!(
                    "could not read `{}` to check who owns it: {error}",
                    slash(&self.document_path)
                ))]);
            }
        };
        let owner = text
            .lines()
            .find_map(|line| line.strip_prefix("generated_from:"))
            .map(|value| value.trim().trim_matches(['"', '\'']));
        if owner.is_some_and(|owner| owner.eq_ignore_ascii_case(&self.generated_from)) {
            return Ok(());
        }
        let holder = match owner {
            Some(owner) => format!("the tree of `{owner}`"),
            None => "a mesh no edit derived".to_string(),
        };
        Err(vec![
            inapplicable(format!(
                "the derived directory `{}` holds {holder}, not the tree of this edit",
                slash(&self.out_dir)
            ))
            .with_help(
                "give this edit an `out` of its own (omitting `out:` takes the document's stem), \
                 or delete that tree if its edit is gone",
            ),
        ])
    }

    /// Byte-compares the committed tree against these bytes, writing nothing: one `DRIFT <path>`
    /// diagnostic (`L1173`) per file that differs, cannot be read, or sits in the directory without
    /// being derived.
    pub fn check(&self) -> Vec<Diagnostic> {
        let mut drifted = Vec::new();
        let files = self.files();
        for (path, bytes) in &files {
            match std::fs::read(path) {
                Ok(committed) if committed == *bytes => {}
                Ok(_) => drifted.push(drift(
                    path,
                    "the committed file differs from what this edit now derives",
                )),
                Err(error) => drifted.push(drift(
                    path,
                    &format!("could not read the committed file to compare: {error}"),
                )),
            }
        }
        match list_existing(&self.out_dir) {
            Ok(existing) => {
                for (_, path) in existing {
                    if files.iter().any(|(written, _)| *written == path) {
                        continue;
                    }
                    drifted.push(drift(&path, "this edit does not derive that file"));
                }
            }
            Err(mut errors) => drifted.append(&mut errors),
        }
        drifted
    }
}

/// The machine-readable account of one derive: per op what it painted, per written map what it
/// holds, and the reminder that intensity lives in the scene.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    /// The `source:` the edit document names, as authored.
    pub source: String,
    /// The derived directory, relative to the edit document.
    pub out: String,
    /// One entry per op, in document order.
    pub ops: Vec<OpReport>,
    /// One entry per map the derive wrote.
    pub maps: Vec<MapReport>,
    /// What the author has to do elsewhere for the edit to be visible.
    pub next: String,
}

/// What one op changed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OpReport {
    /// The op's index in the document's `ops:` list.
    pub index: usize,
    /// The op's verb (`paint`).
    pub op: String,
    /// The map it targets.
    pub map: String,
    /// Texels whose weight was above zero.
    pub texels: usize,
    /// Those texels as a fraction of the whole map.
    pub share: f64,
    /// The model-space box of the surface the op painted.
    pub bounds: Option<Aabb>,
    /// How many distinct triangles that surface spans.
    pub triangles: usize,
}

/// What one written map holds.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MapReport {
    /// The slot this map fills.
    pub map: String,
    /// The file, relative to the derived directory.
    pub path: String,
    /// The map's width in texels.
    pub width: u32,
    /// The map's height in texels.
    pub height: u32,
    /// Texels whose colour is not black — for an emissive map, the lit ones.
    pub non_black_texels: usize,
    /// Whether the derive created the map rather than painting over one the source already had.
    pub created: bool,
}

/// Derives, in memory, the whole tree the edit document at `edit_path` owns.
///
/// # Errors
/// The mesh-edit document's own `L1173`/`L1174`; `L1176` when the edit cannot apply to the source
/// it names (no `uvs`, a map that can be neither loaded nor sized, a `key` or `fill` reading a map
/// nothing provides); `L1177` for an op whose region covers no texel of the surface; `L1178` for a
/// `key` that rejects every texel its region reaches, or a `fill` seed off the surface or outside
/// its radius; `L1015` when an input cannot be read.
pub fn derive(edit_path: &Path) -> Result<Derived, Vec<Diagnostic>> {
    let text = read_text(edit_path)?;
    let edit = ludion_scene::check_mesh_edit(&text)?;
    let edit_dir = edit_path.parent().unwrap_or_else(|| Path::new("."));
    let edit_name = file_name(edit_path);
    let stem = document_stem(&edit_name);
    let out_text = edit.out_dir(&stem);
    let out_rel = folded(out_text);
    // `out_rel` is empty for an `out` of `./`.
    let refusal = relative_refusal(out_text).map(|refusal| refusal.describe());
    if refusal.is_some() || out_rel.is_empty() {
        let reason = refusal.unwrap_or("does not name a directory");
        return Err(vec![
            Diagnostic::new(
                code::MESH_EDIT_DATA,
                format!("`out` (`{out_text}`) {reason}"),
            )
            .with_path("/out")
            .with_help("rename the document or add `out:` naming a directory beside it"),
        ]);
    }

    let source_rel = folded(&edit.source);
    // ASCII-folded: on a case-insensitive filesystem `Self/x` and `self` are one directory.
    let inside_out = source_rel.len() >= out_rel.len()
        && out_rel
            .iter()
            .zip(&source_rel)
            .all(|(out, source)| out.eq_ignore_ascii_case(source));
    if inside_out {
        return Err(vec![
            Diagnostic::new(
                code::MESH_EDIT_DATA,
                "`source` lies inside this edit's own `out`, which the derive rewrites".to_string(),
            )
            .with_path("/source")
            .with_help(
                "point `source` at the mesh the tree was derived from, or give a chained edit \
                 its own `out`",
            ),
        ]);
    }
    let source_path = join(edit_dir, &source_rel);
    let source = read_source(&source_path)?;
    let Some(uvs) = source.uvs.clone() else {
        return Err(vec![
            inapplicable(format!(
                "the source mesh `{}` has no `uvs`, so a texture map has nowhere to paint",
                slash(&source_path)
            ))
            .with_help(
                "re-import or re-generate the mesh with a UV set; a map is sampled at vertex UVs",
            ),
        ]);
    };
    let source_dir_rel = parent(&source_rel);
    let source_dir = join(edit_dir, &source_dir_rel);

    let (painted, ops) = apply_ops(&edit, &source, &uvs, &source_dir)?;
    let out_dir = join(edit_dir, &out_rel);

    let mut derived = source.clone();
    let generated_from = relative(&out_rel, &[edit_name]);
    derived.generated_from = Some(generated_from.clone());
    let mut textures = Vec::new();
    let mut maps = Vec::new();

    for slot in ALL_MAPS {
        let Some(state) = painted.iter().find(|state| state.map == slot) else {
            // An untouched map keeps its pristine file, repointed from the derived directory.
            if let Some(reference) = reference_mut(&mut derived.textures, slot) {
                let target = folded(&sibling(&source_rel.join("/"), &reference.path));
                reference.path = relative(&out_rel, &target);
            }
            continue;
        };
        let rgba = state.data.to_rgba8();
        let bytes = ludion_assets::encode_png(state.data.width(), state.data.height(), &rgba)
            .map_err(|detail| {
                vec![write_io(format!(
                    "encoding the derived `{}` map failed: {detail}",
                    slot.name()
                ))]
            })?;
        let file_name = format!("{:016x}.png", ludion_assets::content_hash(&bytes));
        let path = format!("{DERIVED_TEXTURE_DIR}/{file_name}");
        maps.push(MapReport {
            map: slot.name().to_string(),
            path: path.clone(),
            width: state.data.width(),
            height: state.data.height(),
            non_black_texels: paint::non_black_texels(&rgba),
            created: state.created,
        });
        textures.push(DerivedFile {
            path: out_dir.join(DERIVED_TEXTURE_DIR).join(&file_name),
            bytes,
        });
        match reference_mut(&mut derived.textures, slot) {
            Some(reference) => reference.path = path,
            None => set_reference(&mut derived.textures, slot, TextureRef::new(path)),
        }
    }

    let document = ludion_scene::emit_mesh(&derived);
    self_check(&document)?;

    Ok(Derived {
        document_path: out_dir.join(DERIVED_DOCUMENT),
        out_dir,
        document,
        generated_from,
        textures,
        report: Report {
            source: edit.source.clone(),
            out: out_rel.join("/"),
            ops,
            maps,
            next: NEXT_HINT.to_string(),
        },
    })
}

/// One map under edit: the pixels, and whether the derive created it.
struct PaintedMap {
    map: MeshEditMap,
    created: bool,
    data: WorkingMap,
}

/// Runs the op list in order over the maps it targets, then bleeds each written map outward past
/// its UV islands.
fn apply_ops(
    edit: &MeshEditDocument,
    source: &MeshDocument,
    uvs: &[f64],
    source_dir: &Path,
) -> Result<(Vec<PaintedMap>, Vec<OpReport>), Vec<Diagnostic>> {
    let mut states: Vec<PaintedMap> = Vec::new();
    let mut coverages: Vec<Coverage> = Vec::new();
    let mut reports = Vec::new();
    let mut diagnostics = Vec::new();
    let mesh_bounds = Aabb::of_positions(&source.positions);
    let mut surface: Option<Surface> = None;

    for (index, op) in edit.ops.iter().enumerate() {
        let MeshEditOp::Paint {
            map,
            size,
            color,
            strength,
            region,
        } = op;
        let pointer = format!("/ops/{index}");
        let position = match states.iter().position(|state| state.map == *map) {
            Some(position) => {
                if size.is_some() {
                    diagnostics.push(size_on_existing(&pointer, *map, "an earlier op created it"));
                    continue;
                }
                position
            }
            None => match open_map(source, *map, *size, source_dir, &pointer) {
                Ok(state) => {
                    states.push(state);
                    states.len() - 1
                }
                Err(mut errors) => {
                    diagnostics.append(&mut errors);
                    continue;
                }
            },
        };
        let (width, height) = (
            states[position].data.width(),
            states[position].data.height(),
        );
        let coverage = match coverage_for(&mut coverages, source, uvs, width, height) {
            Ok(coverage) => coverage,
            Err(error) => {
                diagnostics.push(error);
                continue;
            }
        };
        let fill_weights = match &region.shape {
            MeshEditRegionShape::Fill {
                at,
                tolerance,
                max_radius,
                map: fill_map,
                color_feather,
            } => {
                let colours = key_map(
                    fill_map.unwrap_or(MeshEditMap::BaseColor),
                    &states,
                    source,
                    source_dir,
                    "`fill`",
                    format!("{pointer}/region/fill/map"),
                )
                .map(|(rgba, colour_width, colour_height)| {
                    MapSampler::new(rgba, colour_width, colour_height)
                });
                let surface = surface
                    .get_or_insert_with(|| fill::surface(&source.positions, uvs, &source.indices));
                let spec = FillSpec {
                    at: *at,
                    tolerance: *tolerance,
                    max_radius: *max_radius,
                    feather: region.feather,
                    color_feather: *color_feather,
                };
                match colours.and_then(|colours| {
                    flood_region(
                        index,
                        &spec,
                        source,
                        mesh_bounds,
                        coverage,
                        surface,
                        &colours,
                    )
                }) {
                    Ok(weights) => Some(weights),
                    Err(errors) => {
                        diagnostics.extend(errors);
                        continue;
                    }
                }
            }
            _ => None,
        };
        let key_filter = match &region.key {
            Some(key) => match key_map(
                key.map,
                &states,
                source,
                source_dir,
                "`key` filter",
                format!("{pointer}/region/key/map"),
            ) {
                Ok((rgba, key_width, key_height)) => Some(paint::key_filter(
                    &MapSampler::new(rgba, key_width, key_height),
                    key,
                    width,
                    height,
                )),
                Err(errors) => {
                    diagnostics.extend(errors);
                    continue;
                }
            },
            None => None,
        };
        let outcome = paint::paint(
            &mut states[position].data,
            coverage,
            *color,
            strength.unwrap_or(1.0),
            region,
            fill_weights.as_ref(),
            key_filter.as_ref(),
        );
        if outcome.texels == 0 {
            diagnostics.push(match (&region.key, outcome.nearest_rejected) {
                (Some(key), Some(nearest)) if outcome.region_texels > 0 => {
                    key_matched_nothing(index, &pointer, key, outcome.region_texels, nearest)
                }
                _ if outcome.reached_texels > 0 => {
                    zero_weight(index, &pointer, outcome.reached_texels)
                }
                _ => painted_nothing(index, &pointer, region, mesh_bounds),
            });
        }
        reports.push(OpReport {
            index,
            op: op.name().to_string(),
            map: map.name().to_string(),
            texels: outcome.texels,
            share: outcome.texels as f64 / (f64::from(width) * f64::from(height)),
            bounds: outcome.bounds,
            triangles: outcome.triangles,
        });
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    for state in &mut states {
        let (width, height) = (state.data.width(), state.data.height());
        let coverage = coverage_for(&mut coverages, source, uvs, width, height)
            .map_err(|error| vec![error])?;
        paint::dilate(&mut state.data, coverage, DILATE_PAD);
    }
    Ok((states, reports))
}

/// Runs one `fill` op's flood: the seed must lie on the surface and within its radius of a texel.
fn flood_region(
    index: usize,
    spec: &FillSpec,
    source: &MeshDocument,
    mesh_bounds: Option<Aabb>,
    coverage: &Coverage,
    surface: &Surface,
    colours: &MapSampler,
) -> Result<FillWeights, Vec<Diagnostic>> {
    let FillSpec { at, max_radius, .. } = *spec;
    let pointer = format!("/ops/{index}");
    let diagonal = mesh_bounds.map_or(0.0, |bounds| {
        let extent = [0, 1, 2].map(|axis| bounds.max[axis] - bounds.min[axis]);
        sqrt(extent[0] * extent[0] + extent[1] * extent[1] + extent[2] * extent[2])
    });
    let allowance = fill::SEED_EPSILON_FRACTION * diagonal;
    let (distance, nearest) = fill::nearest_surface(&source.positions, &source.indices, at);
    if distance > allowance {
        return Err(vec![
            Diagnostic::new(
                code::MESH_EDIT_MATCH_FAILED,
                format!(
                    "op {index}'s `fill` seed {at:?} is {distance} from the surface, farther than \
                     the {allowance} a seed may sit off it; the nearest surface point is \
                     {nearest:?}"
                ),
            )
            .with_path(format!("{pointer}/region/fill/at"))
            .with_help(
                "`ludion mesh pick` names a point that is certainly on the surface, in the \
                 coordinates a region is written in; use it as `at`",
            ),
        ]);
    }
    match fill::flood(coverage, surface, spec, colours) {
        Ok(weights) => Ok(weights),
        Err(FillRefusal::SeedOutsideRadius { distance }) => Err(vec![
            Diagnostic::new(
                code::MESH_EDIT_MATCH_FAILED,
                format!(
                    "op {index}'s `fill` seed {at:?} is {distance} from the nearest texel of the \
                     surface, not inside its `max_radius` of {max_radius}"
                ),
            )
            .with_path(format!("{pointer}/region/fill/max_radius"))
            .with_help("raise `max_radius` above that distance, or move `at` nearer a texel"),
        ]),
        Err(FillRefusal::NoTexels) => Err(vec![
            Diagnostic::new(
                code::MESH_EDIT_OP_PAINTED_NOTHING,
                format!("op {index} painted nothing: no triangle's UVs cover a texel of this map"),
            )
            .with_path(pointer)
            .with_help("a `fill` walks texels the mesh's UV set covers; check the mesh's `uvs`"),
        ]),
        Err(FillRefusal::TooManySeamCuts { work }) => Err(vec![
            Diagnostic::new(
                code::MESH_EDIT_INAPPLICABLE,
                format!(
                    "op {index}'s `fill` would bridge the mesh's UV seams over {work} texel \
                     visits, past the limit of {}",
                    raster::MAX_RASTER_WORK
                ),
            )
            .with_path(format!("{pointer}/region/fill"))
            .with_help("re-unwrap the mesh with fewer or shorter seams, or lower the map `size`"),
        ]),
    }
}

/// The map an op targets: the source's own when it has one, else a new opaque-black map of `size`,
/// else one the size of the source's base-colour map.
fn open_map(
    source: &MeshDocument,
    map: MeshEditMap,
    size: Option<u32>,
    source_dir: &Path,
    pointer: &str,
) -> Result<PaintedMap, Vec<Diagnostic>> {
    if let Some(reference) = reference(&source.textures, map) {
        if size.is_some() {
            return Err(vec![size_on_existing(
                pointer,
                map,
                "the source mesh already has one",
            )]);
        }
        let image = load_map(source_dir, &reference.path, map)?;
        return Ok(PaintedMap {
            map,
            created: false,
            data: WorkingMap::from_image(&image),
        });
    }
    let (width, height, from_base) = match size {
        Some(size) => (size, size, false),
        None => {
            let base = reference(&source.textures, MeshEditMap::BaseColor).ok_or_else(|| {
                vec![
                    inapplicable(format!(
                        "the source mesh has no `{}` map and no base-colour map to take the \
                         dimensions from",
                        map.name()
                    ))
                    .with_path(format!("{pointer}/map"))
                    .with_help(
                        "add `size:` to the first op that targets this map, e.g. `size: 1024`",
                    ),
                ]
            })?;
            let image = load_map(source_dir, &base.path, MeshEditMap::BaseColor)?;
            (image.width, image.height, true)
        }
    };
    if width == 0 || height == 0 || width > raster::MAX_EXTENT || height > raster::MAX_EXTENT {
        if from_base {
            return Err(vec![
                Diagnostic::new(
                    code::MESH_EDIT_DATA,
                    format!(
                        "the base-colour map is {width}×{height}, outside the 1..={} texel range \
                         this engine derives; add `size:` to create a smaller map",
                        raster::MAX_EXTENT
                    ),
                )
                .with_path(format!("{pointer}/map")),
            ]);
        }
        return Err(vec![
            Diagnostic::new(
                code::MESH_EDIT_DATA,
                format!(
                    "`size` is {width}, outside the 1..={} texel range this engine derives",
                    raster::MAX_EXTENT
                ),
            )
            .with_path(format!("{pointer}/size")),
        ]);
    }
    Ok(PaintedMap {
        map,
        created: true,
        data: WorkingMap::opaque_black(width, height),
    })
}

fn load_map(
    dir: &Path,
    path: &str,
    map: MeshEditMap,
) -> Result<ludion_assets::Image, Vec<Diagnostic>> {
    ludion_assets::load_png(
        ludion_assets::AssetRoot::local_document_unconfined(dir),
        path,
    )
    .map_err(|mut error| {
        error.message = format!("the source's `{}` map: {}", map.name(), error.message);
        vec![error]
    })
}

/// The bytes a colour-reading region member compares against, with their extent: the working map of
/// an earlier op that wrote the slot, else the source's own map — the same order an op reads its
/// target in. `reader` names the member in the message and `pointer` is the map field it read.
fn key_map(
    map: MeshEditMap,
    states: &[PaintedMap],
    source: &MeshDocument,
    source_dir: &Path,
    reader: &str,
    pointer: String,
) -> Result<(Vec<u8>, u32, u32), Vec<Diagnostic>> {
    if let Some(state) = states.iter().find(|state| state.map == map) {
        return Ok((
            state.data.to_rgba8(),
            state.data.width(),
            state.data.height(),
        ));
    }
    let Some(reference) = reference(&source.textures, map) else {
        return Err(vec![
            inapplicable(format!(
                "the {reader} reads the `{}` map, but the source mesh has none and no \
                 earlier op wrote one",
                map.name()
            ))
            .with_path(pointer)
            .with_help("read a map the source mesh has, or paint the map in an earlier op"),
        ]);
    };
    let image = load_map(source_dir, &reference.path, map)?;
    Ok((image.rgba, image.width, image.height))
}

/// The coverage for one map extent, rasterised once and reused by every op of that size.
fn coverage_for<'a>(
    coverages: &'a mut Vec<Coverage>,
    source: &MeshDocument,
    uvs: &[f64],
    width: u32,
    height: u32,
) -> Result<&'a Coverage, Diagnostic> {
    if let Some(position) = coverages
        .iter()
        .position(|coverage| coverage.width() == width && coverage.height() == height)
    {
        return Ok(&coverages[position]);
    }
    let coverage = raster::rasterize(&source.positions, uvs, &source.indices, width, height)?;
    coverages.push(coverage);
    Ok(coverages.last().expect("just pushed"))
}

/// Re-parses the emitted document and asserts it re-emits byte-for-byte, so a derive never commits
/// a file the drift gate would then reject forever.
fn self_check(document: &str) -> Result<(), Vec<Diagnostic>> {
    let reparsed = ludion_scene::check_mesh(document)?;
    if ludion_scene::emit_mesh(&reparsed) != document {
        return Err(vec![
            Diagnostic::new(
                code::MESH_EDIT_STRUCTURE,
                "the derived mesh document did not survive a parse→emit round-trip (an internal \
                 derive invariant failed); nothing was written",
            )
            .with_help("this is a bug in the mesh-edit derive — please report it"),
        ]);
    }
    Ok(())
}

// --- Diagnostics --------------------------------------------------------------------------------

fn painted_nothing(
    index: usize,
    pointer: &str,
    region: &MeshEditRegion,
    mesh: Option<Aabb>,
) -> Diagnostic {
    let region_box = region::bounds(region);
    let mesh_box = match mesh {
        Some(mesh) => box_text(&mesh),
        None => "an empty mesh".to_string(),
    };
    Diagnostic::new(
        code::MESH_EDIT_OP_PAINTED_NOTHING,
        format!(
            "op {index} painted nothing: its `{}` region spans {} while the mesh spans {mesh_box}",
            region.shape.name(),
            box_text(&region_box)
        ),
    )
    .with_path(pointer.to_string())
    .with_help(
        "move or grow the region until the two boxes overlap; `ludion mesh pick` names a point \
         that is certainly on the surface, in the coordinates a region is written in",
    )
}

fn zero_weight(index: usize, pointer: &str, reached_texels: usize) -> Diagnostic {
    Diagnostic::new(
        code::MESH_EDIT_OP_PAINTED_NOTHING,
        format!(
            "op {index} painted nothing: its region reaches {reached_texels} texels, but every \
             weight is zero"
        ),
    )
    .with_path(pointer.to_string())
    .with_help("raise `strength` above 0")
}

fn key_matched_nothing(
    index: usize,
    pointer: &str,
    key: &MeshEditKey,
    region_texels: usize,
    (distance, sample): (f64, [u8; 3]),
) -> Diagnostic {
    let [r, g, b] = sample.map(|byte| f64::from(byte) / 255.0);
    let reach = match key.color_feather {
        Some(color_feather) if color_feather > 0.0 => format!(
            ", with a `color_feather` of {color_feather} fading it out by {}",
            key.tolerance + color_feather
        ),
        _ => String::new(),
    };
    Diagnostic::new(
        code::MESH_EDIT_MATCH_FAILED,
        format!(
            "op {index}'s `key` matched nothing: its region reaches {region_texels} texels, but \
             the nearest is {distance} from the key colour (a texel of [{r}, {g}, {b}] in `{}`) \
             and the `tolerance` is {}{reach}",
            key.map.name(),
            key.tolerance
        ),
    )
    .with_path(format!("{pointer}/region/key"))
    .with_help(
        "raise `tolerance` (or `tolerance` + `color_feather`) past that distance, or re-sample \
         the key `color` from the map at a picked point",
    )
}

fn box_text(bounds: &Aabb) -> String {
    format!(
        "[{}, {}, {}]..[{}, {}, {}]",
        bounds.min[0], bounds.min[1], bounds.min[2], bounds.max[0], bounds.max[1], bounds.max[2]
    )
}

fn size_on_existing(pointer: &str, map: MeshEditMap, why: &str) -> Diagnostic {
    Diagnostic::new(
        code::MESH_EDIT_DATA,
        format!("`size` sizes the `{}` map, but {why}", map.name()),
    )
    .with_path(format!("{pointer}/size"))
    .with_help("`size` is legal only on the first op targeting a map that does not exist yet")
}

fn read_text(path: &Path) -> Result<String, Vec<Diagnostic>> {
    std::fs::read_to_string(path).map_err(|error| {
        vec![
            read_io(format!("could not read `{}`: {error}", slash(path)))
                .with_help("the path is resolved relative to the current directory"),
        ]
    })
}

/// Reads the source mesh document, re-messaging its own diagnostics with the file that raised them
/// — a pointer like `/positions` otherwise reads as a pointer into the edit document.
fn read_source(path: &Path) -> Result<MeshDocument, Vec<Diagnostic>> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        vec![
            Diagnostic::new(
                code::MESH_EDIT_DATA,
                format!(
                    "could not read the `source` mesh `{}`: {error}",
                    slash(path)
                ),
            )
            .with_path("/source")
            .with_help("`source` is resolved relative to the edit document's own directory"),
        ]
    })?;
    ludion_scene::check_mesh(&text).map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|mut diagnostic| {
                diagnostic.message = format!("{}: {}", slash(path), diagnostic.message);
                diagnostic
            })
            .collect()
    })
}

/// Removes a stale file; one already gone (another process swept it) is the goal reached.
fn remove_stale(path: &Path) -> Result<(), Vec<Diagnostic>> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(vec![write_io(format!(
            "could not remove `{}`: {error}",
            slash(path)
        ))]),
        _ => Ok(()),
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Vec<Diagnostic>> {
    write_atomic(path, bytes).map_err(|error| {
        vec![write_io(format!(
            "could not write `{}`: {error}",
            slash(path)
        ))]
    })
}

fn read_io(message: String) -> Diagnostic {
    Diagnostic::new(code::SCENE_IO, message)
}

fn write_io(message: String) -> Diagnostic {
    Diagnostic::new(code::OUTPUT_IO, message)
}

fn inapplicable(message: String) -> Diagnostic {
    Diagnostic::new(code::MESH_EDIT_INAPPLICABLE, message)
}

fn drift(path: &Path, reason: &str) -> Diagnostic {
    Diagnostic::new(
        code::MESH_EDIT_STRUCTURE,
        format!("DRIFT {}: {reason}", slash(path)),
    )
    .with_help("run `ludion mesh apply` (without --check) to regenerate the derived tree")
}

// --- The five slots -----------------------------------------------------------------------------

fn reference(textures: &MeshTextures, map: MeshEditMap) -> Option<&TextureRef> {
    match map {
        MeshEditMap::BaseColor => textures.base_color.as_ref(),
        MeshEditMap::MetallicRoughness => textures.metallic_roughness.as_ref(),
        MeshEditMap::Normal => textures.normal.as_ref(),
        MeshEditMap::Occlusion => textures.occlusion.as_ref(),
        MeshEditMap::Emissive => textures.emissive.as_ref(),
    }
}

fn reference_mut(textures: &mut MeshTextures, map: MeshEditMap) -> Option<&mut TextureRef> {
    match map {
        MeshEditMap::BaseColor => textures.base_color.as_mut(),
        MeshEditMap::MetallicRoughness => textures.metallic_roughness.as_mut(),
        MeshEditMap::Normal => textures.normal.as_mut(),
        MeshEditMap::Occlusion => textures.occlusion.as_mut(),
        MeshEditMap::Emissive => textures.emissive.as_mut(),
    }
}

fn set_reference(textures: &mut MeshTextures, map: MeshEditMap, reference: TextureRef) {
    let slot = match map {
        MeshEditMap::BaseColor => &mut textures.base_color,
        MeshEditMap::MetallicRoughness => &mut textures.metallic_roughness,
        MeshEditMap::Normal => &mut textures.normal,
        MeshEditMap::Occlusion => &mut textures.occlusion,
        MeshEditMap::Emissive => &mut textures.emissive,
    };
    *slot = Some(reference);
}

// --- Lexical paths ------------------------------------------------------------------------------

/// A document-relative path split into components, dropping empty and `.` segments. Both separators
/// are read on every platform: the text is the source of truth, and a document written on one
/// machine is read on the other.
fn components(path: &str) -> Vec<String> {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_string)
        .collect()
}

/// A validated relative path (no root, no drive `:`; `check_mesh_edit` and `sibling_refusal` refuse
/// both) folded textually by core's `normalize`, as components. On such inputs core's spelling
/// agrees with a plain fold: a `..` that cannot fold is kept, so the result stays relative.
fn folded(text: &str) -> Vec<String> {
    components(&normalize(text))
}

/// Everything but the last component.
fn parent(parts: &[String]) -> Vec<String> {
    parts[..parts.len().saturating_sub(1)].to_vec()
}

/// `to` expressed relative to the directory `from`, forward slashes — the spelling every path in a
/// derived document takes. `from` never contains `..` (it is a directory below the edit document),
/// so climbing out of it is a matter of counting its components.
fn relative(from: &[String], to: &[String]) -> String {
    let mut shared = 0;
    while shared < from.len()
        && shared < to.len()
        && from[shared] == to[shared]
        && from[shared] != ".."
    {
        shared += 1;
    }
    let mut parts: Vec<&str> = vec![".."; from.len() - shared];
    parts.extend(to[shared..].iter().map(String::as_str));
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn join(base: &Path, parts: &[String]) -> PathBuf {
    let mut path = base.to_path_buf();
    for part in parts {
        path.push(part);
    }
    path
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A mesh-edit document's own stem: its file name with `.yaml`/`.yml` and then `.mesh-edit`
/// dropped (`idol-lit.mesh-edit.yaml` → `idol-lit`).
fn document_stem(name: &str) -> String {
    let stem = name
        .strip_suffix(".yaml")
        .or_else(|| name.strip_suffix(".yml"))
        .unwrap_or(name);
    stem.strip_suffix(".mesh-edit").unwrap_or(stem).to_string()
}

/// Whether a path inside the derived directory is one the derive itself names.
fn is_derive_owned(relative: &[String]) -> bool {
    match relative {
        [name] => name == DERIVED_DOCUMENT || is_temp_name_for(name, DERIVED_DOCUMENT),
        [dir, name] => dir == DERIVED_TEXTURE_DIR && (is_hash_png(name) || is_hash_png_temp(name)),
        _ => false,
    }
}

/// A leftover temp of a content-hash map: `.<16 hex>.png.<pid>.<n>.tmp`.
fn is_hash_png_temp(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|rest| rest.get(..rest.find(".png")? + ".png".len()))
        .is_some_and(|owned| is_hash_png(owned) && is_temp_name_for(name, owned))
}

fn is_hash_png(name: &str) -> bool {
    name.strip_suffix(".png")
        .is_some_and(|stem| stem.len() == 16 && stem.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Every file under `dir`, as (components relative to `dir`, full path), sorted so a diagnostic
/// names them in the same order on every machine. A directory that does not exist yet has none.
fn list_existing(dir: &Path) -> Result<Vec<(Vec<String>, PathBuf)>, Vec<Diagnostic>> {
    let mut found = Vec::new();
    walk(dir, &mut Vec::new(), &mut found)?;
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

fn walk(
    dir: &Path,
    prefix: &mut Vec<String>,
    found: &mut Vec<(Vec<String>, PathBuf)>,
) -> Result<(), Vec<Diagnostic>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(vec![read_io(format!(
                "could not list `{}`: {error}",
                slash(dir)
            ))]);
        }
    };
    for entry in entries {
        let entry = entry
            .map_err(|error| vec![read_io(format!("could not list `{}`: {error}", slash(dir)))])?;
        let path = entry.path();
        let name = file_name(&path);
        prefix.push(name);
        if path.is_dir() {
            walk(&path, prefix, found)?;
        } else {
            found.push((prefix.clone(), path));
        }
        prefix.pop();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(path: &str) -> Vec<String> {
        folded(path)
    }

    #[test]
    fn a_relative_path_is_folded_textually() {
        let table: [(&str, &[&str]); 10] = [
            ("a/./b/../c", &["a", "c"]),
            ("../textures/x.png", &["..", "textures", "x.png"]),
            ("a\\b", &["a", "b"]),
            ("", &[]),
            ("./", &[]),
            ("a/..", &[]),
            ("a/../../x", &["..", "x"]),
            ("../../a/b/..", &["..", "..", "a"]),
            ("a//b", &["a", "b"]),
            ("meshes\\..\\textures/x.png", &["textures", "x.png"]),
        ];
        for (path, expected) in table {
            assert_eq!(parts(path), expected, "{path:?}");
        }
    }

    /// The two paths a derived document carries: back to the edit document, and out to a pristine
    /// map that lives a directory above the source.
    #[test]
    fn the_derived_document_points_back_and_out() {
        let out = parts("idol-lit");
        assert_eq!(
            relative(&out, &parts("idol-lit.mesh-edit.yaml")),
            "../idol-lit.mesh-edit.yaml"
        );
        assert_eq!(
            relative(&out, &parts("../textures/dc74.png")),
            "../../textures/dc74.png"
        );
        assert_eq!(
            relative(&out, &parts("idol-lit/textures/a.png")),
            "textures/a.png"
        );
        assert_eq!(relative(&parts("a/b"), &parts("a/c")), "../c");
        assert_eq!(relative(&out, &out), ".");
    }

    #[test]
    fn a_stem_drops_both_suffixes() {
        assert_eq!(document_stem("idol-lit.mesh-edit.yaml"), "idol-lit");
        assert_eq!(document_stem("idol-lit.mesh-edit.yml"), "idol-lit");
        assert_eq!(document_stem("plain.yaml"), "plain");
    }

    #[test]
    fn a_stale_file_that_already_vanished_is_not_an_error() {
        let gone = std::env::temp_dir().join(format!("ludion-me-gone-{}.png", std::process::id()));
        assert!(remove_stale(&gone).is_ok());
    }

    #[test]
    fn only_the_derives_own_names_are_owned() {
        assert!(is_derive_owned(&["mesh.mesh.yaml".to_string()]));
        assert!(is_derive_owned(&[
            "textures".to_string(),
            "0123456789abcdef.png".to_string()
        ]));
        assert!(is_derive_owned(&[".mesh.mesh.yaml.123.4.tmp".to_string()]));
        assert!(is_derive_owned(&[
            "textures".to_string(),
            ".0123456789abcdef.png.123.4.tmp".to_string()
        ]));
        assert!(!is_derive_owned(&[".mesh.mesh.yaml.123".to_string()]));
        assert!(!is_derive_owned(&[".notes.md.123.4.tmp".to_string()]));
        assert!(!is_derive_owned(&[
            "textures".to_string(),
            ".hand-painted.png.123.4.tmp".to_string()
        ]));
        assert!(!is_derive_owned(&["notes.md".to_string()]));
        assert!(!is_derive_owned(&[
            "textures".to_string(),
            "hand-painted.png".to_string()
        ]));
        assert!(!is_derive_owned(&[
            "textures".to_string(),
            "0123456789abcdef.png".to_string(),
            "deep.png".to_string()
        ]));
    }
}
