//! Aspect-preserving placement of a fixed-size frame inside a surface of another size.
//!
//! Two callers share this exact problem: `ludion-window`'s presenter fits a streamed frame (whose
//! size is the server's render resolution) into the OS window the user drags to any size, and
//! `ludion-render-graph`'s in-process present target fits the engine's own offscreen frame into the
//! window it draws into directly. Both want the same answer — scale the frame to the largest
//! rectangle that fits the surface while keeping the frame's aspect ratio, and center it, filling the
//! remaining margin with black bars (pillarbox when the surface is wider than the frame, letterbox
//! when it is taller) — so this module lives where both can depend on it without either depending on
//! the other. The result is fed straight to a `set_viewport` call, so the values are physical pixels
//! expressed as `f32`.

/// A placement rectangle in physical pixels, ready for a `set_viewport` call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Left edge, in physical pixels from the surface's left.
    pub x: f32,
    /// Top edge, in physical pixels from the surface's top.
    pub y: f32,
    /// Width in physical pixels.
    pub width: f32,
    /// Height in physical pixels.
    pub height: f32,
}

impl Viewport {
    /// The empty rectangle — used when there is nothing to place (a zero-size frame or surface).
    pub const ZERO: Viewport = Viewport {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    };

    /// Whether the rectangle has a drawable area (both dimensions at least one pixel).
    pub fn is_drawable(&self) -> bool {
        self.width >= 1.0 && self.height >= 1.0
    }
}

/// Computes the centered, aspect-preserving placement of a `frame_w`×`frame_h` image inside a
/// `surface_w`×`surface_h` surface.
///
/// Returns [`Viewport::ZERO`] if any dimension is zero (nothing to draw — the caller just clears to
/// black). Otherwise the frame is scaled to fit and centered: the returned rectangle always lies
/// within the surface, so it is a valid `set_viewport` argument.
pub fn letterbox(frame_w: u32, frame_h: u32, surface_w: u32, surface_h: u32) -> Viewport {
    if frame_w == 0 || frame_h == 0 || surface_w == 0 || surface_h == 0 {
        return Viewport::ZERO;
    }
    // Compare aspect ratios via cross-multiplication (all values are positive) to decide which axis
    // the fit is bound on, avoiding a divide until the sizes are known non-degenerate.
    let frame_w = frame_w as f64;
    let frame_h = frame_h as f64;
    let surface_w = surface_w as f64;
    let surface_h = surface_h as f64;

    // frame_aspect vs surface_aspect: frame_w/frame_h vs surface_w/surface_h.
    if surface_w * frame_h > surface_h * frame_w {
        // Surface is proportionally wider than the frame → bound by height; bars left/right.
        let height = surface_h;
        let width = surface_h * frame_w / frame_h;
        Viewport {
            x: ((surface_w - width) / 2.0) as f32,
            y: 0.0,
            width: width as f32,
            height: height as f32,
        }
    } else {
        // Surface is proportionally taller (or equal) → bound by width; bars top/bottom.
        let width = surface_w;
        let height = surface_w * frame_h / frame_w;
        Viewport {
            x: 0.0,
            y: ((surface_h - height) / 2.0) as f32,
            width: width as f32,
            height: height as f32,
        }
    }
}

/// A sub-rectangle of the surface, in physical pixels — the part a host lets the frame occupy (an
/// iPhone's safe area, clear of the notch and the home indicator).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostRect {
    /// Left edge, in physical pixels from the surface's left.
    pub x: u32,
    /// Top edge, in physical pixels from the surface's top.
    pub y: u32,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

