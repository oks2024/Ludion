//! The cross-file check of a `kind: paired` document: each role's assembly loads, each `clip` and
//! contact part exists, every event lies inside its clips, and every `contacts:` gap is measured on
//! the posed parts (`Plan/paired-animations.md` decisions 18, 20, 21).
//!
//! The file-alone rules (`L1752`, `L1753`) are [`ludion_scene::check_paired`]; this module adds the
//! role mismatches (`L1754`) and the contact-gap warnings (`L1755`). Nothing plays a paired action
//! here: the report is an authoring check and never reaches a hash.
//!
//! # Example
//! ```no_run
//! use ludion_assets::AssetRoot;
//! let dir = std::path::Path::new("examples/gallery/voxels");
//! let root = AssetRoot::content(dir);
//! let report = ludion_engine::check_paired_files(root, "choke.paired.yaml").expect("valid");
//! for contact in &report.contacts {
//!     println!("{} -> {}: {:.2} cells", contact.a, contact.b, contact.gap_cells);
//! }
//! ```

use ludion_assets::AssetRoot;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::{
    self,
    glam::{Mat3, Mat4, Vec3},
};
use ludion_core::suggest;
use ludion_scene::{AssembledGrid, PairedDocument};

use crate::assembly_mesh::AssemblyData;
use crate::extract::part_placements;

/// One role as the check resolved it.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedRoleReport {
    /// The role name.
    pub name: String,
    /// The assembly path as the document spells it.
    pub assembly: String,
    /// The clip the role plays; `None` for an `align_only` role.
    pub clip: Option<String>,
    /// The clip's duration in seconds; `None` without a clip.
    pub duration: Option<f64>,
    /// The aligned position in metres, in the reference entity's frame (zero for the reference).
    pub offset: [f64; 3],
    /// The aligned yaw in degrees (zero for the reference).
    pub yaw_deg: f64,
    /// The planar (XZ) distance from the reference in metres.
    pub distance: f64,
}

/// One contact's measured gap at its event.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedContactReport {
    /// The contact's index in the document.
    pub index: usize,
    /// The first side as `role/part`.
    pub a: String,
    /// The second side as `role/part`.
    pub b: String,
    /// The event the gap is measured at.
    pub at: String,
    /// That event's time in seconds.
    pub t: f64,
    /// The gap in metres; 0 means touching or overlapping.
    pub gap_m: f64,
    /// The gap in the first role's cells (`gap_m / scale`).
    pub gap_cells: f64,
    /// The contact's accepted gap, in the first role's cells.
    pub max_gap: f64,
}

/// What [`check_paired_files`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedReport {
    /// The parsed, validated document.
    pub doc: PairedDocument,
    /// Each role, sorted by name.
    pub roles: Vec<PairedRoleReport>,
    /// Each contact, in document order.
    pub contacts: Vec<PairedContactReport>,
    /// `L1755` warnings, one per contact over its `max_gap`.
    pub warnings: Vec<Diagnostic>,
}

/// A role's assembly with what the gap measurement needs.
struct Loaded {
    data: AssemblyData,
    grid: AssembledGrid,
    rest: Vec<(Mat4, Mat3)>,
}

fn mismatch(pointer: String, message: String, help: String) -> Diagnostic {
    Diagnostic::new(code::PAIRED_ROLE_MISMATCH, message)
        .with_path(pointer)
        .with_help(help)
}

