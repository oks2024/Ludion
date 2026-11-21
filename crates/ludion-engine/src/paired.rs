//! `paired_fit`: whether a paired `kind: paired` action can start from where its cast stands.
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
    CharacterMotor, CharacterMotorState, IgnoreCollisions, MAX_PARTNERS, MotorError,
    character_fit_ignoring,
};
use ludion_scene::{PairedDocument, PairedMoves};

use crate::animation_events::{AnimationEvents, ClipEvent};
use crate::components::{Animation, Mesh, MeshSource, Transform};
use crate::motor::set_character_posture;
use crate::paired_doc::{PairedReport, check_paired_files};
use crate::root_motion::{RootMotion, root_motion_of};

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

/// The heading of the entity's `-Z` axis for Euler XYZ degrees, in radians: decision 10's convention.
pub(crate) fn heading(rotation_deg: [f32; 3]) -> f64 {
    let [x, y, z, w] = euler_xyz_degrees_to_quat(rotation_deg).map(f64::from);
    f64s::atan2(2.0 * (x * z + w * y), w * w - x * x - y * y + z * z)
}

/// A role's place in the frame rule: its offset in the reference's frame, `(translation, yaw)`; the
/// reference itself sits at the origin.
fn offset_of(doc: &PairedDocument, role: &str) -> ([f64; 3], f64) {
    doc.align
        .offsets
        .iter()
        .find(|offset| offset.role == role)
        .map_or(([0.0; 3], 0.0), |offset| {
            (offset.translation, offset.yaw_deg.to_radians())
        })
}

/// The mean pose the `voters` imply for the frame (decision 34): voter `i` implies yaw `y_i - psi_i`
/// and feet `p_i - rot(y*, t_i)`. The anchor is the first voter's implied yaw, so a one-voter mean is
/// exact. `None` without voters.
fn mean_frame(doc: &PairedDocument, voters: &[(&str, [f64; 3], f64)]) -> Option<([f64; 3], f64)> {
    let implied: Vec<([f64; 3], f64, [f64; 3])> = voters
        .iter()
        .map(|(name, feet, yaw)| {
            let (t, psi) = offset_of(doc, name);
            (*feet, yaw - psi, t)
        })
        .collect();
    let (_, anchor, _) = *implied.first()?;
    let n = implied.len() as f64;
    let yaw = anchor
        + implied
            .iter()
            .map(|(_, y, _)| wrap(y - anchor))
            .sum::<f64>()
            / n;
    let (mut x, mut z) = (0.0, 0.0);
    for (feet, _, t) in &implied {
        let at = rot(yaw, *t);
        x += feet[0] - at[0];
        z += feet[2] - at[2];
    }
    Some(([x / n, 0.0, z / n], yaw))
}

