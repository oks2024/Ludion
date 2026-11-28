//! Loading a skinned `kind: assembly` document: the skinned mesh it names plus its bound animation
//! clips.
//!
//! A skinned assembly is a strict-YAML `*.character.yaml` document ([`ludion_scene::check_assembly`]
//! with `mesh:` set) that names one skinned mesh document and binds short clip names to
//! `kind: animation` files, each with an optional playback speed and events overlay
//! (`Plan/animation-clips.md` decisions 10–12). This module is the engine-side IO seam, with the
//! same `load(root, rel)` shape as [`crate::assembly_mesh`]: read the bytes, parse and validate
//! through `ludion-scene`, then run the cross-file rules
//! ([`ludion_scene::check_assembly_mesh_clips`]). The rules live in `ludion-scene`; this module adds
//! only the file reads and the unskinned-mesh check, which needs the mesh document.
//!
//! The character draws through its mesh path exactly like a plain `Mesh.shape: asset`: nothing here
//! meshes or samples anything.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::glam::Vec3;
use ludion_scene::{AnimationDocument, AnimationEvent, AssemblyDocument};

use crate::assembly_mesh::read_clip;
use crate::asset_cache::SourceFiles;
use crate::blend_space::{BlendSpaceData, BoundClip, resolve_blends};
use ludion_core::paths::sibling;

/// One resolved clip binding: the short name `Animation.clip` selects by, the parsed clip document,
/// and the effective playback overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterClipData {
    /// The short name an [`Animation`](crate::Animation) component names.
    pub name: String,
    /// The clip document's path as authored, relative to the character file.
    pub file: String,
    /// The parsed, validated clip.
    pub clip: AnimationDocument,
    /// The clip resolved against the mesh's joints in skin order, bound at load: joint `i` is
    /// palette entry `i`. `sampled_palette` reads a sampler bound to another joint count as no clip.
    pub sampler: ludion_scene::ClipSampler,
    /// The playback speed multiplier the character binds the clip with.
    pub speed: f64,
    /// The clip's events: its own when it has any, else the character's overlay (a clip has exactly
    /// one home for its events).
    pub events: Vec<AnimationEvent>,
}

/// A loaded character: its skinned mesh's path and its resolved clips.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterData {
    /// The skinned mesh document's content-root-relative path, resolved against the character
    /// file's directory.
    pub mesh: String,
    /// The bound clips, sorted by short name (the character document's own canonical order).
    pub clips: Vec<CharacterClipData>,
    /// The bone masks, sorted by name, each resolved to one weight per skin joint.
    pub masks: Vec<CharacterMaskData>,
    /// The blend spaces, sorted by name, each resolved against the bound clips.
    pub blends: Vec<BlendSpaceData>,
}

/// One bone mask resolved against the mesh's joints (`Plan/animation-layers.md` decision 1).
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterMaskData {
    /// The short name an `AnimationLayer.mask` selects by.
    pub name: String,
    /// One weight per skin joint, in skin order; joint `i` is palette entry `i`.
    pub weights: Vec<f32>,
}

impl CharacterData {
    /// The resolved weights of the mask named `name` (one per skin joint), or `None` when this
    /// character defines no such mask.
    pub fn mask(&self, name: &str) -> Option<&[f32]> {
        self.masks
            .iter()
            .find(|mask| mask.name == name)
            .map(|mask| mask.weights.as_slice())
    }

    /// The clip bound under the short `name`, or `None` when this character binds no such clip — the
    /// caller's cue to draw the rest pose, since a clip name is chosen at runtime by game logic.
    pub fn clip(&self, name: &str) -> Option<&CharacterClipData> {
        self.clips.iter().find(|binding| binding.name == name)
    }

    /// The blend space named `name`, or `None` when this character defines no such blend.
    pub fn blend(&self, name: &str) -> Option<&BlendSpaceData> {
        self.blends.iter().find(|blend| blend.name == name)
    }
}

