//! `AnimGraph`: a game's Rust state graph over `Animation` and `AnimationLayers`
//! (`Plan/animation-layers.md` decisions 9 and 50-53, Neon Shroud request 0007 §6).
//!
//! The graph is a `static` table of states and edges plus `fn` conditions, written in the game's own
//! crate; it is not an asset and not a pose source. [`AnimGraph::step`] is called from a game system
//! registered after [`animation_events`](crate::animation_events). It writes `Animation`,
//! `AnimationLayers` and [`AnimGraphState`] only on a transition, so the hash stays quiet between them.
//! An entity starts in a state because the game spawns `AnimGraphState` beside an `Animation` already
//! playing that state's clip: there is no implicit entry.
//!
//! `step` walks `edges` in declaration order; the first edge that is a candidate (its `from` is `None` or
//! the current state, and its `to` is not the current state) and whose `when` and [`AnimGate`] pass is taken.
//! While the current state is `one_shot` and its clip has not ended, only [`AnimGate::Interrupts`] edges
//! qualify.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::suggest::nearest;
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};

use crate::animation_events::AnimationEvents;
use crate::components::{
    AnimGraphState, Animation, AnimationLayers, crossfade, fade_layer, play_layer,
};

/// One state of an [`AnimGraph`]: the clip it plays, the layers it turns on, and whether it must finish.
#[derive(Debug)]
pub struct AnimState<S> {
    /// The id edges refer to.
    pub id: S,
    /// The stable name written into [`AnimGraphState`]; unique within the graph.
    pub name: &'static str,
    /// The base clip this state plays (`Animation.clip`).
    pub plays: &'static str,
    /// `(layer, clip, weight)`: each layer is played and faded to `weight` on entry, and faded to `0`
    /// on leaving a state that lists it when the next state does not.
    pub layers: &'static [(&'static str, &'static str, f32)],
    /// A one-shot state only leaves through an [`AnimGate::Interrupts`] edge until its clip's `end` fires.
    pub one_shot: bool,
}

/// What an [`AnimEdge`] needs besides its `when` condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimGate {
    /// Nothing more.
    None,
    /// Nothing more, and the edge may also leave a one-shot state before it ends.
    Interrupts,
    /// The named event fired this tick on the current state's clip.
    OnEvent(&'static str),
    /// At least this many gameplay ticks passed since the state was entered.
    MinTicks(u64),
}

/// A transition: `from` (`None` matches any state) to `to`, taken when `when(inputs)` and `gate` pass.
#[derive(Debug)]
pub struct AnimEdge<S, I> {
    /// The state this edge leaves, or `None` for any.
    pub from: Option<S>,
    /// The state this edge enters.
    pub to: S,
    /// The game's condition over its own inputs.
    pub when: fn(&I) -> bool,
    /// The crossfade and layer fade length in seconds; `0.0` is a hard cut.
    pub fade: f32,
    /// The extra requirement.
    pub gate: AnimGate,
}

/// A table of states and edges. Write it as a `static` in the game's crate.
///
/// ```
/// use ludion_engine::{AnimEdge, AnimGraph, AnimState, AnimGate};
///
/// #[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// enum Pose {
///     Idle,
///     Run,
/// }
/// struct Inputs {
///     moving: bool,
/// }
///
/// static GRAPH: AnimGraph<Pose, Inputs> = AnimGraph {
///     states: &[
///         AnimState { id: Pose::Idle, name: "idle", plays: "idle", layers: &[], one_shot: false },
///         AnimState { id: Pose::Run, name: "run", plays: "run", layers: &[], one_shot: false },
///     ],
///     edges: &[
///         AnimEdge { from: Some(Pose::Idle), to: Pose::Run, when: |i| i.moving, fade: 0.2, gate: AnimGate::None },
///         AnimEdge { from: Some(Pose::Run), to: Pose::Idle, when: |i| !i.moving, fade: 0.2, gate: AnimGate::MinTicks(10) },
///     ],
/// };
///
/// assert_eq!(GRAPH.describe().lines().count(), 4);
/// ```
#[derive(Debug)]
pub struct AnimGraph<S: 'static, I: 'static> {
    /// The states, in declaration order.
    pub states: &'static [AnimState<S>],
    /// The edges, in the order `step` tries them.
    pub edges: &'static [AnimEdge<S, I>],
}

