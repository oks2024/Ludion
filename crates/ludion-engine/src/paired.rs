//! `paired_fit`: whether a two-role `kind: paired` action can start from where its cast stands.
//!
//! The document is loaded lazily through [`check_paired_files`](crate::check_paired_files) and kept in
//! a [`PairedLibrary`] resource rebuilt at every world build, the `AnimationLibrary` rule: never
//! hashed, and `asset.reload` does not reach it. The aligned pose is the document's `align` block
//! applied to the cast's current poses (`Plan/paired-animations.md` decisions 22–26). Nothing is
//! mutated: a game asks, then decides.
//!
//! # Example
//! ```no_run
//! use ludion_ecs::World;
//! use ludion_engine::{Warp, paired_fit};
//!
//! fn can_choke(world: &World, guard: ludion_core::ids::EntityId, player: ludion_core::ids::EntityId) -> bool {
//!     let cast = [("instigator", player), ("target", guard)];
//!     let warp = Warp { distance: 0.6, yaw_deg: 45.0 };
//!     paired_fit(world, "voxels/choke.paired.yaml", &cast, warp).is_ok_and(|fit| fit.fits)
//! }
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ludion_assets::AssetRoot;
use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, pointer_token};
use ludion_core::ids::EntityId;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::math::f64 as f64s;
use ludion_core::suggest;
use ludion_ecs::{Parent, World};
use ludion_physics::{CharacterMotor, MotorError, character_fit_ignoring};
use ludion_scene::{PairedDocument, PairedMoves};

use crate::components::{Mesh, MeshSource, Transform};
use crate::paired_doc::{PairedReport, check_paired_files};

type Loaded = Result<Arc<PairedReport>, Vec<Diagnostic>>;

/// The paired documents loaded so far, keyed by normalized content-relative path. A load error is
/// cached too and is not retried until the next world build.
pub(crate) struct PairedLibrary {
    root: PathBuf,
    docs: Mutex<BTreeMap<String, Loaded>>,
}

impl PairedLibrary {
    pub(crate) fn new(root: PathBuf) -> Self {
        PairedLibrary {
            root,
            docs: Mutex::new(BTreeMap::new()),
        }
    }

    /// The checked document at `rel`, loaded on first use.
    pub(crate) fn report(&self, rel: &str) -> Loaded {
        let key = ludion_core::paths::normalize(rel);
        let mut docs = self.docs.lock().unwrap_or_else(|e| e.into_inner());
        docs.entry(key.clone())
            .or_insert_with(|| {
                check_paired_files(AssetRoot::content(&self.root), &key)
                    .map(Arc::new)
                    .map_err(|found| {
                        let mut all = vec![
                            Diagnostic::new(
                                code::PAIRED_UNKNOWN,
                                format!("no `kind: paired` document `{key}` loads"),
                            )
                            .with_help(
                                "pass the document's path relative to the content root; the \
                                 diagnostics after this one say what is wrong with it",
                            ),
                        ];
                        all.extend(found);
                        all
                    })
            })
            .clone()
    }
}

/// The largest warp a paired action may use to reach its aligned pose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Warp {
    /// The most any role may move, planar, in metres.
    pub distance: f32,
    /// The most any role may turn, in degrees.
    pub yaw_deg: f32,
}

/// One role's pose after the warp, and how far it is from where the role stands.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct PairedRoleFit {
    /// The role name.
    pub role: String,
    /// The entity cast in it.
    pub entity: EntityId,
    /// The target feet position.
    pub feet: [f32; 3],
    /// The target heading in radians about `+Y`, the entity's `-Z` axis (`atan2(-f.x, -f.z)`).
    pub yaw: f32,
    /// The planar distance from the current pose, in metres.
    pub distance: f32,
    /// The absolute yaw change, in degrees.
    pub yaw_deg: f32,
}

/// What [`paired_fit`] found.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct PairedFit {
    /// Whether the action can start: nothing blocks the target poses and the warp is within bounds.
    pub fits: bool,
    /// The first role whose capsule cannot stand at its target, and the lowest-id collider in the way.
    pub blocked_by: Option<(String, EntityId)>,
    /// The first role (by name) whose warp exceeds the limit.
    pub over_warp: Option<String>,
    /// Every role's target pose, sorted by role name.
    pub roles: Vec<PairedRoleFit>,
}

/// A role's pose after the warp, in f64.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Target {
    pub role: String,
    pub feet: [f64; 3],
    pub yaw: f64,
    pub distance: f64,
    pub yaw_deg: f64,
    pub moves: bool,
}

