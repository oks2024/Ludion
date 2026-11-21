//! The numeric trace: a per-tick JSON Lines record of a scene driven by an input script.
//!
//! Seven channels. `motor` writes one line per [`CharacterMotor`] entity per tick — the tick, the
//! entity's name, its feet and the tick's [`MoveReport`]. `sensors` writes one line per
//! [`SensorEvent`] the tick's physics step produced. `ai` writes one line per `ludion-ai` state
//! transition and `ai_states` one per agent per tick (`ludion docs ai`). `perception` writes the sight, noise
//! and memory changes of the engine's perception (`ludion docs perception`). `nav` writes one line per route
//! plan `nav_steer` attempts and `nav_states` one per `NavAgent` per tick (`ludion docs nav`). One function, [`trace`], builds the records;
//! `ludion trace`, a game binary's `--trace` and a game's `cargo test` ([`trace_with`]) all call it,
//! so they print the same lines. Two runs of the same scene, script and build are byte-identical:
//! compare two runs, never a committed trace.
//!
//! Floats are written by `serde_json` (ryu shortest round-trip), the same writer the wire uses.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;

use ludion_ai::{AiCause, AiRecord, AiTrace};
use ludion_components::nav_route::{NavGoal, NavRoute, NavRouteStatus};
use ludion_components::{NavAgent, NavSteering};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_nav::PathStatus;
use ludion_physics::{
    CharacterMotor, ExitCause, GroundHit, MoveReport, PhysicsWorld, SensorEvent, SensorPhase,
    SurfaceHit,
};
use ludion_protocol::Command;
use ludion_protocol::command::{InputInjectParams, SimStepParams};
use ludion_sim::InputScript;
use serde::Serialize;

use crate::builder::EngineBuilder;
use crate::components::{Name, Perceiver, Sense, StimulusRecord, Transform};
use crate::engine::Engine;
use crate::hierarchy::world_transform;
use crate::nav::{NavPlanLog, NavTrace};
use crate::perception::PerceptionResults;

/// A trace channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceChannel {
    /// One line per `CharacterMotor` entity per tick.
    Motor,
    /// One line per sensor event of the tick.
    Sensors,
    /// One line per `ludion-ai` state transition, with its cause.
    Ai,
    /// One line per `ludion-ai` agent per tick it ran, with its active path.
    AiStates,
    /// One line per sight change, noise delivery and memory record appearing or vanishing.
    Perception,
    /// One line per route plan `nav_steer` attempted, with why it planned and what came out.
    Nav,
    /// One line per `NavAgent` per tick: mode, goal, route status, position and velocity.
    NavStates,
}

impl TraceChannel {
    /// Every channel, the default selection.
    pub const ALL: [TraceChannel; 7] = [
        TraceChannel::Motor,
        TraceChannel::Sensors,
        TraceChannel::Ai,
        TraceChannel::AiStates,
        TraceChannel::Perception,
        TraceChannel::Nav,
        TraceChannel::NavStates,
    ];
}

impl FromStr for TraceChannel {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, String> {
        match name {
            "motor" => Ok(TraceChannel::Motor),
            "sensors" => Ok(TraceChannel::Sensors),
            "ai" => Ok(TraceChannel::Ai),
            "ai_states" => Ok(TraceChannel::AiStates),
            "perception" => Ok(TraceChannel::Perception),
            "nav" => Ok(TraceChannel::Nav),
            "nav_states" => Ok(TraceChannel::NavStates),
            other => Err(format!(
                "unknown trace channel `{other}`; the channels are `motor`, `sensors`, `ai`, `ai_states`, `perception`, `nav` and `nav_states`"
            )),
        }
    }
}

/// Reads and parses a JSON input script (`L4002` on IO failure, `L4003` on parse failure): the
/// loader behind `ludion sim-hash --script` and `ludion trace --script`.
pub fn load_input_script(path: &Path) -> Result<InputScript, Diagnostic> {
    let source = std::fs::read_to_string(path).map_err(|error| {
        Diagnostic::new(
            code::INPUT_SCRIPT_IO,
            format!("could not read `{}`: {error}", path.display()),
        )
        .with_help("check the path (relative to the current directory) and permissions")
    })?;
    serde_json::from_str(&source).map_err(|error| {
        Diagnostic::new(
            code::INPUT_SCRIPT_INVALID,
            format!(
                "could not parse `{}` as an input script: {error}",
                path.display()
            ),
        )
        .with_help(
            "an input script is a JSON array of records like \
             {\"frame\": 10, \"event\": {\"kind\": \"key_down\", \"key\": \"W\"}} \
             (see `ludion explain L4003`)",
        )
    })
}

