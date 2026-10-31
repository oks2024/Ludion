//! Divergence bisection — turning "the hashes differ" into "*this* entity's *this* component, left
//! by *this* system, at *this* frame".
//!
//! A `WorldHash` sequence is the engine's verification oracle, and on its own it localizes nothing: a
//! mismatch reports a frame index and stops. This module is the other half — two tools that share one
//! world-diff core:
//!
//! 1. **[`verify_replay_keyframes`]** re-drives a committed `.lrp` with *today's* code and compares
//!    the live state against every keyframe the recording embedded, reporting the first that differs.
//!    Green on unchanged code; after a deliberate gameplay change it names exactly what drifted, so a
//!    re-record is an inspected change rather than a rubber stamp.
//! 2. **[`bisect_divergence`]** takes two run configurations (a builder plus an ambient scope — a
//!    thread count, a feature flag, anything), finds the first frame whose `WorldHash` differs, then
//!    re-drives *that one frame* under each configuration with a per-system probe, naming the system
//!    after which the two worlds part company and diffing them component by component there.
//!
//! Both are single-threaded and available today; the second is the instrument the parallel-sim gate
//! will reach for on its first divergence, built before the incident rather than during it.
//!
//! # Keyframe alignment
//!
//! The fiddly part of (1) is knowing *when* live state is supposed to equal a keyframe. It is not a
//! frame-number match: a recording writes its keyframe immediately after the delta record that
//! crossed the interval, and a free-run recording batches ticks, so the keyframe's frame is wherever
//! the batch ended. **Record order is the alignment**: the keyframe sits between two delta records in
//! the file, and the state it captured is exactly the state after the earlier one. So keyframes ride
//! the drive as `ReplayAction::Keyframe` (a crate-private replay action) — inert for a plain replay,
//! a checkpoint for this one — and the comparison happens where the record sits.
//!
//! # Example
//! ```no_run
//! use ludion_engine::{verify_replay_keyframes, Engine};
//!
//! let bytes = std::fs::read("run.lrp").unwrap();
//! // A game passes its own `Engine::builder()…` here.
//! let report = verify_replay_keyframes(Engine::builder(), &bytes, ".").unwrap();
//! if !report.is_clean() {
//!     eprintln!("{report}");
//! }
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::{Registry, World};
use ludion_sim::WorldHash;
use serde_json::Value;

use crate::builder::EngineBuilder;
use crate::engine::Engine;
use crate::replay::{
    DriveHook, PlainDrive, ReplayAction, drive_actions_with, prepare_replay,
    require_monotonic_frames, total_frames,
};

/// How many differing entities a report renders in full before summarizing the rest. A divergence
/// that touches thousands of entities is one fact, not thousands; the first few name the shape.
pub const ENTITY_DETAIL_CAP: usize = 8;

/// How wide a single component value is rendered before it is elided.
const VALUE_WIDTH: usize = 120;

/// The per-system probe an [`Engine`] tick calls: the system's name, the world as that system left
/// it, and the registry needed to read that world.
pub(crate) type SystemProbe<'a> = dyn FnMut(&'static str, &World, &Registry) + 'a;

// --- The world-diff core ------------------------------------------------------------------------

/// A world reduced to the comparable shape both sides of every diff normalize to: entity →
/// component name → value. Ascending entity order and sorted component names, so a diff is stable
/// and a rendered report is reproducible.
type WorldDoc = BTreeMap<EntityId, BTreeMap<String, Value>>;

/// One component's difference between two states. Either side may be `None` (the entity has the
/// component on one side only).
#[derive(Debug, Clone)]
pub struct ComponentDiff {
    /// The registry component name.
    pub component: String,
    /// The value on the left side (the keyframe, or the first configuration).
    pub left: Option<Value>,
    /// The value on the right side (the live run, or the second configuration).
    pub right: Option<Value>,
    /// The differing top-level field names, when both sides are JSON objects. Empty when a side is
    /// absent or the values are not objects — the whole value is the difference then.
    pub fields: Vec<String>,
}

/// Every component difference on one entity, ascending by component name.
#[derive(Debug, Clone)]
pub struct EntityDiff {
    /// The entity, identical on both sides (same slot, same generation).
    pub entity: EntityId,
    /// The components that differ.
    pub components: Vec<ComponentDiff>,
}

/// Two worlds compared: which entities only one side has, and — bounded by [`ENTITY_DETAIL_CAP`] —
/// which components differ on the entities both sides have.
#[derive(Debug, Clone, Default)]
pub struct WorldDiff {
    /// Entities the **left** side has and the right does not, ascending.
    pub missing: Vec<EntityId>,
    /// Entities the **right** side has and the left does not, ascending.
    pub spawned: Vec<EntityId>,
    /// Per-entity component differences, ascending, truncated to [`ENTITY_DETAIL_CAP`] entries.
    pub changed: Vec<EntityDiff>,
    /// How many entities differ in total, including the ones the cap elided from `changed`.
    pub changed_total: usize,
}

impl WorldDiff {
    /// Whether the two worlds are identical in entity set and component data.
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.spawned.is_empty() && self.changed_total == 0
    }
}