/// `angle` wrapped into `(-pi, pi]`.
fn wrap(angle: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    let r = angle - tau * ((angle + std::f64::consts::PI) / tau).floor();
    if r <= -std::f64::consts::PI {
        std::f64::consts::PI
    } else {
        r
    }
}

/// `v` turned `theta` radians about `+Y`, planar (the height is left alone by the caller).
fn rot(theta: f64, v: [f64; 3]) -> [f64; 3] {
    let (s, c) = f64s::sin_cos(theta);
    [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c]
}

/// Planar sum: the height is `a`'s and the caller keeps each role's own.
fn plus(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1], a[2] + b[2]]
}

fn minus(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1], a[2] - b[2]]
}

/// The heading of the entity's `-Z` axis for Euler XYZ degrees, in radians: decision 10's convention.
pub(crate) fn heading(rotation_deg: [f32; 3]) -> f64 {
    let [x, y, z, w] = euler_xyz_degrees_to_quat(rotation_deg).map(f64::from);
    f64s::atan2(2.0 * (x * z + w * y), w * w - x * x - y * y + z * z)
}

/// The aligned targets of a two-role document: `poses` is each role's current `(feet, yaw)` and the
/// result is sorted by role name. Decision 24 states the three cases; feet height is never changed.
pub(crate) fn aligned_targets(
    doc: &PairedDocument,
    poses: &[(&str, [f64; 3], f64)],
) -> Vec<Target> {
    let reference = doc.align.reference.as_str();
    let pose_of = |name: &str| {
        let (_, feet, yaw) = poses
            .iter()
            .find(|(n, _, _)| *n == name)
            .expect("a pose for every role");
        (*feet, *yaw)
    };
    let other = doc
        .roles
        .iter()
        .map(|role| role.name.as_str())
        .find(|name| *name != reference)
        .expect("two roles");
    let offset = doc
        .align
        .offsets
        .iter()
        .find(|offset| offset.role == other)
        .expect("an offset for the other role");
    let t = offset.translation;
    let psi = offset.yaw_deg.to_radians();
    let (p_r, y_r) = pose_of(reference);
    let (p_o, y_o) = pose_of(other);

    let (r_star, o_star, r_moves, o_moves) = match &doc.align.moves {
        PairedMoves::Role(name) if name == other => {
            ((p_r, y_r), (plus(p_r, rot(y_r, t)), y_r + psi), false, true)
        }
        PairedMoves::Role(_) => {
            let y = y_o - psi;
            ((minus(p_o, rot(y, t)), y), (p_o, y_o), true, false)
        }
        PairedMoves::Split => {
            let y = y_r + 0.5 * wrap(y_o - psi - y_r);
            let mid = [0.5 * (p_r[0] + p_o[0]), 0.0, 0.5 * (p_r[2] + p_o[2])];
            let p = minus(mid, scaled(rot(y, t), 0.5));
            ((p, y), (plus(p, rot(y, t)), y + psi), true, true)
        }
    };

    let mut out = vec![
        target(reference, p_r, y_r, r_star, r_moves),
        target(other, p_o, y_o, o_star, o_moves),
    ];
    out.sort_by(|a, b| a.role.cmp(&b.role));
    out
}

fn with_height(planar: [f64; 3], keep: [f64; 3]) -> [f64; 3] {
    [planar[0], keep[1], planar[2]]
}

fn scaled(v: [f64; 3], k: f64) -> [f64; 3] {
    [v[0] * k, v[1] * k, v[2] * k]
}

fn target(role: &str, from: [f64; 3], from_yaw: f64, to: ([f64; 3], f64), moves: bool) -> Target {
    let (feet, yaw) = to;
    let feet = with_height(feet, from);
    let (dx, dz) = (feet[0] - from[0], feet[2] - from[2]);
    Target {
        role: role.to_owned(),
        feet,
        yaw,
        distance: f64s::sqrt(dx * dx + dz * dz),
        yaw_deg: wrap(yaw - from_yaw).abs().to_degrees(),
        moves,
    }
}

fn invalid(role: &str, message: String, help: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::PAIRED_CAST_INVALID, message)
        .with_path(format!("/roles/{}", pointer_token(role)))
        .with_help(help)
}

