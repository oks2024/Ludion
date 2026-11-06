//! Mapping winit mouse events to the engine's abstract mouse vocabulary, mirroring `keymap.rs`.
//!
//! The sim's `MouseState::position` (`ludion_sim`) is documented in render-target pixels
//! (origin top-left, +Y down) — the same pixel space the streamed frame lives in. A winit
//! `CursorMoved` position, though, is in *window-surface* physical pixels, which differ from the
//! frame's pixels whenever the window's aspect ratio isn't the frame's: the presenter letterboxes the
//! frame into a centered, aspect-preserving rectangle (`ludion_core::letterbox`) and this module
//! inverts exactly that placement, so a click lands on the pixel it visually sits over regardless of
//! window size.
//!
//! **Cursor positions in the pillarbox/letterbox bars are clamped to the nearest frame edge, not
//! dropped.** Dropping would leave `mouse().position()` silently stuck at its last in-bounds sample
//! while the cursor keeps moving through the bars — a game reading it would see a frozen aim with no
//! signal that anything is wrong. Clamping keeps the value always inside `[0, frame_w] x
//! [0, frame_h]` (no extra branch at the call site) and tracks continuously as the cursor crosses the
//! frame edge, the expected feel for a cursor near a window border.
//!
//! Wheel deltas mirror winit's own two shapes: a traditional wheel reports [`LineDelta`] in units
//! already normalized by winit (one full "click" is usually ±1.0); a high-resolution
//! trackpad/touch surface reports [`PixelDelta`] in physical pixels. Both are folded to the same
//! rough "lines" unit by dividing a `PixelDelta` by `PIXELS_PER_LINE` — a feel constant (not a
//! protocol contract) matching the line-height browsers commonly assume for the same conversion —
//! so game code sees one consistent wheel unit regardless of the input device.
//!
//! [`LineDelta`]: winit::event::MouseScrollDelta::LineDelta
//! [`PixelDelta`]: winit::event::MouseScrollDelta::PixelDelta

use ludion_core::letterbox::{HostRect, letterbox_in};
use winit::event::{ElementState, MouseButton, MouseScrollDelta};

/// See the module docs' "pixels per line" note.
const PIXELS_PER_LINE: f32 = 20.0;

/// Maps a cursor position in window-surface physical pixels to render-target pixels, given the
/// frame's `frame_w`x`frame_h` and the surface's current `surface_w`x`surface_h`. Returns `None`
/// only when there is nothing to map against yet: no frame has been decoded, or the surface has a
/// zero dimension (a minimized window). Otherwise the position is always clamped into
/// `[0, frame_w] x [0, frame_h]`. `rect` is the host rectangle the frame is fitted into (`None`: the
/// whole surface), the same one the present blits with.
pub fn map_cursor_to_frame(
    cursor: (f32, f32),
    frame_w: u32,
    frame_h: u32,
    surface_w: u32,
    surface_h: u32,
    rect: Option<HostRect>,
) -> Option<[f32; 2]> {
    let placed = letterbox_in(frame_w, frame_h, surface_w, surface_h, rect);
    if !placed.is_drawable() {
        return None;
    }
    let x = ((cursor.0 - placed.x) / placed.width * frame_w as f32).clamp(0.0, frame_w as f32);
    let y = ((cursor.1 - placed.y) / placed.height * frame_h as f32).clamp(0.0, frame_h as f32);
    Some([x, y])
}

/// Scales a cursor position from window physical pixels into the surface's pixel space. The
/// surface is the window's size capped at the texture limit, so above it the two differ; equal sizes
/// map to the identity. A zero window side returns the cursor unscaled (nothing to scale against).
pub fn window_to_surface(
    cursor: (f32, f32),
    window: (u32, u32),
    surface: (u32, u32),
) -> (f32, f32) {
    let scale = |value: f32, window: u32, surface: u32| {
        if window == 0 {
            value
        } else {
            value * surface as f32 / window as f32
        }
    };
    (
        scale(cursor.0, window.0, surface.0),
        scale(cursor.1, window.1, surface.1),
    )
}

/// Builds the `input.inject` event `Value` for a new mouse position, in the Wave 1 pinned shape:
/// `{"kind": "mouse_move", "position_bits": [<u32>, <u32>]}`.
pub fn mouse_move_event(position: [f32; 2]) -> serde_json::Value {
    serde_json::json!({
        "kind": "mouse_move",
        "position_bits": [position[0].to_bits(), position[1].to_bits()],
    })
}

