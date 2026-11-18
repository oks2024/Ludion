//! A meter that follows a moving 3D target, with a pop-in tween — the copyable HUD-motion pattern.
//!
//! A presentation entity carries a [`UiRect`] (the meter), a [`UiWorldAnchor`] naming the target and
//! a [`UiTween`] that fades and slides it in. `extract` does the rest: nothing about the meter is
//! written per tick. The headless `main` below prints, as the target slides right across the
//! screen, where the meter lands (`ludion::math::world_anchor`, the same placement `extract` uses)
//! and how far into its pop-in it is (`ui_tween_sample`).
//!
//! ```text
//! cargo run -p ludion --example ui_world_anchor
//! ```

use ludion::math::{Projection, ui_tween_sample, world_anchor};
use ludion::prelude::*;
use ludion_engine::components::ui::{
    UiAnchorBehind, UiAnchorShow, UiClock, UiEase, UiTween, UiTweenField, UiTweenRepeat,
    UiTweenTrack, UiWorldAnchor,
};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// Slides every `Transform` named by the scene's `target` to the right, 4 m/s.
fn slide_target(world: &mut World, ctx: &SystemCtx) {
    for (transform,) in world.query::<(&mut Transform,)>() {
        if transform.translation[1] == 1.0 {
            transform.translation[0] += 4.0 * ctx.dt;
        }
    }
}

pub fn game() -> EngineBuilder {
    Engine::builder().system("slide_target", slide_target)
}

/// The camera sits 10 m back looking at the origin; the target walks along `y = 1`.
const SCENE: &str = "\
format: 1
entities:
  - name: cam
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 10.0
      Camera: {}
  - name: target
    components:
      Transform:
        translation:
          - -4.0
          - 1.0
          - 0.0
";

/// The pop-in: fade over a quarter second while sliding up 24 canvas units.
fn pop_in() -> UiTween {
    let track = |field, from: &[f32], to: &[f32], ease| UiTweenTrack {
        field,
        from: from.to_vec(),
        to: to.to_vec(),
        start: 0,
        duration: 0.25,
        ease,
        repeat: UiTweenRepeat::Once,
        clock: UiClock::Real,
    };
    UiTween {
        tracks: vec![
            track(UiTweenField::Opacity, &[0.0], &[1.0], UiEase::OutQuad),
            track(
                UiTweenField::Offset,
                &[0.0, 24.0],
                &[0.0, 0.0],
                UiEase::OutBack,
            ),
        ],
    }
}

/// Spawns the meter over `target`: 2 m above it, bottom-centre pivot, hidden behind the camera.
fn spawn_meter(world: &mut World, target: EntityId) {
    let meter = world.spawn_presentation();
    world
        .insert(
            meter,
            UiRect {
                anchor: Anchor::BottomCenter,
                size: [80.0, 8.0],
                color: [0.2, 0.85, 0.3, 1.0],
                ..UiRect::default()
            },
        )
        .unwrap();
    world
        .insert(
            meter,
            UiWorldAnchor {
                target,
                world_offset: [0.0, 1.0, 0.0],
                show: UiAnchorShow::Always,
                behind: UiAnchorBehind::Hide,
                margin: 48.0,
                fade: Some([20.0, 40.0]),
            },
        )
        .unwrap();
    world.insert(meter, pop_in()).unwrap();
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
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    let target = engine
        .world()
        .entities()
        .nth(1)
        .expect("the scene has a target");
    spawn_meter(engine.world_mut(), target);

    let tween = pop_in();
    let viewport = (1920, 1080);
    for second in 0..4u64 {
        for _ in 0..30 {
            engine
                .execute(Command::SimStep(command::SimStepParams {
                    ticks: 1,
                    hash: false,
                }))
                .expect("the sim steps");
        }
        let frame = engine.frame().as_u64();
        let at = engine
            .world()
            .get::<Transform>(target)
            .unwrap()
            .unwrap()
            .translation;
        let placed = world_anchor(
            [0.0, 0.0, 10.0],
            [0.0; 3],
            Projection::Perspective { fov_y_deg: 60.0 },
            0.1,
            1000.0,
            [at[0], at[1] + 1.0, at[2]],
            viewport,
            48.0,
        )
        .expect("a valid camera");
        let opacity = ui_tween_sample(
            &tween,
            UiTweenField::Opacity,
            frame,
            frame,
            TickRate::DEFAULT,
        )
        .map_or(1.0, |v| v[0]);
        println!(
            "t={:.1}s target x {:5.1}  meter at ({:6.1}, {:6.1}) on_screen {:5}  pop-in opacity {opacity:.2}",
            (second + 1) as f32 * 0.5,
            at[0],
            placed.pixel[0],
            placed.pixel[1],
            placed.on_screen,
        );
    }
    ExitCode::SUCCESS
}
