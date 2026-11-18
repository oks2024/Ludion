//! `Timer` and `Cooldown` land on the exact gameplay tick.

use ludion_ai::{Cooldown, Timer};
use ludion_core::time::{FrameIndex, GameClock, TickRate};
use ludion_ecs::SystemCtx;

fn ctx(frame: u64) -> SystemCtx {
    SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT)
}

#[test]
fn expires_on_the_exact_tick_at_60_hz() {
    for (seconds, ticks) in [(0.25f32, 15u64), (0.3, 18), (2.0, 120), (30.0, 1800)] {
        let mut timer = Timer::default();
        timer.start(&ctx(100), seconds);
        assert_eq!(timer.deadline, Some(100 + ticks), "{seconds}s");
        assert!(timer.running());
        assert!(!timer.expired(&ctx(100 + ticks - 1)), "{seconds}s early");
        assert!(!timer.just_expired(&ctx(100 + ticks - 1)));
        assert!(timer.expired(&ctx(100 + ticks)) && timer.just_expired(&ctx(100 + ticks)));
        assert!(timer.expired(&ctx(100 + ticks + 1)) && !timer.just_expired(&ctx(100 + ticks + 1)));
        assert_eq!(timer.remaining_ticks(&ctx(100)), ticks);
    }
}

#[test]
fn remaining_reads_seconds_and_stop_clears() {
    let mut timer = Timer::default();
    assert!(!timer.running() && !timer.expired(&ctx(0)) && timer.remaining(&ctx(0)) == 0.0);
    timer.start(&ctx(0), 2.0);
    assert!((timer.remaining(&ctx(60)) - 1.0).abs() < 1e-6);
    timer.stop();
    assert!(!timer.running() && !timer.expired(&ctx(500)));
}

#[test]
fn a_held_clock_freezes_the_timer() {
    let mut clock = GameClock::default();
    let mut timer = Timer::default();
    timer.start(&ctx(0), 1.0); // deadline 60
    // Sim ticks 0..30 advance, then 30 ticks are held: gameplay time stays at 30.
    let mut game_now = 0;
    for frame in 0..60 {
        if frame >= 30 {
            clock.hold();
        }
        let advanced = clock.advance();
        let c = ctx(frame).with_clock(&clock, advanced);
        game_now = c.game_time.ticks();
        assert!(
            !timer.expired(&c),
            "frame {frame} (gameplay tick {game_now})"
        );
        assert!(!timer.just_expired(&c));
    }
    assert!(game_now < 60);
}

#[test]
fn cooldown_gates_in_gameplay_ticks() {
    let mut shot = Cooldown::default();
    assert!(shot.ready(&ctx(0)));
    shot.trigger(&ctx(10), 0.5);
    assert_eq!(shot.ready_at, 40);
    assert!(!shot.ready(&ctx(39)) && shot.ready(&ctx(40)));
    shot.trigger_ticks(&ctx(50), 3);
    assert!(!shot.ready(&ctx(52)) && shot.ready(&ctx(53)));
}

#[test]
fn timers_serialize_compactly() {
    assert_eq!(serde_json::to_string(&Timer::default()).unwrap(), "{}");
    let t: Timer = serde_json::from_str(r#"{"deadline":1800}"#).unwrap();
    assert_eq!(t.deadline, Some(1800));
    let c: Cooldown = serde_json::from_str("{}").unwrap();
    assert_eq!(c.ready_at, 0);
}