impl fmt::Display for WorldDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return write!(
                f,
                "  worlds are identical (same entities, same component data)"
            );
        }
        writeln!(
            f,
            "  {} entity(ies) differ, {} only on the left, {} only on the right",
            self.changed_total,
            self.missing.len(),
            self.spawned.len()
        )?;
        for entity in self.missing.iter().take(ENTITY_DETAIL_CAP) {
            writeln!(f, "  - {entity} is absent on the right")?;
        }
        if self.missing.len() > ENTITY_DETAIL_CAP {
            writeln!(
                f,
                "  - … and {} more",
                self.missing.len() - ENTITY_DETAIL_CAP
            )?;
        }
        for entity in self.spawned.iter().take(ENTITY_DETAIL_CAP) {
            writeln!(f, "  + {entity} is absent on the left")?;
        }
        if self.spawned.len() > ENTITY_DETAIL_CAP {
            writeln!(
                f,
                "  + … and {} more",
                self.spawned.len() - ENTITY_DETAIL_CAP
            )?;
        }
        for diff in &self.changed {
            for component in &diff.components {
                write!(
                    f,
                    "  ~ {} {}: {} → {}",
                    diff.entity,
                    component.component,
                    short(component.left.as_ref()),
                    short(component.right.as_ref())
                )?;
                if component.fields.is_empty() {
                    writeln!(f)?;
                } else {
                    writeln!(f, "   (fields: {})", component.fields.join(", "))?;
                }
            }
        }
        if self.changed_total > self.changed.len() {
            writeln!(
                f,
                "  ~ … and {} more differing entity(ies)",
                self.changed_total - self.changed.len()
            )?;
        }
        Ok(())
    }
}

/// Renders a component value for a report, eliding anything past [`VALUE_WIDTH`] characters.
fn short(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "<absent>".to_string();
    };
    let text = value.to_string();
    if text.chars().count() <= VALUE_WIDTH {
        return text;
    }
    let head: String = text.chars().take(VALUE_WIDTH).collect();
    format!("{head}…")
}

/// Reduces a live world to the comparable [`WorldDoc`] shape, walking every live entity in ascending
/// order and every registered component it has — the same walk `snapshot::capture` performs, so a
/// world and a keyframe document reduce to directly comparable values.
///
/// # Errors
/// A component serialization failure (`L0007`).
fn world_doc(world: &World, registry: &Registry) -> Result<WorldDoc, Diagnostic> {
    let mut doc = WorldDoc::new();
    for id in world.entities() {
        let mut components = BTreeMap::new();
        for info in registry.iter() {
            if let Some(value) = info.serialize(world, id)? {
                components.insert(info.name().to_string(), value);
            }
        }
        doc.insert(id, components);
    }
    Ok(doc)
}

/// An owned reduction of a world to its comparable component data — hold one from a first run while
/// a second run executes, then [`diff`](WorldCapture::diff) them.
#[derive(Debug, Clone)]
pub struct WorldCapture(WorldDoc);

impl WorldCapture {
    /// Reduces `world` (read through `registry`) to a capture.
    ///
    /// # Errors
    /// A component serialization failure (`L0007`).
    pub fn capture(world: &World, registry: &Registry) -> Result<Self, Diagnostic> {
        world_doc(world, registry).map(Self)
    }

    /// Compares `self` (the left side) with `right`.
    pub fn diff(&self, right: &WorldCapture) -> WorldDiff {
        diff_docs(&self.0, &right.0)
    }
}

/// Diffs the worlds of two engines, entity by entity and component by component; `left` is the
/// left side of the result. Each engine is reduced with its own registry.
///
/// # Errors
/// A component serialization failure (`L0007`).
///
/// # Example
/// ```
/// use ludion_engine::{Engine, world_diff};
///
/// let a = Engine::builder().build().unwrap();
/// let b = Engine::builder().build().unwrap();
/// assert!(world_diff(&a, &b).unwrap().is_empty());
/// ```
pub fn world_diff(left: &Engine, right: &Engine) -> Result<WorldDiff, Diagnostic> {
    let left = WorldCapture::capture(left.world(), left.registry())?;
    let right = WorldCapture::capture(right.world(), right.registry())?;
    Ok(left.diff(&right))
}

/// Reduces a recorded keyframe (an `engine.snapshot` document) to the same [`WorldDoc`] shape.
///
/// Every component value is **normalized through its registered Rust type** — deserialized and
/// re-serialized — rather than compared as the raw JSON the file holds. The workspace's `serde_json`
/// has `float_roundtrip`, so a parsed number is the `f64` that was written; normalizing still puts
/// both sides in the form the world itself would hold (an `f32` field's rounding, a defaulted
/// field), so a representation difference is not reported as state drift.
///
/// A value that no longer deserializes — an unregistered component, or a real schema change — keeps
/// its raw form and is reported as the genuine drift it is.
///
/// # Errors
/// `L3008` if the document has no `entities` array of the expected shape.
fn keyframe_doc(snapshot: &Value, registry: &Registry) -> Result<WorldDoc, Diagnostic> {
    let mut scratch = World::new();
    let scratch_entity = scratch.spawn();
    let normalize = |name: &str, value: &Value, scratch: &mut World| -> Value {
        let Some(info) = registry.get(name) else {
            return value.clone();
        };
        if info
            .deserialize_insert(value, scratch, scratch_entity)
            .is_err()
        {
            return value.clone();
        }
        match info.serialize(scratch, scratch_entity) {
            Ok(Some(normalized)) => normalized,
            _ => value.clone(),
        }
    };
    let mut doc = raw_keyframe_doc(snapshot)?;
    for components in doc.values_mut() {
        for (name, value) in components.iter_mut() {
            *value = normalize(name, value, &mut scratch);
        }
    }
    Ok(doc)
}

