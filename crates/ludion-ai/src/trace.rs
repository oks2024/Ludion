//! [`AiTrace`] and [`AiRoster`]: what `ai_tick` records for the engine's `trace` and `engine.describe`.
//!
//! Both are plain resources, like [`AiFaults`](crate::AiFaults): never registered as a component,
//! never hashed or snapshotted. `AiTrace` is opt-in (the trace driver inserts it); `AiRoster` is
//! written by every `ai_tick` call so a describe can list the machines and their agents.

use std::fmt::Debug;

use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use serde::Serialize;
use serde_json::Value;

use crate::machine::Machine;
use crate::mind::{Mind, active_path};

/// Why a transition happened.
#[derive(Clone, Debug, PartialEq)]
pub enum AiCause {
    /// The agent entered its authored state for the first time.
    Start,
    /// An inbox event's callback asked for it; carries the event as JSON.
    Event(Value),
    /// An `update` callback asked for it.
    Update,
    /// A `Cx::go` from `enter`/`exit`, or a transition settled right after the start.
    Chain,
}

/// One transition of one agent: the active path before and after, root first.
#[derive(Clone, Debug, PartialEq)]
pub struct AiTransition {
    /// The frame index the tick ran as.
    pub tick: u64,
    /// The agent.
    pub entity: EntityId,
    /// The machine's `NAME`.
    pub machine: &'static str,
    /// The active path before; empty for a start.
    pub from: Vec<String>,
    /// The active path after, `initial` chain included. Equal to `from` for a `Restart` of the leaf.
    pub to: Vec<String>,
    /// What asked for it.
    pub cause: AiCause,
}

/// One agent's active path at the end of a tick it ran.
#[derive(Clone, Debug, PartialEq)]
pub struct AiState {
    /// The frame index the tick ran as.
    pub tick: u64,
    /// The agent.
    pub entity: EntityId,
    /// The machine's `NAME`.
    pub machine: &'static str,
    /// The active path, root first.
    pub path: Vec<String>,
}

/// A recorded line: a transition or an agent's per-tick state.
#[derive(Clone, Debug, PartialEq)]
pub enum AiRecord {
    /// One transition.
    Transition(AiTransition),
    /// One agent's path after a tick.
    State(AiState),
}

/// The opt-in record sink: while it is in the world, `ai_tick` appends to it; without it `ai_tick`
/// costs one resource lookup per call. Machines run in schedule order, agents in ascending entity
/// order, so the records are deterministic. A tick the game held records nothing.
///
/// # Example
///
/// ```
/// use ludion_ai::{AiCause, AiRecord, AiTrace, Cx, Machine, Mind, ai_tick};
/// use ludion_core::time::{FrameIndex, TickRate};
/// use ludion_ecs::{SystemCtx, World};
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
/// enum Lamp { Root, Off }
///
/// impl Machine for Lamp {
///     type Blackboard = ();
///     type Event = ();
///     const NAME: &'static str = "Lamp";
///     const ROOT: Self = Lamp::Root;
///     const ALL: &'static [Self] = &[Lamp::Root, Lamp::Off];
///     fn parent(s: Self) -> Option<Self> { (s != Lamp::Root).then_some(Lamp::Root) }
///     fn initial(s: Self) -> Option<Self> { (s == Lamp::Root).then_some(Lamp::Off) }
/// }
///
/// let mut world = World::new();
/// let lamp = world.spawn();
/// world.insert(lamp, Mind::<Lamp>::default()).unwrap();
/// world.insert_resource(AiTrace::new(true, false));
/// ai_tick::<Lamp>(&mut world, &SystemCtx::new(FrameIndex::new(0), TickRate::DEFAULT));
/// let records = world.resource_mut::<AiTrace>().unwrap().take();
/// let [AiRecord::Transition(start)] = records.as_slice() else { panic!("one start line") };
/// assert_eq!(start.cause, AiCause::Start);
/// assert_eq!(start.to, ["Root", "Off"]);
/// ```
#[derive(Debug, Default)]
pub struct AiTrace {
    /// Record transitions.
    pub transitions: bool,
    /// Record each agent's path after each tick it ran.
    pub states: bool,
    records: Vec<AiRecord>,
}

impl AiTrace {
    /// A sink recording the chosen kinds.
    pub fn new(transitions: bool, states: bool) -> Self {
        AiTrace {
            transitions,
            states,
            records: Vec::new(),
        }
    }

