//! The action layer: a data-driven map from named actions to physical inputs, with gestures.
//!
//! A game names *what the player means* (`jump`, `crouch`, `move`) and the map says *which physical
//! inputs mean it and how*: press, tap, hold or double-tap on a key, mouse button, pad button,
//! trigger or wheel step; `axis1` and `axis2` actions read sticks, key composites, the wheel and
//! raw mouse motion. The document types ([`InputActionsDoc`]) are plain serde data; [`ActionMap::compile`]
//! validates one into an [`ActionMap`] (every error at once, each with a JSON Pointer and a fix),
//! and the [`Actions`] resource resolves it against the [`InputState`] once per tick.
//!
//! Every gesture threshold is counted in **sim ticks** (the document states seconds, the compiler
//! rounds half up at the tick rate), never in `game_dt`: a held `GameClock` or slow motion never
//! stretches a hold. Resolution is a pure function of the previous gesture state and this tick's
//! input, so a run replays bit for bit and the state round-trips through [`Actions::state`] /
//! [`Actions::restore_state`].
//!
//! This module is serde-only like the rest of the crate: no YAML, no clock.
//!
//! # Example
//! ```
//! use ludion_core::time::TickRate;
//! use ludion_sim::actions::{ActionMap, ActionName, Actions, InputActionsDoc};
//! use ludion_sim::{InputEvent, InputState, Key};
//!
//! let doc: InputActionsDoc = serde_json::from_value(serde_json::json!({
//!     "actions": {
//!         "crouch": { "kind": "button", "bindings": [
//!             { "key": "C", "gesture": "tap" },
//!             { "key": "C", "gesture": "hold" },
//!         ]},
//!     }
//! })).unwrap();
//! let mut actions = Actions::new(ActionMap::compile(&doc, TickRate::DEFAULT).unwrap());
//! const CROUCH: ActionName = ActionName::new("crouch");
//!
//! let mut input = InputState::default();
//! input.begin_tick();
//! input.apply(InputEvent::KeyDown { key: Key::C });
//! actions.resolve(&input);
//! assert!(!actions.pressed(CROUCH)); // neither a tap nor a hold yet
//!
//! for _ in 0..20 {
//!     input.begin_tick();
//!     actions.resolve(&input);
//! }
//! assert!(actions.pressed(CROUCH)); // the hold fired on the 21st tick down (0.35 s at 60 Hz)
//! ```

use core::f32::consts::FRAC_1_SQRT_2;
use std::sync::{Mutex, PoisonError};

use ludion_core::codes::code;
use ludion_core::collections::{StableMap, StableSet};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::TickRate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::input::{GamepadButton, InputState, Key, MouseButton, STICK_DEADZONE, radial_deadzone};

/// The default level at which a `trigger` binding presses.
const TRIGGER_PRESS: f32 = 0.5;
/// A `trigger` binding's default release level is this fraction of its press level.
const TRIGGER_RELEASE_RATIO: f32 = 0.7;

// ───────────────────────────── document types ─────────────────────────────

/// The `kind: input-actions` document: gesture timings and the named actions.
///
/// The engine strips the `format`/`kind` envelope before deserializing, like every other document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputActionsDoc {
    /// Gesture thresholds, in seconds (all optional; the defaults apply).
    #[serde(default)]
    pub gestures: GestureTimes,
    /// The actions, keyed by name (`a-z`, `0-9`, `_`).
    #[schemars(with = "std::collections::BTreeMap<String, ActionDoc>")]
    pub actions: StableMap<String, ActionDoc>,
}

/// A player's binding overrides: for each overridden action, its whole replacement `bindings` list.
///
/// The `kind: input-bindings` file body (the engine strips the envelope). Only actions whose
/// bindings differ from the map's defaults appear, see [`Actions::overrides`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputBindingsDoc {
    /// The overridden actions' bindings, keyed by action name.
    #[schemars(with = "std::collections::BTreeMap<String, Vec<BindingDoc>>")]
    pub actions: StableMap<String, Vec<BindingDoc>>,
}

/// Gesture thresholds in seconds. Compiled to ticks at the sim's tick rate, rounding half up,
/// minimum one tick.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct GestureTimes {
    /// The longest a press may last and still be a tap (default `0.22`).
    pub tap_max: f32,
    /// How long a press must last to fire a hold (default `0.35`).
    pub hold: f32,
    /// How long after a tap a second press still makes a double-tap (default `0.25`).
    pub double_tap_window: f32,
}

impl Default for GestureTimes {
    fn default() -> Self {
        Self {
            tap_max: 0.22,
            hold: 0.35,
            double_tap_window: 0.25,
        }
    }
}

/// What an action reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    /// `pressed` / `held` / `released` reads.
    Button,
    /// One axis in `[-1, 1]` (or a wheel or pad-pair value).
    Axis1,
    /// Two axes (a stick, a key cluster, mouse motion).
    Axis2,
}

/// One action: its kind, its bindings and its rebinding metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionDoc {
    /// What the action reports.
    pub kind: ActionKind,
    /// The context a duplicate check is scoped to (default `default`).
    #[serde(default = "default_context")]
    pub context: String,
    /// Whether a player may not rebind it (parsed here; a rebinder enforces it).
    #[serde(default)]
    pub locked: bool,
    /// The physical inputs that drive the action, in priority order.
    pub bindings: Vec<BindingDoc>,
}

fn default_context() -> String {
    "default".to_owned()
}

/// Which analog stick or trigger.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The left stick or trigger.
    Left,
    /// The right stick or trigger.
    Right,
}

