//! Runtime rebinding of the action layer: direct edits, capture of the next press, conflict
//! resolution and lenient overrides.
//!
//! Everything is sim-side and in-memory. Every call is a sim call driven by recorded input, so a
//! replay reproduces it. A change recompiles the map from the defaults plus the overrides; if the
//! result does not compile, nothing changes and the compile errors come back. An action never ends
//! up with no bindings.
//!
//! # Example
//! ```
//! use ludion_core::time::TickRate;
//! use ludion_sim::actions::{ActionMap, ActionName, Actions, BindingDoc, InputActionsDoc};
//! use ludion_sim::{Key};
//!
//! let doc: InputActionsDoc = serde_json::from_value(serde_json::json!({
//!     "actions": { "jump": { "kind": "button", "bindings": [{ "key": "Space" }] } }
//! })).unwrap();
//! let mut actions = Actions::new(ActionMap::compile(&doc, TickRate::DEFAULT).unwrap());
//!
//! let k = BindingDoc { key: Some(Key::K), ..BindingDoc::default() };
//! actions.rebind("jump", 0, k).unwrap();
//! assert_eq!(actions.overrides().actions.len(), 1);
//! actions.reset("jump").unwrap();
//! assert!(actions.overrides().actions.is_empty());
//! assert!(actions.contains(ActionName::new("jump")));
//! ```

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, Severity};

use crate::actions::{
    ActionDoc, ActionKind, ActionMap, Actions, BindingDoc, Gesture, InputBindingsDoc, Side,
    unknown_action_diagnostic,
};
use crate::input::{GamepadButton, InputState, Key};

/// Which binding a capture replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureSlot {
    /// Binding `i` (any device class); `i == len` appends.
    Index(usize),
    /// The action's first keyboard or mouse binding, or an appended one; only keyboard and mouse
    /// presses are taken.
    Keyboard,
    /// The action's first pad-button or trigger binding, or an appended one; only pad presses are taken.
    Pad,
}

/// How a capture ends early and what the new binding inherits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureOptions {
    /// Keys that cancel the capture (default `[Escape]`).
    pub cancel_keys: Vec<Key>,
    /// Pad buttons that cancel the capture (default none).
    pub cancel_pad: Vec<GamepadButton>,
    /// Whether the new binding takes the replaced binding's `gesture` (default `true`).
    pub keep_gesture: bool,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            cancel_keys: vec![Key::Escape],
            cancel_pad: Vec::new(),
            keep_gesture: true,
        }
    }
}

/// Where a capture stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureResult {
    /// No capture has run.
    Idle,
    /// Waiting for a press.
    Pending,
    /// A press was bound.
    Bound,
    /// The pressed input is already one of the action's bindings.
    Unchanged,
    /// Another action in the same context has the pressed input; call
    /// [`Actions::resolve_conflict`].
    Conflict {
        /// The action that holds it.
        with: String,
    },
    /// Cancelled by a cancel input, `cancel_capture` or `Resolve::Keep`.
    Cancelled,
}

/// How to settle a capture conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolve {
    /// The other action takes the binding we replaced (or loses its binding if we appended).
    Swap,
    /// The other action loses its binding.
    Replace,
    /// Cancel the capture.
    Keep,
}

#[derive(Clone, Debug)]
pub(crate) struct Conflict {
    target: usize,
    candidate: BindingDoc,
    replaced: Option<BindingDoc>,
    other: String,
    other_index: usize,
}

/// A capture in progress.
#[derive(Clone, Debug)]
pub(crate) struct PendingCapture {
    name: String,
    slot: CaptureSlot,
    options: CaptureOptions,
    conflict: Option<Conflict>,
}

type SourceKey = (
    Option<Key>,
    Option<crate::input::MouseButton>,
    Option<GamepadButton>,
    Option<Side>,
    Option<crate::actions::WheelSource>,
    Gesture,
);

/// A button binding's identity for the duplicate rule: its source and gesture.
fn source_key(b: &BindingDoc) -> SourceKey {
    (
        b.key,
        b.mouse,
        b.pad,
        b.trigger,
        b.wheel,
        b.gesture.unwrap_or(Gesture::Press),
    )
}