impl<S: Copy + Eq + std::fmt::Debug, I> AnimGraph<S, I> {
    /// Advances `entity` by one tick: takes the first passing edge, writing `Animation`,
    /// `AnimationLayers` and `AnimGraphState`, and returns the entered state; `Ok(None)` writes nothing.
    ///
    /// # Errors
    /// `L4291` for a malformed table; `L4290` when the entity has no `AnimGraphState` or `Animation`,
    /// its state names no declared state, or a transition lists a layer its `AnimationLayers` lacks.
    /// An error writes nothing.
    pub fn step(
        &self,
        world: &mut World,
        entity: EntityId,
        inputs: &I,
        ctx: &SystemCtx,
    ) -> Result<Option<S>, Diagnostic> {
        self.validate()?;
        let (current, entered) = self.current_state(world, entity)?;
        if world.component::<Animation>(entity).is_none() {
            return Err(invalid(format!(
                "entity {entity} has no `Animation` for the graph state `{}`",
                current.name
            ))
            .with_help("spawn an `Animation` playing the state's clip beside `AnimGraphState`"));
        }
        let elapsed = ctx.game_frame.as_u64().saturating_sub(entered.as_u64());
        let events = world.resource::<AnimationEvents>();
        let fired = |name: &str| events.is_some_and(|e| e.fired(entity, current.plays, name));
        let ended = fired("end");
        let taken = self.edges.iter().find(|edge| {
            if edge.from.is_some_and(|from| from != current.id) || edge.to == current.id {
                return false;
            }
            let gate_ok = match edge.gate {
                AnimGate::None | AnimGate::Interrupts => true,
                AnimGate::OnEvent(name) => fired(name),
                AnimGate::MinTicks(n) => elapsed >= n,
            };
            let blocked = current.one_shot && !ended && edge.gate != AnimGate::Interrupts;
            gate_ok && !blocked && (edge.when)(inputs)
        });
        let Some(edge) = taken else {
            return Ok(None);
        };
        let to = self
            .states
            .iter()
            .find(|state| state.id == edge.to)
            .expect("validate declared every edge target");

        // Check every layer before the first write, so an error writes nothing.
        let leaving: Vec<&str> = current
            .layers
            .iter()
            .map(|(layer, _, _)| *layer)
            .filter(|layer| !to.layers.iter().any(|(entered, _, _)| entered == layer))
            .collect();
        if !to.layers.is_empty() || !leaving.is_empty() {
            let layers = world.component::<AnimationLayers>(entity).ok_or_else(|| {
                invalid(format!(
                    "entity {entity} has no `AnimationLayers`, but graph state `{}` or `{}` lists layers",
                    current.name, to.name
                ))
                .with_help("add an `AnimationLayers` component declaring every layer the graph names")
            })?;
            let named = to
                .layers
                .iter()
                .map(|(layer, _, _)| *layer)
                .chain(leaving.iter().copied());
            for layer in named {
                if !layers.layers.iter().any(|l| l.name == layer) {
                    let have: Vec<&str> = layers.layers.iter().map(|l| l.name.as_str()).collect();
                    return Err(invalid(format!(
                        "entity {entity} has no layer `{layer}` (graph state `{}` -> `{}`)",
                        current.name, to.name
                    ))
                    .with_help(format!(
                        "declare it in `AnimationLayers`; the entity has {have:?}"
                    )));
                }
            }
        }

        let now = ctx.game_time;
        if let Some(animation) = world.get_mut::<Animation>(entity)? {
            crossfade(animation, to.plays, edge.fade, ctx.game_frame);
        }
        if let Some(state) = world.get_mut::<AnimGraphState>(entity)? {
            *state = AnimGraphState {
                state: to.name.to_string(),
                entered: ctx.game_frame,
                previous: current.name.to_string(),
            };
        }
        if (!to.layers.is_empty() || !leaving.is_empty())
            && let Some(layers) = world.get_mut::<AnimationLayers>(entity)?
        {
            for (layer, clip, weight) in to.layers {
                play_layer(layers, layer, clip, now);
                fade_layer(layers, layer, *weight, edge.fade, now);
            }
            for layer in leaving {
                fade_layer(layers, layer, 0.0, edge.fade, now);
            }
        }
        Ok(Some(to.id))
    }