/// Whether a two-role paired action can start from where `cast` stands.
///
/// `paired` is the document's content-root path (`voxels/choke.paired.yaml`); `cast` names an entity
/// for every role. Each role is moved to its aligned pose (decision 24), and a role with a
/// `CharacterMotor` is fit-tested there at its role `posture`, looking through its partner. Nothing
/// is mutated.
///
/// # Errors
/// `L4370` when the document does not load (the loader's diagnostics follow); `L4373` for more than
/// two roles; `L4371` for a cast that is not exactly the document's roles or whose entity cannot play
/// its role; `L4372` for a scale that is not the role's. All cast problems are collected.
///
/// # Example
/// ```no_run
/// use ludion_ecs::World;
/// use ludion_engine::{Warp, paired_fit};
/// # fn demo(world: &World, a: ludion_core::ids::EntityId, b: ludion_core::ids::EntityId) {
/// let warp = Warp { distance: 0.6, yaw_deg: 45.0 };
/// let fit = paired_fit(world, "voxels/choke.paired.yaml", &[("instigator", a), ("target", b)], warp)
///     .expect("a valid cast");
/// if let Some((role, blocker)) = fit.blocked_by {
///     println!("{role} is blocked by {blocker:?}");
/// }
/// # }
/// ```
pub fn paired_fit(
    world: &World,
    paired: &str,
    cast: &[(&str, EntityId)],
    max_warp: Warp,
) -> Result<PairedFit, Vec<Diagnostic>> {
    let report = match world.resource::<PairedLibrary>() {
        Some(library) => library.report(paired)?,
        None => {
            return Err(vec![
                Diagnostic::new(
                    code::PAIRED_UNKNOWN,
                    format!(
                        "no `kind: paired` document `{paired}` loads: the engine has no project"
                    ),
                )
                .with_help(
                    "build the engine with a project root (`Engine::builder().project_root(..)`)",
                ),
            ]);
        }
    };
    let doc = &report.doc;
    if doc.roles.len() != 2 {
        return Err(vec![
            Diagnostic::new(
                code::PAIRED_UNSUPPORTED,
                format!(
                    "paired document `{}` has {} roles; the runtime plays exactly two",
                    doc.name,
                    doc.roles.len()
                ),
            )
            .with_path("/roles")
            .with_help("split the action into pairs of roles"),
        ]);
    }

    let mut roles: Vec<&ludion_scene::PairedRole> = doc.roles.iter().collect();
    roles.sort_by(|a, b| a.name.cmp(&b.name));
    let names = || roles.iter().map(|role| role.name.as_str());

    let mut errors = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for (name, _) in cast {
        if !names().any(|n| n == *name) {
            let help = match suggest::nearest(name, names()) {
                Some(candidate) => format!("nearest role is `{candidate}`"),
                None => format!(
                    "the document's roles are {}",
                    names().collect::<Vec<_>>().join(", ")
                ),
            };
            errors.push(invalid(
                name,
                format!("`{}` has no role `{name}`", doc.name),
                help,
            ));
        } else if seen.contains(name) {
            errors.push(invalid(
                name,
                format!("role `{name}` is cast twice"),
                "cast each role once",
            ));
        }
        seen.push(name);
    }
    for role in &roles {
        if !seen.contains(&role.name.as_str()) {
            errors.push(invalid(
                &role.name,
                format!("role `{}` has no entity in the cast", role.name),
                "cast every role of the document",
            ));
        }
    }

    let mut entities: Vec<(&ludion_scene::PairedRole, EntityId, Transform)> = Vec::new();
    if errors.is_empty() {
        for role in &roles {
            let entity = cast
                .iter()
                .find(|(name, _)| *name == role.name)
                .map(|(_, entity)| *entity)
                .expect("checked above");
            match check_entity(world, paired, role, entity, &mut errors) {
                Some(transform) => entities.push((*role, entity, transform)),
                None => continue,
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let poses: Vec<(&str, [f64; 3], f64)> = entities
        .iter()
        .map(|(role, _, transform)| {
            (
                role.name.as_str(),
                transform.translation.map(f64::from),
                heading(transform.rotation),
            )
        })
        .collect();
    let targets = aligned_targets(doc, &poses);

    let mut fit = PairedFit {
        fits: false,
        blocked_by: None,
        over_warp: None,
        roles: Vec::new(),
    };
    for ((role, entity, _), target) in entities.iter().zip(&targets) {
        fit.roles.push(PairedRoleFit {
            role: role.name.clone(),
            entity: *entity,
            feet: target.feet.map(|c| c as f32),
            yaw: target.yaw as f32,
            distance: target.distance as f32,
            yaw_deg: target.yaw_deg as f32,
        });
    }
    for (index, ((role, entity, _), target)) in entities.iter().zip(&targets).enumerate() {
        if !target.moves || !world.has::<CharacterMotor>(*entity) {
            continue;
        }
        let partner = entities[1 - index].1;
        let at = &fit.roles[index];
        match character_fit_ignoring::<Transform>(
            world,
            *entity,
            role.posture.as_deref(),
            at.feet,
            at.yaw,
            Some(partner),
        ) {
            Ok(Some(blocker)) => {
                if fit.blocked_by.is_none() {
                    fit.blocked_by = Some((role.name.clone(), blocker));
                }
            }
            Ok(None) => {}
            Err(error) => errors.push(motor_refusal(role, &error)),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    fit.over_warp = entities
        .iter()
        .zip(&targets)
        .find(|(_, target)| {
            target.moves
                && (target.distance > f64::from(max_warp.distance)
                    || target.yaw_deg > f64::from(max_warp.yaw_deg))
        })
        .map(|((role, _, _), _)| role.name.clone());
    fit.fits = fit.blocked_by.is_none() && fit.over_warp.is_none();
    Ok(fit)
}

fn motor_refusal(role: &ludion_scene::PairedRole, error: &MotorError) -> Diagnostic {
    let help = match error {
        MotorError::UnknownPosture { .. } => {
            "the role's `posture` must name a posture of the cast entity's `CharacterMotor`"
        }
        _ => "fix the cast entity's `CharacterMotor`",
    };
    invalid(&role.name, format!("role `{}`: {error}", role.name), help)
}

/// Collects the `L4371`/`L4372` problems of `entity` as `role`; returns its `Transform` when it has one
/// and is a root.
fn check_entity(
    world: &World,
    paired: &str,
    role: &ludion_scene::PairedRole,
    entity: EntityId,
    errors: &mut Vec<Diagnostic>,
) -> Option<Transform> {
    let name = role.name.as_str();
    if !world.is_alive(entity) {
        errors.push(invalid(
            name,
            format!("role `{name}`: {entity:?} is not alive"),
            "cast a live entity",
        ));
        return None;
    }
    let mut ok = true;
    let transform = world.get::<Transform>(entity).ok().flatten().cloned();
    if transform.is_none() {
        errors.push(invalid(
            name,
            format!("role `{name}`: {entity:?} has no `Transform`"),
            "cast an entity placed in the world",
        ));
        ok = false;
    }
    if world.has::<Parent>(entity) {
        errors.push(invalid(
            name,
            format!(
                "role `{name}`: {entity:?} has a `Parent`; a paired action writes a world pose"
            ),
            "reparent the entity to the root first",
        ));
        ok = false;
    }
    let expected = ludion_core::paths::sibling(paired, &role.assembly);
    let shape = world
        .get::<Mesh>(entity)
        .ok()
        .flatten()
        .map(|mesh| &mesh.shape);
    match shape {
        Some(MeshSource::Assembly(path)) if ludion_core::paths::normalize(path) == expected => {}
        other => {
            let found = match other {
                Some(MeshSource::Assembly(path)) => format!("the assembly `{path}`"),
                Some(_) => "a mesh that is not an assembly".to_owned(),
                None => "no `Mesh`".to_owned(),
            };
            errors.push(invalid(
                name,
                format!("role `{name}`: {entity:?} draws {found}, not `{expected}`"),
                "cast the entity that draws the role's assembly",
            ));
            ok = false;
        }
    }
    if let Some(transform) = &transform {
        let s = role.scale;
        let off = |c: f32| (f64::from(c) - s).abs() > 1e-5 * s;
        if transform.scale.into_iter().any(off) {
            let [a, b, c] = transform.scale;
            let kind = if a == b && b == c {
                "uniform"
            } else {
                "non-uniform"
            };
            errors.push(
                Diagnostic::new(
                    code::PAIRED_SCALE_MISMATCH,
                    format!(
                        "role `{name}`: {entity:?} has {kind} scale {:?}, the role's `scale` is {}",
                        transform.scale, role.scale
                    ),
                )
                .with_path(format!("/roles/{}", pointer_token(name)))
                .with_help("set the entity's uniform scale to the role's `scale`"),
            );
            ok = false;
        }
    }
    ok.then_some(transform).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_scene::{PairedAlign, PairedOffset, PairedRole};
    use std::f64::consts::{FRAC_PI_2, PI};

    fn doc(moves: PairedMoves) -> PairedDocument {
        let role = |name: &str| PairedRole {
            name: name.to_owned(),
            assembly: "a.assembly.yaml".to_owned(),
            clip: None,
            align_only: true,
            posture: None,
            scale: 1.0,
        };
        PairedDocument {
            name: "t".to_owned(),
            roles: vec![role("instigator"), role("target")],
            align: PairedAlign {
                reference: "instigator".to_owned(),
                moves,
                ticks: 1,
                offsets: vec![PairedOffset {
                    role: "target".to_owned(),
                    translation: [0.0, 0.0, -0.5],
                    yaw_deg: 180.0,
                }],
            },
            events: Vec::new(),
            contacts: Vec::new(),
        }
    }

    fn near(a: [f64; 3], b: [f64; 3]) {
        assert!(
            a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-12),
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn the_target_moves_onto_the_reference_offset() {
        let poses = [
            ("instigator", [1.0, 0.0, 2.0], FRAC_PI_2),
            ("target", [4.0, 0.5, 4.0], 0.0),
        ];
        let out = aligned_targets(&doc(PairedMoves::Role("target".to_owned())), &poses);
        assert_eq!(out[0].role, "instigator");
        assert!(!out[0].moves && out[0].distance == 0.0 && out[0].yaw_deg == 0.0);
        // 0.5 m ahead of an entity facing -X.
        near(out[1].feet, [0.5, 0.5, 2.0]);
        assert!((out[1].yaw - (FRAC_PI_2 + PI)).abs() < 1e-12);
        assert!(out[1].moves);
        let d = (3.5f64 * 3.5 + 2.0 * 2.0).sqrt();
        assert!((out[1].distance - d).abs() < 1e-12);
        assert!((out[1].yaw_deg - 90.0).abs() < 1e-9, "{}", out[1].yaw_deg);
    }

    #[test]
    fn the_instigator_moves_onto_the_target() {
        let poses = [
            ("instigator", [0.0, 0.25, 0.0], 0.0),
            ("target", [2.0, 0.0, 3.0], PI),
        ];
        let out = aligned_targets(&doc(PairedMoves::Role("instigator".to_owned())), &poses);
        // y_R* = PI - PI = 0, p_R* = p_O - (0, 0, -0.5).
        assert!((out[0].yaw).abs() < 1e-12);
        near(out[0].feet, [2.0, 0.25, 3.5]);
        near(out[1].feet, [2.0, 0.0, 3.0]);
        assert!(out[0].moves && !out[1].moves);
        assert!((out[0].distance - (4.0f64 + 12.25).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn split_meets_at_the_midpoint_and_keeps_the_offset() {
        let poses = [
            ("instigator", [0.0, 0.0, 0.0], 0.0),
            ("target", [2.0, 0.0, 0.0], PI),
        ];
        let out = aligned_targets(&doc(PairedMoves::Split), &poses);
        let (r, o) = (&out[0], &out[1]);
        let mid = [
            0.5 * (r.feet[0] + o.feet[0]),
            0.0,
            0.5 * (r.feet[2] + o.feet[2]),
        ];
        near(mid, [1.0, 0.0, 0.0]);
        // The offset holds in the reference frame: the target is 0.5 m ahead of the reference.
        let ahead = rot(r.yaw, [0.0, 0.0, -0.5]);
        near([o.feet[0] - r.feet[0], 0.0, o.feet[2] - r.feet[2]], ahead);
        assert!((wrap(o.yaw - r.yaw) - PI).abs() < 1e-12);
        assert!(r.moves && o.moves);
    }

    #[test]
    fn wrap_maps_to_the_half_open_interval() {
        assert_eq!(wrap(PI), PI);
        assert_eq!(wrap(-PI), PI);
        assert!((wrap(3.0 * PI) - PI).abs() < 1e-12);
        assert!((wrap(0.1 + 2.0 * PI) - 0.1).abs() < 1e-12);
        assert!((wrap(PI + 0.1) - (0.1 - PI)).abs() < 1e-12);
    }

    #[test]
    fn heading_follows_the_negative_z_axis() {
        assert_eq!(heading([0.0; 3]), 0.0);
        assert!((heading([0.0, 90.0, 0.0]) - FRAC_PI_2).abs() < 1e-6);
        assert!((heading([0.0, -90.0, 0.0]) + FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn a_load_error_is_cached_and_not_retried() {
        let dir = std::env::temp_dir().join(format!("ludion-paired-lib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.paired.yaml");
        std::fs::write(&file, "not: a paired doc\n").unwrap();
        let library = PairedLibrary::new(dir.clone());
        let first = library.report("x.paired.yaml").unwrap_err();
        assert_eq!(first[0].code, code::PAIRED_UNKNOWN);
        std::fs::remove_file(&file).unwrap();
        // `./x.paired.yaml` normalizes to the same key, so no IO and the same error.
        let second = library.report("./x.paired.yaml").unwrap_err();
        assert_eq!(first, second);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