/// A mouse-wheel source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WheelSource {
    /// One step up: a button source, down for the tick the wheel's y is positive.
    Up,
    /// One step down: a button source, down for the tick the wheel's y is negative.
    Down,
    /// The wheel's horizontal delta, an `axis1` source.
    X,
    /// The wheel's vertical delta, an `axis1` source.
    Y,
}

/// How a button input is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Gesture {
    /// Fires on the down edge, level while down.
    Press,
    /// Fires on release when the press was short enough.
    Tap,
    /// Fires once the press has lasted long enough, level until release.
    Hold,
    /// Fires on a second press shortly after a tap, level while it is held.
    DoubleTap,
}

impl Gesture {
    fn index(self) -> usize {
        match self {
            Gesture::Press => PRESS,
            Gesture::Tap => TAP,
            Gesture::Hold => HOLD,
            Gesture::DoubleTap => DOUBLE,
        }
    }
}

const PRESS: usize = 0;
const TAP: usize = 1;
const HOLD: usize = 2;
const DOUBLE: usize = 3;

/// The four keys of a digital `axis2` cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyCluster {
    /// The key that pushes `y` positive.
    pub up: Key,
    /// The key that pushes `y` negative.
    pub down: Key,
    /// The key that pushes `x` negative.
    pub left: Key,
    /// The key that pushes `x` positive.
    pub right: Key,
}

/// One binding: exactly one source field plus optional modifiers.
///
/// One flat struct of optional fields rather than a tagged enum, so a compile error names the
/// field. A binding names **one** source:
/// `key`, `mouse`, `pad`, `trigger`, `wheel`, `stick`, `keys`, `key_pair`, `pad_pair` or
/// `mouse_delta: true`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BindingDoc {
    /// A keyboard key (button).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Key>,
    /// A mouse button (button).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse: Option<MouseButton>,
    /// A gamepad button (button).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad: Option<GamepadButton>,
    /// An analog trigger read as a button, with hysteresis (button).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Side>,
    /// A wheel source: `up`/`down` are buttons, `x`/`y` are `axis1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wheel: Option<WheelSource>,
    /// An analog stick (`axis2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stick: Option<Side>,
    /// A four-key cluster (`axis2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<KeyCluster>,
    /// `[negative, positive]` keys (`axis1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_pair: Option<[Key; 2]>,
    /// `[negative, positive]` pad buttons (`axis1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad_pair: Option<[GamepadButton; 2]>,
    /// Raw mouse motion (`axis2`); `true` selects it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mouse_delta: bool,
    /// How a button source is read (default `press`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gesture: Option<Gesture>,
    /// `trigger` only: the level at or above which it presses (default `0.5`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub press: Option<f32>,
    /// `trigger` only: the level below which it releases (default `0.7 × press`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<f32>,
    /// `stick` only: the radial deadzone in `[0, 1)` (default `0.2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadzone: Option<f32>,
    /// Analog sources only: a multiplier (default `1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f32>,
    /// Analog sources only: negate `x` (an `axis1` source's one value).
    #[serde(default, skip_serializing_if = "is_false")]
    pub invert_x: bool,
    /// Analog sources only: negate `y`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub invert_y: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

// ───────────────────────────── compiled map ─────────────────────────────