fn locked(name: &str) -> Diagnostic {
    Diagnostic::new(
        code::INPUT_ACTION_LOCKED,
        format!("the action `{name}` is locked and cannot be rebound"),
    )
    .with_help(
        "leave it alone, or remove `locked` from its entry in the `kind: input-actions` document",
    )
}

fn impossible(message: String, help: &str) -> Diagnostic {
    Diagnostic::new(code::INPUT_CAPTURE_IMPOSSIBLE, message).with_help(help)
}

fn is_keyboard(b: &BindingDoc) -> bool {
    b.key.is_some() || b.mouse.is_some()
}

fn is_pad(b: &BindingDoc) -> bool {
    b.pad.is_some() || b.trigger.is_some()
}

impl Actions {
    fn action_doc(&self, name: &str) -> Result<&ActionDoc, Vec<Diagnostic>> {
        self.map()
            .doc()
            .actions
            .get(&name.to_owned())
            .ok_or_else(|| vec![unknown_action_diagnostic(name)])
    }

    fn editable(&self, name: &str) -> Result<&ActionDoc, Vec<Diagnostic>> {
        let action = self.action_doc(name)?;
        if action.locked {
            return Err(vec![locked(name)]);
        }
        Ok(action)
    }

    /// The action's effective bindings (defaults with any override), or `None` if undefined.
    pub fn bindings(&self, name: &str) -> Option<&[BindingDoc]> {
        self.action_doc(name).ok().map(|a| a.bindings.as_slice())
    }

    /// The overrides in effect: only actions whose bindings differ from the defaults.
    pub fn overrides(&self) -> &InputBindingsDoc {
        &self.overrides
    }

    /// Replaces the effective overrides and recompiles; leaves everything alone on error.
    fn rebuild(&mut self, overrides: InputBindingsDoc) -> Result<(), Vec<Diagnostic>> {
        let mut doc = self.base.clone();
        for (name, bindings) in &overrides.actions {
            if let Some(action) = doc.actions.get_mut(name) {
                action.bindings.clone_from(bindings);
            }
        }
        let map = ActionMap::compile(&doc, self.map().rate())?;
        self.install_map(map);
        self.overrides = overrides;
        Ok(())
    }

    /// Sets several actions' whole binding lists at once (all or nothing).
    fn apply_changes(
        &mut self,
        changes: Vec<(String, Vec<BindingDoc>)>,
    ) -> Result<(), Vec<Diagnostic>> {
        let mut candidate = self.overrides.clone();
        for (name, bindings) in changes {
            self.editable(&name)?;
            let default = self.base.actions.get(&name).map(|a| &a.bindings);
            if default == Some(&bindings) {
                candidate.actions.remove(&name);
            } else {
                candidate.actions.insert(name, bindings);
            }
        }
        self.rebuild(candidate)
    }

    /// Replaces binding `index` of `name`; `index == len` appends.
    ///
    /// # Errors
    /// `L4270` (unknown action), `L4272` (locked), `L4275` (`index` past `len`), or the compile
    /// errors `L1630`–`L1635` (a binding that does not fit the action, or a duplicate); nothing
    /// changes then.
    pub fn rebind(
        &mut self,
        name: &str,
        index: usize,
        binding: BindingDoc,
    ) -> Result<(), Vec<Diagnostic>> {
        let mut bindings = self.editable(name)?.bindings.clone();
        match index.cmp(&bindings.len()) {
            std::cmp::Ordering::Less => bindings[index] = binding,
            std::cmp::Ordering::Equal => bindings.push(binding),
            std::cmp::Ordering::Greater => {
                return Err(vec![impossible(
                    format!(
                        "binding index {index} is past the {} bindings of `{name}`",
                        bindings.len()
                    ),
                    "use an index below the binding count, or the count itself to append",
                )]);
            }
        }
        self.apply_changes(vec![(name.to_owned(), bindings)])
    }