/// One line of a trace; the `channel` field comes first.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "channel", rename_all = "snake_case")]
pub enum TraceRecord {
    /// A motor entity's tick.
    Motor(MotorRecord),
    /// A sensor transition.
    Sensors(SensorRecord),
    /// An agent's state transition.
    Ai(AiTransitionRecord),
    /// An agent's active path after a tick.
    AiStates(AiStatesRecord),
    /// A perception change; the `event` field names which.
    Perception(PerceptionRecord),
    /// A route plan attempt.
    Nav(NavPlanRecord),
    /// A nav agent's state after a tick.
    NavStates(NavStateRecord),
}

/// A perception line: the `event` field comes right after `channel`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum PerceptionRecord {
    /// A `(perceiver, zone, target)` key whose `visible` changed.
    Sight(SightTraceRecord),
    /// One noise reaching one listener.
    Noise(NoiseTraceRecord),
    /// A `StimulusRecord` that appeared.
    Remember(RememberTraceRecord),
    /// A `StimulusRecord` that vanished.
    Forget(ForgetTraceRecord),
}

/// A sight change: the key's `visible` differs from the previous traced tick. A key that dropped out
/// of the results while visible prints `in_zone: false, visible: false` and no sample fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SightTraceRecord {
    /// The frame index the tick ran as.
    pub tick: u64,
    /// The perceiver's `Name`, else its id.
    pub perceiver: String,
    /// The zone's `name`; its index when the zone is unnamed or the perceiver is gone.
    pub zone: String,
    /// The target's `Name`, else its id.
    pub target: String,
    /// Whether any sample point was inside the zone's wedge.
    pub in_zone: bool,
    /// Whether the target's `require` rule was met.
    pub visible: bool,
    /// The bitmask of seen sample points.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points_visible: Option<u8>,
    /// Metres from the eye to the target's origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<f32>,
    /// The nearest blocker of the lowest-index blocked point.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
}

/// One delivery of a noise to one listener.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NoiseTraceRecord {
    /// The frame index the tick ran as.
    pub tick: u64,
    /// The listener's `Name`, else its id.
    pub listener: String,
    /// The noise id.
    pub noise: u64,
    /// The noise kind.
    pub kind: String,
    /// What made it, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Metres from the ear to the noise.
    pub distance: f32,
    /// The radius this listener's ear applied.
    pub effective_radius: f32,
    /// The sound occluders counted between the noise and the ear.
    pub occluders: u8,
}

/// A `StimulusRecord` that appeared.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RememberTraceRecord {
    /// The frame index the tick ran as.
    pub tick: u64,
    /// The perceiver's `Name`, else its id.
    pub perceiver: String,
    /// What was sensed, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// `sight`, `hearing` or `custom:<k>`.
    pub sense: String,
    /// The noise kind of a hearing record.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The record's `last_position`.
    pub at: [f32; 3],
}

/// A `StimulusRecord` that vanished: the last value seen of it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ForgetTraceRecord {
    /// The frame index the tick ran as.
    pub tick: u64,
    /// The perceiver's `Name`, else its id.
    pub perceiver: String,
    /// What was sensed, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// `sight`, `hearing` or `custom:<k>`.
    pub sense: String,
    /// The noise kind of a hearing record.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The record's last `last_position`.
    pub at: [f32; 3],
}

/// A motor entity's tick: where its feet are and, when game code moved it this tick, what the move did.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MotorRecord {
    /// The tick the line describes: the frame index the tick ran as.
    pub tick: u64,
    /// The entity's `Name`, else its id.
    pub entity: String,
    /// The entity's `Transform` translation.
    pub feet: [f32; 3],
    /// The tick's `MoveReport`; absent when nothing moved the entity this tick.
    #[serde(flatten)]
    pub report: Option<ReportRecord>,
}