/// A physical input the resolver tracks gestures for. `Ord` so tables are canonical.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PhysicalInput {
    /// A keyboard key.
    Key(Key),
    /// A mouse button.
    Mouse(MouseButton),
    /// A gamepad button (a trigger button reads at the legacy fixed level).
    Pad(GamepadButton),
    /// An analog trigger with its own press and release levels, as `f32` bit patterns.
    Trigger {
        /// Which trigger.
        side: Side,
        /// The press level's bits.
        press_bits: u32,
        /// The release level's bits.
        release_bits: u32,
    },
    /// One wheel step up.
    WheelUp,
    /// One wheel step down.
    WheelDown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct InputFlags {
    tap: bool,
    hold: bool,
    double: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Axis1Source {
    Wheel {
        index: usize,
        scale: f32,
        invert: bool,
    },
    KeyPair([Key; 2]),
    PadPair([GamepadButton; 2]),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Axis2Source {
    Keys(KeyCluster),
    Stick {
        side: Side,
        deadzone: f32,
        scale: f32,
        invert: [bool; 2],
    },
    MouseDelta {
        scale: f32,
        invert: [bool; 2],
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CompiledBinding {
    Button { input: usize, gesture: Gesture },
    Axis1(Axis1Source),
    Axis2(Axis2Source),
}

#[derive(Clone, Debug, PartialEq)]
struct CompiledAction {
    name: String,
    kind: ActionKind,
    bindings: Vec<CompiledBinding>,
}

/// Gesture thresholds in ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GestureTicks {
    tap_max: u32,
    hold: u32,
    double_tap_window: u32,
}

/// A validated, compiled action map: actions sorted by name, bindings resolved to a canonical
/// table of physical inputs.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionMap {
    doc: InputActionsDoc,
    rate: TickRate,
    ticks: GestureTicks,
    inputs: Vec<PhysicalInput>,
    flags: Vec<InputFlags>,
    actions: Vec<CompiledAction>,
}

/// Seconds to ticks: round half up, minimum one.
fn seconds_to_ticks(seconds: f32, rate: TickRate) -> u32 {
    let ticks = ((f64::from(seconds) * f64::from(rate.hz())) + 0.5).floor();
    (ticks as u32).max(1)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A binding source, after the zero-or-many check.
#[derive(Clone, Copy)]
enum Source {
    Key(Key),
    Mouse(MouseButton),
    Pad(GamepadButton),
    Trigger(Side),
    Wheel(WheelSource),
    Stick(Side),
    Keys(KeyCluster),
    KeyPair([Key; 2]),
    PadPair([GamepadButton; 2]),
    MouseDelta,
}

impl Source {
    fn is_analog(self) -> bool {
        matches!(
            self,
            Source::Stick(_) | Source::MouseDelta | Source::Wheel(WheelSource::X | WheelSource::Y)
        )
    }

    fn fits(self, kind: ActionKind) -> bool {
        match kind {
            ActionKind::Button => matches!(
                self,
                Source::Key(_)
                    | Source::Mouse(_)
                    | Source::Pad(_)
                    | Source::Trigger(_)
                    | Source::Wheel(WheelSource::Up | WheelSource::Down)
            ),
            ActionKind::Axis1 => matches!(
                self,
                Source::Wheel(WheelSource::X | WheelSource::Y)
                    | Source::KeyPair(_)
                    | Source::PadPair(_)
            ),
            ActionKind::Axis2 => matches!(
                self,
                Source::Keys(_) | Source::Stick(_) | Source::MouseDelta
            ),
        }
    }
}

fn sources(binding: &BindingDoc) -> Vec<(&'static str, Source)> {
    let mut out = Vec::new();
    if let Some(v) = binding.key {
        out.push(("key", Source::Key(v)));
    }
    if let Some(v) = binding.mouse {
        out.push(("mouse", Source::Mouse(v)));
    }
    if let Some(v) = binding.pad {
        out.push(("pad", Source::Pad(v)));
    }
    if let Some(v) = binding.trigger {
        out.push(("trigger", Source::Trigger(v)));
    }
    if let Some(v) = binding.wheel {
        out.push(("wheel", Source::Wheel(v)));
    }
    if let Some(v) = binding.stick {
        out.push(("stick", Source::Stick(v)));
    }
    if let Some(v) = binding.keys {
        out.push(("keys", Source::Keys(v)));
    }
    if let Some(v) = binding.key_pair {
        out.push(("key_pair", Source::KeyPair(v)));
    }
    if let Some(v) = binding.pad_pair {
        out.push(("pad_pair", Source::PadPair(v)));
    }
    if binding.mouse_delta {
        out.push(("mouse_delta", Source::MouseDelta));
    }
    out
}

fn kind_name(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::Button => "button",
        ActionKind::Axis1 => "axis1",
        ActionKind::Axis2 => "axis2",
    }
}

fn kind_sources(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::Button => "`key`, `mouse`, `pad`, `trigger` or `wheel: up|down`",
        ActionKind::Axis1 => "`wheel: x|y`, `key_pair` or `pad_pair`",
        ActionKind::Axis2 => "`keys`, `stick` or `mouse_delta`",
    }
}

fn finite_in(value: f32, low: f32, high: f32) -> bool {
    value.is_finite() && value >= low && value <= high
}

impl ActionMap {
    /// Validates and compiles a document, resolving seconds to ticks at `rate`.
    ///
    /// # Errors
    /// Every problem found, not just the first: `L1630`–`L1635` with a JSON Pointer into the
    /// document (`/actions/<name>/bindings/<i>/<field>`) and a fix.
    pub fn compile(doc: &InputActionsDoc, rate: TickRate) -> Result<ActionMap, Vec<Diagnostic>> {
        let mut errors = Vec::new();
        let times = doc.gestures;
        for (field, value) in [
            ("tap_max", times.tap_max),
            ("hold", times.hold),
            ("double_tap_window", times.double_tap_window),
        ] {
            if !(value.is_finite() && value > 0.0) {
                errors.push(
                    Diagnostic::new(
                        code::INPUT_BINDING_RANGE,
                        format!("gesture time `{field}` is {value:?}, which is not a finite number above zero"),
                    )
                    .with_path(format!("/gestures/{field}"))
                    .with_help("write a positive number of seconds, e.g. 0.25"),
                );
            }
        }
        let ticks = GestureTicks {
            tap_max: seconds_to_ticks(times.tap_max, rate),
            hold: seconds_to_ticks(times.hold, rate),
            double_tap_window: seconds_to_ticks(times.double_tap_window, rate),
        };

        let mut physical: Vec<(PhysicalInput, Gesture)> = Vec::new();
        let mut pending: Vec<(String, ActionKind, Vec<Pending>)> = Vec::new();
        let mut seen: StableMap<(String, PhysicalInput, usize), (String, usize)> = StableMap::new();

        for (name, action) in &doc.actions {
            let base = format!("/actions/{name}");
            if !valid_name(name) {
                errors.push(
                    Diagnostic::new(
                        code::INPUT_ACTION_INVALID,
                        format!("action name `{name}` is not lower-case snake_case"),
                    )
                    .with_path(base.clone())
                    .with_help("use only `a-z`, `0-9` and `_`, e.g. `quick_turn`"),
                );
            }
            if action.bindings.is_empty() {
                errors.push(
                    Diagnostic::new(
                        code::INPUT_ACTION_INVALID,
                        format!("action `{name}` has no bindings"),
                    )
                    .with_path(format!("{base}/bindings"))
                    .with_help("add at least one binding, e.g. `- key: Space`"),
                );
            }
            let mut compiled = Vec::new();
            for (i, binding) in action.bindings.iter().enumerate() {
                let path = format!("{base}/bindings/{i}");
                let before = errors.len();
                let Some(source) = check_binding(name, action.kind, binding, &path, &mut errors)
                else {
                    continue;
                };
                if errors.len() != before {
                    continue;
                }
                compiled.push(compile_binding(source, binding));
                if let Pending::Button { input, gesture } = *compiled.last().unwrap() {
                    let key = (action.context.clone(), input, gesture.index());
                    if let Some((first, first_index)) = seen.get(&key) {
                        errors.push(
                            Diagnostic::new(
                                code::INPUT_BINDING_DUPLICATE,
                                format!(
                                    "`{name}` binding {i} repeats the source and gesture of \
                                     `{first}` binding {first_index} in context `{}`",
                                    action.context
                                ),
                            )
                            .with_path(path.clone())
                            .with_help(
                                "change one binding's source or gesture, or give the action another `context`",
                            ),
                        );
                    } else {
                        seen.insert(key, (name.clone(), i));
                    }
                    physical.push((input, gesture));
                }
            }
            pending.push((name.clone(), action.kind, compiled));
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        let mut inputs: Vec<PhysicalInput> = physical.iter().map(|(p, _)| *p).collect();
        inputs.sort();
        inputs.dedup();
        let mut flags = vec![InputFlags::default(); inputs.len()];
        for (p, gesture) in &physical {
            let at = inputs.binary_search(p).expect("collected above");
            match gesture {
                Gesture::Tap => flags[at].tap = true,
                Gesture::Hold => flags[at].hold = true,
                Gesture::DoubleTap => flags[at].double = true,
                Gesture::Press => {}
            }
        }
        // Actions arrive sorted by name (a `StableMap`), so the index is the position.
        let actions = pending
            .into_iter()
            .map(|(name, kind, bindings)| CompiledAction {
                name,
                kind,
                bindings: bindings
                    .into_iter()
                    .map(|b| match b {
                        Pending::Button { input, gesture } => CompiledBinding::Button {
                            input: inputs.binary_search(&input).expect("collected above"),
                            gesture,
                        },
                        Pending::Axis1(a) => CompiledBinding::Axis1(a),
                        Pending::Axis2(a) => CompiledBinding::Axis2(a),
                    })
                    .collect(),
            })
            .collect();
        Ok(ActionMap {
            doc: doc.clone(),
            rate,
            ticks,
            inputs,
            flags,
            actions,
        })
    }

    /// The document this map was compiled from (with any overrides applied).
    pub fn doc(&self) -> &InputActionsDoc {
        &self.doc
    }

    /// The tick rate the gesture times were compiled at.
    pub fn rate(&self) -> TickRate {
        self.rate
    }

    /// The action names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.actions.iter().map(|a| a.name.as_str())
    }

    /// The deduplicated physical inputs the map tracks, in canonical order.
    pub fn inputs(&self) -> &[PhysicalInput] {
        &self.inputs
    }

    pub(crate) fn find(&self, name: &str) -> Option<usize> {
        self.actions
            .binary_search_by(|a| a.name.as_str().cmp(name))
            .ok()
    }
}

/// A binding before its input index is known.
#[derive(Clone, Copy)]
enum Pending {
    Button {
        input: PhysicalInput,
        gesture: Gesture,
    },
    Axis1(Axis1Source),
    Axis2(Axis2Source),
}

/// Checks one binding; pushes every error found and returns its source if there is exactly one.
fn check_binding(
    name: &str,
    kind: ActionKind,
    binding: &BindingDoc,
    path: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<Source> {
    let found = sources(binding);
    if found.len() != 1 {
        let listed: Vec<&str> = found.iter().map(|(f, _)| *f).collect();
        let what = if found.is_empty() {
            "names no source".to_owned()
        } else {
            format!("names {} sources ({})", found.len(), listed.join(", "))
        };
        errors.push(
            Diagnostic::new(
                code::INPUT_BINDING_SOURCES,
                format!("a binding of `{name}` {what}"),
            )
            .with_path(path)
            .with_help(
                "give each binding exactly one source; split the others into their own bindings",
            ),
        );
        return None;
    }
    let (field, source) = found[0];
    if !source.fits(kind) {
        errors.push(
            Diagnostic::new(
                code::INPUT_BINDING_SOURCE_KIND,
                format!(
                    "`{field}` cannot drive `{name}`, a `{}` action",
                    kind_name(kind)
                ),
            )
            .with_path(format!("{path}/{field}"))
            .with_help(format!(
                "a `{}` action takes {}; change the action's `kind` or the source",
                kind_name(kind),
                kind_sources(kind)
            )),
        );
        return None;
    }

    let modifier = |field: &str, why: &str, errors: &mut Vec<Diagnostic>| {
        errors.push(
            Diagnostic::new(
                code::INPUT_BINDING_MODIFIER,
                format!("`{field}` does not apply to this binding: {why}"),
            )
            .with_path(format!("{path}/{field}"))
            .with_help(format!(
                "remove `{field}`, or move it to a binding it applies to"
            )),
        );
    };
    let range = |field: &str, value: f32, want: &str, errors: &mut Vec<Diagnostic>| {
        errors.push(
            Diagnostic::new(
                code::INPUT_BINDING_RANGE,
                format!("`{field}` is {value:?}, which is not {want}"),
            )
            .with_path(format!("{path}/{field}"))
            .with_help(format!("write {want}")),
        );
    };

    let button = matches!(
        source,
        Source::Key(_) | Source::Mouse(_) | Source::Pad(_) | Source::Trigger(_)
    );
    if binding.gesture.is_some() && !button {
        modifier(
            "gesture",
            "only `key`, `mouse`, `pad` and `trigger` take a gesture",
            errors,
        );
    }
    let trigger = matches!(source, Source::Trigger(_));
    if binding.press.is_some() && !trigger {
        modifier("press", "only `trigger` has a press level", errors);
    }
    if binding.release.is_some() && !trigger {
        modifier("release", "only `trigger` has a release level", errors);
    }
    if binding.deadzone.is_some() && !matches!(source, Source::Stick(_)) {
        modifier("deadzone", "only `stick` has a deadzone", errors);
    }
    if !source.is_analog() {
        if binding.scale.is_some() {
            modifier(
                "scale",
                "only analog sources (`stick`, `mouse_delta`, `wheel: x|y`) scale",
                errors,
            );
        }
        if binding.invert_x {
            modifier("invert_x", "only analog sources can be inverted", errors);
        }
        if binding.invert_y {
            modifier("invert_y", "only analog sources can be inverted", errors);
        }
    }

    if trigger {
        let press = binding.press.unwrap_or(TRIGGER_PRESS);
        if binding.press.is_some() && !finite_in(press, 0.0, 1.0) {
            range("press", press, "a finite level in [0, 1]", errors);
        } else {
            let release = binding.release.unwrap_or(press * TRIGGER_RELEASE_RATIO);
            if !finite_in(release, 0.0, 1.0) {
                range("release", release, "a finite level in [0, 1]", errors);
            } else if release > press {
                range("release", release, "at most `press`", errors);
            }
        }
    }
    if let Some(deadzone) = binding.deadzone
        && matches!(source, Source::Stick(_))
        && !(deadzone.is_finite() && (0.0..1.0).contains(&deadzone))
    {
        range("deadzone", deadzone, "a finite value in [0, 1)", errors);
    }
    if let Some(scale) = binding.scale
        && source.is_analog()
        && !scale.is_finite()
    {
        range("scale", scale, "a finite number", errors);
    }
    Some(source)
}

fn compile_binding(source: Source, binding: &BindingDoc) -> Pending {
    let gesture = binding.gesture.unwrap_or(Gesture::Press);
    let invert = [binding.invert_x, binding.invert_y];
    let scale = binding.scale.unwrap_or(1.0);
    match source {
        Source::Key(k) => Pending::Button {
            input: PhysicalInput::Key(k),
            gesture,
        },
        Source::Mouse(b) => Pending::Button {
            input: PhysicalInput::Mouse(b),
            gesture,
        },
        Source::Pad(b) => Pending::Button {
            input: PhysicalInput::Pad(b),
            gesture,
        },
        Source::Trigger(side) => {
            let press = binding.press.unwrap_or(TRIGGER_PRESS);
            let release = binding.release.unwrap_or(press * TRIGGER_RELEASE_RATIO);
            Pending::Button {
                input: PhysicalInput::Trigger {
                    side,
                    press_bits: press.to_bits(),
                    release_bits: release.to_bits(),
                },
                gesture,
            }
        }
        Source::Wheel(WheelSource::Up) => Pending::Button {
            input: PhysicalInput::WheelUp,
            gesture: Gesture::Press,
        },
        Source::Wheel(WheelSource::Down) => Pending::Button {
            input: PhysicalInput::WheelDown,
            gesture: Gesture::Press,
        },
        Source::Wheel(w) => Pending::Axis1(Axis1Source::Wheel {
            index: usize::from(w == WheelSource::Y),
            scale,
            invert: binding.invert_x,
        }),
        Source::KeyPair(p) => Pending::Axis1(Axis1Source::KeyPair(p)),
        Source::PadPair(p) => Pending::Axis1(Axis1Source::PadPair(p)),
        Source::Keys(c) => Pending::Axis2(Axis2Source::Keys(c)),
        Source::Stick(side) => Pending::Axis2(Axis2Source::Stick {
            side,
            deadzone: binding.deadzone.unwrap_or(STICK_DEADZONE),
            scale,
            invert,
        }),
        Source::MouseDelta => Pending::Axis2(Axis2Source::MouseDelta { scale, invert }),
    }
}

// ───────────────────────────── runtime ─────────────────────────────

/// The name of an action, a `const`-constructible handle a game keeps next to its code.
///
/// # Example
/// ```
/// use ludion_sim::actions::ActionName;
/// const JUMP: ActionName = ActionName::new("jump");
/// assert_eq!(JUMP.as_str(), "jump");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionName(&'static str);

impl ActionName {
    /// Names an action. The map decides at read time whether it exists.
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// The name.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// The gesture state of one physical input between ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InputGesture {
    down: bool,
    /// Ticks the input has been down (1 on the press tick).
    n: u32,
    hold_fired: bool,
    double_active: bool,
    /// Ticks until a deferred tap fires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pending: Option<u32>,
    /// Held when the layer first resolved, with no down edge that tick: ignored until released.
    #[serde(skip_serializing_if = "is_false")]
    latched: bool,
}

/// The gesture state of every tracked input: what a snapshot carries so a hold or a pending tap
/// continues across a restore.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(
    from = "Vec<(PhysicalInput, InputGesture)>",
    into = "Vec<(PhysicalInput, InputGesture)>"
)]
pub struct ActionState(pub(crate) StableMap<PhysicalInput, InputGesture>);

