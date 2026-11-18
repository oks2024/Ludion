//! A radar-style HUD panel built from one `UiGroup`: a frame, two blips and a pop-in tween on the
//! group — the copyable pattern for a unit that anchors, scales and fades together.
//!
//! The group is a 360 x 360 box in the top-right corner. Its children anchor inside the box rather
//! than the canvas (`group: <the group>`), so the frame fills the box (`fill_group`) and the blips
//! sit at fractions of it. A `UiTween` on the group scales and fades all three at once, and
//! `clip: circle` cuts the frame to a round radar. The headless `main` prints each drawn instance
//! (pixel rect and colour) at two frames of the pop-in, then `ui_hit` at the box centre and corner.
//!
//! ```text
//! cargo run -p ludion --example ui_groups
//! ```

use ludion::prelude::*;
use ludion_engine::components::ui::{
    UiClipShape, UiClock, UiEase, UiGroup, UiTween, UiTweenField, UiTweenRepeat, UiTweenTrack,
};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

pub fn game() -> EngineBuilder {
    Engine::builder()
}

/// The pop-in: scale from 0.6 and fade from 0 over a third of a second.
fn pop_in() -> UiTween {
    let track = |field, from: f32| UiTweenTrack {
        field,
        from: vec![from],
        to: vec![1.0],
        start: 0,
        duration: 1.0 / 3.0,
        ease: UiEase::OutQuad,
        repeat: UiTweenRepeat::Once,
        clock: UiClock::Real,
    };
    UiTween {
        tracks: vec![
            track(UiTweenField::Scale, 0.6),
            track(UiTweenField::Opacity, 0.0),
        ],
    }
}

/// Spawns the radar: the group, a frame covering its box and two blips anchored inside it. Returns
/// the frame, whose hit-test sees the circular clip.
fn spawn_radar(world: &mut World) -> EntityId {
    let group = world.spawn_presentation();
    world
        .insert(
            group,
            UiGroup {
                anchor: Anchor::TopRight,
                offset: [-24.0, 24.0],
                size: [360.0, 360.0],
                clip: UiClipShape::Circle,
                ..UiGroup::default()
            },
        )
        .unwrap();
    world.insert(group, pop_in()).unwrap();

    let frame = world.spawn_presentation();
    world
        .insert(
            frame,
            UiRect {
                color: [0.05, 0.1, 0.08, 0.8],
                group: Some(group),
                fill_group: true,
                ..UiRect::default()
            },
        )
        .unwrap();
    for (n, (anchor, offset, color)) in [
        (Anchor::Center, [0.0, 0.0], [0.3, 1.0, 0.5, 1.0]),
        (Anchor::TopLeft, [40.0, 60.0], [1.0, 0.4, 0.3, 1.0]),
    ]
    .into_iter()
    .enumerate()
    {
        let blip = world.spawn_presentation();
        world
            .insert(
                blip,
                UiRect {
                    anchor,
                    offset,
                    size: [16.0, 16.0],
                    color,
                    order: 1 + n as i32,
                    group: Some(group),
                    ..UiRect::default()
                },
            )
            .unwrap();
    }
    frame
}

// --- Headless demo `main` ------------------------------------------------------------------------

fn main() -> ExitCode {
    let mut engine = match game().build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene("format: 1\nentities: []\n") {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    let frame = spawn_radar(engine.world_mut());

    for ticks in [1u64, 30] {
        engine
            .execute(Command::SimStep(command::SimStepParams {
                ticks,
                hash: false,
            }))
            .expect("the sim steps");
        let scene = engine.extract_present_scene().expect("extracts").0;
        println!("frame {}:", engine.frame().as_u64());
        for instance in &scene.ui {
            println!("  {instance:?}");
        }
    }

    // The box is top-right at 1:1 canvas scale: its centre is inside the circle, its corner is not.
    let (width, _) = engine.render_size();
    let now = SimTime::from_ticks(engine.frame().as_u64(), TickRate::DEFAULT);
    let viewport = engine.render_size();
    let scale = viewport.1 as f32 / 1080.0;
    let right = width as f32 - 24.0 * scale;
    let centre = [right - 180.0 * scale, (24.0 + 180.0) * scale];
    let corner = [right - 1.0, 24.0 * scale + 1.0];
    println!(
        "ui_hit centre {} corner {}",
        math::ui_hit(engine.world(), frame, viewport, centre, now),
        math::ui_hit(engine.world(), frame, viewport, corner, now),
    );
    ExitCode::SUCCESS
}