/// A `MoveReport` as one trace line carries it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReportRecord {
    /// The feet's total movement.
    pub moved: [f32; 3],
    /// Whether the motor ended on walkable ground.
    pub grounded: bool,
    /// Whether the move started airborne and ended grounded.
    pub landed: bool,
    /// The support under the feet, walkable or not.
    pub ground: Option<GroundRecord>,
    /// The ceiling an upward motion hit.
    pub ceiling: Option<SurfaceRecord>,
    /// The walls the slide hit, in order of travel.
    pub walls: Vec<SurfaceRecord>,
    /// The height a step-up climbed.
    pub stepped: Option<f32>,
    /// What platform carry did.
    pub platform: PlatformRecord,
    /// Whether depenetration could not clear the capsule.
    pub stuck: bool,
    /// The capsule height the move used.
    pub height: f32,
    /// What the lane did; absent when the call was not held to a lane.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lane: Option<LaneRecord>,
}

/// A `LaneReport` as one trace line carries it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LaneRecord {
    /// Metres along the path after the move.
    pub s: f32,
    /// The change in `s` over the move, in metres.
    pub ds: f32,
    /// The blend weight the move used, `0..=1`.
    pub weight: f32,
    /// Signed metres from the lane centre line to the feet along the lane's lateral.
    pub lateral_error: f32,
    /// Whether a wall stopped the pull toward the lane.
    pub pull_blocked: bool,
}

/// The support under the feet.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GroundRecord {
    /// The supporting entity.
    pub entity: String,
    /// World-space contact point.
    pub point: [f32; 3],
    /// World-space surface normal.
    pub normal: [f32; 3],
    /// Whether the normal is within the slope limit.
    pub walkable: bool,
    /// Cast distance from the final pose to the surface.
    pub distance: f32,
    /// The surface name the hit reports; absent when none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
}

/// A wall or ceiling hit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SurfaceRecord {
    /// The blocking entity.
    pub entity: String,
    /// World-space contact point.
    pub point: [f32; 3],
    /// World-space surface normal.
    pub normal: [f32; 3],
    /// The surface name the hit reports; absent when none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
}

/// What platform carry did.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlatformRecord {
    /// The carrying platform, `null` when none.
    pub entity: Option<String>,
    /// The translation the carry applied.
    pub translation: [f32; 3],
    /// The yaw the carry applied, in radians.
    pub yaw: f32,
    /// The platform's surface velocity the carry added.
    pub surface: [f32; 3],
}

/// One sensor transition.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SensorRecord {
    /// The tick whose physics step produced the event.
    pub tick: u64,
    /// The sensor's `Name`, else its id.
    pub sensor: String,
    /// The entity that entered or left.
    pub other: String,
    /// `entered` or `exited`.
    pub phase: &'static str,
    /// Why an exit happened: `separated`, `despawned` or `removed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<&'static str>,
}

/// One `ludion-ai` state transition.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AiTransitionRecord {
    /// The frame index the tick ran as.
    pub tick: u64,
    /// The agent's `Name`, else its id.
    pub entity: String,
    /// The machine's `NAME`.
    pub machine: &'static str,
    /// The active path before, root first; empty for a start.
    pub from: Vec<String>,
    /// The active path after.
    pub to: Vec<String>,
    /// `start`, `event:<Name>`, `update` or `chain`.
    pub cause: String,
    /// The event as JSON, for an `event` cause.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<serde_json::Value>,
}

/// One `ludion-ai` agent's active path after a tick it ran.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AiStatesRecord {
    /// The frame index the tick ran as.
    pub tick: u64,
    /// The agent's `Name`, else its id.
    pub entity: String,
    /// The machine's `NAME`.
    pub machine: &'static str,
    /// The active path, root first.
    pub path: Vec<String>,
}

/// One route plan `nav_steer` attempted: the `nav` channel.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NavPlanRecord {
    /// The frame index the tick ran as.
    pub tick: u64,
    /// The agent's `Name`, else its id.
    pub entity: String,
    /// The agent's nav class; omitted for the default class.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub class: String,
    /// Why it planned: `new`, `retry`, `goal_moved`, `link_missing`, `blocker_closed` or `leg_blocked`.
    pub reason: &'static str,
    /// The agent's position when it planned.
    pub from: [f32; 3],
    /// The goal it planned to.
    pub to: [f32; 3],
    /// `complete`, `partial`, `unreachable`, or `deferred` when the per-tick request cap held it.
    pub status: &'static str,
    /// Nodes the search expanded; `0` when deferred.
    pub expanded: u32,
    /// Route length in metres; omitted when unreachable or deferred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f32>,
    /// Waypoints in the route; `0` when unreachable or deferred.
    pub waypoints: u32,
    /// Link traversals on the route.
    pub links: u32,
    /// Interact-blocker stops on the route.
    pub blockers: u32,
}