impl From<Vec<(PhysicalInput, InputGesture)>> for ActionState {
    fn from(entries: Vec<(PhysicalInput, InputGesture)>) -> Self {
        Self(entries.into_iter().collect())
    }
}

impl From<ActionState> for Vec<(PhysicalInput, InputGesture)> {
    fn from(state: ActionState) -> Self {
        state.0.into_iter().collect()
    }
}

impl ActionState {
    /// Whether no input is mid-gesture (nothing to snapshot).
    pub fn is_idle(&self) -> bool {
        self.0.values().all(|g| *g == InputGesture::default())
    }
}

/// What one physical input did this tick, per gesture (`press`, `tap`, `hold`, `double_tap`).
#[derive(Clone, Copy, Debug, Default)]
struct GestureOut {
    fired: [bool; 4],
    level: [bool; 4],
    ended: [bool; 4],
    /// Ticks down, 0 while up.
    n: u32,
}

#[derive(Clone, Copy, Debug, Default)]
struct ActionOut {
    pressed: bool,
    held: bool,
    released: bool,
    hold_ticks: u32,
    hold_progress: f32,
    axis: [f32; 2],
}

#[derive(Debug, Default)]
struct Unknown {
    seen: StableSet<&'static str>,
    fresh: Vec<&'static str>,
}

