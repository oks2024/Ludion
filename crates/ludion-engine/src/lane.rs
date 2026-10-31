//! Lanes: a `kind: path` document a [`CharacterMotor`](ludion_physics::CharacterMotor) is held to,
//! so a 3D platformer's character runs a corridor.
//!
//! [`LaneConstraint`] names the path, whether the lane is `engaged`, and how it eases in and out;
//! `move_character` honours it. This module also holds the path-level maths the lane is built on:
//! the eased blend ([`advance_blend`], [`smoothstep`]), the lane frame at an arc length
//! ([`frame_at`], `ends: extend` beyond an open end), the near-vertical span scan
//! ([`vertical_spans`]), and the game-facing [`lane_relative`] that turns a stick into a direction
//! along the lane.
//!
//! The lane frame's `forward` is horizontal and `lateral = forward × up` points to its right, both
//! taken from the path's no-roll frame, so a climbing path still gives a flat lane.

use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::number::engine_number;
use ludion_ecs::{SemanticValidate, World};
use ludion_physics::{
    CharacterMotor, LaneEnd, LaneFrame, LaneReport, MotorError, MoveConstraint, MoveReport,
    PhysicsWorld,
};
use ludion_scene::{PathMetric, PathSampler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::components::Transform;
use crate::debug_draw::debug_draw;
use crate::path_follow::PathLibrary;

const CATEGORY: &str = "lane";
/// The lane's path polyline (linear RGBA); its alpha is replaced by the lane's weight.
pub const LANE_PATH: [f32; 4] = [0.2, 0.6, 1.0, 1.0];
/// The cross at the lane frame's point.
pub const LANE_ANCHOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The lane's forward ray from the frame's point.
pub const LANE_FORWARD: [f32; 4] = [0.6, 1.0, 0.2, 1.0];
/// The lane's lateral (right) ray from the frame's point.
pub const LANE_LATERAL: [f32; 4] = [1.0, 0.3, 0.6, 1.0];
/// The capsule-to-anchor line with no lateral error.
pub const LANE_ON: [f32; 4] = [0.2, 0.9, 0.6, 1.0];
/// The capsule-to-anchor line at [`LANE_ERROR_FULL`] metres of lateral error or more.
pub const LANE_OFF: [f32; 4] = [1.0, 0.2, 0.1, 1.0];
/// Lateral error, in metres, at which the error line is fully [`LANE_OFF`].
pub const LANE_ERROR_FULL: f32 = 0.5;
/// Metres between the polyline's samples (capped at 256 segments).
const LANE_DRAW_STEP: f32 = 0.25;
const LANE_ANCHOR_CROSS: f32 = 0.1;
const LANE_RAY_LENGTH: f32 = 0.5;

/// A path span whose horizontal tangent length is under this is near-vertical (≈ 87°): the lane
/// frame there uses the path's carried `right`.
pub(crate) const LANE_VERTICAL_HORIZONTAL: f64 = 0.05;
/// Evenly spaced samples (`+ 1` ends) [`vertical_spans`] reads.
const VERTICAL_SAMPLES: usize = 256;
/// `cos 80°`: below this a screen-right is too close to perpendicular to the lane for the stick's
/// left/right to mean along or against it.
const COS_80: f32 = 0.173_648_18;

/// What a lane does with a position past the end of an open path.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LaneEnds {
    /// The lane continues straight along the last (or first) horizontal direction.
    #[default]
    Extend,
    /// The character stops at the end of the path.
    Stop,
}

/// Holds a [`CharacterMotor`](ludion_physics::CharacterMotor) to a `kind: path` document: the
/// character's displacement is projected onto the path's horizontal direction and a pull keeps it
/// on the centre line, eased by `blend`. An entity needs a `CharacterMotor` for the lane to be read
/// (`L1302`).
///
/// `s` and `blend` are the lane's own state, written by `move_character`; a game writes `engaged`.
/// Changing `path` at runtime must also null `s`: use [`set_path`](Self::set_path).
///
/// # Example
/// ```
/// use ludion_engine::{LaneConstraint, LaneEnds};
///
/// let lane: LaneConstraint = serde_json::from_value(serde_json::json!({
///     "path": "paths/corridor.path.yaml",
///     "engaged": true,
///     "ends": "stop",
/// }))
/// .unwrap();
/// assert!(lane.engaged);
/// assert_eq!(lane.ends, LaneEnds::Stop);
/// assert_eq!(lane.s, None);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct LaneConstraint {
    /// The `kind: path` document, relative to the content root (default `""` — inert). A raw write
    /// must also null `s`; [`set_path`](Self::set_path) does both.
    #[serde(default)]
    #[schemars(with = "ludion_ecs::AssetPathSchema")]
    pub path: String,
    /// Whether the lane holds the character; `blend` eases toward 1 when set and toward 0 when
    /// not (default `false`).
    #[serde(default)]
    pub engaged: bool,
    /// Metres of the lane's centre line to the right of the path, along its lateral (default `0.0`).
    #[serde(default)]
    pub offset: f32,
    /// Seconds of gameplay time for `blend` to rise from 0 to 1; `0` is instant (default `0.2`).
    #[serde(default = "default_engage_time")]
    pub engage_time: f32,
    /// Seconds of gameplay time for `blend` to fall from 1 to 0; `0` is instant (default `0.3`).
    #[serde(default = "default_disengage_time")]
    pub disengage_time: f32,
    /// Metres per second the pull toward the centre line moves the character at full weight
    /// (default `4.0`).
    #[serde(default = "default_pull_speed")]
    pub pull_speed: f32,
    /// Metres of arc length either side of `s` searched for the nearest path point (default `2.0`).
    #[serde(default = "default_search_window")]
    pub search_window: f32,
    /// What happens past the end of an open path (default `extend`).
    #[serde(default)]
    pub ends: LaneEnds,
    /// Metres along the path the character is at; `null` until the lane has found its place, then
    /// rewritten by `move_character`.
    #[serde(default)]
    pub s: Option<f32>,
    /// The eased weight, `0..=1`, written by `move_character` (default `0.0`).
    #[serde(default)]
    pub blend: f32,
}