/// One `NavAgent` after a tick: the `nav_states` channel.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NavStateRecord {
    /// The frame index the tick ran as.
    pub tick: u64,
    /// The agent's `Name`, else its id.
    pub entity: String,
    /// The agent's nav class; omitted for the default class.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub class: String,
    /// `fly` for a flyer, `route` with a `NavGoal`, else `field`.
    pub mode: &'static str,
    /// The `NavGoal`'s point, else the agent's `target`.
    pub goal: [f32; 3],
    /// The stored route's status; omitted with no `NavRoute`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<NavRouteStatus>,
    /// The waypoint index being walked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// Waypoints in the stored route.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waypoints: Option<u32>,
    /// Whether the route only approaches the goal; omitted when it reaches it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    /// The `NavLink` being traversed or waited at, by `Name` else id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// The `NavBlocker` being waited at, by `Name` else id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
    /// The agent's world position; omitted with no pose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    /// The agent's `NavSteering.velocity`, zero before its first steer.
    pub velocity: [f32; 3],
}

/// An entity's `Name`, else its id; a despawned id has no name and prints as its id.
fn label(world: &World, entity: EntityId) -> String {
    match world.get::<Name>(entity) {
        Ok(Some(name)) => name.0.clone(),
        _ => entity.to_string(),
    }
}

/// The declared name of the surface a motor hit reports.
fn surface_label(world: &World, id: Option<ludion_physics::SurfaceId>) -> Option<String> {
    let physics = world.resource::<PhysicsWorld>()?;
    physics.surface_name(id?).map(str::to_owned)
}

fn surface(world: &World, hit: &SurfaceHit) -> SurfaceRecord {
    SurfaceRecord {
        entity: label(world, hit.entity),
        point: hit.point,
        normal: hit.normal,
        surface: surface_label(world, hit.surface),
    }
}

fn ground(world: &World, hit: &GroundHit) -> GroundRecord {
    GroundRecord {
        entity: label(world, hit.entity),
        point: hit.point,
        normal: hit.normal,
        walkable: hit.walkable,
        distance: hit.distance,
        surface: surface_label(world, hit.surface),
    }
}

fn report_record(world: &World, report: &MoveReport) -> ReportRecord {
    ReportRecord {
        moved: report.moved,
        grounded: report.grounded,
        landed: report.landed,
        ground: report.ground.as_ref().map(|hit| ground(world, hit)),
        ceiling: report.ceiling.as_ref().map(|hit| surface(world, hit)),
        walls: report.walls.iter().map(|hit| surface(world, hit)).collect(),
        stepped: report.stepped,
        platform: PlatformRecord {
            entity: report.platform.entity.map(|entity| label(world, entity)),
            translation: report.platform.translation,
            yaw: report.platform.yaw,
            surface: report.platform.surface,
        },
        stuck: report.stuck,
        height: report.height,
        lane: report.lane.as_ref().map(|lane| LaneRecord {
            s: lane.s,
            ds: lane.ds,
            weight: lane.weight,
            lateral_error: lane.lateral_error,
            pull_blocked: lane.pull_blocked,
        }),
    }
}

fn motor_records(world: &World, tick: u64, out: &mut Vec<TraceRecord>) {
    let mut motors: Vec<EntityId> = world
        .entities()
        .filter(|entity| world.has::<CharacterMotor>(*entity))
        .collect();
    motors.sort();
    let physics = world.resource::<PhysicsWorld>();
    for entity in motors {
        let feet = match world.get::<Transform>(entity) {
            Ok(Some(transform)) => transform.translation,
            _ => [0.0; 3],
        };
        let report = physics
            .and_then(|physics| physics.last_move_report(entity))
            .filter(|(frame, _)| frame.as_u64() == tick)
            .map(|(_, report)| report_record(world, report));
        out.push(TraceRecord::Motor(MotorRecord {
            tick,
            entity: label(world, entity),
            feet,
            report,
        }));
    }
}