/// The resolved action layer: a compiled map, the per-input gesture state, and this tick's reads.
///
/// Call [`resolve`](Actions::resolve) once per tick after the [`InputState`] is current (the
/// [`SimDriver`](crate::SimDriver) does when the world holds one), then read by [`ActionName`].
/// An unknown name reads inactive/zero and is reported once through
/// [`take_unknown`](Actions::take_unknown).
///
/// # Example
/// ```
/// use ludion_core::time::TickRate;
/// use ludion_sim::actions::{ActionMap, ActionName, Actions, InputActionsDoc};
/// use ludion_sim::{InputEvent, InputState, Key};
///
/// let doc: InputActionsDoc = serde_json::from_value(serde_json::json!({
///     "actions": { "jump": { "kind": "button", "bindings": [{ "key": "Space" }] } }
/// })).unwrap();
/// let mut actions = Actions::new(ActionMap::compile(&doc, TickRate::DEFAULT).unwrap());
///
/// let mut input = InputState::default();
/// input.begin_tick();
/// input.apply(InputEvent::KeyDown { key: Key::Space });
/// actions.resolve(&input);
/// assert!(actions.pressed(ActionName::new("jump")));
/// ```
#[derive(Debug)]
pub struct Actions {
    map: ActionMap,
    state: ActionState,
    outs: Vec<GestureOut>,
    reads: Vec<ActionOut>,
    unknown: Mutex<Unknown>,
    /// The defaults the overrides apply to.
    pub(crate) base: InputActionsDoc,
    pub(crate) overrides: InputBindingsDoc,
    pub(crate) save_requested: bool,
    /// Set by `restore_state`: the next resolve latches nothing, as the restored state is the
    /// whole truth about what was down.
    restored: bool,
    pub(crate) capture: Option<crate::rebind::PendingCapture>,
    pub(crate) capture_result: crate::rebind::CaptureResult,
}