fn default_engage_time() -> f32 {
    0.2
}

fn default_disengage_time() -> f32 {
    0.3
}

fn default_pull_speed() -> f32 {
    4.0
}

fn default_search_window() -> f32 {
    2.0
}

impl Default for LaneConstraint {
    fn default() -> Self {
        LaneConstraint {
            path: String::new(),
            engaged: false,
            offset: 0.0,
            engage_time: default_engage_time(),
            disengage_time: default_disengage_time(),
            pull_speed: default_pull_speed(),
            search_window: default_search_window(),
            ends: LaneEnds::Extend,
            s: None,
            blend: 0.0,
        }
    }
}

impl LaneConstraint {
    /// Points the lane at another path and forgets where it was on the old one, so the next move
    /// finds its place again (`s = None`).
    pub fn set_path(&mut self, path: impl Into<String>) {
        self.path = path.into();
        self.s = None;
    }
}

/// The first field of `lane` that breaks a range rule, with the rule, in declaration order. The
/// runtime check for a lane built in Rust (`L1300`); the document validator reports every field.
pub(crate) fn invalid_field(lane: &LaneConstraint) -> Option<(&'static str, &'static str)> {
    const NOT_NEGATIVE: &str = "must be a finite number, not negative";
    const FINITE: &str = "must be a finite number";
    let non_negative =
        |field, value: f32| (!value.is_finite() || value < 0.0).then_some((field, NOT_NEGATIVE));
    let finite = |field, value: f32| (!value.is_finite()).then_some((field, FINITE));
    finite("offset", lane.offset)
        .or_else(|| non_negative("engage_time", lane.engage_time))
        .or_else(|| non_negative("disengage_time", lane.disengage_time))
        .or_else(|| non_negative("pull_speed", lane.pull_speed))
        .or_else(|| non_negative("search_window", lane.search_window))
        .or_else(|| lane.s.and_then(|s| finite("s", s)))
        .or_else(|| finite("blend", lane.blend))
        .or_else(|| {
            (!(0.0..=1.0).contains(&lane.blend)).then_some(("blend", "must be between 0 and 1"))
        })
}

impl SemanticValidate for LaneConstraint {
    /// Rejects an out-of-range field (`L1300`, pointer `/<field>`): `engage_time`,
    /// `disengage_time`, `pull_speed` or `search_window` negative or not finite, `offset` or `s`
    /// not finite, `blend` not finite or outside `[0, 1]`. A wrong *shape* is the schema's job.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let number = |field: &str| value.get(field)?.as_f64().map(engine_number);
        let mut found = Vec::new();
        let mut report = |field: &str, number: f64, rule: &str, help: &str| {
            found.push(
                Diagnostic::new(
                    code::LANE_CONSTRAINT_INVALID,
                    format!("`LaneConstraint` `{field}` {number} {rule}"),
                )
                .with_path(format!("/{field}"))
                .with_help(help.to_string()),
            );
        };
        for (field, unit) in [
            ("engage_time", "seconds"),
            ("disengage_time", "seconds"),
            ("pull_speed", "metres per second"),
            ("search_window", "metres"),
        ] {
            if let Some(n) = number(field).filter(|n| !n.is_finite() || *n < 0.0) {
                report(
                    field,
                    n,
                    "is not a finite, non-negative number",
                    &format!("give `{field}` a finite, non-negative number of {unit}"),
                );
            }
        }
        for (field, unit) in [("offset", "metres"), ("s", "metres along the path")] {
            if let Some(n) = number(field).filter(|n| !n.is_finite()) {
                report(
                    field,
                    n,
                    "is not a finite number",
                    &format!("give `{field}` a finite number of {unit}"),
                );
            }
        }
        if let Some(n) = number("blend").filter(|n| !n.is_finite() || !(0.0..=1.0).contains(n)) {
            report(
                "blend",
                n,
                "is not a number from 0 to 1",
                "give `blend` a number from 0 (free) to 1 (held to the lane)",
            );
        }
        found
    }
}