fn nav_plan_records(world: &World, logs: Vec<NavPlanLog>, out: &mut Vec<TraceRecord>) {
    for log in logs {
        let (status, expanded, cost, waypoints, links, blockers) = match &log.outcome {
            None => ("deferred", 0, None, 0, 0, 0),
            Some(o) => (
                match o.status {
                    PathStatus::Complete => "complete",
                    PathStatus::Partial => "partial",
                    PathStatus::Unreachable => "unreachable",
                },
                o.expanded,
                o.cost.is_finite().then_some(o.cost),
                o.waypoints,
                o.links,
                o.blockers,
            ),
        };
        out.push(TraceRecord::Nav(NavPlanRecord {
            tick: log.tick,
            entity: label(world, log.entity),
            class: log.class,
            reason: log.reason.as_str(),
            from: log.from,
            to: log.to,
            status,
            expanded,
            cost,
            waypoints,
            links,
            blockers,
        }));
    }
}

fn nav_state_records(world: &World, tick: u64, out: &mut Vec<TraceRecord>) {
    let mut agents: Vec<EntityId> = world
        .entities()
        .filter(|entity| world.has::<NavAgent>(*entity))
        .collect();
    agents.sort();
    for entity in agents {
        let Ok(Some(agent)) = world.get::<NavAgent>(entity) else {
            continue;
        };
        let goal = world.get::<NavGoal>(entity).ok().flatten();
        let route = world.get::<NavRoute>(entity).ok().flatten();
        let velocity = world
            .get::<NavSteering>(entity)
            .ok()
            .flatten()
            .map_or([0.0; 3], |steering| steering.velocity);
        out.push(TraceRecord::NavStates(NavStateRecord {
            tick,
            entity: label(world, entity),
            class: agent.class.clone(),
            mode: if agent.flying {
                "fly"
            } else if goal.is_some() {
                "route"
            } else {
                "field"
            },
            goal: goal.map_or(agent.target, |goal| goal.at),
            status: route.map(|route| route.status),
            index: route.map(|route| route.index),
            waypoints: route.map(|route| route.waypoints.len() as u32),
            partial: route.is_some_and(|route| route.partial),
            link: route
                .and_then(|route| route.link)
                .map(|id| label(world, id)),
            blocker: route
                .and_then(|route| route.blocker)
                .map(|id| label(world, id)),
            position: world_transform(world, entity).map(|pose| pose.trs.translation),
            velocity,
        }));
    }
}

fn sensor_records(world: &World, tick: u64, out: &mut Vec<TraceRecord>) {
    let Some(physics) = world.resource::<PhysicsWorld>() else {
        return;
    };
    for SensorEvent {
        sensor,
        other,
        phase,
    } in physics.sensor_events()
    {
        let (phase, cause) = match phase {
            SensorPhase::Entered => ("entered", None),
            SensorPhase::Exited { cause } => (
                "exited",
                Some(match cause {
                    ExitCause::Separated => "separated",
                    ExitCause::Despawned => "despawned",
                    ExitCause::Removed => "removed",
                }),
            ),
        };
        out.push(TraceRecord::Sensors(SensorRecord {
            tick,
            sensor: label(world, *sensor),
            other: label(world, *other),
            phase,
            cause,
        }));
    }
}

fn ai_records(world: &World, records: Vec<AiRecord>, out: &mut Vec<TraceRecord>) {
    for record in records {
        out.push(match record {
            AiRecord::Transition(t) => {
                let (cause, event) = match t.cause {
                    AiCause::Start => ("start".to_owned(), None),
                    AiCause::Event(payload) => {
                        let name = event_name(&payload);
                        let cause = if name.is_empty() {
                            "event".to_owned()
                        } else {
                            format!("event:{name}")
                        };
                        (cause, Some(payload))
                    }
                    AiCause::Update => ("update".to_owned(), None),
                    AiCause::Chain => ("chain".to_owned(), None),
                };
                TraceRecord::Ai(AiTransitionRecord {
                    tick: t.tick,
                    entity: label(world, t.entity),
                    machine: t.machine,
                    from: t.from,
                    to: t.to,
                    cause,
                    event,
                })
            }
            AiRecord::State(s) => TraceRecord::AiStates(AiStatesRecord {
                tick: s.tick,
                entity: label(world, s.entity),
                machine: s.machine,
                path: s.path,
            }),
        });
    }
}