impl Actions {
    /// An action layer over a compiled map, every input idle.
    pub fn new(map: ActionMap) -> Self {
        let inputs = map.inputs.len();
        let actions = map.actions.len();
        Self {
            base: map.doc.clone(),
            overrides: InputBindingsDoc::default(),
            save_requested: false,
            restored: false,
            capture: None,
            capture_result: crate::rebind::CaptureResult::Idle,
            map,
            state: ActionState::default(),
            outs: vec![GestureOut::default(); inputs],
            reads: vec![ActionOut::default(); actions],
            unknown: Mutex::new(Unknown::default()),
        }
    }

    /// Swaps in a recompiled map, keeping gesture state, this tick's outputs and reads for what the
    /// new map still tracks. An input newly tracked has no state entry, so the latch rule holds it
    /// until it is released and pressed again.
    pub(crate) fn install_map(&mut self, map: ActionMap) {
        let old_outs: StableMap<PhysicalInput, GestureOut> = self
            .map
            .inputs
            .iter()
            .copied()
            .zip(self.outs.iter().copied())
            .collect();
        let old_reads: StableMap<&str, ActionOut> = self
            .map
            .actions
            .iter()
            .map(|a| a.name.as_str())
            .zip(self.reads.iter().copied())
            .collect();
        let outs = map
            .inputs
            .iter()
            .map(|p| old_outs.get(p).copied().unwrap_or_default())
            .collect();
        let reads = map
            .actions
            .iter()
            .map(|a| old_reads.get(&a.name.as_str()).copied().unwrap_or_default())
            .collect();
        self.state.0 = std::mem::take(&mut self.state.0)
            .into_iter()
            .filter(|(p, _)| map.inputs.binary_search(p).is_ok())
            .collect();
        self.outs = outs;
        self.reads = reads;
        self.map = map;
    }

    /// The compiled map.
    pub fn map(&self) -> &ActionMap {
        &self.map
    }

    /// The gesture state, for a snapshot.
    pub fn state(&self) -> ActionState {
        self.state.clone()
    }

    /// Restores a gesture state, dropping entries for inputs the map does not track.
    ///
    /// # Errors
    /// `L4271` when an entry the map does track is corrupt (a press with no ticks down, or a
    /// pending tap on an input with no `double_tap` binding); nothing is restored then.
    pub fn restore_state(&mut self, state: ActionState) -> Result<(), Diagnostic> {
        let mut kept = StableMap::new();
        for (input, gesture) in state.0 {
            let Ok(at) = self.map.inputs.binary_search(&input) else {
                continue;
            };
            let flags = self.map.flags[at];
            let corrupt = (gesture.down && gesture.n == 0)
                || (!gesture.down && gesture.n != 0)
                || (gesture.pending.is_some() && !flags.double)
                || (gesture.hold_fired && !gesture.down)
                || (gesture.double_active && !gesture.down);
            if corrupt {
                return Err(Diagnostic::new(
                    code::INPUT_STATE_INVALID,
                    format!("restored gesture state for {input:?} cannot continue under the loaded map"),
                )
                .with_help("restore a state saved from the same input map, or start from a fresh `Actions`"));
            }
            kept.insert(input, gesture);
        }
        self.state = ActionState(kept);
        self.restored = true;
        Ok(())
    }

