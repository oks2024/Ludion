//! [`UiNav`], the menu intent a game feeds to [`UiList::navigate`].

use ludion_sim::{GamepadButton, InputState, Key};

use crate::{UiList, UiListItem};

/// One frame's menu intent: three edge flags a game feeds to [`UiList::navigate`].
///
/// Built from an [`InputState`] by [`UiNav::from_input`] under the engine's fixed bindings, or by
/// hand — the fields are public precisely so a game with its own bindings never needs an engine
/// change. There is no action-mapping layer and no engine-side focus manager: exactly one list is
/// navigated per frame, the one the game chooses to call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiNav {
    /// Move the cursor to the previous selectable item.
    pub prev: bool,
    /// Move the cursor to the next selectable item.
    pub next: bool,
    /// Activate the item under the cursor.
    pub activate: bool,
}

impl UiNav {
    /// Reads this frame's menu intent under the engine's default bindings: `Up`/`DpadUp` is `prev`,
    /// `Down`/`DpadDown` is `next`, and `Enter`/`Space`/`GamepadButton::South` is `activate`.
    ///
    /// **Discrete edges only** (`was_pressed`, never `is_held`): one press is one step, a held
    /// direction does not repeat, and no hidden repeat-latch state exists to desynchronize. An analog
    /// stick is deliberately not read here for the same reason.
    ///
    /// # Example
    ///
    /// ```
    /// use ludion_components::UiNav;
    /// use ludion_sim::{InputEvent, InputState, Key};
    ///
    /// let mut input = InputState::default();
    /// input.begin_tick();
    /// input.apply(InputEvent::KeyDown { key: Key::Down });
    /// assert!(UiNav::from_input(&input).next);
    ///
    /// input.begin_tick(); // the key is still held, but the edge is gone
    /// assert!(!UiNav::from_input(&input).next);
    /// ```
    pub fn from_input(input: &InputState) -> Self {
        let pad = input.gamepad();
        UiNav {
            prev: input.was_pressed(Key::Up) || pad.was_pressed(GamepadButton::DpadUp),
            next: input.was_pressed(Key::Down) || pad.was_pressed(GamepadButton::DpadDown),
            activate: input.was_pressed(Key::Enter)
                || input.was_pressed(Key::Space)
                || pad.was_pressed(GamepadButton::South),
        }
    }
}

impl UiList {
    /// Applies one frame's [`UiNav`] to the cursor and reports an activation.
    ///
    /// Moves `selected` (also clamping a stale value into range) and returns `Some(index)` when
    /// `activate` fired over an **enabled** item — the activation is this call's return value, never a
    /// field, so there is no stale flag and no clearing protocol to forget.
    ///
    /// The cursor **wraps** at both ends and **never lands on a disabled item**; a list whose items
    /// are all disabled keeps its cursor where it is and refuses activation. An empty list moves
    /// nothing and returns `None`.
    ///
    /// # Example
    ///
    /// ```
    /// use ludion_components::{UiList, UiListItem};
    /// use ludion_components::UiNav;
    ///
    /// let item = |text: &str, enabled: bool| UiListItem { text: text.to_string(), enabled };
    /// let mut list = UiList {
    ///     items: vec![item("BUY", true), item("SOLD OUT", false), item("LEAVE", true)],
    ///     ..UiList::default()
    /// };
    ///
    /// let next = UiNav { next: true, ..UiNav::default() };
    /// assert_eq!(list.navigate(next), None); // the sold-out item is skipped
    /// assert_eq!(list.selected, 2);
    ///
    /// let activate = UiNav { activate: true, ..UiNav::default() };
    /// assert_eq!(list.navigate(activate), Some(2));
    /// ```
    pub fn navigate(&mut self, nav: UiNav) -> Option<u32> {
        let count = self.items.len();
        if count == 0 {
            return None;
        }
        let mut cursor = (self.selected as usize).min(count - 1);
        if nav.prev {
            cursor = step_cursor(&self.items, cursor, false);
        }
        if nav.next {
            cursor = step_cursor(&self.items, cursor, true);
        }
        self.selected = cursor as u32;
        (nav.activate && self.items[cursor].enabled).then_some(cursor as u32)
    }
}

/// The next enabled index from `from`, wrapping — or `from` itself when nothing else is selectable.
/// Bounded by one full lap, so an all-disabled list terminates with the cursor where it was.
fn step_cursor(items: &[UiListItem], from: usize, forward: bool) -> usize {
    let count = items.len();
    let mut index = from;
    for _ in 0..count {
        index = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        if items[index].enabled {
            return index;
        }
    }
    from
}