/// Reads the paired document at `root`/`rel`, checks it alone, then against each role's assembly
/// (resolved relative to the document's directory), and measures every contact.
///
/// # Errors
/// `L1752`/`L1753` from the file alone; `L1754` for an assembly that does not load (followed by the
/// loader's own diagnostics), an unknown clip, an event past a clip's duration, or an unknown
/// contact part, all collected.
pub fn check_paired_files(root: AssetRoot<'_>, rel: &str) -> Result<PairedReport, Vec<Diagnostic>> {
    let bytes = ludion_assets::load_bytes(root, rel).map_err(|diagnostic| vec![diagnostic])?;
    let source = std::str::from_utf8(&bytes.bytes).map_err(|_| {
        vec![
            Diagnostic::new(
                code::PAIRED_DOCUMENT_STRUCTURE,
                format!("paired asset `{rel}` is not valid UTF-8 text"),
            )
            .with_help("a paired document is a strict-YAML text file (`*.paired.yaml`)"),
        ]
    })?;
    let doc = ludion_scene::check_paired(source)?;

    let mut roles: Vec<&ludion_scene::PairedRole> = doc.roles.iter().collect();
    roles.sort_by(|a, b| a.name.cmp(&b.name));

    let mut errors = Vec::new();
    let mut loaded: Vec<(&str, Loaded)> = Vec::new();
    for role in &roles {
        let file = ludion_core::paths::sibling(rel, &role.assembly);
        match crate::assembly_mesh::load_meshed_documents(root, &file) {
            Ok((data, assembly, parts)) => {
                let grid = ludion_scene::assembly_assembled_grid(&assembly, &parts);
                let rest = part_placements(&data, None);
                loaded.push((role.name.as_str(), Loaded { data, grid, rest }));
            }
            Err(mut found) => {
                errors.push(mismatch(
                    format!("/roles/{}/assembly", role.name),
                    format!("role `{}`: assembly `{file}` does not load", role.name),
                    "fix the assembly named here; its own diagnostics follow".to_string(),
                ));
                errors.append(&mut found);
            }
        }
    }

    let load_of = |name: &str| loaded.iter().find(|(n, _)| *n == name).map(|(_, l)| l);
    let mut durations: Vec<(&str, Option<f64>)> = Vec::new();
    for role in &roles {
        let Some(load) = load_of(&role.name) else {
            continue;
        };
        let duration = match &role.clip {
            Some(clip) => match load.data.clip(clip) {
                Some(document) => Some(document.duration),
                None => {
                    let help = match suggest::nearest(
                        clip,
                        load.data.clips.iter().map(|binding| binding.name.as_str()),
                    ) {
                        Some(candidate) => format!("nearest clip is `{candidate}`"),
                        None => "the assembly's `clips:` names no clip close to it".to_string(),
                    };
                    errors.push(mismatch(
                        format!("/roles/{}/clip", role.name),
                        format!(
                            "role `{}`: assembly `{}` has no clip `{clip}`",
                            role.name, role.assembly
                        ),
                        help,
                    ));
                    None
                }
            },
            None => None,
        };
        durations.push((role.name.as_str(), duration));
    }

    for (index, event) in doc.events.iter().enumerate() {
        for role in &roles {
            let (Some(clip), Some(Some(duration))) = (
                &role.clip,
                durations
                    .iter()
                    .find(|(n, _)| *n == role.name)
                    .map(|(_, d)| *d),
            ) else {
                continue;
            };
            if event.t > duration {
                errors.push(mismatch(
                    format!("/events/{index}/t"),
                    format!(
                        "event `{}` at {} s is past role `{}`'s clip `{clip}`, which lasts {duration} s",
                        event.name, event.t, role.name
                    ),
                    "move the event inside the clip, or lengthen the clip".to_string(),
                ));
            }
        }
    }

    for (index, contact) in doc.contacts.iter().enumerate() {
        for (key, (role, part)) in [("a", &contact.a), ("b", &contact.b)] {
            let Some(load) = load_of(role) else {
                continue;
            };
            if !load
                .data
                .parts
                .iter()
                .any(|candidate| candidate.name == *part)
            {
                let help = match suggest::nearest(
                    part,
                    load.data
                        .parts
                        .iter()
                        .map(|candidate| candidate.name.as_str()),
                ) {
                    Some(candidate) => format!("nearest part is `{candidate}`"),
                    None => "the assembly has no part with a similar name".to_string(),
                };
                errors.push(mismatch(
                    format!("/contacts/{index}/{key}"),
                    format!("role `{role}`'s assembly has no part `{part}`"),
                    help,
                ));
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let placements = Placements::new(&doc);
    let mut role_reports = Vec::with_capacity(roles.len());
    for role in &roles {
        let (offset, yaw_deg) = placements.of(&role.name);
        let duration = durations
            .iter()
            .find(|(n, _)| *n == role.name)
            .and_then(|(_, d)| *d);
        role_reports.push(PairedRoleReport {
            name: role.name.clone(),
            assembly: role.assembly.clone(),
            clip: role.clip.clone(),
            duration,
            offset,
            yaw_deg,
            distance: math::sqrt((offset[0] * offset[0] + offset[2] * offset[2]) as f32) as f64,
        });
    }

    let mut contacts = Vec::with_capacity(doc.contacts.len());
    let mut warnings = Vec::new();
    for (index, contact) in doc.contacts.iter().enumerate() {
        let t = doc
            .events
            .iter()
            .find(|event| event.name == contact.at)
            .map_or(0.0, |event| event.t);
        let side = |(role, part): &(String, String)| {
            let load = load_of(role).expect("checked above");
            let spec = doc
                .roles
                .iter()
                .find(|r| r.name == *role)
                .expect("validated");
            let clip = spec.clip.as_deref().and_then(|name| load.data.clip(name));
            let (offset, yaw_deg) = placements.of(role);
            posed_cells(load, part, clip, t, offset, yaw_deg, spec.scale)
        };
        let (cells_a, size_a) = side(&contact.a);
        let (cells_b, size_b) = side(&contact.b);
        let mut nearest = f32::INFINITY;
        for a in &cells_a {
            for b in &cells_b {
                nearest = nearest.min(a.distance_squared(*b));
            }
        }
        let centres = if nearest.is_finite() {
            math::sqrt(nearest) as f64
        } else {
            0.0
        };
        let gap_m = (centres - (size_a + size_b) * 0.5).max(0.0);
        let gap_cells = gap_m / size_a;
        let name = |(role, part): &(String, String)| format!("{role}/{part}");
        let report = PairedContactReport {
            index,
            a: name(&contact.a),
            b: name(&contact.b),
            at: contact.at.clone(),
            t,
            gap_m,
            gap_cells,
            max_gap: contact.max_gap,
        };
        if gap_cells > contact.max_gap {
            warnings.push(
                Diagnostic::new(
                    code::PAIRED_CONTACT_GAP,
                    format!(
                        "contact {} -> {} at `{}` (t {t}) has a gap of {gap_cells:.3} cells \
                         ({gap_m:.4} m), over its `max_gap` of {}",
                        report.a, report.b, contact.at, contact.max_gap
                    ),
                )
                .with_path(format!("/contacts/{index}"))
                .with_help(
                    "move the target's `align.offsets` entry closer, change the event time, or \
                     raise `max_gap`; check each role's `scale` too",
                ),
            );
        }
        contacts.push(report);
    }

    Ok(PairedReport {
        doc,
        roles: role_reports,
        contacts,
        warnings,
    })
}

/// Each role's aligned offset and yaw; the reference sits at the identity.
struct Placements(Vec<(String, [f64; 3], f64)>);

impl Placements {
    fn new(doc: &PairedDocument) -> Self {
        let mut entries: Vec<(String, [f64; 3], f64)> = doc
            .align
            .offsets
            .iter()
            .map(|offset| (offset.role.clone(), offset.translation, offset.yaw_deg))
            .collect();
        entries.push((doc.align.reference.clone(), [0.0; 3], 0.0));
        Placements(entries)
    }

    fn of(&self, role: &str) -> ([f64; 3], f64) {
        self.0
            .iter()
            .find(|(name, ..)| name == role)
            .map_or(([0.0; 3], 0.0), |(_, offset, yaw)| (*offset, *yaw))
    }
}

/// The centres, in the reference frame in metres, of every cell of `part`, carried through its
/// posed·rest⁻¹ delta at `t` (rest without a clip) and the role entity's `T · R_y · S` placement; with
/// the role's cell size in metres.
fn posed_cells(
    load: &Loaded,
    part: &str,
    clip: Option<&ludion_scene::AnimationDocument>,
    t: f64,
    offset: [f64; 3],
    yaw_deg: f64,
    scale: f64,
) -> (Vec<Vec3>, f64) {
    let index = load
        .data
        .parts
        .iter()
        .position(|candidate| candidate.name == part)
        .expect("checked above");
    let posed = match clip {
        Some(clip) => part_placements(&load.data, Some((clip, t))),
        None => load.rest.clone(),
    };
    #[allow(clippy::disallowed_methods)] // authoring report, never reaches a hash
    let delta = posed[index].0 * load.rest[index].0.inverse();

    let (sin, cos) = math::sin_cos((yaw_deg as f32).to_radians());
    let scale_f = scale as f32;
    let origin = Vec3::new(offset[0] as f32, offset[1] as f32, offset[2] as f32);
    let [sx, _, sz] = load.grid.voxel.size;
    let (hx, hz) = (sx as f32 * 0.5, sz as f32 * 0.5);
    let [_, sy, _] = load.grid.voxel.size;

    let mut cells = Vec::new();
    for y in 0..sy {
        for z in 0..sz {
            for x in 0..sx {
                if !load.grid.owners_at(x, y, z).contains(&index) {
                    continue;
                }
                let centre = Vec3::new(x as f32 - hx + 0.5, y as f32 + 0.5, z as f32 - hz + 0.5);
                let local = delta.transform_point3(centre) * scale_f;
                let rotated = Vec3::new(
                    cos * local.x + sin * local.z,
                    local.y,
                    -sin * local.x + cos * local.z,
                );
                cells.push(origin + rotated);
            }
        }
    }
    (cells, scale)
}

/// The first rest-pose cell of `role/part`: its centre in the assembly frame and in the reference
/// frame as `posed_cells` places it, for the paired sheet's agreement test.
#[cfg(test)]
pub(crate) fn first_rest_cell(
    root: AssetRoot<'_>,
    rel: &str,
    role: &str,
    part: &str,
) -> (Vec3, Vec3) {
    let report = check_paired_files(root, rel).expect("valid document");
    let spec = report
        .doc
        .roles
        .iter()
        .find(|candidate| candidate.name == role)
        .expect("role");
    let file = ludion_core::paths::sibling(rel, &spec.assembly);
    let (data, _, parts) = crate::assembly_mesh::load_meshed_documents(root, &file).expect("loads");
    let assembly = crate::assembly_mesh::load_meshed_documents(root, &file)
        .expect("loads")
        .1;
    let grid = ludion_scene::assembly_assembled_grid(&assembly, &parts);
    let rest = part_placements(&data, None);
    let load = Loaded { data, grid, rest };
    let index = load
        .data
        .parts
        .iter()
        .position(|candidate| candidate.name == part)
        .expect("part");
    let [sx, sy, sz] = load.grid.voxel.size;
    let (hx, hz) = (sx as f32 * 0.5, sz as f32 * 0.5);
    let local = (0..sy)
        .flat_map(|y| (0..sz).flat_map(move |z| (0..sx).map(move |x| (x, y, z))))
        .find(|&(x, y, z)| load.grid.owners_at(x, y, z).contains(&index))
        .map(|(x, y, z)| Vec3::new(x as f32 - hx + 0.5, y as f32 + 0.5, z as f32 - hz + 0.5))
        .expect("the part owns a cell");
    let placements = Placements::new(&report.doc);
    let (offset, yaw_deg) = placements.of(role);
    let (cells, _) = posed_cells(&load, part, None, 0.0, offset, yaw_deg, spec.scale);
    (local, cells[0])
}