    /// The id of the state `entity`'s `AnimGraphState` names.
    ///
    /// # Errors
    /// `L4290` when the entity has no `AnimGraphState` or it names no declared state; `L4291` for a
    /// malformed table.
    pub fn current(&self, world: &World, entity: EntityId) -> Result<S, Diagnostic> {
        self.validate()?;
        self.current_state(world, entity).map(|(state, _)| state.id)
    }

    /// The table as text: one line per state (`name plays [layer=clip@weight ...] one_shot`), then one
    /// line per edge (`from -> to gate fade`, `*` for any `from`), in declaration order.
    pub fn describe(&self) -> String {
        let name_of = |id: S| {
            self.states
                .iter()
                .find(|state| state.id == id)
                .map_or("?", |state| state.name)
        };
        let mut out = String::new();
        for state in self.states {
            let layers: Vec<String> = state
                .layers
                .iter()
                .map(|(layer, clip, weight)| format!("{layer}={clip}@{weight}"))
                .collect();
            out.push_str(&format!(
                "{} {} [{}] {}\n",
                state.name,
                state.plays,
                layers.join(" "),
                state.one_shot
            ));
        }
        for edge in self.edges {
            let gate = match edge.gate {
                AnimGate::None => "none".to_string(),
                AnimGate::Interrupts => "interrupts".to_string(),
                AnimGate::OnEvent(name) => format!("on_event({name})"),
                AnimGate::MinTicks(n) => format!("min_ticks({n})"),
            };
            out.push_str(&format!(
                "{} -> {} {} {}\n",
                edge.from.map_or("*", name_of),
                name_of(edge.to),
                gate,
                edge.fade
            ));
        }
        out
    }

    fn current_state(
        &self,
        world: &World,
        entity: EntityId,
    ) -> Result<(&AnimState<S>, FrameIndex), Diagnostic> {
        let Some(held) = world.component::<AnimGraphState>(entity) else {
            return Err(invalid(format!("entity {entity} has no `AnimGraphState`"))
                .with_help("spawn `AnimGraphState { state: \"<a declared name>\", .. }` beside an `Animation` playing that state's clip"));
        };
        self.states
            .iter()
            .find(|state| state.name == held.state)
            .map(|state| (state, held.entered))
            .ok_or_else(|| {
                let declared: Vec<&str> = self.states.iter().map(|s| s.name).collect();
                let help = match nearest(&held.state, declared.iter().copied()) {
                    Some(near) => format!("the nearest declared name is `{near}`; declared: {declared:?}"),
                    None => format!("declared names: {declared:?}"),
                };
                invalid(format!(
                    "entity {entity}'s `AnimGraphState` names `{}`, which the graph does not declare",
                    held.state
                ))
                .with_help(help)
            })
    }

