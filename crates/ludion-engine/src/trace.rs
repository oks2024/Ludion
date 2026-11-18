//! The numeric trace: a per-tick JSON Lines record of a scene driven by an input script.
//!
//! Four channels. `motor` writes one line per [`CharacterMotor`] entity per tick — the tick, the
//! entity's name, its feet and the tick's [`MoveReport`]. `sensors` writes one line per
//! [`SensorEvent`] the tick's physics step produced. `ai` writes one line per `ludion-ai` state
//! transition and `ai_states` one per agent per tick (`ludion docs ai`). One function, [`trace`], builds the records;
//! `ludion trace`, a game binary's `--trace` and a game's `cargo test` ([`trace_with`]) all call it,
//! so they print the same lines. Two runs of the same scene, script and build are byte-identical:
//! compare two runs, never a committed trace.
//!
//! Floats are written by `serde_json` (ryu shortest round-trip), the same writer the wire uses.

use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;

use ludion_ai::{AiCause, AiRecord, AiTrace};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_physics::{
    CharacterMotor, ExitCause, GroundHit, MoveReport, PhysicsWorld, SensorEvent, SensorPhase,
    SurfaceHit,
};
use ludion_protocol::Command;
use ludion_protocol::command::{InputInjectParams, SimStepParams};
use ludion_sim::InputScript;
use serde::Serialize;

use crate::builder::EngineBuilder;
use crate::components::{Name, Transform};
use crate::engine::Engine;

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
}

impl TraceChannel {
    /// Every channel, the default selection.
    pub const ALL: [TraceChannel; 4] = [
        TraceChannel::Motor,
        TraceChannel::Sensors,
        TraceChannel::Ai,
        TraceChannel::AiStates,
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
            other => Err(format!(
                "unknown trace channel `{other}`; the channels are `motor`, `sensors`, `ai` and `ai_states`"
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

/// An entity's `Name`, else its id; a despawned id has no name and prints as its id.
fn label(world: &World, entity: EntityId) -> String {
    match world.get::<Name>(entity) {
        Ok(Some(name)) => name.0.clone(),
        _ => entity.to_string(),
    }
}

fn surface(world: &World, hit: &SurfaceHit) -> SurfaceRecord {
    SurfaceRecord {
        entity: label(world, hit.entity),
        point: hit.point,
        normal: hit.normal,
    }
}

fn ground(world: &World, hit: &GroundHit) -> GroundRecord {
    GroundRecord {
        entity: label(world, hit.entity),
        point: hit.point,
        normal: hit.normal,
        walkable: hit.walkable,
        distance: hit.distance,
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
/// order, agents ascending; a tick the game held prints none), whatever order `channels` names them
/// in. A motor line carries the `MoveReport` only when
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
    if ai || ai_states {
        engine
            .world_mut()
            .insert_resource(AiTrace::new(ai, ai_states));
    }
    let mut records = Vec::new();
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
    use super::event_name;
    use serde_json::json;

    #[test]
    fn event_name_reads_unit_and_tagged_variants() {
        assert_eq!(event_name(&json!("Noise")), "Noise");
        assert_eq!(event_name(&json!({"Saw": {"who": 3}})), "Saw");
        assert_eq!(event_name(&json!({"a": 1, "b": 2})), "");
        assert_eq!(event_name(&json!(7)), "");
    }
}