/// Builds the `input.inject` event `Value` for raw device motion:
/// `{"kind": "mouse_motion", "delta_bits": [<u32>, <u32>]}`.
pub fn mouse_motion_event(delta: [f32; 2]) -> serde_json::Value {
    serde_json::json!({
        "kind": "mouse_motion",
        "delta_bits": [delta[0].to_bits(), delta[1].to_bits()],
    })
}

/// Maps a winit mouse button to the engine's abstract name, or `None` for a button the engine's
/// milestone-minimal [`MouseButton`](ludion_sim's) set has no name for (`Back`/`Forward`/`Other`).
fn button_name(button: MouseButton) -> Option<&'static str> {
    match button {
        MouseButton::Left => Some("left"),
        MouseButton::Right => Some("right"),
        MouseButton::Middle => Some("middle"),
        _ => None,
    }
}

/// Translates mouse button transitions into the `input.inject` events to inject, remembering which
/// buttons it forwarded as down so a focus loss (the OS delivers no release for a button let go in
/// another window) can release them.
#[derive(Default)]
pub struct MouseButtons {
    /// The buttons forwarded as down, in press order.
    held: Vec<&'static str>,
}

impl MouseButtons {
    /// The event `Value` for a button transition, or `None` for an unmapped button.
    pub fn map(&mut self, button: MouseButton, state: ElementState) -> Option<serde_json::Value> {
        let name = button_name(button)?;
        let kind = match state {
            ElementState::Pressed => {
                if !self.held.contains(&name) {
                    self.held.push(name);
                }
                "mouse_down"
            }
            ElementState::Released => {
                self.held.retain(|&held| held != name);
                "mouse_up"
            }
        };
        Some(serde_json::json!({"kind": kind, "button": name}))
    }

    /// A `mouse_up` per held button in press order, forgetting them.
    pub fn release_held(&mut self) -> Vec<serde_json::Value> {
        self.held
            .drain(..)
            .map(|name| serde_json::json!({"kind": "mouse_up", "button": name}))
            .collect()
    }
}