/// Reads and validates the character at `root`/`rel`, together with its mesh document and every
/// clip file it names.
///
/// `mesh` and each clip's `clip` resolve **relative to the character file's own directory**; `..`
/// segments are legal. Clip diagnostics accumulate across all clips, each prefixed with the binding
/// and its path.
///
/// # Errors
/// - `L1101` if the character, its mesh or any clip file cannot be read.
/// - `L1137`/`L1138` for a malformed or inconsistent skinned assembly, including a voxel assembly, a
///   mesh with no skin, an overlay on a clip that has its own events, and a clip track naming a joint
///   the mesh lacks.
/// - `L1141`/`L1142`/`L1198` for a malformed clip document.
pub fn load(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<CharacterData, Vec<Diagnostic>> {
    load_with_sources(root, rel).map(|(data, _)| data)
}

/// [`load`], additionally reporting **every file it read** — the character document first, then its
/// mesh document, then each clip, as `root`-relative paths, each with the content hash of the bytes it
/// decoded. That list is the character's reload
/// dependency set (see [`crate::asset_cache`]). The mesh's own texture PNGs stay under the mesh
/// entry's edges.
pub(crate) fn load_with_sources(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<(CharacterData, SourceFiles), Vec<Diagnostic>> {
    let (doc, doc_hash) = read_character(root, rel)?;
    let mesh = sibling(rel, doc.mesh.as_deref().unwrap_or_default());
    let (mesh_joints, mesh_hash) =
        crate::mesh_asset::with_mesh_source(root, &mesh, ludion_scene::read_mesh_joints)?;
    let joints: Vec<&str> = match &mesh_joints {
        Some(joints) if !joints.is_empty() => {
            joints.iter().map(|joint| joint.name.as_str()).collect()
        }
        _ => {
            return Err(vec![
                Diagnostic::new(
                    code::ASSEMBLY_DATA_CONSISTENCY,
                    format!(
                        "mesh `{mesh}` has no `joints:`: a skinned assembly needs a skinned mesh"
                    ),
                )
                .with_path("/mesh")
                .with_help(
                    "point `mesh` at a mesh document exported with its armature (e.g. through \
                     `ludion import`)",
                ),
            ]);
        }
    };

    let mut sources = vec![(rel.to_string(), doc_hash), (mesh.clone(), mesh_hash)];
    let mut diagnostics = Vec::new();
    let mut clips = Vec::with_capacity(doc.clips.len());
    for binding in &doc.clips {
        let file = sibling(rel, &binding.file);
        match read_clip(root, &file, &binding.name) {
            Ok((clip, hash)) => {
                clips.push(clip);
                sources.push((file, hash));
            }
            Err(mut errors) => diagnostics.append(&mut errors),
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let cross = ludion_scene::check_assembly_mesh_clips(&doc, &clips, &joints);
    if !cross.is_empty() {
        return Err(cross);
    }

    let clips: Vec<CharacterClipData> = doc
        .clips
        .iter()
        .zip(clips)
        .map(|(binding, clip)| CharacterClipData {
            name: binding.name.clone(),
            file: binding.file.clone(),
            speed: binding.speed,
            sampler: ludion_scene::ClipSampler::new(&clip, joints.iter().copied()),
            events: if clip.events.is_empty() {
                binding.events.clone()
            } else {
                clip.events.clone()
            },
            clip,
        })
        .collect();
    let skin: Vec<(&str, Option<u32>)> = mesh_joints
        .iter()
        .flatten()
        .map(|joint| (joint.name.as_str(), joint.parent))
        .collect();
    let masks = doc
        .masks
        .iter()
        .map(|mask| CharacterMaskData {
            name: mask.name.clone(),
            weights: ludion_scene::resolve_rig_mask(mask, &skin),
        })
        .collect();
    let bound: Vec<BoundClip<'_>> = clips
        .iter()
        .map(|binding| BoundClip {
            name: &binding.name,
            duration: binding.clip.duration,
            speed: binding.speed,
            events: &binding.events,
        })
        .collect();
    let blends = resolve_blends(&doc.blends, &bound);
    Ok((
        CharacterData {
            mesh,
            clips,
            masks,
            blends,
        },
        sources,
    ))
}

/// Sampling rate of the [`contact_heights`] report, in samples per clip second.
const CONTACT_SAMPLE_RATE: f64 = 30.0;

/// How far above the mesh's rest floor a leaf joint's lowest vertex may sit and still count as a
/// ground-contact joint, as a fraction of the mesh's rest height.
const CONTACT_BAND: f32 = 0.1;

/// One contact joint's lowest-vertex height over a clip.
#[derive(Clone, Debug, PartialEq)]
pub struct JointHeight {
    /// The contact joint's name.
    pub joint: String,
    /// The lowest Y the joint's vertices reach over the clip, in mesh units.
    pub min: f32,
    /// The clip time `min` is reached at, in seconds.
    pub min_t: f64,
    /// The highest the joint's lowest vertex gets over the clip.
    pub max: f32,
    /// The clip time `max` is reached at, in seconds.
    pub max_t: f64,
}

/// A clip's ground-contact report: empty `joints` when the mesh has no contact joint.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipContact {
    /// The clip's short name.
    pub clip: String,
    /// One entry per contact joint, in the skin's joint order.
    pub joints: Vec<JointHeight>,
}

/// Where a character's feet go over each of its clips — the `check-scene` ground-contact report.
///
/// The contact joints are the leaf joints whose dominant-weight vertices' lowest rest Y is within
/// `CONTACT_BAND` of the mesh's rest height above its rest floor (name-free). Each clip is sampled
/// at 30 Hz plus its last instant, posed by the same `sampled_palette` the renderer draws with,
/// and each contact joint's vertices are skinned by that joint's own palette entry alone.
///
/// # Errors
/// The mesh's own load diagnostics.
pub fn contact_heights(
    root: ludion_assets::AssetRoot<'_>,
    data: &CharacterData,
) -> Result<Vec<ClipContact>, Vec<Diagnostic>> {
    let mesh = crate::mesh_asset::load(root, &data.mesh)?;
    let joints = &mesh.skin_joints;
    let vertices = mesh.positions.len() / 3;
    let position = |v: usize| {
        Vec3::new(
            mesh.positions[3 * v],
            mesh.positions[3 * v + 1],
            mesh.positions[3 * v + 2],
        )
    };
    let dominant = |v: usize| {
        let weights = &mesh.weights[4 * v..4 * v + 4];
        let best = (1..4).fold(
            0,
            |best, k| if weights[k] > weights[best] { k } else { best },
        );
        mesh.bone_indices[4 * v + best] as usize
    };

    let mut owned: Vec<Vec<usize>> = vec![Vec::new(); joints.len()];
    for v in 0..vertices {
        if let Some(list) = owned.get_mut(dominant(v)) {
            list.push(v);
        }
    }
    let (floor, ceiling) = (0..vertices).fold((f32::MAX, f32::MIN), |(lo, hi), v| {
        (lo.min(position(v).y), hi.max(position(v).y))
    });
    let band = floor + (ceiling - floor) * CONTACT_BAND;
    let contact: Vec<usize> = (0..joints.len())
        .filter(|&j| !joints.iter().any(|child| child.parent == Some(j as u32)))
        .filter(|&j| {
            owned[j]
                .iter()
                .map(|&v| position(v).y)
                .reduce(f32::min)
                .is_some_and(|lowest| lowest <= band)
        })
        .collect();

    let mut reports = Vec::with_capacity(data.clips.len());
    let mut palette = Vec::with_capacity(joints.len());
    let mut scratch = Vec::with_capacity(joints.len());
    for binding in &data.clips {
        let duration = binding.clip.duration;
        let mut heights: Vec<JointHeight> = contact
            .iter()
            .map(|&j| JointHeight {
                joint: joints[j].name.clone(),
                min: f32::MAX,
                min_t: 0.0,
                max: f32::MIN,
                max_t: 0.0,
            })
            .collect();
        if !heights.is_empty() {
            let steps = (duration * CONTACT_SAMPLE_RATE).floor() as u32;
            let mut times: Vec<f64> = (0..=steps)
                .map(|i| f64::from(i) / CONTACT_SAMPLE_RATE)
                .collect();
            if times.last() != Some(&duration) {
                times.push(duration);
            }
            for &t in &times {
                palette.clear();
                crate::extract::sampled_palette(
                    joints,
                    Some((&binding.sampler, t)),
                    None,
                    &mut scratch,
                    &mut palette,
                );
                for (height, &j) in heights.iter_mut().zip(&contact) {
                    let lowest = owned[j]
                        .iter()
                        .map(|&v| palette[j].transform_point3(position(v)).y)
                        .fold(f32::MAX, f32::min);
                    if lowest < height.min {
                        (height.min, height.min_t) = (lowest, t);
                    }
                    if lowest > height.max {
                        (height.max, height.max_t) = (lowest, t);
                    }
                }
            }
        }
        reports.push(ClipContact {
            clip: binding.name.clone(),
            joints: heights,
        });
    }
    Ok(reports)
}

/// The character's mesh path alone, reading and parsing only the character document — what
/// `SkinBone` needs to find a character's skin without loading its clips.
pub(crate) fn mesh_path(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<String, Vec<Diagnostic>> {
    read_character(root, rel).map(|(doc, _)| sibling(rel, doc.mesh.as_deref().unwrap_or_default()))
}

/// Reads and validates the skinned assembly at `root`/`rel`, returning it with its bytes' content
/// hash. The returned document's `mesh` is always `Some`.
fn read_character(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<(AssemblyDocument, u64), Vec<Diagnostic>> {
    let (doc, hash) = crate::assembly_body::read_document(root, rel)?;
    if doc.mesh.is_none() {
        return Err(vec![
            Diagnostic::new(
                code::ASSEMBLY_DATA_CONSISTENCY,
                format!(
                    "`{rel}` is a voxel assembly; `Mesh.shape: {{ character: … }}` draws a skinned \
                     assembly"
                ),
            )
            .with_path("/mesh")
            .with_help("draw a voxel assembly with `Mesh.shape: { assembly: <path> }`"),
        ]);
    }
    Ok((doc, hash))
}