/// The keyframe's entities exactly as the file holds them, before normalization.
///
/// # Errors
/// `L3008` if the document has no `entities` array of the expected shape.
fn raw_keyframe_doc(snapshot: &Value) -> Result<WorldDoc, Diagnostic> {
    let malformed = || {
        Diagnostic::new(
            code::PROTOCOL_SNAPSHOT_INVALID,
            "replay keyframe is not a valid engine snapshot document (no `entities` array)"
                .to_string(),
        )
        .with_help(
            "re-record the replay with this engine build; keyframe formats are version-locked",
        )
    };
    let entities = snapshot
        .get("entities")
        .and_then(Value::as_array)
        .ok_or_else(malformed)?;
    let mut doc = WorldDoc::new();
    for entity in entities {
        let index = entity
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(malformed)?;
        let generation = entity
            .get("generation")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(malformed)?;
        let components = entity
            .get("components")
            .and_then(Value::as_object)
            .ok_or_else(malformed)?;
        doc.insert(
            EntityId::new(index, generation),
            components
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        );
    }
    Ok(doc)
}

/// Compares two worlds in the [`WorldDoc`] shape, bounding the per-entity detail at
/// [`ENTITY_DETAIL_CAP`] while counting every differing entity.
fn diff_docs(left: &WorldDoc, right: &WorldDoc) -> WorldDiff {
    let mut diff = WorldDiff {
        missing: left
            .keys()
            .filter(|id| !right.contains_key(id))
            .copied()
            .collect(),
        spawned: right
            .keys()
            .filter(|id| !left.contains_key(id))
            .copied()
            .collect(),
        ..WorldDiff::default()
    };

    for (entity, left_components) in left {
        let Some(right_components) = right.get(entity) else {
            continue;
        };
        let names: BTreeSet<&String> = left_components
            .keys()
            .chain(right_components.keys())
            .collect();
        let components: Vec<ComponentDiff> = names
            .into_iter()
            .filter_map(|name| {
                let l = left_components.get(name);
                let r = right_components.get(name);
                if l == r {
                    return None;
                }
                Some(ComponentDiff {
                    component: name.clone(),
                    left: l.cloned(),
                    right: r.cloned(),
                    fields: changed_fields(l, r),
                })
            })
            .collect();
        if components.is_empty() {
            continue;
        }
        diff.changed_total += 1;
        if diff.changed.len() < ENTITY_DETAIL_CAP {
            diff.changed.push(EntityDiff {
                entity: *entity,
                components,
            });
        }
    }

    diff
}

/// The top-level field names whose values differ, when both sides are JSON objects. Empty otherwise —
/// a side that is absent, or a non-object value, has no field-level story to tell.
fn changed_fields(left: Option<&Value>, right: Option<&Value>) -> Vec<String> {
    let (Some(Value::Object(left)), Some(Value::Object(right))) = (left, right) else {
        return Vec::new();
    };
    let names: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
    names
        .into_iter()
        .filter(|name| left.get(*name) != right.get(*name))
        .cloned()
        .collect()
}

// --- 1. Keyframe drift ---------------------------------------------------------------------------

/// The first keyframe a replay drive failed to reproduce, and how.
#[derive(Debug, Clone)]
pub struct KeyframeDivergence {
    /// The frame the keyframe was recorded at.
    pub frame: FrameIndex,
    /// The frame the live drive was at when it reached this keyframe record. Equal to `frame` unless
    /// the replay's own stepping has drifted, which is a structural failure rather than a data one.
    pub live_frame: FrameIndex,
    /// The keyframe's world (left) against the live world (right).
    pub world: WorldDiff,
    /// Whether the RNG state differs. The world hash deliberately excludes it, so this is a
    /// divergence that has not surfaced in component data *yet* but certainly will.
    pub rng_differs: bool,
    /// Whether the generational allocator's free list differs — the same "not yet visible in the
    /// hash, but the next spawn will mint a different id" story as `rng_differs`.
    pub allocator_differs: bool,
}

/// The verdict of [`verify_replay_keyframes`]: how many keyframes reproduced, and the first that did
/// not. [`is_clean`](KeyframeDriftReport::is_clean) is the gate.
#[derive(Debug, Clone)]
pub struct KeyframeDriftReport {
    /// The replay's total keyframe count.
    pub keyframes: usize,
    /// How many keyframes the drive reproduced exactly before it stopped.
    pub verified: usize,
    /// The first keyframe that did not reproduce, if any.
    pub divergence: Option<KeyframeDivergence>,
}