    fn validate(&self) -> Result<(), Diagnostic> {
        for (index, state) in self.states.iter().enumerate() {
            if self.states[..index]
                .iter()
                .any(|other| other.id == state.id)
            {
                return Err(malformed(format!("two states share the id {:?}", state.id))
                    .with_help("give every `AnimState` a unique `id`"));
            }
            if self.states[..index]
                .iter()
                .any(|other| other.name == state.name)
            {
                return Err(malformed(format!("two states are named `{}`", state.name))
                    .with_help("give every `AnimState` a unique `name`"));
            }
        }
        for (index, edge) in self.edges.iter().enumerate() {
            let declared = |id: S| self.states.iter().any(|state| state.id == id);
            let bad = if !declared(edge.to) {
                Some(("to", format!("{:?}", edge.to)))
            } else {
                edge.from
                    .filter(|from| !declared(*from))
                    .map(|from| ("from", format!("{from:?}")))
            };
            if let Some((side, id)) = bad {
                return Err(malformed(format!(
                    "edge {index} names the undeclared state {id} in `{side}`"
                ))
                .with_help("declare every state an edge uses in `states`"));
            }
        }
        Ok(())
    }
}

fn invalid(message: String) -> Diagnostic {
    Diagnostic::new(code::ANIM_GRAPH_STATE_INVALID, message)
}

