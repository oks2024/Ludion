//! The one device-event → `input.inject` path both native window hosts share: this crate's attach
//! viewer and the `ludion` facade's in-process window. Routing keyboard, mouse, raw motion and focus events
//! through the same code is what makes a session recorded through either window carry the same
//! events.

use serde_json::Value;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::keymap::KeyMapper;
use crate::mouse::{self, MouseButtons};

/// Maps winit window events to `input.inject` event `Value`s, remembering what is held so a focus
/// loss can release it.
#[derive(Default)]
pub struct HostInput {
    keys: KeyMapper,
    mouse: MouseButtons,
    /// Raw device counts not yet emitted; summed in `f64` and narrowed once at emit.
    motion: [f64; 2],
}

impl HostInput {
    /// Adds raw device motion to the pending total.
    pub fn motion(&mut self, dx: f64, dy: f64) {
        self.motion[0] += dx;
        self.motion[1] += dy;
    }

    /// Emits the pending motion as one event (nothing when it is zero) and clears it.
    pub fn flush_motion(&mut self, mut emit: impl FnMut(Value)) {
        let [dx, dy] = std::mem::take(&mut self.motion);
        if dx != 0.0 || dy != 0.0 {
            emit(mouse::mouse_motion_event([dx as f32, dy as f32]));
        }
    }

    /// The event for a key transition, or `None` when nothing should be sent (see
    /// [`KeyMapper::map`]).
    pub fn key(&mut self, code: KeyCode, state: ElementState, repeat: bool) -> Option<Value> {
        self.keys
            .map(code, state, repeat)
            .map(|wire| wire.to_value())
    }

    /// The event for a mouse button transition, or `None` for an unmapped button.
    pub fn button(&mut self, button: MouseButton, state: ElementState) -> Option<Value> {
        self.mouse.map(button, state)
    }

    /// A release for everything held: keys (modifiers first), then mouse buttons.
    pub fn focus_lost(&mut self) -> Vec<Value> {
        let mut out: Vec<Value> = self
            .keys
            .release_held()
            .into_iter()
            .map(|wire| wire.to_value())
            .collect();
        out.extend(self.mouse.release_held());
        out
    }

    /// Routes one window event to `emit`; events that carry no input emit nothing.
    /// `map_cursor` turns a window-surface position into render-target pixels (`None` when it has
    /// nowhere to land yet).
    pub fn route(
        &mut self,
        event: &WindowEvent,
        map_cursor: impl FnOnce(f32, f32) -> Option<[f32; 2]>,
        mut emit: impl FnMut(Value),
    ) {
        self.flush_motion(&mut emit);
        match event {
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && let Some(value) = self.key(code, event.state, event.repeat)
                {
                    emit(value);
                }
            }
            // A key or button let go while another window had focus never gets a release.
            WindowEvent::Focused(false) => {
                for value in self.focus_lost() {
                    emit(value);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(pos) = map_cursor(position.x as f32, position.y as f32) {
                    emit(mouse::mouse_move_event(pos));
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(value) = self.button(*button, *state) {
                    emit(value);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => emit(mouse::mouse_wheel_event(*delta)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held_input() -> HostInput {
        let mut input = HostInput::default();
        input.key(KeyCode::KeyW, ElementState::Pressed, false);
        input.key(KeyCode::ShiftLeft, ElementState::Pressed, false);
        input.button(MouseButton::Left, ElementState::Pressed);
        input
    }

    fn expected_release() -> Vec<Value> {
        vec![
            serde_json::json!({"kind": "key_up", "key": "Shift"}),
            serde_json::json!({"kind": "key_up", "key": "W"}),
            serde_json::json!({"kind": "mouse_up", "button": "left"}),
        ]
    }

    #[test]
    fn focus_lost_releases_modifiers_then_keys_then_buttons_once() {
        let mut input = held_input();
        assert_eq!(input.focus_lost(), expected_release());
        assert!(input.focus_lost().is_empty());
    }

    fn motion_json(x: f32, y: f32) -> Value {
        mouse::mouse_motion_event([x, y])
    }

    #[test]
    fn motion_calls_coalesce_into_one_event_then_nothing() {
        let mut input = HostInput::default();
        input.motion(1.0, 2.0);
        input.motion(0.5, -1.0);
        input.motion(1.0, 0.0);
        let mut out = Vec::new();
        input.flush_motion(|v| out.push(v));
        assert_eq!(out, vec![motion_json(2.5, 1.0)]);
        input.flush_motion(|v| out.push(v));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn flush_without_motion_emits_nothing() {
        let mut out = Vec::new();
        HostInput::default().flush_motion(|v| out.push(v));
        assert!(out.is_empty());
    }

    #[test]
    fn motion_precedes_a_routed_click() {
        let mut input = HostInput::default();
        input.motion(3.0, 0.0);
        let event = WindowEvent::MouseInput {
            device_id: winit::event::DeviceId::dummy(),
            state: ElementState::Pressed,
            button: MouseButton::Left,
        };
        let mut out = Vec::new();
        input.route(&event, |_, _| None, |v| out.push(v));
        assert_eq!(
            out,
            vec![
                motion_json(3.0, 0.0),
                serde_json::json!({"kind": "mouse_down", "button": "left"}),
            ]
        );
    }

    #[test]
    fn route_releases_on_focus_loss_and_ignores_focus_gain() {
        let mut input = held_input();
        let mut out = Vec::new();
        input.route(&WindowEvent::Focused(true), |_, _| None, |v| out.push(v));
        assert!(out.is_empty());
        input.route(&WindowEvent::Focused(false), |_, _| None, |v| out.push(v));
        assert_eq!(out, expected_release());
    }
}