/// What the `perception` channel remembers of the previous traced tick, to print only changes.
/// Nothing is recorded sim-side: the lines derive from `PerceptionResults` and the `StimulusRecord`
/// entities.
#[derive(Default)]
struct PerceptionDiff {
    /// The `(perceiver, zone, target)` keys that were visible; an absent key was not.
    visible: BTreeMap<(EntityId, u8, EntityId), ()>,
    /// Every `StimulusRecord` entity seen, with its last value.
    records: BTreeMap<EntityId, StimulusRecord>,
    /// The `results.tick()` whose noises were last printed.
    last_tick: Option<u64>,
}

fn sense_label(sense: Sense) -> String {
    match sense {
        Sense::Sight => "sight".to_owned(),
        Sense::Hearing => "hearing".to_owned(),
        Sense::Custom(k) => format!("custom:{k}"),
    }
}

fn zone_label(world: &World, perceiver: EntityId, zone: u8) -> String {
    match world.get::<Perceiver>(perceiver) {
        Ok(Some(p)) => match p.sight.get(usize::from(zone)) {
            Some(z) if !z.name.is_empty() => z.name.clone(),
            _ => zone.to_string(),
        },
        _ => zone.to_string(),
    }
}

impl PerceptionDiff {
    fn step(&mut self, world: &World, tick: u64, out: &mut Vec<TraceRecord>) {
        if let Some(results) = world.resource::<PerceptionResults>() {
            self.sight(world, results, tick, out);
            if self.last_tick != Some(results.tick()) {
                self.last_tick = Some(results.tick());
                for heard in results.heard_all() {
                    out.push(TraceRecord::Perception(PerceptionRecord::Noise(
                        NoiseTraceRecord {
                            tick,
                            listener: label(world, heard.listener),
                            noise: heard.noise.0,
                            kind: heard.kind.clone(),
                            source: heard.source.map(|s| label(world, s)),
                            distance: heard.distance,
                            effective_radius: heard.effective_radius,
                            occluders: heard.occluders,
                        },
                    )));
                }
            }
        }
        self.memory(world, tick, out);
    }

    fn sight(
        &mut self,
        world: &World,
        results: &PerceptionResults,
        tick: u64,
        out: &mut Vec<TraceRecord>,
    ) {
        let mut changed: BTreeMap<(EntityId, u8, EntityId), SightTraceRecord> = BTreeMap::new();
        let mut now = BTreeMap::new();
        for sample in results.samples() {
            let key = (sample.perceiver, sample.zone, sample.target);
            if sample.visible {
                now.insert(key, ());
            }
            if sample.visible != self.visible.contains_key(&key) {
                changed.insert(
                    key,
                    SightTraceRecord {
                        tick,
                        perceiver: label(world, sample.perceiver),
                        zone: zone_label(world, sample.perceiver, sample.zone),
                        target: label(world, sample.target),
                        in_zone: sample.in_zone,
                        visible: sample.visible,
                        points_visible: Some(sample.points_visible),
                        distance: Some(sample.distance),
                        blocker: sample.blocker.map(|b| label(world, b)),
                    },
                );
            }
        }
        for key in self.visible.keys() {
            if !now.contains_key(key) && !changed.contains_key(key) {
                changed.insert(
                    *key,
                    SightTraceRecord {
                        tick,
                        perceiver: label(world, key.0),
                        zone: zone_label(world, key.0, key.1),
                        target: label(world, key.2),
                        in_zone: false,
                        visible: false,
                        points_visible: None,
                        distance: None,
                        blocker: None,
                    },
                );
            }
        }
        self.visible = now;
        out.extend(
            changed
                .into_values()
                .map(|r| TraceRecord::Perception(PerceptionRecord::Sight(r))),
        );
    }

    fn memory(&mut self, world: &World, tick: u64, out: &mut Vec<TraceRecord>) {
        let mut current: BTreeMap<EntityId, StimulusRecord> = BTreeMap::new();
        for entity in world.entities() {
            if let Ok(Some(record)) = world.get::<StimulusRecord>(entity) {
                current.insert(entity, record.clone());
            }
        }
        let line = |record: &StimulusRecord| {
            (
                label(world, record.perceiver),
                record.source.map(|s| label(world, s)),
                sense_label(record.sense),
            )
        };
        for (entity, record) in &self.records {
            if !current.contains_key(entity) {
                let (perceiver, source, sense) = line(record);
                out.push(TraceRecord::Perception(PerceptionRecord::Forget(
                    ForgetTraceRecord {
                        tick,
                        perceiver,
                        source,
                        sense,
                        kind: record.kind.clone(),
                        at: record.last_position,
                    },
                )));
            }
        }
        for (entity, record) in &current {
            if !self.records.contains_key(entity) {
                let (perceiver, source, sense) = line(record);
                out.push(TraceRecord::Perception(PerceptionRecord::Remember(
                    RememberTraceRecord {
                        tick,
                        perceiver,
                        source,
                        sense,
                        kind: record.kind.clone(),
                        at: record.last_position,
                    },
                )));
            }
        }
        self.records = current;
    }
}