/// The aligned targets of a paired document: `poses` is the current `(feet, yaw)` of every role that
/// is present (all of them for a start, fewer once a role has left) and the result is sorted by role
/// name. Decision 34 states the three frame cases; feet height is never changed. A role that does
/// not move, or whose frame has no voter left, holds its pose.
pub(crate) fn aligned_targets(
    doc: &PairedDocument,
    poses: &[(&str, [f64; 3], f64)],
) -> Vec<Target> {
    let reference = doc.align.reference.as_str();
    let mut present: Vec<(&str, [f64; 3], f64)> = poses.to_vec();
    present.sort_by(|a, b| (a.0 != reference, a.0).cmp(&(b.0 != reference, b.0)));
    let non_reference = || present.iter().filter(|(name, _, _)| *name != reference);

    // The frame, and the roles that go to it (`None`: every role holds).
    let (frame, goes): (Option<([f64; 3], f64)>, Vec<&str>) = match &doc.align.moves {
        PairedMoves::Role(mover) if mover != reference => {
            let frame = present
                .iter()
                .find(|(name, _, _)| *name == reference)
                .map(|(_, feet, yaw)| (*feet, *yaw));
            (frame, vec![mover.as_str()])
        }
        PairedMoves::Role(_) => {
            let voters: Vec<_> = non_reference().copied().collect();
            (mean_frame(doc, &voters), vec![reference])
        }
        PairedMoves::Split => (
            mean_frame(doc, &present),
            present.iter().map(|(name, _, _)| *name).collect(),
        ),
    };

    let mut out: Vec<Target> = present
        .iter()
        .map(|&(name, feet, yaw)| {
            let place = frame.filter(|_| goes.contains(&name)).map(|(p, y)| {
                if name == reference {
                    (p, y)
                } else {
                    let (t, psi) = offset_of(doc, name);
                    (plus(p, rot(y, t)), y + psi)
                }
            });
            target(
                name,
                feet,
                yaw,
                place.unwrap_or((feet, yaw)),
                place.is_some(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.role.cmp(&b.role));
    out
}

fn with_height(planar: [f64; 3], keep: [f64; 3]) -> [f64; 3] {
    [planar[0], keep[1], planar[2]]
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
    if doc.roles.len() > MAX_PARTNERS + 1 {
        return Err(vec![
            Diagnostic::new(
                code::PAIRED_UNSUPPORTED,
                format!(
                    "paired document `{}` has {} roles; the runtime plays at most {}",
                    doc.name,
                    doc.roles.len(),
                    MAX_PARTNERS + 1
                ),
            )
            .with_path("/roles")
            .with_help(
                "a participant's motor looks through at most 4 partners; split the action into smaller ones",
            ),
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

/// Whether a paired action can start from where `cast` stands.
///
/// `paired` is the document's content-root path (`voxels/choke.paired.yaml`); `cast` names an entity
/// for every role. Each role is moved to its aligned pose (decisions 24, 34), and a role with a
/// `CharacterMotor` is fit-tested there at its role `posture`, looking through every other cast
/// entity. Nothing is mutated.
///
/// # Errors
/// `L4370` when the document does not load (the loader's diagnostics follow); `L4373` for more than five
/// roles; `L4371` for a cast that
/// is not exactly the document's roles or whose entity cannot play its role; `L4372` for a scale that is not the role's. All cast problems are collected.
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
        let others: Vec<EntityId> = entities
            .iter()
            .map(|(_, other, _)| *other)
            .filter(|other| other != entity)
            .collect();
        let at = &fit.roles[index];
        match character_fit_ignoring::<Transform>(
            world,
            *entity,
            role.posture.as_deref(),
            at.feet,
            at.yaw,
            Some(&others),
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

/// Ends `entity`'s role now: its `PairedAction` and `IgnoreCollisions` go, and it leaves every group
/// member's `IgnoreCollisions` list (an emptied component goes too). The others keep playing.
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
            .is_some_and(|ignore| ignore.with.contains(&entity));
        if !looks_through {
            continue;
        }
        let emptied = world
            .get_mut::<IgnoreCollisions>(partner)
            .ok()
            .flatten()
            .is_some_and(|ignore| {
                ignore.with.retain(|other| *other != entity);
                ignore.with.is_empty()
            });
        if emptied {
            let _ = world.remove::<IgnoreCollisions>(partner);
        }
    }
    let _ = world.remove::<PairedAction>(entity);
    let _ = world.remove::<IgnoreCollisions>(entity);
}

/// Starts a paired action: every role is stamped, the movers' postures set, the participants
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
        let Some(ignore) = world
            .get_unrecorded::<IgnoreCollisions>(*entity)
            .ok()
            .flatten()
        else {
            continue;
        };
        let foreign = ignore.with.iter().find(|with| {
            !resolved.entities.iter().any(|(_, other, _)| other == *with)
                && !in_action_with(world, *entity, **with)
        });
        let Some(foreign) = foreign else {
            continue;
        };
        errors.push(invalid(
            name,
            format!("role `{name}`: {entity:?} already ignores {foreign:?}; remove it first"),
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
        let mut others: Vec<EntityId> = resolved
            .entities
            .iter()
            .map(|(_, other, _)| *other)
            .filter(|other| other != entity)
            .collect();
        others.sort_unstable();
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
        let _ = world.insert(*entity, IgnoreCollisions { with: others });
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
/// `(role, entity)` sorted by role. `to` is `(role, clip)` pairs: each named role's entity crossfades
/// to its clip over `fade` seconds (`0` cuts) at `game_time`, the gameplay time of this tick; an empty
/// `to` only ends the action and leaves the animation to the game.
///
/// # Errors
/// `L4375` when `entity` is not in an action; `L4371` for a `to` role the action does not have, or
/// one listed twice. Nothing is mutated then.
///
/// # Example
/// ```no_run
/// # fn demo(world: &mut ludion_ecs::World, guard: ludion_core::ids::EntityId, now: ludion_core::time::GameTime) {
/// if let Ok(cast) = ludion_engine::abort_paired(world, guard, &[("target", "stagger")], 0.1, now) {
///     for (role, entity) in cast {
///         println!("{role} ({entity:?}) is free");
///     }
/// }
/// # }
/// ```
pub fn abort_paired(
    world: &mut World,
    entity: EntityId,
    to: &[(&str, &str)],
    fade: f32,
    game_time: impl Into<GameTime>,
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
    let mut named: Vec<&str> = Vec::new();
    for (role, _) in to {
        if !cast.iter().any(|(name, _)| name == role) {
            let help = match suggest::nearest(role, cast.iter().map(|(name, _)| name.as_str())) {
                Some(candidate) => format!("nearest role is `{candidate}`"),
                None => format!(
                    "the action's roles are {}",
                    cast.iter()
                        .map(|(name, _)| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            return Err(invalid(
                role,
                format!("`{}` has no role `{role}` to crossfade", action.action),
                help,
            ));
        }
        if named.contains(role) {
            return Err(invalid(
                role,
                format!("role `{role}` is listed twice in `to`"),
                "name each role once",
            ));
        }
        named.push(role);
    }
    for id in members {
        end_role(world, id);
    }
    let started = FrameIndex::new(game_time.into().ticks_floor());
    for (role, clip) in to {
        let Some((_, member)) = cast.iter().find(|(name, _)| name == role) else {
            continue;
        };
        if !world.has::<Animation>(*member) {
            let _ = world.insert(*member, Animation::default());
        }
        if let Ok(Some(animation)) = world.get_mut::<Animation>(*member) {
            crossfade(animation, clip, fade, started);
        }
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
/// recomputed from every present participant's current `Transform` (decision 34), so a partner that
/// moves is followed. After the warp the mover holds the relative pose.
///
/// **Root motion** (decision 35): a role whose clip carries a `$root` track (0007) is driven by it
/// whenever it is not warping: a role that does not move from the first tick, a mover from the tick
/// after its warp completes (travel inside the warp ticks is not applied). Without a `$root` track a
/// mover holds the relative pose, and a role that does not move, a frame with no participant to
/// follow and a held frame give a zero step. **Do not also apply [`root_motion_delta`](crate::root_motion_delta)
/// to a paired entity**: this is its only source of motion. Feed the result to `move_character` and
/// `turn_character` each tick, before [`paired_update`].
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
    let Some(report) = world
        .resource::<PairedLibrary>()
        .and_then(|library| library.report(&action.document).ok())
    else {
        return Some(hold);
    };
    let doc = &report.doc;
    let mut poses: Vec<(&str, [f64; 3], f64)> = Vec::new();
    for member in group_of(world, action) {
        let role = world.get_unrecorded::<PairedAction>(member).ok().flatten();
        let transform = world.get_unrecorded::<Transform>(member).ok().flatten();
        if let (Some(role), Some(transform)) = (role, transform) {
            poses.push((
                role.role.as_str(),
                transform.translation.map(f64::from),
                heading(transform.rotation),
            ));
        }
    }
    let feet_a = here.translation.map(f64::from);
    let yaw_a = yaw_now;
    let targets = aligned_targets(doc, &poses);
    let target = targets
        .iter()
        .find(|target| target.role == action.role)
        .filter(|target| target.moves);

    let ticks = doc.align.ticks as f64;
    let elapsed = ctx.game_instant.ticks_since(action.started);
    let x = (elapsed / ticks).clamp(0.0, 1.0);
    let xp = ((elapsed - f64::from(ctx.game_scale())) / ticks).clamp(0.0, 1.0);
    let warping = target.is_some() && xp < 1.0;
    if let Some(root) = root_motion(world, entity, here, ctx).filter(|_| !warping) {
        return Some(PairedMotion {
            displacement: root.translation,
            yaw: (yaw_now + f64::from(root.yaw)) as f32,
            warping: false,
        });
    }
    let Some(target) = target else {
        return Some(hold);
    };
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
        warping,
    })
}

/// `entity`'s `$root` travel this tick, read unrecorded like the rest of [`paired_motion`] so a game's
/// driver needs no query declarations.
fn root_motion(
    world: &World,
    entity: EntityId,
    here: &Transform,
    ctx: &SystemCtx,
) -> Option<RootMotion> {
    let animation = world.get_unrecorded::<Animation>(entity).ok().flatten()?;
    let mesh = world.get_unrecorded::<Mesh>(entity).ok().flatten()?;
    let MeshSource::Assembly(rel) = &mesh.shape else {
        return None;
    };
    root_motion_of(world, animation, rel, here, ctx)
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

    // A despawned participant leaves no `PairedAction` to end, so its partners stop looking through it
    // here; the component is written only when an entry is dead.
    for (id, _) in &live {
        let dead: Vec<EntityId> = world
            .get_unrecorded::<IgnoreCollisions>(*id)
            .ok()
            .flatten()
            .map(|ignore| {
                ignore
                    .with
                    .iter()
                    .copied()
                    .filter(|other| !world.is_alive(*other))
                    .collect()
            })
            .unwrap_or_default();
        if dead.is_empty() {
            continue;
        }
        let emptied = world
            .get_mut::<IgnoreCollisions>(*id)
            .ok()
            .flatten()
            .is_some_and(|ignore| {
                ignore.with.retain(|other| !dead.contains(other));
                ignore.with.is_empty()
            });
        if emptied {
            let _ = world.remove::<IgnoreCollisions>(*id);
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
