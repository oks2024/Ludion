//! A ring countdown built from `UiShape`, and `ui_shape_hit` answering which part of it a cursor is on.
//! Pure functions of the component, so no engine is needed (`ludion docs ui-shapes`).
//!
//! ```text
//! cargo run -p ludion --example ui_shapes
//! ```

use ludion::math::ui_shape_hit;
use ludion::prelude::*;

fn main() {
    let viewport = (1920, 1080);
    let mut gauge = UiShape {
        anchor: Anchor::Center,
        shape: UiShapeType::Ring {
            radius: 80.0,
            thickness: 14.0,
            start_deg: 0.0,
            sweep_deg: 360.0,
        },
        fill: Some([1.0, 0.8, 0.2, 1.0]),
        ..UiShape::default()
    };

    // A game writes `sweep_deg` as the timer runs down; extract draws whatever the field says.
    for remaining in [1.0_f32, 0.75, 0.25] {
        if let UiShapeType::Ring { sweep_deg, .. } = &mut gauge.shape {
            *sweep_deg = 360.0 * remaining;
        }
        // Just past 12 o'clock on the band, then 9 o'clock (the last quarter to fill).
        let top = ui_shape_hit(&gauge, viewport, [975.0, 540.0 - 72.0]);
        let left = ui_shape_hit(&gauge, viewport, [960.0 - 73.0, 540.0]);
        println!("remaining {remaining}: 12 o'clock {top}, 9 o'clock {left}");
    }
    assert!(ui_shape_hit(&gauge, viewport, [975.0, 540.0 - 72.0]));
    assert!(!ui_shape_hit(&gauge, viewport, [960.0 - 73.0, 540.0]));

    // A concave "U" polygon: its arms hit, the notch between them does not.
    let bracket = UiShape {
        shape: UiShapeType::Polygon {
            points: vec![
                [0.0, 0.0],
                [100.0, 0.0],
                [100.0, 200.0],
                [70.0, 200.0],
                [70.0, 50.0],
                [30.0, 50.0],
                [30.0, 200.0],
                [0.0, 200.0],
            ],
            join: UiLineJoin::default(),
        },
        fill: Some([0.2, 0.9, 0.5, 1.0]),
        ..UiShape::default()
    };
    assert!(ui_shape_hit(&bracket, viewport, [10.0, 100.0]));
    assert!(!ui_shape_hit(&bracket, viewport, [50.0, 120.0]));
    println!("polygon: arm hit, notch miss");

    // A stroke-only polyline: a zigzag with mitred joins hits on its band, not inside its bends.
    let zigzag = UiShape {
        shape: UiShapeType::Polyline {
            points: vec![[0.0, 0.0], [60.0, 0.0], [60.0, 60.0], [120.0, 60.0]],
            closed: false,
            join: UiLineJoin::Miter,
            cap: UiLineCap::Butt,
        },
        stroke: Some(UiStroke {
            width: 10.0,
            color: [1.0, 0.8, 0.2, 1.0],
        }),
        ..UiShape::default()
    };
    assert!(ui_shape_hit(&zigzag, viewport, [30.0, 2.0]));
    assert!(ui_shape_hit(&zigzag, viewport, [60.0, 30.0]));
    assert!(!ui_shape_hit(&zigzag, viewport, [30.0, 30.0]));
    println!("polyline: band hit, bend miss");

    // The same zigzag dashed: it hits on a dash and misses in the gap between two.
    let dashed = UiShape {
        dash: Some(UiDash {
            on: 20.0,
            off: 20.0,
        }),
        ..zigzag
    };
    assert!(ui_shape_hit(&dashed, viewport, [10.0, 2.0]));
    assert!(!ui_shape_hit(&dashed, viewport, [30.0, 2.0]));
    println!("dash: dash hit, gap miss");
}