/// The variant name of a serialized machine event: a unit variant is a string, an externally tagged
/// variant a one-key object. Empty when the payload has neither shape.
fn event_name(payload: &serde_json::Value) -> String {
    match payload {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) if map.len() == 1 => {
            map.keys().next().cloned().unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// Drives `engine` through `script` for `frames` ticks and returns the trace records of `channels`.
///
/// Each tick is stepped alone, then read: `tick` is the frame index the tick ran as (an input
/// addressed to frame `N` applies at the start of tick `N`). Within a tick the `motor` lines come
/// first, ordered by entity id, then the `sensors` lines in the physics world's `(sensor, other)`
/// order, then the `ai` and `ai_states` lines as `ai_tick` recorded them (machines in schedule
/// order, agents ascending; a tick the game held prints none), then the `perception` lines
/// (`sight` changes by `(perceiver, zone, target)`, `noise` deliveries by `(listener, noise)` on a
/// tick whose gameplay time advanced, then `forget` and `remember` lines by record entity), then the
/// `nav` lines in plan order (ascending entity; only where `nav_steer` runs) and the `nav_states` lines
/// by ascending entity, whatever order `channels` names them in. A motor line carries the `MoveReport` only when
/// game code called `move_character` on that entity during the tick. A sensor line is the tick's own
/// physics step. No physics world means no sensor lines.
///
/// # Errors
/// An input the engine refuses, or a failed step.
///
/// # Example
/// A body falling into a static sensor is reported entering it.
/// ```
/// use ludion_engine::{Engine, TraceChannel, trace};
/// use ludion_sim::InputScript;
///
/// const SCENE: &str = "format: 1
/// physics:
///   space: free
/// entities:
///   - name: zone
///     components:
///       Transform:
///         translation:
///           - 0.0
///           - 0.0
///           - 0.0
///       Collider:
///         shape:
///           box:
///             half_extents:
///               - 2.0
///               - 0.5
///               - 2.0
///         body: static
///         sensor: true
///   - name: ball
///     components:
///       Transform:
///         translation:
///           - 0.0
///           - 1.0
///           - 0.0
///       Collider:
///         shape:
///           sphere:
///             radius: 0.25
///         body:
///           rigid:
///             mass: 1.0
/// ";
///
/// let mut engine = Engine::new();
/// engine.load_inline_scene(SCENE).unwrap();
/// let records = trace(&mut engine, &InputScript::new(), 60, &[TraceChannel::Sensors]).unwrap();
/// assert!(!records.is_empty(), "the ball falls into the zone");
/// ```
pub fn trace(
    engine: &mut Engine,
    script: &InputScript,
    frames: u64,
    channels: &[TraceChannel],
) -> Result<Vec<TraceRecord>, Diagnostic> {
    for scripted in script.events() {
        let event = serde_json::to_value(scripted.event).expect("InputEvent always serializes");
        engine.execute(Command::InputInject(InputInjectParams {
            frame: Some(scripted.frame),
            event,
        }))?;
    }

    let motor = channels.contains(&TraceChannel::Motor);
    let sensors = channels.contains(&TraceChannel::Sensors);
    let ai = channels.contains(&TraceChannel::Ai);
    let ai_states = channels.contains(&TraceChannel::AiStates);
    let perception_on = channels.contains(&TraceChannel::Perception);
    if ai || ai_states {
        engine
            .world_mut()
            .insert_resource(AiTrace::new(ai, ai_states));
    }
    let nav = channels.contains(&TraceChannel::Nav);
    let nav_states = channels.contains(&TraceChannel::NavStates);
    if nav {
        engine.world_mut().insert_resource(NavTrace::default());
    }
    let mut records = Vec::new();
    let mut perception = perception_on.then(PerceptionDiff::default);
    for _ in 0..frames {
        let tick = engine.frame().as_u64();
        engine.execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))?;
        if motor {
            motor_records(engine.world(), tick, &mut records);
        }
        if sensors {
            sensor_records(engine.world(), tick, &mut records);
        }
        if (ai || ai_states)
            && let Some(sink) = engine.world_mut().resource_mut::<AiTrace>()
        {
            let taken = sink.take();
            ai_records(engine.world(), taken, &mut records);
        }
        if let Some(diff) = perception.as_mut() {
            diff.step(engine.world(), tick, &mut records);
        }
        if nav && let Some(sink) = engine.world_mut().resource_mut::<NavTrace>() {
            let taken = sink.take();
            nav_plan_records(engine.world(), taken, &mut records);
        }
        if nav_states {
            nav_state_records(engine.world(), tick, &mut records);
        }
    }
    Ok(records)
}