    /// Advances every gesture one tick against `input` and recomputes every action's reads.
    /// Call exactly once per tick, after the tick's events are applied.
    ///
    /// An input with no state yet (every input on a layer's first tick, one newly tracked after a
    /// rebind) that is already at level with no down edge that tick (a key held across a load or
    /// restore, the key just pressed to capture a binding) is **latched**: it fires nothing and reads
    /// inactive until released. Wheel steps are never latched.
    pub fn resolve(&mut self, input: &InputState) {
        let ticks = self.map.ticks;
        let restored = std::mem::take(&mut self.restored);
        for (at, physical) in self.map.inputs.iter().enumerate() {
            let flags = self.map.flags[at];
            let untracked = !restored && !self.state.0.contains_key(physical);
            let gesture = self.state.0.entry(*physical).or_default();
            let level = read_level(*physical, gesture.down, input);
            if untracked && level && !has_down_edge(*physical, input) {
                gesture.latched = true;
            }
            if !level {
                gesture.latched = false;
            }
            let now = level && !gesture.latched;
            // A wheel step is a one-tick press: a notch on consecutive ticks is a release then a
            // press, so a continuous scroll fires `pressed` every tick.
            if now
                && gesture.down
                && matches!(physical, PhysicalInput::WheelUp | PhysicalInput::WheelDown)
            {
                step_gesture(gesture, false, flags, ticks);
            }
            self.outs[at] = step_gesture(gesture, now, flags, ticks);
        }
        for (read, action) in self.reads.iter_mut().zip(&self.map.actions) {
            *read = read_action(action, &self.outs, input, ticks);
        }
        self.resolve_capture(input);
    }

    /// Whether the map defines `name`. Unlike a read, it never reports `L4270`.
    pub fn contains(&self, name: ActionName) -> bool {
        self.map.find(name.0).is_some()
    }

    fn lookup(&self, name: ActionName) -> Option<&ActionOut> {
        match self.map.find(name.0) {
            Some(at) => Some(&self.reads[at]),
            None => {
                let mut unknown = self.unknown.lock().unwrap_or_else(PoisonError::into_inner);
                if unknown.seen.insert(name.0) {
                    unknown.fresh.push(name.0);
                }
                None
            }
        }
    }

    fn read_button(&self, name: ActionName) -> Option<&ActionOut> {
        let out = self.lookup(name)?;
        let at = self.map.find(name.0)?;
        (self.map.actions[at].kind == ActionKind::Button).then_some(out)
    }

    /// Whether the action fired this tick (a button down edge, a completed tap, a reached hold, a
    /// double-tap).
    pub fn pressed(&self, name: ActionName) -> bool {
        self.read_button(name).is_some_and(|o| o.pressed)
    }

    /// Whether any of the action's bindings is at level this tick.
    pub fn held(&self, name: ActionName) -> bool {
        self.read_button(name).is_some_and(|o| o.held)
    }

    /// Whether a binding's level ended this tick and none of the action's bindings is still held.
    pub fn released(&self, name: ActionName) -> bool {
        self.read_button(name).is_some_and(|o| o.released)
    }

    /// The most ticks any of the action's `hold` bindings has been down (0 if none is).
    pub fn held_ticks(&self, name: ActionName) -> u32 {
        self.read_button(name).map_or(0, |o| o.hold_ticks)
    }

    /// How far the action's `hold` is toward firing, in `[0, 1]`.
    pub fn hold_progress(&self, name: ActionName) -> f32 {
        self.read_button(name).map_or(0.0, |o| o.hold_progress)
    }

    /// An `axis1` action's value (0 for any other kind).
    pub fn axis(&self, name: ActionName) -> f32 {
        let Some(out) = self.lookup(name) else {
            return 0.0;
        };
        match self.map.find(name.0).map(|at| self.map.actions[at].kind) {
            Some(ActionKind::Axis1) => out.axis[0],
            _ => 0.0,
        }
    }

    /// An `axis2` action's `[x, y]` (zero for any other kind).
    pub fn axis2(&self, name: ActionName) -> [f32; 2] {
        let Some(out) = self.lookup(name) else {
            return [0.0, 0.0];
        };
        match self.map.find(name.0).map(|at| self.map.actions[at].kind) {
            Some(ActionKind::Axis2) => out.axis,
            _ => [0.0, 0.0],
        }
    }

    /// The names read since the last call that the map does not define, each reported once.
    pub fn take_unknown(&mut self) -> Vec<&'static str> {
        let unknown = self
            .unknown
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut unknown.fresh)
    }
}

/// The `L4270` warning for an action name [`Actions::take_unknown`] returned.
pub fn unknown_action_diagnostic(name: &str) -> Diagnostic {
    Diagnostic::new(
        code::INPUT_ACTION_UNKNOWN,
        format!("the action `{name}` is not defined by the loaded input map, so it reads inactive"),
    )
    .with_help("add the action to the `kind: input-actions` document, or correct the name")
}

fn trigger_value(input: &InputState, side: Side) -> f32 {
    match side {
        Side::Left => input.gamepad().left_trigger(),
        Side::Right => input.gamepad().right_trigger(),
    }
}

fn read_level(physical: PhysicalInput, was_down: bool, input: &InputState) -> bool {
    match physical {
        PhysicalInput::Key(k) => input.is_held(k),
        PhysicalInput::Mouse(b) => input.mouse().is_held(b),
        PhysicalInput::Pad(b) => input.gamepad().is_held(b),
        PhysicalInput::Trigger {
            side,
            press_bits,
            release_bits,
        } => {
            let value = trigger_value(input, side);
            if was_down {
                value >= f32::from_bits(release_bits)
            } else {
                value >= f32::from_bits(press_bits)
            }
        }
        PhysicalInput::WheelUp => input.mouse().wheel()[1] > 0.0,
        PhysicalInput::WheelDown => input.mouse().wheel()[1] < 0.0,
    }
}