/// The `L1301` diagnostic for a lane whose `path` failed to load, carrying the loader's `causes`.
pub(crate) fn path_unloaded(path: &str, causes: &[Diagnostic]) -> Diagnostic {
    let why = causes
        .iter()
        .map(|cause| cause.message.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    Diagnostic::new(
        code::LANE_PATH_UNLOADED,
        format!("`LaneConstraint` path `{path}` failed to load: {why}"),
    )
    .with_path("/path")
    .with_help("point `path` at an existing `kind: path` document, relative to the content root")
}

/// The `L1303` warning for a lane `path` with near-vertical `spans` (from [`vertical_spans`]).
pub(crate) fn path_vertical(path: &str, spans: &[(f32, f32)]) -> Diagnostic {
    let ranges = spans
        .iter()
        .map(|(first, last)| format!("{first:.2}..{last:.2} m"))
        .collect::<Vec<_>>()
        .join(", ");
    Diagnostic::new(
        code::LANE_PATH_VERTICAL,
        format!("lane path `{path}` is near-vertical over {ranges}; the lane frame there is carried from the neighbouring stretch"),
    )
    .with_path("/path")
    .with_help("tilt the span so it keeps some horizontal travel, or shorten it")
}

/// The smooth weight for a linear `blend`: `b²(3 − 2b)`.
pub(crate) fn smoothstep(blend: f32) -> f32 {
    blend * blend * (3.0 - 2.0 * blend)
}

/// `blend` moved one gameplay tick of `dt` seconds toward 1 (`engaged`, over `engage_time`) or 0
/// (over `disengage_time`), clamped to `[0, 1]`. A time of `0` is instant when `dt > 0`; a `dt` of
/// `0` never moves it.
pub(crate) fn advance_blend(
    blend: f32,
    engaged: bool,
    engage_time: f32,
    disengage_time: f32,
    dt: f32,
) -> f32 {
    if dt <= 0.0 {
        return blend;
    }
    let (target, time) = if engaged {
        (1.0, engage_time)
    } else {
        (0.0, disengage_time)
    };
    if time <= 0.0 {
        return target;
    }
    let step = dt / time;
    let moved = if blend < target {
        (blend + step).min(target)
    } else {
        (blend - step).max(target)
    };
    moved.clamp(0.0, 1.0)
}

/// The horizontal direction of travel for a path frame's `right`: `right` turned a quarter.
fn horizontal_forward(right: [f64; 3]) -> [f64; 3] {
    [right[2], 0.0, -right[0]]
}

/// The lane frame `s` metres along `sampler`, `offset` metres to the right of the path, carrying
/// `weight`.
///
/// `forward` is the path's `right` turned a quarter (so `lateral = forward × up`), horizontal
/// wherever the path is, and carried across a vertical run. A closed path wraps `s`. Beyond an open
/// end the frame is the end's, its point continued straight along `forward` by the overshoot
/// (`ends: extend`); `ends: stop` never passes an out-of-range `s`.
pub(crate) fn frame_at(sampler: &PathSampler, s: f64, offset: f32, weight: f32) -> LaneFrame {
    let length = sampler.length();
    let (at, overshoot) = if sampler.closed() {
        (s.rem_euclid(length), 0.0)
    } else {
        let clamped = s.clamp(0.0, length);
        (clamped, s - clamped)
    };
    let frame = sampler.frame_at(at);
    let right = frame.right;
    let forward = horizontal_forward(right);
    let offset = f64::from(offset);
    #[allow(clippy::cast_possible_truncation)]
    let narrow = |v: [f64; 3]| [v[0] as f32, v[1] as f32, v[2] as f32];
    let point = [
        frame.position[0] + offset * right[0] + overshoot * forward[0],
        frame.position[1] + offset * right[1] + overshoot * forward[1],
        frame.position[2] + offset * right[2] + overshoot * forward[2],
    ];
    #[allow(clippy::cast_possible_truncation)]
    let reported_s = (if sampler.closed() { at } else { s }) as f32;
    LaneFrame {
        s: reported_s,
        point: narrow(point),
        tangent: narrow(frame.tangent),
        forward: narrow(forward),
        lateral: narrow(right),
        weight,
    }
}

/// The path ranges `(s_first, s_last)` whose horizontal tangent length is under
/// [`LANE_VERTICAL_HORIZONTAL`]: [`VERTICAL_SAMPLES`] `+ 1` evenly spaced samples, consecutive
/// near-vertical ones merged into one range.
pub(crate) fn vertical_spans(sampler: &PathSampler) -> Vec<(f32, f32)> {
    let length = sampler.length();
    let mut spans = Vec::new();
    let mut open: Option<(f64, f64)> = None;
    for i in 0..=VERTICAL_SAMPLES {
        #[allow(clippy::cast_precision_loss)]
        let s = length * i as f64 / VERTICAL_SAMPLES as f64;
        let t = sampler.tangent_at(s);
        let vertical = (t[0] * t[0] + t[2] * t[2]).sqrt() < LANE_VERTICAL_HORIZONTAL;
        match (vertical, open) {
            (true, Some((first, _))) => open = Some((first, s)),
            (true, None) => open = Some((s, s)),
            (false, Some(range)) => {
                spans.push(range);
                open = None;
            }
            (false, None) => {}
        }
    }
    spans.extend(open);
    #[allow(clippy::cast_possible_truncation)]
    let narrowed = spans
        .into_iter()
        .map(|(first, last)| (first as f32, last as f32))
        .collect();
    narrowed
}

/// A path's lane centre line, shared: `(position, right)` per sample.
pub(crate) type CentreLine = Arc<[([f32; 3], [f32; 3])]>;

/// The `(position, right)` of `sampler` at the samples a lane draws its centre line through: every
/// [`LANE_DRAW_STEP`] metres, capped at 256 segments.
pub(crate) fn centre_line(sampler: &PathSampler) -> Vec<([f32; 3], [f32; 3])> {
    let length = sampler.length();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let segments = ((length / f64::from(LANE_DRAW_STEP)).ceil() as usize).clamp(1, 256);
    #[allow(clippy::cast_possible_truncation)]
    let narrow = |v: [f64; 3]| [v[0] as f32, v[1] as f32, v[2] as f32];
    (0..=segments)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let s = length * i as f64 / segments as f64;
            let frame = sampler.frame_at(s);
            (narrow(frame.position), narrow(frame.right))
        })
        .collect()
}

/// Metres an open lane's `s` may fall short of an end and still count as at it: the stop clamp lands
/// within float rounding of the end, not exactly on it.
const AT_END_EPS: f64 = 1e-3;

/// The lane fields the hook and the report read, copied out of the component for one call.
struct Pull {
    sampler: Arc<PathSampler>,
    /// The path's centre line, from [`centre_line`]; drawn without sampling.
    centre: CentreLine,
    offset: f32,
    ends: LaneEnds,
    search_window: f32,
    pull_speed: f32,
    weight: f32,
    dt: f32,
}

impl Pull {
    /// The nearest `s` to `p` within the search window grown by `travel`, with `ends: extend`
    /// continuing an open lane straight past either end.
    fn refind(&self, p: [f32; 3], hint: f64, travel: f32) -> f64 {
        let q = p.map(f64::from);
        let window = f64::from(self.search_window + travel);
        let s = self.sampler.nearest_s(q, hint, window, PathMetric::Full);
        if self.sampler.closed() || self.ends != LaneEnds::Extend {
            return s;
        }
        let end = self.sampler.length();
        let beyond = |at: f64| {
            let frame = self.sampler.frame_at(at);
            let forward = horizontal_forward(frame.right);
            (0..3)
                .map(|i| (q[i] - frame.position[i]) * forward[i])
                .sum::<f64>()
        };
        if s <= 0.0 {
            let along = beyond(0.0);
            if along < 0.0 {
                return along;
            }
        } else if s >= end {
            let along = beyond(end);
            if along > 0.0 {
                return end + along;
            }
        }
        s
    }