    /// Removes binding `index` of `name`.
    ///
    /// # Errors
    /// As [`rebind`](Actions::rebind); removing the last binding is `L1635` (an action keeps at
    /// least one), and an `index` at or past `len` is `L4275`.
    pub fn unbind(&mut self, name: &str, index: usize) -> Result<(), Vec<Diagnostic>> {
        let mut bindings = self.editable(name)?.bindings.clone();
        if index >= bindings.len() {
            return Err(vec![impossible(
                format!(
                    "binding index {index} is past the {} bindings of `{name}`",
                    bindings.len()
                ),
                "use an index below the binding count",
            )]);
        }
        bindings.remove(index);
        self.apply_changes(vec![(name.to_owned(), bindings)])
    }

    /// Restores `name`'s default bindings.
    ///
    /// # Errors
    /// `L4270` (unknown action) or `L4272` (locked).
    pub fn reset(&mut self, name: &str) -> Result<(), Vec<Diagnostic>> {
        self.editable(name)?;
        let default = self
            .base
            .actions
            .get(&name.to_owned())
            .map(|a| a.bindings.clone())
            .unwrap_or_default();
        self.apply_changes(vec![(name.to_owned(), default)])
    }

    /// Drops every override.
    pub fn reset_all(&mut self) {
        // The defaults compiled when the layer was built, so this cannot fail.
        let _ = self.rebuild(InputBindingsDoc::default());
    }

    /// Applies a player's overrides leniently, replacing the current ones: each action is judged on
    /// its own, and an entry that cannot apply is dropped with a warning (`/actions/<name>`) while
    /// the rest still apply.
    ///
    /// An unknown action is `L4273`, a locked one `L4272`, an entry that does not compile (alone or
    /// against the others) `L4274` and that action keeps its defaults.
    pub fn apply_overrides(&mut self, doc: &InputBindingsDoc) -> Vec<Diagnostic> {
        let mut warnings = Vec::new();
        let warn = |code, message: String, name: &str, help: &str| {
            Diagnostic::new(code, message)
                .with_severity(Severity::Warning)
                .with_path(format!("/actions/{name}"))
                .with_help(help)
        };
        let mut accepted = InputBindingsDoc::default();
        for (name, bindings) in &doc.actions {
            let Some(action) = self.base.actions.get(name) else {
                warnings.push(warn(
                    code::INPUT_OVERRIDE_UNKNOWN,
                    format!("the override for `{name}` names no action of the loaded input map"),
                    name,
                    "the entry is ignored; delete it, or correct the action name",
                ));
                continue;
            };
            if action.locked {
                warnings.push(warn(
                    code::INPUT_ACTION_LOCKED,
                    format!("the override for `{name}` is ignored: the action is locked"),
                    name,
                    "delete the entry, or remove `locked` from the action",
                ));
                continue;
            }
            if *bindings != action.bindings {
                accepted.actions.insert(name.clone(), bindings.clone());
            }
        }
        // Compile all together; drop the actions the errors point at and retry.
        loop {
            let mut effective = self.base.clone();
            for (name, bindings) in &accepted.actions {
                if let Some(action) = effective.actions.get_mut(name) {
                    action.bindings.clone_from(bindings);
                }
            }
            match ActionMap::compile(&effective, self.map().rate()) {
                Ok(map) => {
                    self.install_map(map);
                    self.overrides = accepted;
                    return warnings;
                }
                Err(errors) => {
                    let mut bad: Vec<String> = errors
                        .iter()
                        .filter_map(|e| e.path.as_deref())
                        .filter_map(|p| p.strip_prefix("/actions/"))
                        .map(|rest| rest.split('/').next().unwrap_or_default().to_owned())
                        .filter(|n| accepted.actions.contains_key(n))
                        .collect();
                    bad.sort();
                    bad.dedup();
                    if bad.is_empty() {
                        bad = accepted.actions.iter().map(|(n, _)| n.clone()).collect();
                    }
                    for name in bad {
                        accepted.actions.remove(&name);
                        warnings.push(warn(
                            code::INPUT_OVERRIDE_INVALID,
                            format!(
                                "the override for `{name}` does not compile, so it keeps its defaults"
                            ),
                            &name,
                            "correct the bindings (one source each, fitting the action's kind, no duplicate in its context) or delete the entry",
                        ));
                    }
                }
            }
        }
    }

