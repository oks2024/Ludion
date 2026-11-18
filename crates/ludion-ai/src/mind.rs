//! [`Mind`]: the per-agent component, and the free fns that reach it.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{Registry, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::faults::fault;
use crate::group::{Group, GroupMember};
use crate::machine::Machine;

fn root<M: Machine>() -> M {
    M::ROOT
}

/// An agent's machine: its current state, when each active level was entered, and its event inbox.
///
/// A scene authors only `state` (any level; the agent starts there and descends through
/// [`initial`](Machine::initial)). `entered` is the runtime stamp, root to leaf, in gameplay ticks;
/// empty means not started. Registered as `"{NAME}Mind"` by [`register_machine`].
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(bound = "")]
pub struct Mind<M: Machine> {
    /// The authored start state; at runtime, the active leaf.
    #[serde(default = "root::<M>")]
    pub state: M,
    /// The gameplay tick each active level was entered, root to leaf.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entered: Vec<u64>,
    /// Events waiting for the next tick, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inbox: Vec<M::Event>,
    /// How many events overflowed the inbox since the agent was created.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dropped: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl<M: Machine> Default for Mind<M> {
    fn default() -> Self {
        Mind {
            state: M::ROOT,
            entered: Vec::new(),
            inbox: Vec::new(),
            dropped: 0,
        }
    }
}

impl<M: Machine> Mind<M> {
    /// A mind that will start in `state`.
    pub fn starting_in(state: M) -> Self {
        Mind {
            state,
            ..Mind::default()
        }
    }

    /// Queues `event`, dropping the oldest when the inbox is full. Returns whether one was dropped.
    pub(crate) fn push(&mut self, event: M::Event) -> bool {
        let overflow = self.inbox.len() >= M::INBOX_CAPACITY.max(1);
        if overflow {
            self.inbox.remove(0);
            self.dropped = self.dropped.saturating_add(1);
        }
        self.inbox.push(event);
        overflow
    }
}

/// `s` and its ancestors, root first. Bounded by `M::ALL.len()`, so a malformed hierarchy cannot
/// loop (registration refuses one).
pub(crate) fn path_to<M: Machine>(s: M) -> Vec<M> {
    let mut path = vec![s];
    let mut at = s;
    while let Some(parent) = M::parent(at) {
        if path.len() > M::ALL.len() {
            break;
        }
        path.push(parent);
        at = parent;
    }
    path.reverse();
    path
}

/// The states active for the agent: root first, leaf last. Empty when the agent has no `Mind<M>`
/// or has not started.
///
/// # Example
///
/// ```
/// # use ludion_ai::{Machine, Mind, active_path};
/// # use schemars::JsonSchema;
/// # use serde::{Deserialize, Serialize};
/// # #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
/// # enum S { Root, Idle }
/// # impl Machine for S {
/// #     type Blackboard = (); type Event = ();
/// #     const NAME: &'static str = "S";
/// #     const ROOT: Self = S::Root;
/// #     const ALL: &'static [Self] = &[S::Root, S::Idle];
/// #     fn parent(s: Self) -> Option<Self> { (s != S::Root).then_some(S::Root) }
/// # }
/// let mut world = ludion_ecs::World::new();
/// let agent = world.spawn();
/// world.insert(agent, Mind::<S>::default()).unwrap();
/// assert!(active_path::<S>(&world, agent).is_empty()); // not started yet
/// ```
pub fn active_path<M: Machine>(world: &World, agent: EntityId) -> Vec<M> {
    match world.component::<Mind<M>>(agent) {
        Some(mind) if !mind.entered.is_empty() => path_to(mind.state),
        _ => Vec::new(),
    }
}

/// Queues `event` on `to`'s inbox for the machine `M`; the agent handles it on its next dispatch (this
/// tick when it has not run yet, otherwise the next). For systems outside the machine, such as
/// perception. Returns `false` and raises `L4123` when `to` has no `Mind<M>`; a full inbox drops its
/// oldest event and raises `L4121`.
///
/// # Example
///
/// ```
/// # use ludion_ai::{Machine, Mind, send};
/// # use schemars::JsonSchema;
/// # use serde::{Deserialize, Serialize};
/// # #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
/// # enum S { Root }
/// # impl Machine for S {
/// #     type Blackboard = (); type Event = u8;
/// #     const NAME: &'static str = "S";
/// #     const ROOT: Self = S::Root;
/// #     const ALL: &'static [Self] = &[S::Root];
/// #     fn parent(_: Self) -> Option<Self> { None }
/// # }
/// let mut world = ludion_ecs::World::new();
/// let agent = world.spawn();
/// world.insert(agent, Mind::<S>::default()).unwrap();
/// assert!(send::<S>(&mut world, agent, 7));
/// assert_eq!(world.component::<Mind<S>>(agent).unwrap().inbox, vec![7]);
/// ```
pub fn send<M: Machine>(world: &mut World, to: EntityId, event: M::Event) -> bool {
    let Some(mind) = world.component_mut::<Mind<M>>(to) else {
        fault(
            world,
            to,
            code::AI_SEND_NO_MIND,
            format!(
                "an event was sent to {to}, which has no `{}Mind`; it was dropped",
                M::NAME
            ),
        );
        return false;
    };
    if mind.push(event) {
        overflow_fault::<M>(world, to);
    }
    true
}