    fn frame(&self, s: f64) -> LaneFrame {
        frame_at(&self.sampler, s, self.offset, self.weight)
    }
}

/// The `MoveConstraint` that holds the motor to the lane for one `move_character` call.
struct LaneHook {
    pull: Pull,
    /// The lane position the feet are at: the call's start, then wherever the correction found it.
    s: f64,
    /// Where the feet were when the call began; the tick's travel is measured from it.
    anchor: [f32; 3],
    blocked: bool,
}

impl LaneHook {
    fn travel(&self, feet: [f32; 3]) -> f32 {
        norm(sub(feet, self.anchor))
    }
}

impl MoveConstraint for LaneHook {
    fn project(&mut self, feet: [f32; 3], step: [f32; 3]) -> [f32; 3] {
        let frame = self.pull.frame(self.s);
        let w = self.pull.weight;
        let across = dot(step, frame.lateral);
        let mut out = [
            step[0] - w * across * frame.lateral[0],
            step[1],
            step[2] - w * across * frame.lateral[2],
        ];
        if !self.pull.sampler.closed() && self.pull.ends == LaneEnds::Stop {
            #[allow(clippy::cast_possible_truncation)]
            let (end, here) = (
                self.pull.sampler.length() as f32,
                self.s as f32 + dot(sub(feet, frame.point), frame.forward),
            );
            let along = dot(out, frame.forward);
            let held = along.clamp(-here, end - here);
            for (component, forward) in out.iter_mut().zip(frame.forward) {
                *component -= w * (along - held) * forward;
            }
        }
        out
    }

    fn correction(&mut self, feet: [f32; 3]) -> [f32; 3] {
        self.s = self.pull.refind(feet, self.s, self.travel(feet));
        let frame = self.pull.frame(self.s);
        let error = dot(sub(feet, frame.point), frame.lateral);
        let cap = self.pull.weight * self.pull.pull_speed * self.pull.dt;
        if error == 0.0 || cap == 0.0 {
            return [0.0; 3];
        }
        let pull = -error.signum() * error.abs().min(cap);
        frame.lateral.map(|c| c * pull)
    }