    /// Asks the host to persist the overrides. The layer does not write anything itself.
    pub fn save(&mut self) {
        self.save_requested = true;
    }

    /// Whether a [`save`](Actions::save) is waiting; clears it.
    pub fn take_save_request(&mut self) -> bool {
        std::mem::take(&mut self.save_requested)
    }

    // ───────────────────────────── capture ─────────────────────────────

    /// Starts capturing the next press as a binding of `name`.
    ///
    /// Inputs already held at this call have no press edge and are ignored. The result is read with
    /// [`capture_result`](Actions::capture_result).
    ///
    /// # Errors
    /// `L4270` (unknown), `L4272` (locked), `L4275` (not a `button` action, or `Index` past `len`).
    pub fn capture(
        &mut self,
        name: &str,
        slot: CaptureSlot,
        options: CaptureOptions,
    ) -> Result<(), Diagnostic> {
        let action = self.editable(name).map_err(|mut e| e.remove(0))?;
        if action.kind != ActionKind::Button {
            return Err(impossible(
                format!("`{name}` is not a `button` action, so a press cannot be captured for it"),
                "capture works for `button` actions; edit an axis action with `rebind`",
            ));
        }
        if let CaptureSlot::Index(i) = slot
            && i > action.bindings.len()
        {
            return Err(impossible(
                format!(
                    "binding index {i} is past the {} bindings of `{name}`",
                    action.bindings.len()
                ),
                "use an index below the binding count, or the count itself to append",
            ));
        }
        self.capture = Some(PendingCapture {
            name: name.to_owned(),
            slot,
            options,
            conflict: None,
        });
        self.capture_result = CaptureResult::Pending;
        Ok(())
    }

    /// Where the last capture stands.
    pub fn capture_result(&self) -> &CaptureResult {
        &self.capture_result
    }

    /// Abandons a capture in progress or waiting on a conflict (result `Cancelled`).
    pub fn cancel_capture(&mut self) {
        if self.capture.take().is_some() {
            self.capture_result = CaptureResult::Cancelled;
        }
    }

    /// Settles a [`CaptureResult::Conflict`].
    ///
    /// # Errors
    /// `L4276` with no conflict pending; `L4272` when the other action is locked (only `Keep` is
    /// allowed); `L4275` when it would leave the other action with no bindings. The conflict stays
    /// pending after an error.
    pub fn resolve_conflict(&mut self, how: Resolve) -> Result<(), Vec<Diagnostic>> {
        let Some(cap) = &self.capture else {
            return Err(vec![no_conflict()]);
        };
        let Some(conflict) = cap.conflict.clone() else {
            return Err(vec![no_conflict()]);
        };
        let name = cap.name.clone();
        if how == Resolve::Keep {
            self.capture = None;
            self.capture_result = CaptureResult::Cancelled;
            return Ok(());
        }
        if self.action_doc(&conflict.other)?.locked {
            return Err(vec![locked(&conflict.other)]);
        }
        let mut ours = self.action_doc(&name)?.bindings.clone();
        if conflict.target < ours.len() {
            ours[conflict.target] = conflict.candidate.clone();
        } else {
            ours.push(conflict.candidate.clone());
        }
        let mut theirs = self.action_doc(&conflict.other)?.bindings.clone();
        match (how, &conflict.replaced) {
            (Resolve::Swap, Some(replaced)) => theirs[conflict.other_index] = replaced.clone(),
            _ => {
                theirs.remove(conflict.other_index);
            }
        }
        if theirs.is_empty() {
            return Err(vec![impossible(
                format!("`{}` would be left with no bindings", conflict.other),
                "resolve with Swap when the capture replaced a binding, or Keep",
            )]);
        }
        self.apply_changes(vec![(name, ours), (conflict.other, theirs)])?;
        self.capture = None;
        self.capture_result = CaptureResult::Bound;
        Ok(())
    }