impl KeyframeDriftReport {
    /// Whether every embedded keyframe reproduced exactly.
    pub fn is_clean(&self) -> bool {
        self.divergence.is_none()
    }

    /// The report as a `Result`: `Ok(())` when clean, else the `L3024` diagnostic carrying the full
    /// rendered divergence. The shape a test or CLI wants.
    ///
    /// # Errors
    /// `L3024` when any keyframe failed to reproduce.
    pub fn into_result(self) -> Result<(), Diagnostic> {
        if self.is_clean() {
            return Ok(());
        }
        let message = self.to_string();
        Err(Diagnostic::new(code::REPLAY_KEYFRAME_DRIFT, message).with_help(
            "if the gameplay change was intended, re-record the replay so its keyframes match the \
             code that produced them; if it was not, the named component is where the unintended \
             behavior change is",
        ))
    }
}

impl fmt::Display for KeyframeDriftReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(divergence) = &self.divergence else {
            return write!(
                f,
                "keyframe check clean: all {} recorded keyframe(s) reproduced exactly",
                self.keyframes
            );
        };
        writeln!(
            f,
            "replay diverged from its recorded keyframe at frame {} ({} of {} keyframe(s) \
             reproduced first)",
            divergence.frame.as_u64(),
            self.verified,
            self.keyframes
        )?;
        if divergence.live_frame != divergence.frame {
            writeln!(
                f,
                "  the drive was at frame {} when it reached this keyframe — the replay's stepping \
                 itself has drifted, not just its data",
                divergence.live_frame.as_u64()
            )?;
        }
        write!(f, "{}", divergence.world)?;
        if divergence.rng_differs {
            writeln!(
                f,
                "  RNG state differs (invisible to the world hash — the two runs have drawn a \
                 different number of values, or from different streams)"
            )?;
        }
        if divergence.allocator_differs {
            writeln!(
                f,
                "  entity free list differs (invisible to the world hash — the next spawn would \
                 mint a different id)"
            )?;
        }
        Ok(())
    }
}

/// Re-drives the recorded session in `bytes` with `builder`'s engine and checks the live state
/// against every keyframe the recording embedded, reporting the **first** that fails to reproduce.
///
/// This is the single-threaded, useful-today half of divergence bisection, and the answer to
/// replay churn: after an intentional gameplay change a committed `.lrp`'s keyframes are stale, and
/// re-recording it is a rubber stamp unless something says what drifted. This does.
///
/// The comparison covers the world (entity set + every registered component), the RNG state, and the
/// allocator free list. It deliberately does **not** compare run-state, speed, scene name, or the
/// embedded profile text: a free-run recording keyframes while `Running` where a replay steps, so
/// those differ on a perfectly good replay. Presentation entities and their allocator are not
/// compared either: they are unhashed, so a presentation difference is never drift.
///
/// The drive stops at the first divergence — everything after it is downstream of the same cause.
///
/// # Errors
/// Every error [`replay_hashes_with`](crate::replay_hashes_with) can raise, plus `L3008` if a
/// keyframe record is not a valid snapshot document.
pub fn verify_replay_keyframes(
    builder: EngineBuilder,
    bytes: &[u8],
    project_root: impl AsRef<Path>,
) -> Result<KeyframeDriftReport, Diagnostic> {
    let (mut engine, actions) = prepare_replay(builder, bytes, project_root)?;
    let keyframes = actions
        .iter()
        .filter(|action| matches!(action, ReplayAction::Keyframe { .. }))
        .count();

    let mut check = KeyframeCheck {
        verified: 0,
        divergence: None,
    };
    // The keyframes are the oracle, and the verify rides the one drive loop so it steps exactly as a
    // plain replay does.
    drive_actions_with(&mut engine, actions, &mut check)?;

    Ok(KeyframeDriftReport {
        keyframes,
        verified: check.verified,
        divergence: check.divergence,
    })
}

/// The [`DriveHook`] behind [`verify_replay_keyframes`]: compares at each keyframe record and stops
/// the drive at the first that differs.
struct KeyframeCheck {
    verified: usize,
    divergence: Option<KeyframeDivergence>,
}

impl DriveHook for KeyframeCheck {
    fn keyframe(
        &mut self,
        engine: &Engine,
        frame: FrameIndex,
        snapshot: &Value,
    ) -> Result<bool, Diagnostic> {
        let registry = engine.registry();
        let world = diff_docs(
            &keyframe_doc(snapshot, registry)?,
            &world_doc(engine.world(), registry)?,
        );
        let rng_differs = snapshot.get("rng") != serde_json::to_value(engine.rng()?).ok().as_ref();
        let allocator_differs = snapshot.get("allocator").unwrap_or(&Value::Null)
            != &engine
                .world()
                .allocator_state()
                .and_then(|state| serde_json::to_value(state).ok())
                .unwrap_or(Value::Null);
        let live_frame = engine.frame();

        if world.is_empty() && !rng_differs && !allocator_differs && live_frame == frame {
            self.verified += 1;
            return Ok(true);
        }
        self.divergence = Some(KeyframeDivergence {
            frame,
            live_frame,
            world,
            rng_differs,
            allocator_differs,
        });
        Ok(false)
    }
}