    /// Takes every record, in the order recorded.
    pub fn take(&mut self) -> Vec<AiRecord> {
        std::mem::take(&mut self.records)
    }

    pub(crate) fn push(&mut self, record: AiRecord) {
        self.records.push(record);
    }
}

/// A state of a machine and its place in the hierarchy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StateInfo {
    /// The state's serde name.
    pub state: String,
    /// Its parent; `None` for the root.
    pub parent: Option<String>,
    /// The child entered after it; `None` for a leaf.
    pub initial: Option<String>,
}

/// One live agent of a machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentInfo {
    /// The agent.
    pub entity: EntityId,
    /// The active path, root first; empty before the agent starts.
    pub path: Vec<String>,
    /// The gameplay tick each active level was entered, root to leaf.
    pub entered: Vec<u64>,
    /// Events waiting in the inbox.
    pub inbox: usize,
    /// Events that overflowed the inbox since the agent was created.
    pub dropped: u32,
}

/// One machine the world has ticked: its static shape and a way to list its agents.
#[derive(Clone, Debug, Serialize)]
pub struct MachineInfo {
    /// The machine's `NAME`.
    pub machine: &'static str,
    /// The component name, `"{NAME}Mind"`.
    pub component: String,
    /// The root state's serde name.
    pub root: String,
    /// Whether dispatch walks the path leaf first.
    pub leaf_first: bool,
    /// Events an inbox holds before the oldest is dropped.
    pub inbox_capacity: usize,
    /// Every state, in `ALL` order.
    pub states: Vec<StateInfo>,
    /// The gameplay tick of the machine's latest `ai_tick` call.
    pub game_tick: u64,
    #[serde(skip)]
    agents: fn(&World) -> Vec<AgentInfo>,
}

impl MachineInfo {
    /// The machine's agents, ascending entity order.
    pub fn agents(&self, world: &World) -> Vec<AgentInfo> {
        (self.agents)(world)
    }
}

/// Every machine `ai_tick` has run in this world, in first-run order. Present once any machine has
/// ticked, so its absence means the world has no ai.
#[derive(Debug, Default)]
pub struct AiRoster {
    /// The machines.
    pub machines: Vec<MachineInfo>,
}

/// A state's serde name (the one a scene uses), falling back to `Debug`.
pub(crate) fn state_name<M: Serialize + Debug>(s: M) -> String {
    match serde_json::to_value(&s) {
        Ok(Value::String(name)) => name,
        _ => format!("{s:?}"),
    }
}

pub(crate) fn path_names<M: Machine>(path: impl IntoIterator<Item = M>) -> Vec<String> {
    path.into_iter().map(state_name).collect()
}

fn agents_of<M: Machine>(world: &World) -> Vec<AgentInfo> {
    let mut ids: Vec<EntityId> = world
        .entities()
        .filter(|&id| world.component::<Mind<M>>(id).is_some())
        .collect();
    ids.sort();
    ids.into_iter()
        .filter_map(|entity| {
            let mind = world.component::<Mind<M>>(entity)?;
            Some(AgentInfo {
                entity,
                path: path_names(active_path::<M>(world, entity)),
                entered: mind.entered.clone(),
                inbox: mind.inbox.len(),
                dropped: mind.dropped,
            })
        })
        .collect()
}

/// Inserts the roster on first use and upserts `M`'s entry, stamping its gameplay tick.
pub(crate) fn roster_upsert<M: Machine>(world: &mut World, ctx: &SystemCtx) {
    if world.resource::<AiRoster>().is_none() {
        world.insert_resource(AiRoster::default());
    }
    let Some(roster) = world.resource_mut::<AiRoster>() else {
        return;
    };
    let tick = ctx.game_time.ticks();
    if let Some(info) = roster.machines.iter_mut().find(|m| m.machine == M::NAME) {
        info.game_tick = tick;
        return;
    }
    roster.machines.push(MachineInfo {
        machine: M::NAME,
        component: format!("{}Mind", M::NAME),
        root: state_name(M::ROOT),
        leaf_first: M::LEAF_FIRST,
        inbox_capacity: M::INBOX_CAPACITY,
        states: M::ALL
            .iter()
            .map(|&s| StateInfo {
                state: state_name(s),
                parent: M::parent(s).map(state_name),
                initial: M::initial(s).map(state_name),
            })
            .collect(),
        game_tick: tick,
        agents: agents_of::<M>,
    });
}