fn malformed(message: String) -> Diagnostic {
    Diagnostic::new(code::ANIM_GRAPH_INVALID, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation_events::ClipEvent;
    use ludion_core::time::FrameIndex;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum S {
        Idle,
        Run,
        Wave,
        Carry,
    }

    struct In {
        go: bool,
        wave: bool,
    }

    const OFF: In = In {
        go: false,
        wave: false,
    };
    const GO: In = In {
        go: true,
        wave: false,
    };
    const WAVE: In = In {
        go: false,
        wave: true,
    };

    static GRAPH: AnimGraph<S, In> = AnimGraph {
        states: &[
            AnimState {
                id: S::Idle,
                name: "idle",
                plays: "idle",
                layers: &[],
                one_shot: false,
            },
            AnimState {
                id: S::Run,
                name: "run",
                plays: "run",
                layers: &[],
                one_shot: false,
            },
            AnimState {
                id: S::Wave,
                name: "wave",
                plays: "wave",
                layers: &[],
                one_shot: true,
            },
            AnimState {
                id: S::Carry,
                name: "carry",
                plays: "idle",
                layers: &[("carry", "carry", 1.0)],
                one_shot: false,
            },
        ],
        edges: &[
            AnimEdge {
                from: None,
                to: S::Wave,
                when: |i| i.wave,
                fade: 0.0,
                gate: AnimGate::Interrupts,
            },
            AnimEdge {
                from: Some(S::Idle),
                to: S::Run,
                when: |i| i.go,
                fade: 0.25,
                gate: AnimGate::MinTicks(5),
            },
            AnimEdge {
                from: Some(S::Run),
                to: S::Idle,
                when: |i| !i.go,
                fade: 0.1,
                gate: AnimGate::None,
            },
            AnimEdge {
                from: Some(S::Wave),
                to: S::Idle,
                when: |_| true,
                fade: 0.15,
                gate: AnimGate::OnEvent("end"),
            },
            AnimEdge {
                from: Some(S::Idle),
                to: S::Carry,
                when: |i| i.go && i.wave,
                fade: 0.2,
                gate: AnimGate::None,
            },
            AnimEdge {
                from: Some(S::Carry),
                to: S::Idle,
                when: |i| !i.go,
                fade: 0.2,
                gate: AnimGate::None,
            },
        ],
    };

    fn spawn(state: &str, entered: u64, with_layer: bool) -> (World, EntityId) {
        let mut world = World::new();
        let entity = world.spawn();
        let clip = GRAPH
            .states
            .iter()
            .find(|s| s.name == state)
            .map_or("", |s| s.plays);
        world
            .insert(
                entity,
                Animation {
                    clip: clip.into(),
                    ..Animation::default()
                },
            )
            .unwrap();
        world
            .insert(
                entity,
                AnimGraphState {
                    state: state.into(),
                    entered: FrameIndex(entered),
                    previous: String::new(),
                },
            )
            .unwrap();
        if with_layer {
            let layers = AnimationLayers {
                layers: vec![crate::components::AnimationLayer {
                    name: "carry".into(),
                    weight: if state == "carry" { 1.0 } else { 0.0 },
                    ..Default::default()
                }],
            };
            world.insert(entity, layers).unwrap();
        }
        (world, entity)
    }

    fn ctx(frame: u64) -> SystemCtx {
        SystemCtx::new(FrameIndex(frame), ludion_core::time::TickRate::DEFAULT)
    }

    fn fire(world: &mut World, entity: EntityId, clip: &str, name: &str) {
        world.insert_resource(AnimationEvents {
            fired: vec![ClipEvent {
                entity,
                clip: clip.into(),
                name: name.into(),
                layer: String::new(),
            }],
        });
    }

    fn json(world: &World, entity: EntityId) -> String {
        format!(
            "{}{}{}",
            serde_json::to_string(world.component::<Animation>(entity).unwrap()).unwrap(),
            serde_json::to_string(world.component::<AnimGraphState>(entity).unwrap()).unwrap(),
            world
                .component::<AnimationLayers>(entity)
                .map_or(String::new(), |l| serde_json::to_string(l).unwrap()),
        )
    }

    fn step(world: &mut World, entity: EntityId, inputs: &In, frame: u64) -> Option<S> {
        GRAPH.step(world, entity, inputs, &ctx(frame)).unwrap()
    }

    #[test]
    fn the_first_passing_edge_in_declaration_order_wins() {
        let (mut world, entity) = spawn("idle", 0, true);
        // `wave` (from None) and `go && wave` (Idle -> Carry) both pass: the earlier edge wins.
        let both = In {
            go: true,
            wave: true,
        };
        assert_eq!(step(&mut world, entity, &both, 10), Some(S::Wave));
        assert_eq!(GRAPH.current(&world, entity).unwrap(), S::Wave);
    }

    #[test]
    fn a_from_none_edge_never_restarts_its_own_target() {
        let (mut world, entity) = spawn("wave", 0, false);
        assert_eq!(step(&mut world, entity, &WAVE, 3), None);
    }

    #[test]
    fn each_gate_filters_its_edge() {
        let (mut world, entity) = spawn("idle", 100, false);
        assert_eq!(
            step(&mut world, entity, &GO, 104),
            None,
            "MinTicks(5) not yet"
        );
        assert_eq!(step(&mut world, entity, &GO, 105), Some(S::Run));
        let (mut world, entity) = spawn("run", 0, false);
        assert_eq!(
            step(&mut world, entity, &OFF, 1),
            Some(S::Idle),
            "AnimGate::None"
        );
        let (mut world, entity) = spawn("wave", 0, false);
        assert_eq!(step(&mut world, entity, &OFF, 9), None, "no end event");
        fire(&mut world, entity, "other", "end");
        assert_eq!(step(&mut world, entity, &OFF, 9), None, "wrong clip");
        fire(&mut world, entity, "wave", "end");
        assert_eq!(
            step(&mut world, entity, &OFF, 9),
            Some(S::Idle),
            "OnEvent end"
        );
    }

    #[test]
    fn a_one_shot_finishes_unless_an_interrupts_edge_fires() {
        let (mut world, entity) = spawn("wave", 0, false);
        // `Wave -> Idle` is OnEvent("end"), so without the event nothing leaves.
        assert_eq!(step(&mut world, entity, &GO, 4), None);
        // A `AnimGate::None` edge from the one-shot does not qualify before the end either.
        static NONE_GRAPH: AnimGraph<S, In> = AnimGraph {
            states: &[
                AnimState {
                    id: S::Idle,
                    name: "idle",
                    plays: "idle",
                    layers: &[],
                    one_shot: false,
                },
                AnimState {
                    id: S::Wave,
                    name: "wave",
                    plays: "wave",
                    layers: &[],
                    one_shot: true,
                },
            ],
            edges: &[AnimEdge {
                from: Some(S::Wave),
                to: S::Idle,
                when: |_| true,
                fade: 0.0,
                gate: AnimGate::None,
            }],
        };
        assert_eq!(
            NONE_GRAPH.step(&mut world, entity, &OFF, &ctx(4)).unwrap(),
            None
        );
        fire(&mut world, entity, "wave", "end");
        assert_eq!(
            NONE_GRAPH.step(&mut world, entity, &OFF, &ctx(5)).unwrap(),
            Some(S::Idle),
            "after the end it leaves"
        );
        // An Interrupts edge leaves a one-shot that has not ended.
        static INT_GRAPH: AnimGraph<S, In> = AnimGraph {
            states: &[
                AnimState {
                    id: S::Idle,
                    name: "idle",
                    plays: "idle",
                    layers: &[],
                    one_shot: false,
                },
                AnimState {
                    id: S::Wave,
                    name: "wave",
                    plays: "wave",
                    layers: &[],
                    one_shot: true,
                },
            ],
            edges: &[AnimEdge {
                from: Some(S::Wave),
                to: S::Idle,
                when: |_| true,
                fade: 0.0,
                gate: AnimGate::Interrupts,
            }],
        };
        let (mut world, entity) = spawn("wave", 0, false);
        assert_eq!(
            INT_GRAPH.step(&mut world, entity, &OFF, &ctx(2)).unwrap(),
            Some(S::Idle)
        );
    }

    #[test]
    fn nothing_firing_writes_nothing() {
        let (mut world, entity) = spawn("idle", 0, true);
        let before = json(&world, entity);
        assert_eq!(step(&mut world, entity, &OFF, 50), None);
        assert_eq!(json(&world, entity), before);
    }

    #[test]
    fn a_transition_writes_the_clip_the_state_and_the_layers() {
        let (mut world, entity) = spawn("idle", 0, true);
        let both = In {
            go: true,
            wave: false,
        };
        // Run first (declared before Carry, MinTicks(5)): wait until frame 5, but Carry needs `wave`.
        assert_eq!(step(&mut world, entity, &both, 7), Some(S::Run));
        let state = world.component::<AnimGraphState>(entity).unwrap();
        assert_eq!(
            (state.state.as_str(), state.entered, state.previous.as_str()),
            ("run", FrameIndex(7), "idle")
        );
        let animation = world.component::<Animation>(entity).unwrap();
        assert_eq!(
            (
                animation.clip.as_str(),
                animation.from.as_str(),
                animation.started
            ),
            ("run", "idle", FrameIndex(7))
        );
        // Leaving `carry` for `idle` fades the layer it listed to 0.
        let (mut world, entity) = spawn("carry", 0, true);
        assert_eq!(step(&mut world, entity, &OFF, 9), Some(S::Idle));
        let layer = &world.component::<AnimationLayers>(entity).unwrap().layers[0];
        assert_eq!(
            layer.weight, 0.0,
            "the layer the next state does not list fades to 0"
        );
        assert_eq!(layer.fade, 0.2);
    }

    #[test]
    fn errors_write_nothing() {
        // No layers on the entity, but entering Carry lists one.
        let (mut world, entity) = spawn("idle", 0, false);
        let before = json(&world, entity);
        let err = CARRY_ONLY
            .step(&mut world, entity, &GO, &ctx(3))
            .expect_err("a missing AnimationLayers is an error");
        assert_eq!(err.code.to_string(), "L4290");
        assert_eq!(json(&world, entity), before);

        let (mut world, entity) = spawn("idle", 0, true);
        world
            .get_mut::<AnimationLayers>(entity)
            .unwrap()
            .unwrap()
            .layers[0]
            .name = "other".into();
        let before = json(&world, entity);
        let err = CARRY_ONLY
            .step(&mut world, entity, &GO, &ctx(3))
            .expect_err("a missing layer");
        assert_eq!(err.code.to_string(), "L4290");
        assert_eq!(json(&world, entity), before);

        let (mut world, entity) = spawn("idel", 0, false);
        let err = GRAPH
            .step(&mut world, entity, &GO, &ctx(3))
            .expect_err("an unknown state");
        assert_eq!(err.code.to_string(), "L4290");
        assert!(
            err.help.unwrap_or_default().contains("`idle`"),
            "names the nearest"
        );

        let mut world = World::new();
        let entity = world.spawn();
        assert_eq!(
            GRAPH
                .step(&mut world, entity, &GO, &ctx(3))
                .unwrap_err()
                .code
                .to_string(),
            "L4290"
        );
        world
            .insert(
                entity,
                AnimGraphState {
                    state: "idle".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            GRAPH
                .step(&mut world, entity, &GO, &ctx(3))
                .unwrap_err()
                .code
                .to_string(),
            "L4290",
            "no Animation"
        );
    }

    static CARRY_ONLY: AnimGraph<S, In> = AnimGraph {
        states: &[
            AnimState {
                id: S::Idle,
                name: "idle",
                plays: "idle",
                layers: &[],
                one_shot: false,
            },
            AnimState {
                id: S::Carry,
                name: "carry",
                plays: "idle",
                layers: &[("carry", "carry", 1.0)],
                one_shot: false,
            },
        ],
        edges: &[AnimEdge {
            from: Some(S::Idle),
            to: S::Carry,
            when: |i| i.go,
            fade: 0.2,
            gate: AnimGate::None,
        }],
    };

    #[test]
    fn a_malformed_table_is_l4291() {
        static DUPLICATE: AnimGraph<S, In> = AnimGraph {
            states: &[
                AnimState {
                    id: S::Idle,
                    name: "x",
                    plays: "a",
                    layers: &[],
                    one_shot: false,
                },
                AnimState {
                    id: S::Run,
                    name: "x",
                    plays: "b",
                    layers: &[],
                    one_shot: false,
                },
            ],
            edges: &[],
        };
        static UNDECLARED: AnimGraph<S, In> = AnimGraph {
            states: &[AnimState {
                id: S::Idle,
                name: "idle",
                plays: "a",
                layers: &[],
                one_shot: false,
            }],
            edges: &[AnimEdge {
                from: None,
                to: S::Run,
                when: |_| true,
                fade: 0.0,
                gate: AnimGate::None,
            }],
        };
        let (mut world, entity) = spawn("idle", 0, false);
        for graph in [&DUPLICATE, &UNDECLARED] {
            let err = graph.step(&mut world, entity, &GO, &ctx(1)).unwrap_err();
            assert_eq!(err.code.to_string(), "L4291");
        }
        assert_eq!(
            UNDECLARED
                .current(&world, entity)
                .unwrap_err()
                .code
                .to_string(),
            "L4291"
        );
    }

    #[test]
    fn duplicate_state_ids_are_l4291() {
        static SAME_ID: AnimGraph<S, In> = AnimGraph {
            states: &[
                AnimState {
                    id: S::Idle,
                    name: "a",
                    plays: "a",
                    layers: &[],
                    one_shot: false,
                },
                AnimState {
                    id: S::Idle,
                    name: "b",
                    plays: "b",
                    layers: &[],
                    one_shot: false,
                },
            ],
            edges: &[],
        };
        let (mut world, entity) = spawn("idle", 0, false);
        let err = SAME_ID.step(&mut world, entity, &GO, &ctx(1)).unwrap_err();
        assert_eq!(err.code.to_string(), "L4291");
        assert!(err.message.contains("share the id"), "{}", err.message);
    }

    #[test]
    fn describe_pins_the_table_as_text() {
        assert_eq!(
            GRAPH.describe(),
            "\
idle idle [] false
run run [] false
wave wave [] true
carry idle [carry=carry@1] false
* -> wave interrupts 0
idle -> run min_ticks(5) 0.25
run -> idle none 0.1
wave -> idle on_event(end) 0.15
idle -> carry none 0.2
carry -> idle none 0.2
"
        );
    }
}
