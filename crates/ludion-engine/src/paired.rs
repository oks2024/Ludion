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
use ludion_components::crossfade;
use ludion_components::paired_action::PairedAction;
use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, pointer_token};
use ludion_core::ids::EntityId;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::math::f64 as f64s;
use ludion_core::suggest;
use ludion_core::time::{FrameIndex, GameTime};
use ludion_ecs::{Parent, SystemCtx, World};
use ludion_physics::{
    CharacterMotor, CharacterMotorState, IgnoreCollisions, MotorError, character_fit_ignoring,
};
use ludion_scene::{PairedDocument, PairedMoves};

use crate::animation_events::{AnimationEvents, ClipEvent};
use crate::components::{Animation, Mesh, MeshSource, Transform};
use crate::motor::set_character_posture;
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

/// A cast resolved against its document: every check of decisions 25 and 28 passed.
pub(crate) struct Resolved {
    pub report: Arc<PairedReport>,
    /// Each role's name, entity and current `Transform`, sorted by role name.
    pub entities: Vec<(String, EntityId, Transform)>,
    /// The aligned targets, in the same order.
    pub targets: Vec<Target>,
}

/// Loads the document and checks the cast: the `L4370`–`L4373` rules, all cast problems collected.
pub(crate) fn resolve(
    world: &World,
    paired: &str,
    cast: &[(&str, EntityId)],
) -> Result<Resolved, Vec<Diagnostic>> {
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
    let entities = entities
        .into_iter()
        .map(|(role, entity, transform)| (role.name.clone(), entity, transform))
        .collect();
    Ok(Resolved {
        report: Arc::clone(&report),
        entities,
        targets,
    })
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
    let Resolved {
        report,
        entities,
        targets,
    } = resolve(world, paired, cast)?;
    let doc = &report.doc;
    let role_of = |name: &str| {
        doc.roles
            .iter()
            .find(|role| role.name == name)
            .expect("a resolved role is in the document")
    };
    let mut errors = Vec::new();
    let mut fit = PairedFit {
        fits: false,
        blocked_by: None,
        over_warp: None,
        roles: Vec::new(),
    };
    for ((role, entity, _), target) in entities.iter().zip(&targets) {
        fit.roles.push(PairedRoleFit {
            role: role.clone(),
            entity: *entity,
            feet: target.feet.map(|c| c as f32),
            yaw: target.yaw as f32,
            distance: target.distance as f32,
            yaw_deg: target.yaw_deg as f32,
        });
    }
    for (index, ((name, entity, _), target)) in entities.iter().zip(&targets).enumerate() {
        let role = role_of(name);
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
        .map(|((role, _, _), _)| role.clone());
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

// ---- playing a paired action (decisions 27-31) ----

/// One tick of a paired mover's warp: feed it to `move_character` and `turn_character`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PairedMotion {
    /// The planar displacement to apply this tick, in metres (`y` is 0).
    pub displacement: [f32; 3],
    /// The absolute heading to turn to, in radians about `+Y` (the `-Z` axis convention).
    pub yaw: f32,
    /// Whether the warp is still closing (`false` once the aligned pose is held, and for a role that
    /// does not move).
    pub warping: bool,
}

/// The entities whose [`PairedAction`] shares `entity`'s `(document, reference, started)`, ascending,
/// `entity` included.
fn group_of(world: &World, action: &PairedAction) -> Vec<EntityId> {
    world
        .entities()
        .filter(|id| {
            world
                .get_unrecorded::<PairedAction>(*id)
                .ok()
                .flatten()
                .is_some_and(|other| same_group(other, action))
        })
        .collect()
}

fn same_group(a: &PairedAction, b: &PairedAction) -> bool {
    a.document == b.document && a.reference == b.reference && a.started == b.started
}

/// Ends `entity`'s role now: its `PairedAction` and `IgnoreCollisions` go, and so does any partner's
/// `IgnoreCollisions` that looks through it. The partners keep playing.
pub(crate) fn end_role(world: &mut World, entity: EntityId) {
    let Some(action) = world
        .get_unrecorded::<PairedAction>(entity)
        .ok()
        .flatten()
        .cloned()
    else {
        return;
    };
    for partner in group_of(world, &action) {
        if partner == entity {
            continue;
        }
        let looks_through = world
            .get_unrecorded::<IgnoreCollisions>(partner)
            .ok()
            .flatten()
            .is_some_and(|ignore| ignore.with == entity);
        if looks_through {
            let _ = world.remove::<IgnoreCollisions>(partner);
        }
    }
    let _ = world.remove::<PairedAction>(entity);
    let _ = world.remove::<IgnoreCollisions>(entity);
}

/// Starts a two-role paired action: both roles are stamped, the movers' postures set, the partners
/// made transparent to each other's motors, and each clip role crossfades to its clip at speed 1.
///
/// **Register [`paired_update`] after [`animation_events`](crate::animation_events) and before any
/// reader of [`AnimationEvents`], or the `paired:` events vanish.** `fade` is the crossfade in
/// seconds (`0` cuts). `game_time` is the gameplay time of this tick (as in `switch_to_ragdoll`): both
/// `Animation.started` and `PairedAction.started` are its floor. It does not re-test the fit: call
/// [`paired_fit`] first. A cast entity already in an action leaves it (its old partner plays on).
///
/// # Errors
/// What [`paired_fit`] refuses (`L4370`–`L4373`), and `L4371` for an entity that already ignores
/// another entity's collider, or a role `posture` that is blocked or refused. Nothing is mutated
/// then.
///
/// # Example
/// ```no_run
/// use ludion_ecs::SystemCtx;
/// use ludion_engine::{start_paired, paired_fit, Warp};
/// # fn demo(world: &mut ludion_ecs::World, ctx: &SystemCtx, a: ludion_core::ids::EntityId, b: ludion_core::ids::EntityId) {
/// let cast = [("instigator", a), ("target", b)];
/// let warp = Warp { distance: 0.6, yaw_deg: 45.0 };
/// if paired_fit(world, "voxels/choke.paired.yaml", &cast, warp).is_ok_and(|fit| fit.fits) {
///     start_paired(world, "voxels/choke.paired.yaml", &cast, 0.1, ctx.game_instant).expect("fits");
/// }
/// # }
/// ```
pub fn start_paired(
    world: &mut World,
    paired: &str,
    cast: &[(&str, EntityId)],
    fade: f32,
    game_time: impl Into<GameTime>,
) -> Result<(), Vec<Diagnostic>> {
    let resolved = resolve(world, paired, cast)?;
    let doc = &resolved.report.doc;
    let role_of = |name: &str| {
        doc.roles
            .iter()
            .find(|role| role.name == name)
            .expect("a resolved role is in the document")
    };

    let mut errors = Vec::new();
    for (name, entity, _) in &resolved.entities {
        let partner = resolved
            .entities
            .iter()
            .map(|(_, other, _)| *other)
            .find(|other| other != entity);
        let Some(ignore) = world
            .get_unrecorded::<IgnoreCollisions>(*entity)
            .ok()
            .flatten()
        else {
            continue;
        };
        if Some(ignore.with) == partner || in_action_with(world, *entity, ignore.with) {
            continue;
        }
        errors.push(invalid(
            name,
            format!(
                "role `{name}`: {entity:?} already ignores {:?}; remove it first",
                ignore.with
            ),
            "remove the entity's `IgnoreCollisions` before pairing it",
        ));
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut saved: Vec<(EntityId, Option<String>)> = Vec::new();
    for (name, entity, _) in &resolved.entities {
        let role = role_of(name);
        let Some(posture) = role.posture.as_deref() else {
            continue;
        };
        if !world.has::<CharacterMotor>(*entity) {
            continue;
        }
        let before = world
            .get_unrecorded::<CharacterMotorState>(*entity)
            .ok()
            .flatten()
            .and_then(|state| state.posture.clone());
        let refused = match set_character_posture(world, *entity, Some(posture)) {
            Ok(report) => report.blocked_by.map(|blocker| {
                invalid(
                    name,
                    format!("role `{name}`: posture `{posture}` is blocked by {blocker:?}"),
                    "clear the space the role's posture needs, or start the action elsewhere",
                )
            }),
            Err(error) => Some(motor_refusal(role, &error)),
        };
        if let Some(diagnostic) = refused {
            for (earlier, posture) in saved.into_iter().rev() {
                let _ = set_character_posture(world, earlier, posture.as_deref());
            }
            return Err(vec![diagnostic]);
        }
        saved.push((*entity, before));
    }

    for (_, entity, _) in &resolved.entities {
        end_role(world, *entity);
    }

    let reference = resolved
        .entities
        .iter()
        .find(|(name, _, _)| *name == doc.align.reference)
        .map(|(_, entity, _)| *entity)
        .expect("the reference role is cast");
    let started = FrameIndex::new(game_time.into().ticks_floor());
    let document = ludion_core::paths::normalize(paired);
    for (name, entity, _) in &resolved.entities {
        let partner = resolved
            .entities
            .iter()
            .map(|(_, other, _)| *other)
            .find(|other| other != entity)
            .expect("two roles");
        let _ = world.insert(
            *entity,
            PairedAction {
                document: document.clone(),
                action: doc.name.clone(),
                role: name.clone(),
                reference,
                started,
                fired: 0,
            },
        );
        let _ = world.insert(*entity, IgnoreCollisions { with: partner });
        if let Some(clip) = &role_of(name).clip {
            if !world.has::<Animation>(*entity) {
                let _ = world.insert(*entity, Animation::default());
            }
            if let Ok(Some(animation)) = world.get_mut::<Animation>(*entity) {
                crossfade(animation, clip, fade, started);
                animation.speed = 1.0;
            }
        }
    }
    Ok(())
}

/// Whether `entity` is in a paired action whose participants include `other`.
fn in_action_with(world: &World, entity: EntityId, other: EntityId) -> bool {
    let Some(action) = world.get_unrecorded::<PairedAction>(entity).ok().flatten() else {
        return false;
    };
    group_of(world, action).contains(&other)
}

/// Ends the paired action `entity` is in, on every participant now, and returns the cast as
/// `(role, entity)` sorted by role, so the game can crossfade each to the clip it wants.
///
/// # Errors
/// `L4375` when `entity` is not in an action.
///
/// # Example
/// ```no_run
/// # fn demo(world: &mut ludion_ecs::World, guard: ludion_core::ids::EntityId) {
/// if let Ok(cast) = ludion_engine::abort_paired(world, guard) {
///     for (role, entity) in cast {
///         println!("{role} ({entity:?}) is free");
///     }
/// }
/// # }
/// ```
pub fn abort_paired(
    world: &mut World,
    entity: EntityId,
) -> Result<Vec<(String, EntityId)>, Diagnostic> {
    let Some(action) = world
        .get_unrecorded::<PairedAction>(entity)
        .ok()
        .flatten()
        .cloned()
    else {
        return Err(Diagnostic::new(
            code::PAIRED_NOT_PLAYING,
            format!("{entity:?} is not in a paired action"),
        )
        .with_help("check for a `PairedAction` before aborting"));
    };
    let members = group_of(world, &action);
    let mut cast: Vec<(String, EntityId)> = members
        .iter()
        .filter_map(|id| {
            let role = world.get_unrecorded::<PairedAction>(*id).ok().flatten()?;
            Some((role.role.clone(), *id))
        })
        .collect();
    cast.sort();
    for id in members {
        end_role(world, id);
    }
    Ok(cast)
}

/// Seconds of slack for comparing a tick-derived clip time with an authored one.
const EPSILON: f64 = 1e-9;

fn smooth(u: f64) -> f64 {
    u * u * (3.0 - 2.0 * u)
}

/// This tick's step of `entity`'s warp onto its aligned pose, or `None` off an action.
///
/// The warp is closed-loop: each tick closes the share of the remaining error that a smoothstep over
/// `align.ticks` closes between the previous and this gameplay instant, toward the aligned pose
/// recomputed from both current `Transform`s, so a partner that moves is followed. After the warp the
/// mover holds the relative pose. A role that does not move, a partner that is gone and a held
/// frame give a zero step. Feed the result to `move_character` and `turn_character` each tick, before
/// [`paired_update`].
///
/// # Example
/// ```no_run
/// use ludion_ecs::{SystemCtx, World};
/// use ludion_engine::{move_character, paired_motion, turn_character};
/// fn drive(world: &mut World, ctx: &SystemCtx, entity: ludion_core::ids::EntityId) {
///     if let Some(step) = paired_motion(world, entity, ctx) {
///         let _ = move_character(world, entity, step.displacement);
///         let _ = turn_character(world, entity, step.yaw);
///     }
/// }
/// ```
pub fn paired_motion(world: &World, entity: EntityId, ctx: &SystemCtx) -> Option<PairedMotion> {
    let action = world
        .get_unrecorded::<PairedAction>(entity)
        .ok()
        .flatten()?;
    let here = world.get_unrecorded::<Transform>(entity).ok().flatten()?;
    let yaw_now = heading(here.rotation);
    let hold = PairedMotion {
        displacement: [0.0; 3],
        yaw: yaw_now as f32,
        warping: false,
    };
    let Some(partner) = group_of(world, action).into_iter().find(|id| *id != entity) else {
        return Some(hold);
    };
    let Some(other) = world.get_unrecorded::<PairedAction>(partner).ok().flatten() else {
        return Some(hold);
    };
    let Some(there) = world.get_unrecorded::<Transform>(partner).ok().flatten() else {
        return Some(hold);
    };
    let Some(report) = world
        .resource::<PairedLibrary>()
        .and_then(|library| library.report(&action.document).ok())
    else {
        return Some(hold);
    };
    let doc = &report.doc;
    if doc.roles.len() != 2 {
        return Some(hold);
    }
    let pose = |transform: &Transform| {
        (
            transform.translation.map(f64::from),
            heading(transform.rotation),
        )
    };
    let (feet_a, yaw_a) = pose(here);
    let (feet_b, yaw_b) = pose(there);
    let poses = [
        (action.role.as_str(), feet_a, yaw_a),
        (other.role.as_str(), feet_b, yaw_b),
    ];
    if !doc
        .roles
        .iter()
        .all(|role| poses.iter().any(|(name, _, _)| *name == role.name))
    {
        return Some(hold);
    }
    let targets = aligned_targets(doc, &poses);
    let Some(target) = targets.iter().find(|target| target.role == action.role) else {
        return Some(hold);
    };
    if !target.moves {
        return Some(hold);
    }

    let ticks = doc.align.ticks as f64;
    let elapsed = ctx.game_instant.ticks_since(action.started);
    let x = (elapsed / ticks).clamp(0.0, 1.0);
    let xp = ((elapsed - f64::from(ctx.game_scale())) / ticks).clamp(0.0, 1.0);
    let f = if ctx.game_scale() <= 0.0 {
        0.0
    } else if smooth(xp) >= 1.0 {
        1.0
    } else {
        (smooth(x) - smooth(xp)) / (1.0 - smooth(xp))
    };
    let displacement = [
        ((target.feet[0] - feet_a[0]) * f) as f32,
        0.0,
        ((target.feet[2] - feet_a[2]) * f) as f32,
    ];
    let yaw = yaw_a + wrap(target.yaw - yaw_a) * f;
    Some(PairedMotion {
        displacement,
        yaw: yaw as f32,
        warping: xp < 1.0,
    })
}

/// Fires the document events of every playing paired action, ends the ones whose clips are done, and
/// aborts the ones a game write took out of lockstep.
///
/// **Register it after [`animation_events`](crate::animation_events) and before any reader of
/// [`AnimationEvents`], or the `paired:` events vanish**: `animation_events` rewrites the resource
/// each tick and this system appends to it. The engine schedules neither; adding a default system
/// would change every game's schedule and traces.
///
/// Events fire once per participant as a [`ClipEvent`] named for the document event under the clip
/// `paired:<action>`, in ascending entity order after the clip events. When the longest clip role
/// ends (or `align.ticks` for a document of `align_only` roles), the remaining events and an implicit
/// `end` fire and every participant's `PairedAction` and `IgnoreCollisions` go on that tick. A
/// participant whose `Animation` or assembly `Mesh` is gone (a ragdoll switch, a despawn) ends its
/// own role only. A changed `clip`, `started` or `speed` aborts the whole action and warns `L4374`.
/// With no `PairedAction` in the world, or on a held frame, nothing is touched.
///
/// # Example
/// ```no_run
/// use ludion_engine::{Engine, animation_events, paired_update};
/// // The order matters: paired_update appends to what animation_events just wrote.
/// let engine = Engine::builder()
///     .system("animation_events", animation_events)
///     .system("paired_update", paired_update)
///     .build()
///     .unwrap();
/// ```
pub fn paired_update(world: &mut World, ctx: &SystemCtx) {
    if !ctx.game_advanced() {
        return;
    }
    let mut groups: BTreeMap<(String, EntityId, u64), Vec<EntityId>> = BTreeMap::new();
    for id in world.entities() {
        if let Ok(Some(action)) = world.get_unrecorded::<PairedAction>(id) {
            groups
                .entry((
                    action.document.clone(),
                    action.reference,
                    action.started.as_u64(),
                ))
                .or_default()
                .push(id);
        }
    }
    if groups.is_empty() {
        return;
    }
    let mut fired: Vec<ClipEvent> = Vec::new();
    for ((document, _, started), members) in groups {
        let started = FrameIndex::new(started);
        let report = match world
            .resource::<PairedLibrary>()
            .map(|library| library.report(&document))
        {
            Some(Ok(report)) => report,
            _ => {
                for id in members {
                    end_role(world, id);
                }
                continue;
            }
        };
        play_group(world, ctx, &report, started, members, &mut fired);
    }
    if fired.is_empty() {
        return;
    }
    if world.resource::<AnimationEvents>().is_none() {
        world.insert_resource(AnimationEvents::default());
    }
    let events = world
        .resource_mut::<AnimationEvents>()
        .expect("inserted above");
    for event in fired {
        events.push(event);
    }
}

fn play_group(
    world: &mut World,
    ctx: &SystemCtx,
    report: &PairedReport,
    started: FrameIndex,
    members: Vec<EntityId>,
    fired: &mut Vec<ClipEvent>,
) {
    let doc = &report.doc;
    let role_name = |world: &World, id: EntityId| {
        world
            .get_unrecorded::<PairedAction>(id)
            .ok()
            .flatten()
            .map(|action| action.role.clone())
    };
    let clip_of = |name: &str| {
        doc.roles
            .iter()
            .find(|role| role.name == name)
            .and_then(|role| role.clip.as_deref())
    };

    // 1. A participant whose body or clip is gone ends its own role.
    let mut live = Vec::new();
    for id in members {
        let Some(role) = role_name(world, id) else {
            continue;
        };
        let assembly = matches!(
            world
                .get_unrecorded::<Mesh>(id)
                .ok()
                .flatten()
                .map(|mesh| &mesh.shape),
            Some(MeshSource::Assembly(_))
        );
        let animated = clip_of(&role).is_none() || world.has::<Animation>(id);
        if assembly && animated {
            live.push((id, role));
        } else {
            end_role(world, id);
        }
    }

    // 2. A game write to a role's clip, start or speed breaks lockstep: the whole action ends.
    let mut broken = false;
    for (id, role) in &live {
        let Some(clip) = clip_of(role) else { continue };
        let Some(animation) = world.get_unrecorded::<Animation>(*id).ok().flatten() else {
            continue;
        };
        if animation.clip != clip || animation.started != started || animation.speed != 1.0 {
            broken = true;
            let key = format!(
                "{}/PairedAction",
                crate::diagnostics::offender_key(world, *id)
            );
            crate::diagnostics::report(
                world,
                code::PAIRED_LOCKSTEP_BROKEN,
                key,
                format!(
                    "{id:?} playing `{}` as `{role}` had its `Animation` changed by a game write, so \
                     the action was aborted",
                    doc.name
                ),
            );
        }
    }
    if broken {
        for (id, _) in live {
            end_role(world, id);
        }
        return;
    }

    // 3. Events, once each per participant, in ascending entity order.
    let clip = format!("paired:{}", doc.name);
    let seconds = crate::extract::clock_elapsed(ctx.game_instant, started, 1.0, None);
    let longest = report
        .roles
        .iter()
        .filter_map(|role| role.duration)
        .fold(None, |best: Option<f64>, d| {
            Some(best.map_or(d, |b| b.max(d)))
        });
    let end = longest
        .unwrap_or_else(|| doc.align.ticks as f64 * ctx.game_instant.rate().seconds_per_tick());
    let ended = seconds + EPSILON >= end;
    let due = if ended {
        doc.events.len()
    } else {
        doc.events
            .iter()
            .take_while(|event| event.t <= seconds + EPSILON)
            .count()
    };
    for (id, _) in &live {
        let Ok(Some(action)) = world.get_mut::<PairedAction>(*id) else {
            continue;
        };
        let from = action.fired as usize;
        for event in doc.events.iter().take(due).skip(from) {
            fired.push(ClipEvent {
                entity: *id,
                clip: clip.clone(),
                name: event.name.clone(),
                layer: String::new(),
            });
        }
        action.fired = action.fired.max(due as u32);
    }

    // 4. The clip end ends every participant on the same tick.
    if ended {
        for (id, _) in &live {
            fired.push(ClipEvent {
                entity: *id,
                clip: clip.clone(),
                name: "end".to_owned(),
                layer: String::new(),
            });
        }
        for (id, _) in live {
            end_role(world, id);
        }
    }
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
