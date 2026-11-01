//! Tests of the animation clock: `set_speed`, the held grid and crossfade anchors.

use ludion_core::time::{FrameIndex, SimTime, TickRate};

use crate::components::{Animation, crossfade};
use crate::extract::{clip_elapsed, clip_ticks, from_clip_elapsed, held};
use crate::set_speed;

fn playing() -> Animation {
    Animation {
        clip: "walk".into(),
        ..Animation::default()
    }
}

fn at(ticks: u64) -> SimTime {
    SimTime::from_ticks(ticks, TickRate::DEFAULT)
}

#[test]
fn set_speed_keeps_clip_time_continuous_at_the_anchor() {
    use clip_elapsed;
    let mut animation = playing();
    let before = clip_elapsed(&animation, at(59));
    set_speed(&mut animation, 0.5, at(59));
    assert_eq!(clip_elapsed(&animation, at(59)).to_bits(), before.to_bits());
    for n in 0..300u64 {
        let closed = 59.0 / 60.0 + n as f64 / 60.0 * 0.5;
        let got = clip_elapsed(&animation, at(59 + n));
        assert!((got - closed).abs() < 1e-12, "n = {n}: {got} vs {closed}");
    }
}

#[test]
fn a_set_speed_anchor_round_trips_through_json_text_bit_for_bit() {
    for n in 1..400u64 {
        let mut animation = playing();
        set_speed(&mut animation, 0.5, at(n));
        let text = serde_json::to_string(&animation).unwrap();
        let back: Animation = serde_json::from_str(&text).unwrap();
        assert_eq!(back, animation, "tick {n}: {text}");
    }
    for n in 1..400u64 {
        let mut animation = Animation {
            hold: 5,
            ..playing()
        };
        set_speed(&mut animation, 0.5, at(n));
        let text = serde_json::to_string(&animation).unwrap();
        let back: Animation = serde_json::from_str(&text).unwrap();
        assert_eq!(back, animation, "hold 5, tick {n}: {text}");
    }
}

fn held_pose(animation: &Animation, t: u64) -> f64 {
    clip_ticks(animation, held(animation, at(t)))
}

#[test]
fn set_speed_between_steps_keeps_the_held_pose() {
    let mut animation = Animation {
        hold: 5,
        ..playing()
    };
    let pose = held_pose(&animation, 10);
    set_speed(&mut animation, 0.25, at(12));
    assert_eq!(animation.anchor.unwrap().grid_ticks, Some(10.0));
    for t in 12..=14 {
        assert_eq!(held_pose(&animation, t), pose, "tick {t}");
    }
    assert_eq!(held_pose(&animation, 15), 12.0 + 3.0 * 0.25);
    assert_eq!(clip_ticks(&animation, at(13)), 12.25);
}

#[test]
fn set_speed_every_tick_still_steps_on_the_grid() {
    let mut animation = Animation {
        hold: 5,
        ..playing()
    };
    let mut exact = 0.0_f64;
    let mut previous = held_pose(&animation, 0);
    for t in 1..=40u64 {
        // The speed in force over tick t - 1 -> t is the one set at t - 1.
        exact += animation.speed as f64;
        set_speed(&mut animation, if t % 2 == 0 { 0.5 } else { 2.0 }, at(t));
        let got = clip_ticks(&animation, at(t));
        assert!((got - exact).abs() < 1e-9, "tick {t}: {got} vs {exact}");
        let pose = held_pose(&animation, t);
        if t % 5 == 0 {
            assert_ne!(pose, previous, "tick {t} should step");
        } else {
            assert_eq!(pose, previous, "tick {t} must not step");
        }
        previous = pose;
    }
}

#[test]
fn hold_one_never_writes_grid_ticks() {
    let mut animation = playing();
    for t in 1..=20u64 {
        set_speed(&mut animation, if t % 2 == 0 { 0.5 } else { 2.0 }, at(t));
        assert_eq!(animation.anchor.unwrap().grid_ticks, None);
        let json = serde_json::to_value(animation.anchor.unwrap()).unwrap();
        assert!(json.get("grid_ticks").is_none());
    }
}

#[test]
fn a_crossfade_carries_the_grid_clock_to_the_outgoing_side() {
    use {clip_elapsed, from_clip_elapsed, held};
    let mut animation = Animation {
        hold: 5,
        ..playing()
    };
    let speed_one = clip_elapsed(&playing(), at(10));
    set_speed(&mut animation, 0.5, at(12));
    crossfade(&mut animation, "run", 0.25, FrameIndex(13));
    assert_eq!(
        from_clip_elapsed(&animation, held(&animation, at(13))).to_bits(),
        speed_one.to_bits()
    );
}

#[test]
fn set_speed_to_the_current_speed_writes_nothing() {
    let mut animation = playing();
    set_speed(&mut animation, 1.0, at(40));
    assert_eq!(animation, playing());
    for bad in [-1.0, f32::NAN, f32::INFINITY] {
        set_speed(&mut animation, bad, at(40));
        assert_eq!(animation.speed, 0.0);
        animation.speed = 1.0;
        animation.anchor = None;
    }
}

#[test]
fn set_speed_before_the_first_tick_leaves_no_anchor() {
    let mut animation = Animation {
        started: FrameIndex(10),
        ..playing()
    };
    set_speed(&mut animation, 2.0, at(10));
    assert_eq!((animation.speed, animation.anchor), (2.0, None));
    set_speed(&mut animation, 3.0, at(4));
    assert_eq!((animation.speed, animation.anchor), (3.0, None));
}

#[test]
fn a_restamp_ignores_a_stale_anchor() {
    use clip_elapsed;
    let mut animation = playing();
    set_speed(&mut animation, 0.5, at(30));
    assert!(animation.anchor.is_some());
    animation.started = FrameIndex(100);
    animation.speed = 1.0;
    assert_eq!(clip_elapsed(&animation, at(160)), 1.0);
}

#[test]
fn crossfade_carries_the_anchor_to_the_outgoing_side() {
    use {clip_elapsed, from_clip_elapsed};
    let mut animation = playing();
    set_speed(&mut animation, 0.5, at(30));
    let before = clip_elapsed(&animation, at(80));
    crossfade(&mut animation, "run", 0.25, FrameIndex(80));
    assert_eq!(animation.anchor, None);
    assert_eq!(
        from_clip_elapsed(&animation, at(80)).to_bits(),
        before.to_bits()
    );
    // The outgoing side keeps its own rate after the fade begins.
    assert!((from_clip_elapsed(&animation, at(140)) - (before + 0.5)).abs() < 1e-12);
}

#[test]
fn a_hard_cut_clears_both_anchors() {
    let mut animation = playing();
    set_speed(&mut animation, 0.5, at(30));
    crossfade(&mut animation, "run", 0.25, FrameIndex(80));
    crossfade(&mut animation, "idle", 0.0, FrameIndex(90));
    assert_eq!((animation.anchor, animation.from_anchor), (None, None));
}

/// `held` snaps down onto the global grid; a zero reads as one.
#[test]
fn held_snaps_to_the_hold_grid() {
    use held;
    let five = Animation {
        hold: 5,
        ..Animation::default()
    };
    for ticks in 10..=14 {
        assert_eq!(held(&five, at(ticks)), at(10));
    }
    assert_eq!(held(&five, at(15)), at(15));
    let zero = Animation {
        hold: 0,
        ..Animation::default()
    };
    assert_eq!(held(&zero, at(13)), at(13));
}