// --- 2. Two-run localization ---------------------------------------------------------------------

/// Proof that a [`RunConfig`] scope actually ran the drive it was handed.
///
/// Only the drive body constructs one, and a scope must return one, so a scope that forgets to call
/// its body is a compile error rather than a silently skipped run.
#[derive(Debug)]
pub struct DriveDone(());

/// The ambient scope a [`RunConfig`] drive runs inside: it is handed the drive body and must run it
/// and return its token. `|body| body()` is the identity scope; a thread-count experiment installs
/// its pool around the call, a feature-flag experiment sets and restores its flag.
pub type ScopeFn<'a> = dyn Fn(&mut dyn FnMut() -> DriveDone) -> DriveDone + 'a;

/// One side of a two-run comparison: a name for reports, a factory for the game's [`EngineBuilder`]
/// (building consumes it, and a comparison drives several times), and the ambient scope each drive
/// runs inside.
///
/// The scope is what makes this configuration-agnostic: the bisector never knows whether the two
/// sides differ by thread count, by a feature flag, or by the builder alone.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, RunConfig};
///
/// let single = RunConfig::new("1 thread", Engine::builder);
/// let many = RunConfig::new("8 threads", Engine::builder)
///     .scoped(|body| {
///         // install whatever ambient configuration this side means, then run the drive
///         body()
///     });
/// assert_eq!(single.name(), "1 thread");
/// assert_eq!(many.name(), "8 threads");
/// ```
pub struct RunConfig<'a> {
    name: String,
    builder: Box<dyn FnMut() -> EngineBuilder + 'a>,
    scope: Box<ScopeFn<'a>>,
}

impl<'a> RunConfig<'a> {
    /// A configuration that drives `builder`'s engine with no ambient scope.
    pub fn new(name: impl Into<String>, builder: impl FnMut() -> EngineBuilder + 'a) -> Self {
        Self {
            name: name.into(),
            builder: Box::new(builder),
            scope: Box::new(|body| body()),
        }
    }

    /// Wraps every drive this configuration performs in `scope` — the seam a thread count, a feature
    /// flag, or any other ambient configuration enters through.
    #[must_use]
    pub fn scoped(
        mut self,
        scope: impl Fn(&mut dyn FnMut() -> DriveDone) -> DriveDone + 'a,
    ) -> Self {
        self.scope = Box::new(scope);
        self
    }

    /// This configuration's name, as it appears in reports.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Runs `body` inside this configuration's scope and returns its result.
    fn run<T>(
        &mut self,
        mut body: impl FnMut(EngineBuilder) -> Result<T, Diagnostic>,
    ) -> Result<T, Diagnostic> {
        let Self { builder, scope, .. } = self;
        let mut out = None;
        let _done = scope(&mut || {
            out = Some(body(builder()));
            DriveDone(())
        });
        // Unreachable otherwise: `DriveDone` has no public constructor, so the only way a scope
        // returns one is by running the body above.
        out.expect("a RunConfig scope must run the drive body it is handed")
    }
}

/// The localized cause of a divergence between two configurations.
#[derive(Debug, Clone)]
pub struct BisectReport {
    /// The left configuration's name.
    pub left: String,
    /// The right configuration's name.
    pub right: String,
    /// The first frame whose `WorldHash` differs.
    pub frame: FrameIndex,
    /// The system after which the two worlds first differ within that frame, or `None` when every
    /// system boundary agreed — the divergence then came from outside the schedule (a non-stepping
    /// command applied at that frame, or a frame-boundary drain).
    pub system: Option<String>,
    /// How many system boundaries were compared.
    pub systems_probed: usize,
    /// The two worlds at that point: left is the left configuration's.
    pub world: WorldDiff,
}

impl fmt::Display for BisectReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "`{}` and `{}` diverge at frame {}",
            self.left,
            self.right,
            self.frame.as_u64()
        )?;
        match &self.system {
            Some(system) => writeln!(
                f,
                "  first differing system: `{system}` (of {} probed) — the worlds agree at every \
                 boundary before it",
                self.systems_probed
            )?,
            None => writeln!(
                f,
                "  all {} system boundaries agreed — the divergence entered outside the schedule \
                 (a command applied at this frame, or a frame-boundary drain)",
                self.systems_probed
            )?,
        }
        write!(f, "{}", self.world)
    }
}

/// The first frame at which two `WorldHash` traces disagree, or `None` when they agree over their
/// common prefix.
///
/// Traces are compared position by position, which is how [`drive_hashes`](crate::drive_hashes)
/// produces them (one entry per frame, in ascending order). A pure length difference is not a
/// divergence this reports — the runs agreed for as long as both ran.
pub fn first_divergent_frame(
    left: &[(FrameIndex, WorldHash)],
    right: &[(FrameIndex, WorldHash)],
) -> Option<FrameIndex> {
    left.iter()
        .zip(right)
        .find(|(l, r)| l != r)
        .map(|(l, _)| l.0)
}