    fn corrected(&mut self, _applied: [f32; 3], blocked: bool) {
        self.blocked = blocked;
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn norm(v: [f32; 3]) -> f32 {
    dot(v, v).sqrt()
}

/// A lane read from the world with its path loaded.
struct Resolved {
    lane: LaneConstraint,
    sampler: Arc<PathSampler>,
    centre: CentreLine,
}

/// Reads `entity`'s lane and loads its path; `None` when the lane is inert (absent, no path, fully
/// off, a field out of range, or a path that fails to load — the last two reported once).
fn resolve(world: &mut World, entity: EntityId) -> Option<Resolved> {
    let lane = world.get::<LaneConstraint>(entity).ok().flatten()?.clone();
    if lane.path.is_empty() || (lane.blend == 0.0 && !lane.engaged) {
        return None;
    }
    if let Some((field, rule)) = invalid_field(&lane) {
        let diagnostic = Diagnostic::new(
            code::LANE_CONSTRAINT_INVALID,
            format!("{entity} has a `LaneConstraint` `{field}` that {rule}, so the lane stays off"),
        )
        .with_path(format!("/{field}"))
        .with_help(format!("give `{field}` a value in its range"));
        crate::diagnostics::report_full(
            world,
            format!("{entity}/LaneConstraint/{field}"),
            diagnostic,
        );
        return None;
    }
    let loaded = world.resource_mut::<PathLibrary>().map(|library| {
        library.sampler(&lane.path).map(|sampler| {
            let spans = library.lane_spans(&lane.path).to_vec();
            let centre = library.lane_centre(&lane.path);
            (sampler, spans, centre)
        })
    })?;
    match loaded {
        Ok((sampler, spans, centre)) => {
            if !spans.is_empty() {
                let diagnostic = path_vertical(&lane.path, &spans);
                crate::diagnostics::report_full(
                    world,
                    format!("{entity}/LaneConstraint/vertical"),
                    diagnostic,
                );
            }
            Some(Resolved {
                lane,
                sampler,
                centre,
            })
        }
        Err(causes) => {
            let diagnostic = causes
                .iter()
                .cloned()
                .fold(path_unloaded(&lane.path, &causes), Diagnostic::with_related);
            crate::diagnostics::report_full(
                world,
                format!("{entity}/LaneConstraint/path"),
                diagnostic,
            );
            None
        }
    }
}

/// Moves `entity` under its [`LaneConstraint`]: `None` when the lane is inert, so the caller takes
/// the unconstrained path. Otherwise the motor runs with a hook that projects every horizontal step
/// onto the lane and pulls the feet back to its centre line, the lane's `s` and `blend` are
/// written back and `MoveReport.lane` describes the call.
pub(crate) fn move_on_lane(
    world: &mut World,
    entity: EntityId,
    displacement: [f32; 3],
) -> Option<Result<MoveReport, MotorError>> {
    let Resolved {
        lane,
        sampler,
        centre,
    } = resolve(world, entity)?;
    let dt = world
        .resource::<PhysicsWorld>()
        .map(|physics| physics.gameplay_dt(world))?;
    let feet = world.get::<Transform>(entity).ok().flatten()?.translation;
    let blend = advance_blend(
        lane.blend,
        lane.engaged,
        lane.engage_time,
        lane.disengage_time,
        dt,
    );
    let pull = Pull {
        sampler,
        centre,
        offset: lane.offset,
        ends: lane.ends,
        search_window: lane.search_window,
        pull_speed: lane.pull_speed,
        weight: smoothstep(blend),
        dt,
    };
    let s0 = match lane.s.filter(|_| lane.blend > 0.0) {
        Some(s) => f64::from(s),
        None => pull
            .sampler
            .nearest_s_global(feet.map(f64::from), PathMetric::Full),
    };
    let mut hook = LaneHook {
        pull,
        s: s0,
        anchor: feet,
        blocked: false,
    };
    let result = crate::motor::move_character_with(world, entity, displacement, Some(&mut hook));
    Some(result.map(|mut report| {
        let lane_report = settle(world, entity, &lane, &hook, s0, blend);
        if let Some(physics) = world.resource_mut::<PhysicsWorld>() {
            physics.attach_lane_report(entity, lane_report);
        }
        draw(world, entity, &hook.pull, &lane_report);
        report.lane = Some(lane_report);
        report
    }))
}

/// Draws one lane call's `lane` primitives: the path polyline (alpha = the weight), the frame's
/// anchor cross and forward and lateral rays, and the capsule-to-anchor line coloured by the error.
fn draw(world: &mut World, entity: EntityId, pull: &Pull, report: &LaneReport) {
    let Some(feet) = world
        .get::<Transform>(entity)
        .ok()
        .flatten()
        .map(|pose| pose.translation)
    else {
        return;
    };
    let points: Vec<[f32; 3]> = pull
        .centre
        .iter()
        .map(|(position, right)| [0, 1, 2].map(|axis| position[axis] + pull.offset * right[axis]))
        .collect();
    let mut path_colour = LANE_PATH;
    path_colour[3] = report.weight;
    let frame = &report.frame;
    let scaled = |v: [f32; 3]| v.map(|c| c * LANE_RAY_LENGTH);
    let mut pen = debug_draw(world, CATEGORY);
    pen.path(&points, path_colour)
        .cross(frame.point, LANE_ANCHOR_CROSS, LANE_ANCHOR)
        .line(feet, frame.point, lane_error_colour(report.lateral_error))
        .ray(frame.point, scaled(frame.forward), LANE_FORWARD)
        .ray(frame.point, scaled(frame.lateral), LANE_LATERAL);
}

/// The colour of the capsule-to-lane line for a signed lateral `error`: [`LANE_ON`] on the centre
/// line, [`LANE_OFF`] from [`LANE_ERROR_FULL`] metres out, a linear mix between.
pub fn lane_error_colour(error: f32) -> [f32; 4] {
    let t = (error.abs() / LANE_ERROR_FULL).min(1.0);
    let mix = |i: usize| LANE_ON[i] * (1.0 - t) + LANE_OFF[i] * t;
    [mix(0), mix(1), mix(2), 1.0]
}

/// Writes the lane's `s` and `blend` back after a successful call and describes the call.
fn settle(
    world: &mut World,
    entity: EntityId,
    lane: &LaneConstraint,
    hook: &LaneHook,
    s0: f64,
    blend: f32,
) -> LaneReport {
    let pull = &hook.pull;
    let end = world
        .get::<Transform>(entity)
        .ok()
        .flatten()
        .map_or(hook.anchor, |pose| pose.translation);
    let s1 = pull.refind(end, hook.s, hook.travel(end));
    if let Ok(Some(stored)) = world.get_mut::<LaneConstraint>(entity) {
        #[allow(clippy::cast_possible_truncation)]
        let narrowed = s1 as f32;
        stored.s = Some(narrowed);
        stored.blend = blend;
    }
    let sampler = &pull.sampler;
    let path_length = sampler.length();
    let mut ds = s1 - s0;
    if sampler.closed() {
        ds = ds.rem_euclid(path_length);
        if ds > path_length / 2.0 {
            ds -= path_length;
        }
    }
    let at_end = if sampler.closed() {
        None
    } else if s1 <= AT_END_EPS {
        Some(LaneEnd::Start)
    } else if s1 >= path_length - AT_END_EPS {
        Some(LaneEnd::End)
    } else {
        None
    };
    let frame = pull.frame(s1);
    #[allow(clippy::cast_possible_truncation)]
    let ds = ds as f32;
    LaneReport {
        s: frame.s,
        ds,
        weight: pull.weight,
        engaged: lane.engaged,
        easing: 0.0 < blend && blend < 1.0,
        lateral_error: dot(sub(end, frame.point), frame.lateral),
        pull_blocked: hook.blocked,
        frame,
        at_end,
    }
}

/// The lane frame of `entity` as the last `move_character` left it, for a game system that reads
/// the lane before it moves (a stick turned into [`lane_relative`] input). `None` without a
/// `LaneConstraint`, with no path, before the lane has found its place (`s` is `null`), when the
/// lane is fully off. A path no move has loaded yet is read uncached, so the answer depends only on
/// the world and the project's files, never on what ran before.
///
/// # Example
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::lane_frame;
///
/// let mut world = World::new();
/// let entity = world.spawn();
/// assert_eq!(lane_frame(&world, entity), None);
/// ```
pub fn lane_frame(world: &World, entity: EntityId) -> Option<LaneFrame> {
    let lane = world.get::<LaneConstraint>(entity).ok().flatten()?;
    if lane.path.is_empty() || (lane.blend == 0.0 && !lane.engaged) {
        return None;
    }
    let s = lane.s?;
    let sampler = world.resource::<PathLibrary>()?.peek(&lane.path)?;
    Some(frame_at(
        &sampler,
        f64::from(s),
        lane.offset,
        smoothstep(lane.blend),
    ))
}

/// Reports `L1302` once per [`LaneConstraint`] entity that has no `CharacterMotor`.
pub(crate) fn report_without_motor(world: &mut World) {
    let ids: Vec<EntityId> = world
        .query_ids::<(&LaneConstraint,)>()
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        if world.has::<CharacterMotor>(id) {
            continue;
        }
        crate::diagnostics::report(
            world,
            code::LANE_WITHOUT_MOTOR,
            id.to_string(),
            format!(
                "{id} has a `LaneConstraint` but no `CharacterMotor`, so nothing reads the lane: \
                 add a `CharacterMotor` to the entity"
            ),
        );
    }
}

/// A stick read against a lane: see [`lane_relative`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaneInput {
    /// The horizontal world direction of the stick's along-lane part: `along × forward`.
    pub direction: [f32; 3],
    /// The stick's along-lane part, signed: positive runs along the lane's `forward`.
    pub along: f32,
    /// The stick's across-lane part: `stick[1]`, unsigned by the screen.
    pub across: f32,
}