/// Builds the `input.inject` event `Value` for a wheel motion, folding a `PixelDelta` into the same
/// rough unit as a `LineDelta` (see the module docs).
pub fn mouse_wheel_event(delta: MouseScrollDelta) -> serde_json::Value {
    let [dx, dy] = match delta {
        MouseScrollDelta::LineDelta(x, y) => [x, y],
        MouseScrollDelta::PixelDelta(pos) => [
            pos.x as f32 / PIXELS_PER_LINE,
            pos.y as f32 / PIXELS_PER_LINE,
        ],
    };
    serde_json::json!({
        "kind": "mouse_wheel",
        "delta_bits": [dx.to_bits(), dy.to_bits()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_to_surface_is_the_identity_at_equal_sizes() {
        assert_eq!(
            window_to_surface((12.5, 40.0), (800, 600), (800, 600)),
            (12.5, 40.0)
        );
    }

    #[test]
    fn window_to_surface_scales_a_window_beyond_the_surface_cap() {
        let (window, surface) = ((10240, 2160), (8192, 1728));
        assert_eq!(
            window_to_surface((10240.0, 2160.0), window, surface),
            (8192.0, 1728.0)
        );
        assert_eq!(
            window_to_surface((5120.0, 1080.0), window, surface),
            (4096.0, 864.0)
        );
    }

    #[test]
    fn window_to_surface_leaves_a_zero_window_side_unscaled() {
        assert_eq!(window_to_surface((3.0, 4.0), (0, 0), (8, 8)), (3.0, 4.0));
    }

    #[test]
    fn exact_aspect_scales_uniformly_with_no_bars() {
        // Frame and surface share an aspect ratio (2:1) at a 2x surface scale: no bars, so a
        // surface-pixel position maps to exactly half its coordinates in frame pixels.
        let pos = map_cursor_to_frame((50.0, 25.0), 200, 100, 400, 200, None).unwrap();
        assert_eq!(pos, [25.0, 12.5]);
    }

    #[test]
    fn pillarboxed_cursor_maps_through_the_letterbox_rect() {
        // 100x100 frame in a 200x100 surface: fit by height, 50px bars left/right (see letterbox.rs
        // tests). A cursor at the surface's exact center (100, 50) sits at the rect's center too.
        let pos = map_cursor_to_frame((100.0, 50.0), 100, 100, 200, 100, None).unwrap();
        assert_eq!(pos, [50.0, 50.0]);
    }

    #[test]
    fn cursor_in_the_bars_clamps_to_the_frame_edge() {
        // Same 100x100-in-200x100 layout: the rect is x in [50, 150]. A cursor at x=10 (left bar)
        // clamps to the frame's left edge; a cursor past the right bar clamps to the right edge.
        assert_eq!(
            map_cursor_to_frame((10.0, 50.0), 100, 100, 200, 100, None).unwrap(),
            [0.0, 50.0]
        );
        assert_eq!(
            map_cursor_to_frame((199.0, 50.0), 100, 100, 200, 100, None).unwrap(),
            [100.0, 50.0]
        );
    }

    #[test]
    fn cursor_maps_through_the_host_rect() {
        // 100x100 frame in the right half (x in [100, 200]) of a 200x100 surface: the rect's center
        // is the frame's center, and the left half is a bar that clamps to the frame's left edge.
        let rect = Some(HostRect {
            x: 100,
            y: 0,
            width: 100,
            height: 100,
        });
        assert_eq!(
            map_cursor_to_frame((150.0, 50.0), 100, 100, 200, 100, rect).unwrap(),
            [50.0, 50.0]
        );
        assert_eq!(
            map_cursor_to_frame((10.0, 50.0), 100, 100, 200, 100, rect).unwrap(),
            [0.0, 50.0]
        );
    }

    #[test]
    fn zero_size_surface_or_frame_yields_none() {
        assert_eq!(
            map_cursor_to_frame((0.0, 0.0), 0, 100, 200, 200, None),
            None
        );
        assert_eq!(
            map_cursor_to_frame((0.0, 0.0), 100, 100, 0, 200, None),
            None
        );
    }

    #[test]
    fn mouse_motion_event_carries_delta_bits() {
        assert_eq!(
            mouse_motion_event([1.5, -2.0]),
            serde_json::json!({
                "kind": "mouse_motion",
                "delta_bits": [1.5f32.to_bits(), (-2.0f32).to_bits()],
            })
        );
    }

    #[test]
    fn mouse_move_event_carries_bit_patterns_not_json_numbers() {
        let event = mouse_move_event([12.5, -3.0]);
        assert_eq!(
            event,
            serde_json::json!({
                "kind": "mouse_move",
                "position_bits": [12.5f32.to_bits(), (-3.0f32).to_bits()],
            })
        );
    }

    #[test]
    fn button_events_map_to_the_pinned_shape() {
        let mut buttons = MouseButtons::default();
        assert_eq!(
            buttons.map(MouseButton::Left, ElementState::Pressed),
            Some(serde_json::json!({"kind": "mouse_down", "button": "left"}))
        );
        assert_eq!(
            buttons.map(MouseButton::Right, ElementState::Released),
            Some(serde_json::json!({"kind": "mouse_up", "button": "right"}))
        );
        assert_eq!(
            buttons.map(MouseButton::Middle, ElementState::Pressed),
            Some(serde_json::json!({"kind": "mouse_down", "button": "middle"}))
        );
    }

    #[test]
    fn unmapped_buttons_are_ignored_and_untracked() {
        let mut buttons = MouseButtons::default();
        assert_eq!(buttons.map(MouseButton::Back, ElementState::Pressed), None);
        assert_eq!(
            buttons.map(MouseButton::Other(4), ElementState::Pressed),
            None
        );
        assert!(buttons.release_held().is_empty());
    }

    #[test]
    fn focus_loss_releases_held_buttons_in_press_order_once() {
        let mut buttons = MouseButtons::default();
        buttons.map(MouseButton::Left, ElementState::Pressed);
        buttons.map(MouseButton::Right, ElementState::Pressed);
        buttons.map(MouseButton::Middle, ElementState::Pressed);
        buttons.map(MouseButton::Middle, ElementState::Released);
        assert_eq!(
            buttons.release_held(),
            vec![
                serde_json::json!({"kind": "mouse_up", "button": "left"}),
                serde_json::json!({"kind": "mouse_up", "button": "right"}),
            ]
        );
        assert!(buttons.release_held().is_empty());
    }

    #[test]
    fn line_delta_passes_through_unscaled() {
        let event = mouse_wheel_event(MouseScrollDelta::LineDelta(0.0, 1.0));
        assert_eq!(
            event,
            serde_json::json!({
                "kind": "mouse_wheel",
                "delta_bits": [0.0f32.to_bits(), 1.0f32.to_bits()],
            })
        );
    }

    #[test]
    fn pixel_delta_is_scaled_down_to_the_same_rough_unit() {
        let event = mouse_wheel_event(MouseScrollDelta::PixelDelta(winit::dpi::PhysicalPosition {
            x: 0.0,
            y: PIXELS_PER_LINE as f64,
        }));
        assert_eq!(
            event,
            serde_json::json!({
                "kind": "mouse_wheel",
                "delta_bits": [0.0f32.to_bits(), 1.0f32.to_bits()],
            })
        );
    }
}