/// Loads `scene` through `builder` (a game's own, so its systems run) and [`trace`]s it: the
/// `cargo test` entry point.
///
/// # Errors
/// The scene's load diagnostics, or anything [`trace`] reports.
pub fn trace_with(
    builder: EngineBuilder,
    scene: &Path,
    script: &InputScript,
    frames: u64,
    channels: &[TraceChannel],
) -> Result<Vec<TraceRecord>, Diagnostic> {
    let mut engine = builder.load_scene_file(scene)?;
    trace(&mut engine, script, frames, channels)
}

/// Writes `records` as JSON Lines: one record, then `\n`.
///
/// # Errors
/// An I/O error from `out`.
pub fn write_jsonl(records: &[TraceRecord], out: &mut impl Write) -> io::Result<()> {
    for record in records {
        serde_json::to_writer(&mut *out, record)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{NavPlanRecord, NoiseTraceRecord, PerceptionRecord, TraceRecord, event_name};
    use serde_json::json;

    #[test]
    fn a_nav_line_leads_with_channel_and_skips_a_missing_cost() {
        let mut record = NavPlanRecord {
            tick: 4,
            entity: "guard".into(),
            class: String::new(),
            reason: "new",
            from: [0.0, 0.0, 0.0],
            to: [3.0, 0.0, 1.5],
            status: "complete",
            expanded: 12,
            cost: Some(4.5),
            waypoints: 3,
            links: 0,
            blockers: 0,
        };
        assert_eq!(
            serde_json::to_string(&TraceRecord::Nav(record.clone())).unwrap(),
            r#"{"channel":"nav","tick":4,"entity":"guard","reason":"new","from":[0.0,0.0,0.0],"to":[3.0,0.0,1.5],"status":"complete","expanded":12,"cost":4.5,"waypoints":3,"links":0,"blockers":0}"#
        );
        record.status = "unreachable";
        record.cost = None;
        record.class = "tall".into();
        assert_eq!(
            serde_json::to_string(&TraceRecord::Nav(record)).unwrap(),
            r#"{"channel":"nav","tick":4,"entity":"guard","class":"tall","reason":"new","from":[0.0,0.0,0.0],"to":[3.0,0.0,1.5],"status":"unreachable","expanded":12,"waypoints":3,"links":0,"blockers":0}"#
        );
    }

    #[test]
    fn a_perception_line_leads_with_channel_then_event_then_tick() {
        let line = TraceRecord::Perception(PerceptionRecord::Noise(NoiseTraceRecord {
            tick: 7,
            listener: "guard".into(),
            noise: 3,
            kind: "footstep".into(),
            source: None,
            distance: 2.5,
            effective_radius: 6.0,
            occluders: 0,
        }));
        assert_eq!(
            serde_json::to_string(&line).unwrap(),
            r#"{"channel":"perception","event":"noise","tick":7,"listener":"guard","noise":3,"kind":"footstep","distance":2.5,"effective_radius":6.0,"occluders":0}"#
        );
    }

    #[test]
    fn event_name_reads_unit_and_tagged_variants() {
        assert_eq!(event_name(&json!("Noise")), "Noise");
        assert_eq!(event_name(&json!({"Saw": {"who": 3}})), "Saw");
        assert_eq!(event_name(&json!({"a": 1, "b": 2})), "");
        assert_eq!(event_name(&json!(7)), "");
    }
}