/// Drives `bytes` under both configurations, finds the first frame whose `WorldHash` differs, and
/// localizes it to a system and a component diff — the whole tool in one call.
///
/// Returns `Ok(None)` when the two configurations produce identical hash traces (nothing to bisect).
/// Both drives run in this one process, so a configuration can be ambient state the scope installs
/// and removes around each drive.
///
/// # Errors
/// Every error [`replay_hashes_with`](crate::replay_hashes_with) can raise, from either side.
pub fn bisect_divergence(
    mut left: RunConfig<'_>,
    mut right: RunConfig<'_>,
    bytes: &[u8],
    project_root: impl AsRef<Path>,
) -> Result<Option<BisectReport>, Diagnostic> {
    let root = project_root.as_ref();
    let left_trace = left.run(|builder| crate::replay_hashes_with(builder, bytes, root))?;
    let right_trace = right.run(|builder| crate::replay_hashes_with(builder, bytes, root))?;
    let Some(frame) = first_divergent_frame(&left_trace, &right_trace) else {
        return Ok(None);
    };
    localize_divergence(left, right, bytes, root, frame).map(Some)
}

/// Localizes an already-known divergence at `frame`: re-drives both configurations to the frame
/// before it, steps that one frame with a per-system `WorldHash` probe under each, and reports the
/// first system after which the two disagree plus the component diff at that point.
///
/// The entry point for a caller that already holds two traces (say, the parallel-sim gate's
/// thread-count comparison) — pair it with [`first_divergent_frame`].
///
/// Re-driving is deterministic, so this reproduces the run exactly; it re-drives four times (a
/// hash-probing pass and a capture pass per side) rather than holding a snapshot per system
/// boundary, so its memory cost is one world regardless of how many systems the game has. The
/// re-drives skip per-frame hashing, which is what makes them cheap.
///
/// # Errors
/// Every error [`replay_hashes_with`](crate::replay_hashes_with) can raise, plus `L3021` if the
/// replay never reaches `frame` or resets its frame numbering (a `scene.load` after a tick, an
/// `engine.restore`).
pub fn localize_divergence(
    mut left: RunConfig<'_>,
    mut right: RunConfig<'_>,
    bytes: &[u8],
    project_root: impl AsRef<Path>,
    frame: FrameIndex,
) -> Result<BisectReport, Diagnostic> {
    let root = project_root.as_ref();

    // Frame 0 is the loaded world before any tick, so no system produced it: diff the two directly.
    if frame == FrameIndex::ZERO {
        let left_doc = left.run(|builder| initial_doc(builder, bytes, root))?;
        let right_doc = right.run(|builder| initial_doc(builder, bytes, root))?;
        return Ok(BisectReport {
            left: left.name().to_string(),
            right: right.name().to_string(),
            frame,
            system: None,
            systems_probed: 0,
            world: diff_docs(&left_doc, &right_doc),
        });
    }

    let left_systems = left
        .run(|b| probe_frame(b, bytes, root, frame, ProbeMode::Hash))?
        .systems;
    let right_systems = right
        .run(|b| probe_frame(b, bytes, root, frame, ProbeMode::Hash))?
        .systems;

    let index = left_systems
        .iter()
        .zip(&right_systems)
        .position(|(l, r)| l != r);
    let mode = index.map_or(ProbeMode::CaptureEnd, ProbeMode::CaptureAfter);
    let system = index.map(|i| left_systems[i].0.to_string());

    let left_doc = left.run(|b| probe_frame(b, bytes, root, frame, mode))?.doc;
    let right_doc = right.run(|b| probe_frame(b, bytes, root, frame, mode))?.doc;

    Ok(BisectReport {
        left: left.name().to_string(),
        right: right.name().to_string(),
        frame,
        system,
        systems_probed: left_systems.len().min(right_systems.len()),
        world: diff_docs(
            &left_doc.unwrap_or_default(),
            &right_doc.unwrap_or_default(),
        ),
    })
}

/// [`prepare_replay`] for a bisect drive: refuses a replay whose frame numbers are not monotonic.
fn prepare_bisect(
    builder: EngineBuilder,
    bytes: &[u8],
    project_root: &Path,
) -> Result<(Engine, Vec<ReplayAction>), Diagnostic> {
    let (engine, actions) = prepare_replay(builder, bytes, project_root)?;
    require_monotonic_frames("bisect", &actions)?;
    Ok((engine, actions))
}

/// The world the trace's frame-0 hash covers: the loaded scene plus every action recorded at frame 0.
fn initial_doc(
    builder: EngineBuilder,
    bytes: &[u8],
    project_root: &Path,
) -> Result<WorldDoc, Diagnostic> {
    let (mut engine, actions) = prepare_bisect(builder, bytes, project_root)?;
    let mut capture = InitialCapture { doc: None };
    drive_actions_with(&mut engine, actions, &mut capture)?;
    match capture.doc {
        Some(doc) => Ok(doc),
        None => world_doc(engine.world(), engine.registry()),
    }
}

/// Captures the world at the drive's first settled call — frame 0 with all its actions applied — and
/// ends the drive.
struct InitialCapture {
    doc: Option<WorldDoc>,
}

impl DriveHook for InitialCapture {
    fn settled(&mut self, engine: &mut Engine) -> Result<bool, Diagnostic> {
        self.doc = Some(world_doc(engine.world(), engine.registry())?);
        Ok(false)
    }
}