pub(crate) fn overflow_fault<M: Machine>(world: &mut World, to: EntityId) {
    fault(
        world,
        to,
        code::AI_INBOX_OVERFLOW,
        format!(
            "the `{}Mind` inbox of {to} is full ({} events); the oldest event was dropped",
            M::NAME,
            M::INBOX_CAPACITY
        ),
    );
}

fn refuse(code: ludion_core::diagnostics::DiagnosticCode, message: String) -> Diagnostic {
    Diagnostic::new(code, message)
}

/// Checks the machine's tables and registers `Mind<M>` as `"{NAME}Mind"`, plus
/// [`Group`] and [`GroupMember`] when no machine has yet.
///
/// # Errors
///
/// `L1512` for an `ALL` that is empty, lacks `ROOT` or repeats a variant; `L1510` for a parent
/// chain that does not reach `ROOT` (or a `ROOT` with a parent); `L1511` for an `initial` that is
/// not a child of its state; the registry's own refusals otherwise (`L0005` for a name taken,
/// `L1392` for an event type holding a marked entity reference).
///
/// # Example
///
/// ```
/// # use ludion_ai::{Machine, register_machine};
/// # use schemars::JsonSchema;
/// # use serde::{Deserialize, Serialize};
/// # #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
/// # enum S { Root, Idle }
/// # impl Machine for S {
/// #     type Blackboard = (); type Event = ();
/// #     const NAME: &'static str = "Guard";
/// #     const ROOT: Self = S::Root;
/// #     const ALL: &'static [Self] = &[S::Root, S::Idle];
/// #     fn parent(s: Self) -> Option<Self> { (s != S::Root).then_some(S::Root) }
/// # }
/// let mut registry = ludion_ecs::Registry::new();
/// register_machine::<S>(&mut registry).unwrap();
/// assert!(registry.get("GuardMind").is_some() && registry.get("Group").is_some());
/// ```
pub fn register_machine<M: Machine>(registry: &mut Registry) -> Result<(), Diagnostic> {
    validate::<M>()?;
    if registry.get("Group").is_none() {
        registry.register::<Group>("Group")?;
        registry.register::<GroupMember>("GroupMember")?;
    }
    registry.register::<Mind<M>>(&format!("{}Mind", M::NAME))
}

fn validate<M: Machine>() -> Result<(), Diagnostic> {
    let name = M::NAME;
    if M::ALL.is_empty() || !M::ALL.contains(&M::ROOT) {
        return Err(refuse(
            code::AI_MACHINE_VARIANTS,
            format!(
                "machine `{name}`: `ALL` must list every variant and include `ROOT` ({:?})",
                M::ROOT
            ),
        ));
    }
    for (i, s) in M::ALL.iter().enumerate() {
        if M::ALL[..i].contains(s) {
            return Err(refuse(
                code::AI_MACHINE_VARIANTS,
                format!("machine `{name}`: `ALL` lists {s:?} more than once"),
            ));
        }
    }
    if let Some(parent) = M::parent(M::ROOT) {
        return Err(refuse(
            code::AI_MACHINE_HIERARCHY,
            format!(
                "machine `{name}`: `ROOT` ({:?}) must have no parent, but `parent` returns {parent:?}",
                M::ROOT
            ),
        ));
    }
    for &s in M::ALL {
        let mut at = s;
        let mut steps = 0;
        while at != M::ROOT {
            match M::parent(at) {
                Some(parent) if steps < M::ALL.len() => at = parent,
                _ => {
                    return Err(refuse(
                        code::AI_MACHINE_HIERARCHY,
                        format!(
                            "machine `{name}`: the parent chain of {s:?} does not reach `ROOT` ({:?}); \
                             give every non-root state a `parent`, without cycles",
                            M::ROOT
                        ),
                    ));
                }
            }
            steps += 1;
        }
        if let Some(child) = M::initial(s)
            && M::parent(child) != Some(s)
        {
            return Err(refuse(
                code::AI_MACHINE_INITIAL,
                format!(
                    "machine `{name}`: `initial({s:?})` is {child:?}, whose `parent` is {:?}, not {s:?}",
                    M::parent(child)
                ),
            ));
        }
    }
    Ok(())
}