/// Whether the input went down this tick. Triggers have no edge; wheel steps count as one.
fn has_down_edge(physical: PhysicalInput, input: &InputState) -> bool {
    match physical {
        PhysicalInput::Key(k) => input.was_pressed(k),
        PhysicalInput::Mouse(b) => input.mouse().was_pressed(b),
        PhysicalInput::Pad(b) => input.gamepad().was_pressed(b),
        PhysicalInput::Trigger { .. } => false,
        PhysicalInput::WheelUp | PhysicalInput::WheelDown => true,
    }
}

/// Advances one input's gesture state a tick and reports what fired.
fn step_gesture(s: &mut InputGesture, now: bool, flags: InputFlags, t: GestureTicks) -> GestureOut {
    let mut o = GestureOut::default();
    if let Some(left) = s.pending {
        if left <= 1 {
            s.pending = None;
            o.fired[TAP] = true;
        } else {
            s.pending = Some(left - 1);
        }
    }
    match (now, s.down) {
        (true, false) => {
            s.down = true;
            s.n = 1;
            s.hold_fired = false;
            o.fired[PRESS] = true;
            if s.pending.take().is_some() {
                s.double_active = true;
                o.fired[DOUBLE] = true;
            } else {
                s.double_active = false;
            }
        }
        (true, true) => s.n = s.n.saturating_add(1),
        (false, true) => {
            o.ended[PRESS] = true;
            o.ended[HOLD] = s.hold_fired;
            o.ended[DOUBLE] = s.double_active;
            if flags.tap && !s.hold_fired && !s.double_active && s.n <= t.tap_max {
                if flags.double {
                    s.pending = Some(t.double_tap_window);
                } else {
                    o.fired[TAP] = true;
                }
            }
            s.down = false;
            s.hold_fired = false;
            s.double_active = false;
            s.n = 0;
        }
        (false, false) => {}
    }
    if s.down && flags.hold && !s.hold_fired && s.n >= t.hold {
        s.hold_fired = true;
        o.fired[HOLD] = true;
    }
    o.level[PRESS] = s.down;
    o.level[HOLD] = s.down && s.hold_fired;
    o.level[DOUBLE] = s.down && s.double_active;
    o.n = s.n;
    o
}

fn read_action(
    action: &CompiledAction,
    outs: &[GestureOut],
    input: &InputState,
    t: GestureTicks,
) -> ActionOut {
    let mut read = ActionOut::default();
    let mut ended = false;
    for binding in &action.bindings {
        match *binding {
            CompiledBinding::Button { input: at, gesture } => {
                let o = &outs[at];
                let g = gesture.index();
                read.pressed |= o.fired[g];
                read.held |= o.level[g];
                ended |= o.ended[g];
                if gesture == Gesture::Hold && o.n > 0 {
                    read.hold_ticks = read.hold_ticks.max(o.n);
                    let progress = (o.n as f32 / t.hold as f32).min(1.0);
                    read.hold_progress = read.hold_progress.max(progress);
                }
            }
            CompiledBinding::Axis1(source) => {
                if read.axis[0] == 0.0 {
                    read.axis[0] = axis1_value(source, input);
                }
            }
            CompiledBinding::Axis2(source) => {
                if read.axis == [0.0, 0.0] {
                    read.axis = axis2_value(source, input);
                }
            }
        }
    }
    read.released = ended && !read.held;
    read
}

fn pair_value(negative: bool, positive: bool) -> f32 {
    f32::from(u8::from(positive)) - f32::from(u8::from(negative))
}

fn axis1_value(source: Axis1Source, input: &InputState) -> f32 {
    match source {
        Axis1Source::Wheel {
            index,
            scale,
            invert,
        } => {
            let value = input.mouse().wheel()[index] * scale;
            if invert { -value } else { value }
        }
        Axis1Source::KeyPair([negative, positive]) => {
            pair_value(input.is_held(negative), input.is_held(positive))
        }
        Axis1Source::PadPair([negative, positive]) => pair_value(
            input.gamepad().is_held(negative),
            input.gamepad().is_held(positive),
        ),
    }
}

fn axis2_value(source: Axis2Source, input: &InputState) -> [f32; 2] {
    let flip = |v: [f32; 2], invert: [bool; 2]| {
        [
            if invert[0] { -v[0] } else { v[0] },
            if invert[1] { -v[1] } else { v[1] },
        ]
    };
    match source {
        Axis2Source::Keys(c) => {
            let x = pair_value(input.is_held(c.left), input.is_held(c.right));
            let y = pair_value(input.is_held(c.down), input.is_held(c.up));
            if x != 0.0 && y != 0.0 {
                [x * FRAC_1_SQRT_2, y * FRAC_1_SQRT_2]
            } else {
                [x, y]
            }
        }
        Axis2Source::Stick {
            side,
            deadzone,
            scale,
            invert,
        } => {
            let raw = match side {
                Side::Left => input.gamepad().left_stick(),
                Side::Right => input.gamepad().right_stick(),
            };
            let v = radial_deadzone(raw, deadzone);
            flip([v[0] * scale, v[1] * scale], invert)
        }
        Axis2Source::MouseDelta { scale, invert } => {
            let d = input.mouse().delta();
            flip([d[0] * scale, d[1] * scale], invert)
        }
    }
}