/// What a probed drive does at each system boundary of the target frame.
#[derive(Clone, Copy)]
enum ProbeMode {
    /// Hash the world after every system — the discovery pass that finds the divergent boundary.
    Hash,
    /// Capture the world after the system at this index — the detail pass, once the index is known.
    CaptureAfter(usize),
    /// Capture the world at the end of the frame: no boundary was to blame.
    CaptureEnd,
}

/// What a probed drive produced: the per-system hashes (in [`ProbeMode::Hash`]) or the captured
/// world (in either capture mode).
struct ProbeOutcome {
    systems: Vec<(&'static str, WorldHash)>,
    doc: Option<WorldDoc>,
}

/// Re-drives the replay to the frame before `frame`, then steps that one frame with the schedule
/// probed, per `mode`.
///
/// # Errors
/// Every error a replay drive can raise, plus `L3021` if the replay never reaches `frame` or resets
/// its frame numbering.
fn probe_frame(
    builder: EngineBuilder,
    bytes: &[u8],
    project_root: &Path,
    frame: FrameIndex,
    mode: ProbeMode,
) -> Result<ProbeOutcome, Diagnostic> {
    let (mut engine, actions) = prepare_bisect(builder, bytes, project_root)?;
    let total = total_frames(&actions)?;
    let mut probe = ProbeDrive {
        target: frame,
        mode,
        ran: false,
        systems: Vec::new(),
        doc: None,
        error: None,
    };
    // No per-frame hashing: this drive exists only to reach `frame`, and hashing every tick on the
    // way there is what makes a long replay slow.
    drive_actions_with(&mut engine, actions, &mut probe)?;
    if let Some(error) = probe.error {
        return Err(error);
    }
    if !probe.ran {
        return Err(Diagnostic::new(
            code::REPLAY_FRAME_WINDOW,
            format!(
                "cannot localize a divergence at frame {}: this replay never reaches it (it \
                 records frames 0..={total})",
                frame.as_u64()
            ),
        )
        .with_help(format!("bisect against a frame in 0..={total}")));
    }
    Ok(ProbeOutcome {
        systems: probe.systems,
        doc: probe.doc,
    })
}

/// The [`DriveHook`] behind [`probe_frame`]: every tick is a plain one until the tick that produces
/// the target frame, which runs probed — and ends the drive, since everything after it is irrelevant.
struct ProbeDrive {
    target: FrameIndex,
    mode: ProbeMode,
    ran: bool,
    systems: Vec<(&'static str, WorldHash)>,
    doc: Option<WorldDoc>,
    error: Option<Diagnostic>,
}

impl DriveHook for ProbeDrive {
    fn tick(&mut self, engine: &mut Engine) -> Result<bool, Diagnostic> {
        // The tick that *produces* frame N starts at frame N-1.
        if engine.frame().as_u64() + 1 != self.target.as_u64() {
            return PlainDrive.tick(engine);
        }

        let Self {
            mode,
            systems,
            doc,
            error,
            ..
        } = self;
        let mode = *mode;
        let mut index = 0usize;
        engine.step_ticks_probed(1, &mut |name, world, registry| {
            let result = match mode {
                ProbeMode::Hash => {
                    WorldHash::compute(world, registry).map(|hash| systems.push((name, hash)))
                }
                ProbeMode::CaptureAfter(target) if target == index => {
                    world_doc(world, registry).map(|captured| *doc = Some(captured))
                }
                _ => Ok(()),
            };
            if let Err(failure) = result {
                error.get_or_insert(failure);
            }
            index += 1;
        });
        self.ran = true;
        // What `sim.step` does after its ticks; whether this panic is the recorded reproduction is
        // decided by the drive loop.
        if let Some(panic) = engine.take_system_panic() {
            return Err(panic);
        }
        // Keep driving: the frame's own recorded actions apply before the next settled call, which
        // is where `CaptureEnd` reads the world (the state the trace's frame-N hash covers).
        Ok(true)
    }