/// [`letterbox`] into a host rectangle of the surface instead of the whole of it.
///
/// `None`, or a rectangle that clamps to nothing inside the surface, places the frame against the
/// whole surface. Otherwise the rectangle is clamped to the surface (a viewport outside the
/// attachment is a validation error) and the frame is fitted and centered inside it.
pub fn letterbox_in(
    frame_w: u32,
    frame_h: u32,
    surface_w: u32,
    surface_h: u32,
    rect: Option<HostRect>,
) -> Viewport {
    let clamped = rect.and_then(|r| {
        let width = r.width.min(surface_w.saturating_sub(r.x));
        let height = r.height.min(surface_h.saturating_sub(r.y));
        (width > 0 && height > 0).then_some(HostRect {
            x: r.x,
            y: r.y,
            width,
            height,
        })
    });
    let Some(r) = clamped else {
        return letterbox(frame_w, frame_h, surface_w, surface_h);
    };
    let inner = letterbox(frame_w, frame_h, r.width, r.height);
    if inner == Viewport::ZERO {
        return inner;
    }
    Viewport {
        x: inner.x + r.x as f32,
        y: inner.y + r.y as f32,
        ..inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_rect_is_the_whole_surface() {
        assert_eq!(
            letterbox_in(1385, 720, 2532, 1170, None),
            letterbox(1385, 720, 2532, 1170)
        );
    }

    #[test]
    fn iphone_frame_centers_inside_the_safe_rect() {
        // 2532x1170 surface, inset 141px each side: the frame's aspect is the rect's (2250x1170) to
        // rounding, so it fills the rect, offset by the inset.
        let rect = HostRect {
            x: 141,
            y: 0,
            width: 2250,
            height: 1170,
        };
        let v = letterbox_in(1385, 720, 2532, 1170, Some(rect));
        assert_eq!(v.x, 141.0);
        assert!(v.y.abs() < 1.0);
        assert!((v.width - 2250.0).abs() < 1.0);
        assert!((v.height - 1170.0).abs() < 1.0);
    }

    #[test]
    fn taller_rect_bars_top_and_bottom_inside_it() {
        // 16:9 frame in a 1000x1000 rect at (100, 50) of a 1200x1200 surface.
        let rect = HostRect {
            x: 100,
            y: 50,
            width: 1000,
            height: 1000,
        };
        let v = letterbox_in(1600, 900, 1200, 1200, Some(rect));
        assert_eq!(v.x, 100.0);
        assert_eq!(v.width, 1000.0);
        assert!((v.height - 562.5).abs() < 1e-3);
        assert!((v.y - (50.0 + 218.75)).abs() < 1e-3);
    }

    #[test]
    fn rect_past_the_surface_clamps() {
        let rect = HostRect {
            x: 100,
            y: 0,
            width: 500,
            height: 100,
        };
        // Only 100px of the rect lies inside a 200x100 surface: a square frame fills it.
        let v = letterbox_in(100, 100, 200, 100, Some(rect));
        assert_eq!(
            v,
            Viewport {
                x: 100.0,
                y: 0.0,
                width: 100.0,
                height: 100.0
            }
        );
    }

    #[test]
    fn empty_rect_falls_back_to_the_whole_surface() {
        let empty = HostRect {
            x: 0,
            y: 0,
            width: 0,
            height: 50,
        };
        let outside = HostRect {
            x: 400,
            y: 0,
            width: 50,
            height: 50,
        };
        for rect in [empty, outside] {
            assert_eq!(
                letterbox_in(100, 100, 200, 100, Some(rect)),
                letterbox(100, 100, 200, 100)
            );
        }
    }

    #[test]
    fn exact_aspect_fills_the_surface() {
        // Frame and surface share an aspect ratio (2:1): the whole surface is used, no bars.
        let v = letterbox(200, 100, 400, 200);
        assert_eq!(
            v,
            Viewport {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 200.0
            }
        );
    }

    #[test]
    fn wider_surface_pillarboxes() {
        // 100x100 frame in a 200x100 surface: fit by height, bars on the left and right.
        let v = letterbox(100, 100, 200, 100);
        assert_eq!(
            v,
            Viewport {
                x: 50.0,
                y: 0.0,
                width: 100.0,
                height: 100.0
            }
        );
    }

    #[test]
    fn taller_surface_letterboxes() {
        // 100x100 frame in a 100x200 surface: fit by width, bars on top and bottom.
        let v = letterbox(100, 100, 100, 200);
        assert_eq!(
            v,
            Viewport {
                x: 0.0,
                y: 50.0,
                width: 100.0,
                height: 100.0
            }
        );
    }

    #[test]
    fn scales_up_preserving_aspect() {
        // A 320x180 (16:9) frame in a 1920x1080 (16:9) surface scales to fill exactly.
        let v = letterbox(320, 180, 1920, 1080);
        assert_eq!(
            v,
            Viewport {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0
            }
        );
    }

    #[test]
    fn wide_frame_in_square_surface_letterboxes() {
        // 16:9 frame in a 1000x1000 square: width-bound, height 562.5, centered vertically.
        let v = letterbox(1600, 900, 1000, 1000);
        assert_eq!(v.x, 0.0);
        assert_eq!(v.width, 1000.0);
        assert!((v.height - 562.5).abs() < 1e-3);
        assert!((v.y - 218.75).abs() < 1e-3);
    }

    #[test]
    fn zero_sizes_yield_an_empty_undrawable_rect() {
        assert_eq!(letterbox(0, 100, 200, 200), Viewport::ZERO);
        assert_eq!(letterbox(100, 0, 200, 200), Viewport::ZERO);
        assert_eq!(letterbox(100, 100, 0, 200), Viewport::ZERO);
        assert_eq!(letterbox(100, 100, 200, 0), Viewport::ZERO);
        assert!(!letterbox(0, 0, 0, 0).is_drawable());
    }
}