    /// Runs after the gesture step of [`resolve`](Actions::resolve): ends or advances a pending
    /// capture from this tick's press edges.
    pub(crate) fn resolve_capture(&mut self, input: &InputState) {
        if self.capture_result != CaptureResult::Pending {
            return;
        }
        let Some(cap) = &self.capture else { return };
        let cancelled = cap
            .options
            .cancel_keys
            .iter()
            .any(|k| input.was_pressed(*k))
            || cap
                .options
                .cancel_pad
                .iter()
                .any(|b| input.gamepad().was_pressed(*b));
        if cancelled {
            self.capture = None;
            self.capture_result = CaptureResult::Cancelled;
            return;
        }
        let Some(pressed) = first_press(cap.slot, input) else {
            return;
        };
        let name = cap.name.clone();
        let keep_gesture = cap.options.keep_gesture;
        let slot = cap.slot;
        let Ok(action) = self.action_doc(&name) else {
            self.capture = None;
            self.capture_result = CaptureResult::Cancelled;
            return;
        };
        let target = match slot {
            CaptureSlot::Index(i) => i,
            CaptureSlot::Keyboard => action
                .bindings
                .iter()
                .position(is_keyboard)
                .unwrap_or(action.bindings.len()),
            CaptureSlot::Pad => action
                .bindings
                .iter()
                .position(is_pad)
                .unwrap_or(action.bindings.len()),
        };
        let replaced = action.bindings.get(target).cloned();
        let mut candidate = pressed;
        if keep_gesture {
            candidate.gesture = replaced.as_ref().and_then(|b| b.gesture);
        }
        let key = source_key(&candidate);
        let context = action.context.clone();
        if action.bindings.iter().any(|b| source_key(b) == key) {
            self.capture = None;
            self.capture_result = CaptureResult::Unchanged;
            return;
        }
        let other = self.map().doc().actions.iter().find_map(|(n, a)| {
            if *n == name || a.context != context || a.kind != ActionKind::Button {
                return None;
            }
            a.bindings
                .iter()
                .position(|b| source_key(b) == key)
                .map(|at| (n.clone(), at))
        });
        if let Some((other, other_index)) = other {
            self.capture_result = CaptureResult::Conflict {
                with: other.clone(),
            };
            if let Some(cap) = &mut self.capture {
                cap.conflict = Some(Conflict {
                    target,
                    candidate,
                    replaced,
                    other,
                    other_index,
                });
            }
            return;
        }
        let result = self.rebind(&name, target, candidate);
        self.capture = None;
        self.capture_result = if result.is_ok() {
            CaptureResult::Bound
        } else {
            CaptureResult::Cancelled
        };
    }
}

fn no_conflict() -> Diagnostic {
    Diagnostic::new(
        code::INPUT_CAPTURE_IDLE,
        "there is no binding conflict to resolve",
    )
    .with_help("call `resolve_conflict` only while `capture_result()` is `Conflict`")
}

/// The first press edge of the slot's device class: keys, then mouse buttons, then pad buttons,
/// each in `Ord` order. A trigger button becomes a `trigger` binding.
fn first_press(slot: CaptureSlot, input: &InputState) -> Option<BindingDoc> {
    let (keyboard, pad) = match slot {
        CaptureSlot::Keyboard => (true, false),
        CaptureSlot::Pad => (false, true),
        CaptureSlot::Index(_) => (true, true),
    };
    if keyboard {
        if let Some(key) = input.pressed_keys().next() {
            return Some(BindingDoc {
                key: Some(key),
                ..BindingDoc::default()
            });
        }
        if let Some(button) = input.mouse().pressed_buttons().next() {
            return Some(BindingDoc {
                mouse: Some(button),
                ..BindingDoc::default()
            });
        }
    }
    if pad && let Some(button) = input.gamepad().pressed_buttons().next() {
        return Some(match button {
            GamepadButton::LeftTrigger => BindingDoc {
                trigger: Some(Side::Left),
                ..BindingDoc::default()
            },
            GamepadButton::RightTrigger => BindingDoc {
                trigger: Some(Side::Right),
                ..BindingDoc::default()
            },
            other => BindingDoc {
                pad: Some(other),
                ..BindingDoc::default()
            },
        });
    }
    None
}
