//! The controller integrates on gameplay time: a clock hold freezes the hero's feet while the run
//! is held, and the run moves it before and after.

use std::path::PathBuf;

use ludion::prelude::*;
use ludion_testkit::GameplayDriver;

/// Holds raw frames 60..=69, so ticks 61..=70 are held.
fn hold_mid_run(world: &mut World, ctx: &SystemCtx) {
    if (60..=69).contains(&ctx.frame.as_u64()) {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
}

#[test]
fn the_controller_freezes_under_a_clock_hold() {
    let scenes = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes");
    let mut driver = GameplayDriver::new(
        character_course::game()
            .system("hold", hold_mid_run)
            .project_root(scenes),
        include_str!("../scenes/main.yaml"),
    );
    let mut feet = Vec::new();
    for frame in 0..90u64 {
        if frame == 2 {
            driver.inject("key_down", "Up");
        }
        driver.step(1);
        let pose = driver
            .engine_mut()
            .world_mut()
            .query::<(&character_course::Runner, &Transform)>()
            .next()
            .map(|(_, pose)| pose.translation)
            .unwrap();
        feet.push(pose);
    }
    let step = |tick: usize| {
        (0..3)
            .map(|i| {
                let d = feet[tick][i] - feet[tick - 1][i];
                d * d
            })
            .sum::<f32>()
            .sqrt()
    };
    // `step(t)` is the move between the records at t - 1 and t.
    for tick in 61..=70 {
        assert!(step(tick) < 1e-3, "tick {tick} moved {}", step(tick));
    }
    assert!(step(60) > 0.05, "before: {}", step(60));
    assert!(step(71) > 0.05, "after: {}", step(71));
}
