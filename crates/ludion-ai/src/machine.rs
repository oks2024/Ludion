//! The [`Machine`] trait, its [`Step`] verdict, and the [`Cx`] every callback receives.

use std::fmt::Debug;

use ludion_core::ids::EntityId;
use ludion_ecs::{Component, SystemCtx, World};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::group;
use crate::mind;
use crate::trace::AiCause;

/// What a callback asks the machine to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step<M> {
    /// Stay where it is; the walk continues to the next level.
    Stay,
    /// Move to this state. A no-op when it is already on the active path.
    Go(M),
    /// Exit through this state and enter it again. Acts as [`Go`](Step::Go) when it is inactive.
    Restart(M),
}

/// A hierarchical state machine over a game enum: the enum's variants are the states, its
/// [`parent`](Machine::parent) fn is the hierarchy, and every callback is plain Rust.
///
/// `ai_tick::<M>` drives it; see the crate docs for the dispatch contract. The enum is the
/// component's `state` field (`"{NAME}Mind"` in a scene), so it serializes by variant name.
///
/// # Example
///
/// ```
/// use ludion_ai::{Cx, Machine, Step};
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
/// enum Door { Root, Closed, Open }
///
/// impl Machine for Door {
///     type Blackboard = ();
///     type Event = ();
///     const NAME: &'static str = "Door";
///     const ROOT: Self = Door::Root;
///     const ALL: &'static [Self] = &[Door::Root, Door::Closed, Door::Open];
///     fn parent(s: Self) -> Option<Self> {
///         (s != Door::Root).then_some(Door::Root)
///     }
///     fn initial(s: Self) -> Option<Self> {
///         (s == Door::Root).then_some(Door::Closed)
///     }
///     fn on_event(_: &mut Cx<'_, Self>, s: Self, _: &()) -> Step<Self> {
///         if s == Door::Closed { Step::Go(Door::Open) } else { Step::Stay }
///     }
/// }
/// assert_eq!(Door::parent(Door::Open), Some(Door::Root));
/// ```
pub trait Machine:
    Copy + Eq + Debug + Serialize + DeserializeOwned + JsonSchema + Send + Sync + 'static
{
    /// Per-agent state the callbacks share, stored as a component on the agent. `()` stores
    /// nothing. Must be registered by the game when it is not `()`.
    type Blackboard: Component + Default;
    /// What other systems and agents can send an agent. Keep it free of entity references: a
    /// reference inside the inbox list is refused at registration (`L1392`); carry a reference in
    /// a single blackboard field instead.
    type Event: Clone + Debug + Serialize + DeserializeOwned + JsonSchema + Send + Sync + 'static;

    /// The component registers as `"{NAME}Mind"`.
    const NAME: &'static str;
    /// The top of the hierarchy: every state descends from it, and it has no parent.
    const ROOT: Self;
    /// Every variant, validated at registration.
    const ALL: &'static [Self];
    /// Walk the active path leaf first instead of root first when dispatching events and updates.
    const LEAF_FIRST: bool = false;
    /// How many events an inbox holds before the oldest is dropped.
    const INBOX_CAPACITY: usize = 16;

    /// The enclosing state, `None` only for [`ROOT`](Machine::ROOT).
    fn parent(s: Self) -> Option<Self>;
    /// The child entered after `s` is entered, `None` for a leaf. Must have `s` as its parent.
    fn initial(_s: Self) -> Option<Self> {
        None
    }
    /// Runs when `s` is entered.
    fn enter(_cx: &mut Cx<'_, Self>, _s: Self) {}
    /// Runs when `s` is exited.
    fn exit(_cx: &mut Cx<'_, Self>, _s: Self) {}
    /// Runs on every tick gameplay time moves, for each active level, after the events. Under a
    /// gameplay-time scale integrate `ctx.game_dt`, not a fixed step.
    fn update(_cx: &mut Cx<'_, Self>, _s: Self) -> Step<Self> {
        Step::Stay
    }
    /// Runs for each inbox event, for each active level.
    fn on_event(_cx: &mut Cx<'_, Self>, _s: Self, _e: &Self::Event) -> Step<Self> {
        Step::Stay
    }
}

/// What a [`Machine`] callback can see and do: the world, the clock, the agent's blackboard and its
/// squad.
///
/// Inside a callback the agent's own `Mind` and blackboard are taken out of the world (they read as
/// defaults through [`world`](Cx::world)): use [`bb`](Cx::bb), and [`send`](Cx::send) /
/// [`go`](Cx::go) rather than writing the component.
pub struct Cx<'a, M: Machine> {
    /// The whole world, minus this agent's own `Mind` and blackboard.
    pub world: &'a mut World,
    /// The tick's context; read gameplay time through `ctx.game_time`.
    pub ctx: &'a SystemCtx,
    /// The agent being run.
    pub agent: EntityId,
    /// The agent's blackboard.
    pub bb: &'a mut M::Blackboard,
    /// The agent's squad entity, when its [`GroupMember`](crate::GroupMember) names a live
    /// [`Group`](crate::Group).
    pub group: Option<EntityId>,
    pub(crate) active: Vec<(M, u64)>,
    pub(crate) pending: Option<(M, bool)>,
    pub(crate) own_inbox: Vec<M::Event>,
    pub(crate) transitions: u32,
    pub(crate) capped: bool,
    pub(crate) tracing: bool,
    pub(crate) cause: Option<AiCause>,
}

impl<M: Machine> Cx<'_, M> {
    /// The gameplay tick `s` was entered, if it is active.
    pub fn ticks_in(&self, s: M) -> Option<u64> {
        self.active
            .iter()
            .find(|(state, _)| *state == s)
            .map(|(_, tick)| self.ctx.game_time.ticks().saturating_sub(*tick))
    }

    /// Gameplay seconds since `s` was entered; `0.0` when it is not active.
    pub fn time_in(&self, s: M) -> f32 {
        self.ticks_in(s).unwrap_or(0) as f32 / self.ctx.game_time.rate().hz() as f32
    }

    /// Sends `event` to `to` (this agent included: it arrives next tick). `false` when `to` has no
    /// `Mind` of this machine (`L4123`).
    pub fn send(&mut self, to: EntityId, event: M::Event) -> bool {
        if to == self.agent {
            self.own_inbox.push(event);
            true
        } else {
            mind::send::<M>(self.world, to, event)
        }
    }

    /// Sends `event` to every other member of `group`, in `(role, entity)` order.
    pub fn broadcast(&mut self, group: EntityId, event: &M::Event) {
        for member in group::members(self.world, group) {
            if member != self.agent {
                self.send(member, event.clone());
            }
        }
    }

    /// Requests a move to `target`, applied once the running callback returns. Valid in `enter`
    /// and `exit`, where a [`Step`] cannot be returned; the last request wins.
    pub fn go(&mut self, target: M) {
        self.pending = Some((target, false));
    }
}