    fn settled(&mut self, engine: &mut Engine) -> Result<bool, Diagnostic> {
        if !self.ran {
            return Ok(true);
        }
        if matches!(self.mode, ProbeMode::CaptureEnd) {
            self.doc = Some(world_doc(engine.world(), engine.registry())?);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    //! Hash neutrality is the contract the whole module rests on: a probed tick must leave exactly
    //! what a plain tick leaves, or the bisector would be reporting its own instrumentation. Tested
    //! in-crate because [`Engine::step_ticks_probed`] is `pub(crate)` — the public surface exposes
    //! the *tools*, not the tick primitive.

    use super::*;
    use ludion_protocol::Command;
    use ludion_protocol::command::SimStepParams;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    struct Health {
        hp: f32,
    }

    fn regen(world: &mut World, _ctx: &ludion_ecs::SystemCtx) {
        for (h,) in world.query::<(&mut Health,)>() {
            h.hp += 1.5;
        }
    }

    const SCENE: &str = "\
format: 1
seed: 3
entities:
  - components:
      Health:
        hp: 1.0
  - components:
      Health:
        hp: 4.0
";

    fn engine() -> Engine {
        let mut engine = Engine::builder()
            .components(|r| r.register::<Health>("Health"))
            .system("regen", regen)
            .build()
            .unwrap();
        engine.load_inline_scene(SCENE).unwrap();
        engine
    }

    /// A keyframe value one ULP off the `f64` its `f32` field holds is not a state difference —
    /// the field rounds it straight back — and reporting it as one would make every real `.lrp`
    /// fail (it did, on breakout's committed replay, before values were normalized through their
    /// type, when `serde_json` still parsed floats without `float_roundtrip`).
    #[test]
    fn a_keyframe_value_one_ulp_off_its_f32_is_not_a_divergence() {
        let mut registry = Registry::new();
        registry.register::<Health>("Health").unwrap();

        let exact = 1.5f32 / 7.0;
        let widened = f64::from(exact);
        // One ULP away in `f64` — a value no `f32` can hold, so it cannot be real state drift.
        let drifted = f64::from_bits(widened.to_bits() + 1);
        assert_ne!(widened, drifted);

        let snapshot = serde_json::json!({
            "entities": [{
                "index": 0,
                "generation": 0,
                "components": { "Health": { "hp": drifted } },
            }],
        });

        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Health { hp: exact }).unwrap();
        let live = world_doc(&world, &registry).unwrap();

        assert!(
            diff_docs(&keyframe_doc(&snapshot, &registry).unwrap(), &live).is_empty(),
            "normalizing through the component's own type makes the ULP invisible"
        );
        assert!(
            !diff_docs(&raw_keyframe_doc(&snapshot).unwrap(), &live).is_empty(),
            "…and the raw JSON comparison it replaces would have reported it"
        );
    }

    #[derive(Clone, Default)]
    struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn builder() -> EngineBuilder {
        Engine::builder()
            .components(|r| r.register::<Health>("Health"))
            .system("regen", regen)
    }

    fn set_hp(entity: EntityId, hp: f32) -> Command {
        Command::ComponentSet(ludion_protocol::command::ComponentSetParams {
            entity,
            component: "Health".into(),
            value: serde_json::json!({ "hp": hp }),
        })
    }

    fn step(ticks: u64) -> Command {
        Command::SimStep(SimStepParams { ticks, hash: false })
    }

    /// Records a session with a `component.set` at frame 0 and another at frame 2, after a step.
    fn session_with_sets() -> Vec<u8> {
        let sink = Sink::default();
        let mut live = engine();
        let entity = live.world().entities().next().unwrap();
        live.set_recording(sink.clone()).unwrap();
        for command in [
            set_hp(entity, 100.0),
            step(2),
            set_hp(entity, 500.0),
            step(1),
        ] {
            live.execute(command).unwrap();
        }
        live.stop_recording().unwrap();
        sink.0.lock().unwrap().clone()
    }

    fn first_hp(doc: &WorldDoc) -> f64 {
        doc.values().next().unwrap()["Health"]["hp"]
            .as_f64()
            .unwrap()
    }

    /// The captures are what the trace's per-frame hash covered: the frame's recorded commands are
    /// applied before the capture, not after it.
    #[test]
    fn frame_captures_include_the_actions_recorded_at_that_frame() {
        let bytes = session_with_sets();

        let initial = initial_doc(builder(), &bytes, Path::new(".")).unwrap();
        assert_eq!(first_hp(&initial), 100.0, "the frame-0 set is applied");

        let outcome = probe_frame(
            builder(),
            &bytes,
            Path::new("."),
            FrameIndex::new(2),
            ProbeMode::CaptureEnd,
        )
        .unwrap();
        assert_eq!(
            first_hp(&outcome.doc.unwrap()),
            500.0,
            "the set recorded at frame 2 is applied"
        );
    }

    #[test]
    fn a_frame_past_the_replays_end_is_l3021() {
        let bytes = session_with_sets();
        let error = probe_frame(
            builder(),
            &bytes,
            Path::new("."),
            FrameIndex::new(8),
            ProbeMode::CaptureEnd,
        )
        .err()
        .expect("the replay ends at frame 3");
        assert_eq!(error.code, code::REPLAY_FRAME_WINDOW);
        assert!(error.message.contains("0..=3"), "{}", error.message);
    }

    #[test]
    fn probing_a_tick_does_not_change_the_world_hash() {
        let mut plain = engine();
        let mut probed = engine();
        for tick in 0..8 {
            plain
                .execute(Command::SimStep(SimStepParams {
                    ticks: 1,
                    hash: false,
                }))
                .unwrap();

            let mut seen = Vec::new();
            probed.step_ticks_probed(1, &mut |name, world, registry| {
                seen.push((name, WorldHash::compute(world, registry).unwrap()));
            });

            assert_eq!(seen.len(), 1, "one boundary per registered system");
            assert_eq!(seen[0].0, "regen");
            assert_eq!(
                plain.world_hash().unwrap(),
                probed.world_hash().unwrap(),
                "a probed tick is bit-identical to a plain one (tick {tick})"
            );
            assert_eq!(plain.frame(), probed.frame());
            assert_eq!(
                seen[0].1,
                probed.world_hash().unwrap(),
                "the last boundary's hash is the frame's hash — nothing runs after the last system"
            );
        }
    }
}