/// Reads a `[x, y]` stick against a lane frame so the stick's horizontal axis runs the lane in
/// whichever direction the camera sees it: `along = stick[0]` signed by whether the lane's
/// `forward` points to the screen's right (`screen_right`, a world direction) or to its left,
/// `across = stick[1]`, and `direction = along · forward`.
///
/// When `forward` is within 10° of perpendicular to `screen_right` (|`forward · screen_right`| below
/// cos 80°) the lane runs into or out of the screen and left/right cannot tell the ways apart.
/// Below that threshold the helper returns `+forward`: it is pure and has no previous frame to keep
/// the last sign from, so a caller that wants hysteresis holds its own sign.
///
/// # Example
/// ```
/// use ludion_engine::{LaneInput, lane_relative};
/// use ludion_physics::LaneFrame;
///
/// let frame = LaneFrame {
///     s: 0.0,
///     point: [0.0; 3],
///     tangent: [1.0, 0.0, 0.0],
///     forward: [1.0, 0.0, 0.0],
///     lateral: [0.0, 0.0, 1.0],
///     weight: 1.0,
/// };
/// // The camera's right is the lane's forward: pushing right runs forward.
/// let input = lane_relative([1.0, 0.0], &frame, [1.0, 0.0, 0.0]);
/// assert_eq!(input.along, 1.0);
/// // The camera looks back down the lane: pushing right runs against forward.
/// let input = lane_relative([1.0, 0.0], &frame, [-1.0, 0.0, 0.0]);
/// assert_eq!(input.along, -1.0);
/// ```
pub fn lane_relative(stick: [f32; 2], frame: &LaneFrame, screen_right: [f32; 3]) -> LaneInput {
    let f = frame.forward;
    let d = f[0] * screen_right[0] + f[1] * screen_right[1] + f[2] * screen_right[2];
    let sign = if d.abs() >= COS_80 { d.signum() } else { 1.0 };
    let along = stick[0] * sign;
    LaneInput {
        direction: [along * f[0], along * f[1], along * f[2]],
        along,
        across: stick[1],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sampler(closed: bool, points: &[[f32; 3]]) -> PathSampler {
        let mut text = format!(
            "format: 1\nkind: path\nname: lane\ninterpolation: linear\nclosed: {closed}\npoints:\n"
        );
        for p in points {
            text.push_str(&format!(
                "  - - {:?}\n    - {:?}\n    - {:?}\n",
                p[0], p[1], p[2]
            ));
        }
        let doc = ludion_scene::check_path(&text).unwrap_or_else(|e| panic!("{e:?}"));
        PathSampler::new(&doc)
    }

    fn straight() -> PathSampler {
        sampler(false, &[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]])
    }

    fn close(a: [f32; 3], b: [f32; 3]) {
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() < 1e-5, "{a:?} != {b:?}");
        }
    }

    #[test]
    fn a_lane_call_draws_its_path_anchor_axes_and_error_line() {
        let mut world = World::new();
        world.insert_resource(crate::debug_draw::DebugDraw::default());
        let entity = world.spawn();
        let feet = [4.0, 0.0, 0.4];
        world
            .insert(
                entity,
                Transform {
                    translation: feet,
                    ..Transform::default()
                },
            )
            .unwrap();
        let sampler = Arc::new(straight());
        let pull = Pull {
            centre: centre_line(&sampler).into(),
            sampler,
            offset: 0.0,
            ends: LaneEnds::Extend,
            search_window: 2.0,
            pull_speed: 4.0,
            weight: 0.5,
            dt: 1.0 / 60.0,
        };
        let frame = pull.frame(4.0);
        let report = LaneReport {
            s: 4.0,
            ds: 0.0,
            weight: 0.5,
            engaged: true,
            easing: true,
            lateral_error: 0.4,
            pull_blocked: false,
            frame,
            at_end: None,
        };
        draw(&mut world, entity, &pull, &report);
        let drawn = world
            .resource::<crate::debug_draw::DebugDraw>()
            .unwrap()
            .pending(CATEGORY);
        let colour = |c: [f32; 4]| drawn.iter().filter(|s| s.color == c).count();
        let faded = [LANE_PATH[0], LANE_PATH[1], LANE_PATH[2], 0.5];
        assert_eq!(colour(faded), 40, "10 m every 0.25 m, alpha = the weight");
        assert_eq!(colour(LANE_ANCHOR), 3);
        assert_eq!(colour(LANE_FORWARD), 1);
        assert_eq!(colour(LANE_LATERAL), 1);
        let error = lane_error_colour(0.4);
        assert_eq!(colour(error), 1);
        assert_eq!(drawn.len(), 40 + 3 + 1 + 1 + 1);
        let line = drawn.iter().find(|s| s.color == error).unwrap();
        assert_eq!((line.a, line.b), (feet, frame.point));
    }

    #[test]
    fn the_centre_line_samples_position_and_right_every_quarter_metre() {
        let path = straight();
        let centre = centre_line(&path);
        assert_eq!(centre.len(), 41);
        let right = path.frame_at(0.0).right.map(|c| c as f32);
        for (i, (position, r)) in centre.iter().enumerate() {
            close(*position, [0.25 * i as f32, 0.0, 0.0]);
            close(*r, right);
        }
    }

    #[test]
    fn the_error_colour_runs_from_on_to_off() {
        assert_eq!(lane_error_colour(0.0), LANE_ON);
        assert_eq!(lane_error_colour(-LANE_ERROR_FULL), LANE_OFF);
        assert_eq!(lane_error_colour(9.0), LANE_OFF);
    }

    #[test]
    fn defaults_and_serde() {
        let lane: LaneConstraint = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(lane, LaneConstraint::default());
        assert_eq!(lane.engage_time, 0.2);
        assert_eq!(lane.ends, LaneEnds::Extend);

        let full = serde_json::json!({
            "path": "paths/a.path.yaml", "engaged": true, "offset": 0.5, "engage_time": 0.125,
            "disengage_time": 0.5, "pull_speed": 3.0, "search_window": 1.5, "ends": "stop",
            "s": null, "blend": 0.25,
        });
        let lane: LaneConstraint = serde_json::from_value(full.clone()).unwrap();
        assert_eq!(lane.ends, LaneEnds::Stop);
        assert_eq!(serde_json::to_value(&lane).unwrap(), full);
        assert!(serde_json::to_value(LaneConstraint::default()).unwrap()["s"].is_null());
    }

    #[test]
    fn set_path_nulls_s() {
        let mut lane = LaneConstraint {
            s: Some(3.0),
            ..LaneConstraint::default()
        };
        lane.set_path("paths/b.path.yaml");
        assert_eq!(lane.path, "paths/b.path.yaml");
        assert_eq!(lane.s, None);
    }

    #[test]
    fn the_validator_reports_each_rule_at_its_pointer() {
        assert!(LaneConstraint::validate_semantics(&serde_json::json!({})).is_empty());
        for (field, value) in [
            ("engage_time", serde_json::json!(-1.0)),
            ("disengage_time", serde_json::json!(-0.1)),
            ("pull_speed", serde_json::json!(-4.0)),
            ("search_window", serde_json::json!(1e39)),
            ("offset", serde_json::json!(1e39)),
            ("s", serde_json::json!(-1e39)),
            ("blend", serde_json::json!(1.5)),
            ("blend", serde_json::json!(-0.5)),
        ] {
            let found = LaneConstraint::validate_semantics(&serde_json::json!({ field: value }));
            assert_eq!(found.len(), 1, "{field}: {found:?}");
            assert_eq!(found[0].code, code::LANE_CONSTRAINT_INVALID);
            assert_eq!(found[0].path.as_deref(), Some(format!("/{field}").as_str()));
        }
        let ok = serde_json::json!({ "s": null, "offset": -2.0, "blend": 1.0 });
        assert!(LaneConstraint::validate_semantics(&ok).is_empty());
    }

    #[test]
    fn invalid_field_names_the_first_broken_field() {
        assert_eq!(invalid_field(&LaneConstraint::default()), None);
        let lane = LaneConstraint {
            pull_speed: f32::NAN,
            blend: 2.0,
            ..LaneConstraint::default()
        };
        assert_eq!(
            invalid_field(&lane).map(|(field, _)| field),
            Some("pull_speed")
        );
        let lane = LaneConstraint {
            blend: 2.0,
            ..LaneConstraint::default()
        };
        assert_eq!(invalid_field(&lane).map(|(field, _)| field), Some("blend"));
        let lane = LaneConstraint {
            s: Some(f32::INFINITY),
            ..LaneConstraint::default()
        };
        assert_eq!(invalid_field(&lane).map(|(field, _)| field), Some("s"));
    }

    #[test]
    fn the_blend_eases_on_gameplay_time() {
        let dt = 1.0 / 60.0;
        let mut blend = 0.0;
        for _ in 0..12 {
            blend = advance_blend(blend, true, 0.2, 0.3, dt);
        }
        assert!((blend - 1.0).abs() < 1e-6, "{blend}");
        assert_eq!(advance_blend(blend, true, 0.2, 0.3, dt), 1.0);

        let half = advance_blend(advance_blend(0.0, true, 0.2, 0.3, dt), true, 0.2, 0.3, dt);
        let back = advance_blend(half, false, 0.2, 0.3, dt);
        assert!((back - (half - dt / 0.3)).abs() < 1e-6);

        assert_eq!(advance_blend(0.4, true, 0.2, 0.3, 0.0), 0.4);
        assert_eq!(advance_blend(0.4, true, 0.0, 0.3, dt), 1.0);
        assert_eq!(advance_blend(0.4, false, 0.2, 0.0, dt), 0.0);
    }

    #[test]
    fn the_path_diagnostics_carry_their_codes() {
        let cause = Diagnostic::new(code::PATH_DOCUMENT_STRUCTURE, "missing");
        let unloaded = path_unloaded("paths/a.path.yaml", &[cause]);
        assert_eq!(unloaded.code, code::LANE_PATH_UNLOADED);
        assert!(unloaded.message.contains("missing"));
        let vertical = path_vertical("paths/a.path.yaml", &[(4.0, 8.0)]);
        assert_eq!(vertical.code, code::LANE_PATH_VERTICAL);
        assert!(vertical.message.contains("4.00..8.00"));
    }

    #[test]
    fn smoothstep_hits_its_anchors() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(0.5), 0.5);
        assert_eq!(smoothstep(1.0), 1.0);
    }

    #[test]
    fn the_frame_of_a_straight_path_runs_forward_with_lateral_to_the_right() {
        let path = straight();
        let frame = frame_at(&path, 4.0, 0.0, 0.5);
        close(frame.point, [4.0, 0.0, 0.0]);
        close(frame.forward, [1.0, 0.0, 0.0]);
        close(frame.tangent, [1.0, 0.0, 0.0]);
        close(frame.lateral, [0.0, 0.0, 1.0]);
        assert_eq!(frame.weight, 0.5);
        let right = path.frame_at(4.0).right;
        close(
            frame.lateral,
            [right[0] as f32, right[1] as f32, right[2] as f32],
        );

        let shifted = frame_at(&path, 4.0, 1.5, 1.0);
        close(shifted.point, [4.0, 0.0, 1.5]);
    }

    #[test]
    fn an_open_lane_extends_past_both_ends() {
        let path = straight();
        close(frame_at(&path, -1.0, 0.0, 1.0).point, [-1.0, 0.0, 0.0]);
        close(frame_at(&path, 11.0, 0.0, 1.0).point, [11.0, 0.0, 0.0]);
        let past = frame_at(&path, 12.0, 1.0, 1.0);
        close(past.point, [12.0, 0.0, 1.0]);
        close(past.forward, [1.0, 0.0, 0.0]);
        assert_eq!(past.s, 12.0);
    }

    #[test]
    fn a_closed_lane_wraps() {
        let square = sampler(
            true,
            &[
                [0.0, 0.0, 0.0],
                [4.0, 0.0, 0.0],
                [4.0, 0.0, 4.0],
                [0.0, 0.0, 4.0],
            ],
        );
        let length = square.length();
        assert_eq!(
            frame_at(&square, length + 0.5, 0.25, 1.0),
            frame_at(&square, 0.5, 0.25, 1.0)
        );
    }

    #[test]
    fn lane_relative_signs_the_stick_by_the_screen() {
        let frame = frame_at(&straight(), 1.0, 0.0, 1.0);
        let along = lane_relative([0.5, 0.25], &frame, [1.0, 0.0, 0.0]);
        assert_eq!((along.along, along.across), (0.5, 0.25));
        assert_eq!(along.direction, [0.5, 0.0, 0.0]);

        let flipped = lane_relative([0.5, 0.25], &frame, [-1.0, 0.0, 0.0]);
        assert_eq!(flipped.along, -0.5);
        assert_eq!(flipped.direction, [-0.5, 0.0, 0.0]);

        // 85° off forward: inside the dead zone, the helper returns +forward.
        let (sin, cos) = (0.996_194_7_f32, 0.087_155_74_f32);
        let steep = lane_relative([0.5, 0.0], &frame, [cos, 0.0, sin]);
        assert_eq!(steep.along, 0.5);
        let steep = lane_relative([0.5, 0.0], &frame, [-cos, 0.0, sin]);
        assert_eq!(steep.along, 0.5);
    }

    fn hook(path: PathSampler, ends: LaneEnds, weight: f32, s: f64) -> LaneHook {
        LaneHook {
            pull: Pull {
                centre: centre_line(&path).into(),
                sampler: Arc::new(path),
                offset: 0.0,
                ends,
                search_window: 2.0,
                pull_speed: 4.0,
                weight,
                dt: 1.0 / 60.0,
            },
            s,
            anchor: [s as f32, 0.0, 0.0],
            blocked: false,
        }
    }

    #[test]
    fn the_projection_removes_the_lateral_part_by_weight() {
        let mut full = hook(straight(), LaneEnds::Extend, 1.0, 4.0);
        assert_eq!(
            full.project([4.0, 0.0, 0.0], [0.3, -0.1, 0.2]),
            [0.3, -0.1, 0.0]
        );
        let mut half = hook(straight(), LaneEnds::Extend, 0.5, 4.0);
        assert_eq!(
            half.project([4.0, 0.0, 0.0], [0.3, -0.1, 0.2]),
            [0.3, -0.1, 0.1]
        );
        let mut off = hook(straight(), LaneEnds::Extend, 0.0, 4.0);
        assert_eq!(
            off.project([4.0, 0.0, 0.0], [0.3, -0.1, 0.2]),
            [0.3, -0.1, 0.2]
        );
    }

    #[test]
    fn a_stop_lane_clamps_the_step_at_both_ends() {
        let mut stop = hook(straight(), LaneEnds::Stop, 1.0, 9.9);
        let out = stop.project([9.9, 0.0, 0.0], [0.5, 0.0, 0.0]);
        assert!((out[0] - 0.1).abs() < 1e-5, "{out:?}");
        let mut start = hook(straight(), LaneEnds::Stop, 1.0, 0.1);
        let out = start.project([0.1, 0.0, 0.0], [-0.5, 0.0, 0.0]);
        assert!((out[0] + 0.1).abs() < 1e-5, "{out:?}");
        let mut extend = hook(straight(), LaneEnds::Extend, 1.0, 9.9);
        assert_eq!(extend.project([9.9, 0.0, 0.0], [0.5, 0.0, 0.0])[0], 0.5);
    }

    #[test]
    fn the_correction_is_capped_by_weight_and_speed() {
        let mut pull = hook(straight(), LaneEnds::Extend, 1.0, 4.0);
        let cap = 4.0 / 60.0;
        let out = pull.correction([4.0, 0.0, 1.0]);
        assert!((out[2] + cap).abs() < 1e-6, "{out:?}");
        let out = pull.correction([4.0, 0.0, -0.01]);
        assert!((out[2] - 0.01).abs() < 1e-6, "{out:?}");
        assert_eq!(pull.correction([4.0, 0.0, 0.0]), [0.0; 3]);
        let mut off = hook(straight(), LaneEnds::Extend, 0.0, 4.0);
        assert_eq!(off.correction([4.0, 0.0, 1.0]), [0.0; 3]);
    }

    #[test]
    fn refind_continues_an_extend_lane_past_either_end() {
        let pull = hook(straight(), LaneEnds::Extend, 1.0, 9.0).pull;
        assert!((pull.refind([12.5, 0.0, 0.7], 9.0, 4.0) - 12.5).abs() < 1e-9);
        assert!((pull.refind([-1.5, 0.0, 0.0], 1.0, 4.0) + 1.5).abs() < 1e-9);
        assert!((pull.refind([5.0, 0.0, 0.7], 4.0, 4.0) - 5.0).abs() < 1e-9);
        let stop = hook(straight(), LaneEnds::Stop, 1.0, 9.0).pull;
        assert_eq!(stop.refind([12.5, 0.0, 0.0], 9.0, 4.0), 10.0);
    }

    #[test]
    fn vertical_spans_find_the_climb() {
        let flat = straight();
        assert!(vertical_spans(&flat).is_empty());

        let climb = sampler(
            false,
            &[
                [0.0, 0.0, 0.0],
                [4.0, 0.0, 0.0],
                [4.0, 4.0, 0.0],
                [8.0, 4.0, 0.0],
            ],
        );
        let spans = vertical_spans(&climb);
        assert_eq!(spans.len(), 1, "{spans:?}");
        let (first, last) = spans[0];
        assert!(
            first <= 4.1 && last >= 7.9 && first >= 3.9 && last <= 8.1,
            "{first}..{last}"
        );
    }
}
