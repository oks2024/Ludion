//! The rig pipeline, tick by tick, on hand-built paths.

use ludion_camera::{
    BlendCurve, BlendIn, CameraRig, LookAhead, RigConfig, RigInput, RigMode, RigOutput, Springs,
    TrackRig, add_trauma, camera_relative, camera_relative_yaw, snap_rig, step_rig,
};
use ludion_core::math::{euler_xyz_degrees_to_quat, quat_rotate};
use ludion_core::time::FrameIndex;
use ludion_scene::{PathDocument, PathInterpolation, PathSampler};

const DT: f32 = 1.0 / 60.0;

fn sampler(points: &[[f64; 3]], interpolation: PathInterpolation, closed: bool) -> PathSampler {
    PathSampler::new(&PathDocument {
        interpolation,
        closed,
        points: points.to_vec(),
        ..PathDocument::default()
    })
}

/// A straight path running −Z for 100 m: tangent −Z, right +X, back +Z.
fn straight() -> PathSampler {
    sampler(
        &[[0.0, 0.0, 0.0], [0.0, 0.0, -100.0]],
        PathInterpolation::Linear,
        false,
    )
}

fn rigid(offset: [f32; 3]) -> RigConfig {
    RigConfig {
        mode: RigMode::Track(TrackRig {
            offset,
            ..TrackRig::default()
        }),
        springs: Springs {
            position_half_life: 0.0,
            rotation_half_life: 0.0,
        },
        look_ahead: LookAhead {
            distance: 0.0,
            ..LookAhead::default()
        },
        ..RigConfig::default()
    }
}

fn rig_with(cfg: RigConfig) -> CameraRig {
    CameraRig {
        rig: cfg,
        ..CameraRig::default()
    }
}

/// One tick of gameplay at frame `frame`.
fn tick(rig: &mut CameraRig, path: &PathSampler, target: [f32; 3], frame: u64) -> RigOutput {
    let input = RigInput {
        target: Some(target),
        path: Some(path),
        game_dt: DT,
        dt: DT,
        game_frame: FrameIndex::new(frame),
        camera_fov_y_deg: None,
    };
    step_rig(rig, &input).expect("target and path present")
}

fn near(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= eps)
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

// ── track offsets ────────────────────────────────────────────────────────────────────────────────

#[test]
fn offsets_land_behind_beside_and_in_front_on_a_straight_path() {
    let path = straight();
    for (offset, eye) in [
        ([0.0, 2.2, 4.5], [0.0, 2.2, -15.5]),
        ([6.0, 1.5, 0.0], [6.0, 1.5, -20.0]),
        ([0.0, 2.0, -5.0], [0.0, 2.0, -25.0]),
    ] {
        let mut rig = rig_with(rigid(offset));
        let out = tick(&mut rig, &path, [0.0, 1.0, -20.0], 0);
        assert!(
            near(out.translation, eye, 1e-4),
            "{offset:?}: {:?}",
            out.translation
        );
    }
}

#[test]
fn the_camera_keeps_zero_roll_on_a_curve() {
    let path = sampler(
        &[
            [0.0, 0.0, 0.0],
            [0.0, 0.0, -20.0],
            [20.0, 0.0, -40.0],
            [40.0, 0.0, -40.0],
        ],
        PathInterpolation::CatmullRom,
        false,
    );
    let mut rig = rig_with(RigConfig::default());
    let length = path.length() as f32;
    for k in 0..300 {
        let s = (k as f32 / 300.0) * length;
        let p = path.point_at(f64::from(s));
        let out = tick(&mut rig, &path, [p[0] as f32, 0.0, p[2] as f32], k);
        let right = quat_rotate(euler_xyz_degrees_to_quat(out.rotation_deg), [1.0, 0.0, 0.0]);
        assert!(right[1].abs() < 1e-5, "tick {k}: right.y {}", right[1]);
    }
}

// ── projection continuity ────────────────────────────────────────────────────────────────────────

#[test]
fn the_projection_stays_on_its_leg_through_a_switchback() {
    let path = sampler(
        &[
            [0.0, 0.0, 0.0],
            [20.0, 0.0, 0.0],
            [20.0, 0.0, 3.0],
            [0.0, 0.0, 3.0],
            [0.0, 0.0, 6.0],
            [20.0, 0.0, 6.0],
        ],
        PathInterpolation::Linear,
        false,
    );
    let mut rig = rig_with(rigid([0.0, 1.0, 0.0]));
    let mut last_eye = None;
    let mut s = 0.0_f64;
    let mut frame = 0;
    while s < path.length() {
        let p = path.point_at(s);
        tick(&mut rig, &path, [p[0] as f32, 0.0, p[2] as f32], frame);
        let param = f64::from(rig.state.param.unwrap());
        assert!((param - s).abs() < 0.05, "param {param} vs s {s}");
        if let Some(prev) = last_eye {
            assert!(dist(prev, rig.state.eye) < 0.11, "eye jumped at s {s}");
        }
        last_eye = Some(rig.state.eye);
        s += 0.1;
        frame += 1;
    }
}

#[test]
fn the_projection_wraps_around_a_closed_loop() {
    let path = sampler(
        &[
            [-20.0, 0.0, -20.0],
            [20.0, 0.0, -20.0],
            [20.0, 0.0, 20.0],
            [-20.0, 0.0, 20.0],
        ],
        PathInterpolation::CatmullRom,
        true,
    );
    let length = path.length();
    let mut rig = rig_with(rigid([0.0, 1.0, 3.0]));
    let mut last_eye = None;
    let mut s = 0.0_f64;
    for frame in 0..((length * 1.3 / 0.1) as u64) {
        let p = path.point_at(s.rem_euclid(length));
        tick(&mut rig, &path, [p[0] as f32, 0.0, p[2] as f32], frame);
        let param = f64::from(rig.state.param.unwrap());
        let gap = (param - s.rem_euclid(length)).abs();
        assert!(gap.min(length - gap) < 0.15, "param {param} vs s {s}");
        if let Some(prev) = last_eye {
            assert!(dist(prev, rig.state.eye) < 0.3, "eye jumped at s {s}");
        }
        last_eye = Some(rig.state.eye);
        s += 0.1;
    }
}

// ── look-ahead and the vertical anchor ───────────────────────────────────────────────────────────

fn look_ahead_after(speed: f32) -> [f32; 2] {
    let path = straight();
    let mut cfg = rigid([0.0, 2.0, 4.0]);
    cfg.look_ahead = LookAhead {
        distance: 1.0,
        full_speed: 6.0,
        half_life: 0.05,
    };
    let mut rig = rig_with(cfg);
    let mut z = -10.0;
    for frame in 0..180 {
        tick(&mut rig, &path, [0.0, 0.0, z], frame);
        z -= speed * DT;
    }
    rig.state.look_ahead
}

#[test]
fn look_ahead_clamps_at_its_distance_and_scales_with_speed() {
    let full = look_ahead_after(12.0);
    assert!(
        full[0].abs() < 1e-3 && (full[1] + 1.0).abs() < 1e-3,
        "{full:?}"
    );
    let half = look_ahead_after(3.0);
    assert!(
        half[0].abs() < 1e-3 && (half[1] + 0.5).abs() < 1e-3,
        "{half:?}"
    );
    assert_eq!(look_ahead_after(0.0), [0.0, 0.0]);
}

fn anchored(grounded: bool, y: f32, ticks: u64, rig: &mut CameraRig, path: &PathSampler) {
    rig.grounded = grounded;
    for k in 0..ticks {
        tick(rig, path, [0.0, y, -20.0], k);
    }
}

#[test]
fn the_anchor_holds_in_the_air_and_follows_on_landing_or_past_the_slack() {
    let path = straight();
    let mut rig = rig_with(rigid([0.0, 2.0, 4.0]));
    anchored(true, 0.0, 5, &mut rig, &path);
    assert_eq!(rig.state.anchor_y, 0.0);

    anchored(false, 1.0, 120, &mut rig, &path);
    assert_eq!(
        rig.state.anchor_y, 0.0,
        "a jump inside the up slack leaves the anchor"
    );

    anchored(true, 2.0, 240, &mut rig, &path);
    assert!(
        (rig.state.anchor_y - 2.0).abs() < 1e-3,
        "landing higher moves it"
    );

    anchored(false, 5.0, 240, &mut rig, &path);
    assert!(
        (rig.state.anchor_y - 3.5).abs() < 1e-3,
        "past up_slack it follows: {}",
        rig.state.anchor_y
    );

    anchored(false, -3.0, 240, &mut rig, &path);
    assert!(
        (rig.state.anchor_y - -2.0).abs() < 1e-3,
        "past down_slack it follows: {}",
        rig.state.anchor_y
    );
}

// ── blend ────────────────────────────────────────────────────────────────────────────────────────

fn blended_cfg(offset: [f32; 3], seconds: f32) -> RigConfig {
    RigConfig {
        mode: RigMode::Track(TrackRig {
            offset,
            ..TrackRig::default()
        }),
        blend_in: BlendIn {
            seconds,
            curve: BlendCurve::EaseInOut,
        },
        ..RigConfig::default()
    }
}

#[test]
fn a_config_write_blends_from_the_previous_pose_to_the_new_one() {
    let path = straight();
    let target = [0.0, 1.0, -20.0];
    let mut rig = rig_with(blended_cfg([0.0, 2.2, 4.5], 1.0));
    let mut previous = tick(&mut rig, &path, target, 0);
    for frame in 1..30 {
        previous = tick(&mut rig, &path, target, frame);
    }

    rig.rig = blended_cfg([6.0, 1.5, 0.0], 1.0);
    let mut frame = 30;
    let first = tick(&mut rig, &path, target, frame);
    assert_eq!(
        first, previous,
        "tick 0 of a blend is the previous output, bit for bit"
    );

    let mut last = first;
    let mut ticks = 0;
    while rig.state.blend.is_some() {
        frame += 1;
        ticks += 1;
        let out = tick(&mut rig, &path, target, frame);
        assert!(
            dist(last.translation, out.translation) < 0.25,
            "no cut mid-blend (tick {ticks})"
        );
        last = out;
        assert!(ticks < 80, "the blend must end");
    }
    assert!((59..=61).contains(&ticks), "1 s at 60 Hz, got {ticks}");
    assert_eq!(
        last.translation, rig.state.eye,
        "the blend ends on the rig pose itself"
    );
    assert!(near(rig.state.eye, [6.0, 1.5, -20.0], 1e-3));
}

fn wrapped(degrees: f32) -> f32 {
    degrees - 360.0 * ((degrees + 180.0) / 360.0).floor()
}

#[test]
fn a_half_turn_blend_keeps_its_arc_when_the_live_yaw_crosses_the_seam() {
    let path = straight();
    let cut = |offset| RigConfig {
        springs: Springs {
            position_half_life: 0.0,
            rotation_half_life: 0.0,
        },
        ..blended_cfg(offset, 1.0)
    };
    // The target drifts across x = 0, which flips the front rig's live yaw between ±180.
    let target = |k: u64| [-0.5 + k as f32 / 60.0, 1.0, -20.0];
    let mut rig = rig_with(cut([0.0, 2.2, 4.5]));
    tick(&mut rig, &path, target(0), 0);
    rig.rig = cut([0.0, 2.0, -5.0]);
    let mut last = rig.state.pose.as_ref().unwrap().yaw_deg;
    let mut ticks = 0;
    let mut frame = 1;
    loop {
        tick(&mut rig, &path, target(frame), frame);
        let yaw = rig.state.pose.as_ref().unwrap().yaw_deg;
        assert!(
            wrapped(yaw - last).abs() < 10.0,
            "tick {ticks}: yaw jumped {last} -> {yaw}"
        );
        last = yaw;
        if rig.state.blend.is_none() {
            break;
        }
        frame += 1;
        ticks += 1;
        assert!(ticks < 80, "the blend must end");
    }
    assert_eq!(
        last, rig.state.yaw_deg,
        "the blend ends on the rig pose itself"
    );
}

#[test]
fn a_blend_stamped_in_the_future_snaps_instead_of_freezing() {
    let path = straight();
    let target = [0.0, 1.0, -20.0];
    let mut rig = rig_with(blended_cfg([0.0, 2.2, 4.5], 1.0));
    tick(&mut rig, &path, target, 0);
    rig.rig = blended_cfg([6.0, 1.5, 0.0], 1.0);
    for frame in 100..110 {
        tick(&mut rig, &path, target, frame);
    }
    assert!(rig.state.blend.is_some());

    // A scene saved mid-blend and reloaded: the game clock restarts below the blend's stamp.
    let mut reloaded: CameraRig =
        serde_json::from_value(serde_json::to_value(&rig).unwrap()).unwrap();
    let out = tick(&mut reloaded, &path, target, 0);
    assert!(reloaded.state.blend.is_none(), "the stale blend is dropped");
    assert!(near(out.translation, [6.0, 1.5, -20.0], 1e-3), "{out:?}");
}

#[test]
fn a_write_mid_blend_starts_from_the_blended_pose() {
    let path = straight();
    let target = [0.0, 1.0, -20.0];
    let mut rig = rig_with(blended_cfg([0.0, 2.2, 4.5], 1.0));
    tick(&mut rig, &path, target, 0);
    rig.rig = blended_cfg([6.0, 1.5, 0.0], 1.0);
    let mut out = tick(&mut rig, &path, target, 1);
    for frame in 2..22 {
        out = tick(&mut rig, &path, target, frame);
    }
    assert!(rig.state.blend.is_some());

    rig.rig = blended_cfg([-6.0, 1.0, 0.0], 0.5);
    let next = tick(&mut rig, &path, target, 22);
    assert_eq!(
        next.translation, out.translation,
        "the new blend starts where the old one was"
    );
    assert!(dist(next.translation, out.translation) < 1e-6);
}

#[test]
fn a_zero_second_blend_cuts() {
    let path = straight();
    let target = [0.0, 1.0, -20.0];
    let mut rig = rig_with(blended_cfg([0.0, 2.2, 4.5], 1.0));
    tick(&mut rig, &path, target, 0);
    rig.rig = blended_cfg([6.0, 1.5, 0.0], 0.0);
    let out = tick(&mut rig, &path, target, 1);
    assert!(rig.state.blend.is_none());
    assert!(near(out.translation, [6.0, 1.5, -20.0], 1e-4));
}

#[test]
fn a_snap_cuts_in_one_tick_with_no_residual_blend() {
    let path = straight();
    let cfg = blended_cfg([0.0, 2.2, 4.5], 1.0);
    let mut rig = rig_with(cfg.clone());
    tick(&mut rig, &path, [0.0, 0.0, -10.0], 0);
    for frame in 1..40 {
        tick(&mut rig, &path, [0.0, 0.0, -10.0 - frame as f32], frame);
    }
    rig.rig = blended_cfg([6.0, 1.5, 0.0], 1.0);
    tick(&mut rig, &path, [0.0, 0.0, -50.0], 40);
    assert!(rig.state.blend.is_some());

    let far = [0.0, 3.0, -80.0];
    snap_rig(&mut rig);
    let out = tick(&mut rig, &path, far, 41);
    assert!(rig.state.blend.is_none() && !rig.state.snap);

    let mut twin = rig_with(rig.rig.clone());
    twin.grounded = rig.grounded;
    let expected = tick(&mut twin, &path, far, 0);
    assert_eq!(out, expected, "a snap equals a rig's first tick");
}

// ── shake ────────────────────────────────────────────────────────────────────────────────────────

fn shaken(seed: u64, trauma: f32, ticks: u64) -> (CameraRig, Vec<RigOutput>) {
    let path = straight();
    let mut rig = rig_with(RigConfig::default());
    rig.shake.seed = seed;
    rig.trauma = trauma;
    let outs = (0..ticks)
        .map(|k| tick(&mut rig, &path, [0.0, 0.0, -10.0 - k as f32 * 0.05], k))
        .collect();
    (rig, outs)
}

#[test]
fn shake_is_deterministic_and_seeded() {
    let (_, a) = shaken(7, 0.8, 20);
    let (_, b) = shaken(7, 0.8, 20);
    let (_, c) = shaken(8, 0.8, 20);
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn zero_trauma_is_bit_identical_to_a_never_shaken_twin_and_never_moves_yaw() {
    let (shaken_rig, shaken_outs) = shaken(1, 0.5, 60);
    let (twin_rig, twin_outs) = shaken(1, 0.0, 60);
    assert_ne!(
        shaken_outs[0], twin_outs[0],
        "the shake does something while trauma lasts"
    );
    assert_eq!(
        shaken_rig.trauma, 0.0,
        "0.5 trauma at 1.2/s is gone within 60 ticks"
    );
    // Once trauma has hit zero the shaken rig's output is the twin's, bit for bit.
    assert_eq!(shaken_outs[59], twin_outs[59]);
    assert_eq!(shaken_rig.yaw_deg().to_bits(), twin_rig.yaw_deg().to_bits());
    assert_eq!(shaken_rig.state.pose, twin_rig.state.pose);
}

#[test]
fn trauma_decays_to_zero_in_one_over_its_rate() {
    let path = straight();
    let mut rig = rig_with(RigConfig::default());
    rig.shake.decay_per_second = 2.0;
    rig.trauma = 1.0;
    for k in 0..28 {
        tick(&mut rig, &path, [0.0, 0.0, -10.0], k);
    }
    assert!(rig.trauma > 0.0);
    for k in 28..32 {
        tick(&mut rig, &path, [0.0, 0.0, -10.0], k);
    }
    assert_eq!(rig.trauma, 0.0, "1.0 at 2/s is gone after 0.5 s, ±1 tick");
}

#[test]
fn add_trauma_clamps_and_ignores_non_finite() {
    let mut rig = CameraRig::default();
    add_trauma(&mut rig, 0.7);
    add_trauma(&mut rig, 0.7);
    assert_eq!(rig.trauma, 1.0);
    add_trauma(&mut rig, f32::NAN);
    add_trauma(&mut rig, f32::INFINITY);
    assert_eq!(rig.trauma, 1.0);
    add_trauma(&mut rig, -5.0);
    assert_eq!(rig.trauma, 0.0);
}

// ── hold ─────────────────────────────────────────────────────────────────────────────────────────

/// Drives a rig through a mid-blend, mid-shake window; `held` ticks of frozen time sit between
/// gameplay tick 12 and 13.
fn run(held: u64) -> CameraRig {
    let path = straight();
    let mut rig = rig_with(blended_cfg([0.0, 2.2, 4.5], 1.0));
    rig.trauma = 0.9;
    let target = |k: u64| [0.0, 0.0, -10.0 - k as f32 * 0.1];
    for k in 0..12 {
        tick(&mut rig, &path, target(k), k);
    }
    rig.rig = blended_cfg([6.0, 1.5, 0.0], 1.0);
    tick(&mut rig, &path, target(12), 12);
    let frozen = serde_json::to_value(&rig).unwrap();
    for _ in 0..held {
        let input = RigInput {
            target: Some(target(12)),
            path: Some(&path),
            game_dt: 0.0,
            dt: DT,
            game_frame: FrameIndex::new(12),
            camera_fov_y_deg: None,
        };
        step_rig(&mut rig, &input).unwrap();
        assert_eq!(
            serde_json::to_value(&rig).unwrap(),
            frozen,
            "a held tick changes no state"
        );
    }
    for k in 13..40 {
        tick(&mut rig, &path, target(k), k);
    }
    rig
}

#[test]
fn a_held_clock_freezes_the_rig_and_the_run_resumes_identically() {
    assert_eq!(
        serde_json::to_value(run(9)).unwrap(),
        serde_json::to_value(run(0)).unwrap()
    );
}

// ── camera_relative ──────────────────────────────────────────────────────────────────────────────

#[test]
fn camera_relative_maps_the_stick_through_the_yaw() {
    let cases = [
        (0.0, [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        (90.0, [0.0, 0.0, -1.0], [-1.0, 0.0, 0.0]),
        (180.0, [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
    ];
    for (yaw, right, forward) in cases {
        assert!(
            near(camera_relative_yaw([1.0, 0.0], yaw), right, 1e-5),
            "right at {yaw}"
        );
        assert!(
            near(camera_relative_yaw([0.0, 1.0], yaw), forward, 1e-5),
            "forward at {yaw}"
        );
        let mapped = camera_relative_yaw([0.6, 0.8], yaw);
        let length = (mapped[0] * mapped[0] + mapped[2] * mapped[2]).sqrt();
        assert!((length - 1.0).abs() < 1e-5, "magnitude at {yaw}");
        assert_eq!(mapped[1], 0.0);
    }
    assert_eq!(camera_relative_yaw([0.0, 0.0], 37.0), [0.0; 3]);
}

#[test]
fn camera_relative_reads_the_rigs_posed_yaw_before_shake() {
    let path = straight();
    let mut rig = rig_with(rigid([0.0, 2.0, 4.0]));
    assert_eq!(
        camera_relative([0.0, 1.0], &rig),
        [0.0, 0.0, -1.0],
        "no pose yet is yaw 0"
    );
    rig.trauma = 1.0;
    tick(&mut rig, &path, [0.0, 0.0, -10.0], 0);
    let yaw = rig.yaw_deg();
    assert!(yaw.abs() < 1e-3, "looking down the path: {yaw}");
    assert_eq!(
        camera_relative([0.0, 1.0], &rig),
        camera_relative_yaw([0.0, 1.0], yaw)
    );
}

// ── degenerate and absent inputs ─────────────────────────────────────────────────────────────────

#[test]
fn a_coincident_focus_and_eye_keep_the_last_rotation() {
    let path = straight();
    let mut rig = rig_with(rigid([0.0, 2.0, 4.0]));
    tick(&mut rig, &path, [0.0, 0.0, -10.0], 0);
    let (yaw, pitch) = (rig.state.yaw_deg, rig.state.pitch_deg);
    assert!(pitch > 1.0, "the first look is downward: {pitch}");

    rig.rig.mode = RigMode::Track(TrackRig {
        offset: [0.0; 3],
        ..TrackRig::default()
    });
    rig.rig.blend_in.seconds = 0.0;
    let out = tick(&mut rig, &path, [0.0, 0.0, -10.0], 1);
    assert!(
        out.translation
            .iter()
            .chain(&out.rotation_deg)
            .all(|v| v.is_finite())
    );
    assert_eq!((rig.state.yaw_deg, rig.state.pitch_deg), (yaw, pitch));
}

#[test]
fn no_target_or_no_path_returns_none_and_touches_nothing() {
    let path = straight();
    let mut rig = rig_with(RigConfig::default());
    tick(&mut rig, &path, [0.0, 0.0, -10.0], 0);
    let before = rig.clone();
    let mut input = RigInput {
        target: None,
        path: Some(&path),
        game_dt: DT,
        dt: DT,
        game_frame: FrameIndex::new(1),
        camera_fov_y_deg: None,
    };
    assert_eq!(step_rig(&mut rig, &input), None);
    input.target = Some([0.0; 3]);
    input.path = None;
    assert_eq!(step_rig(&mut rig, &input), None);
    assert_eq!(rig, before);
}

// ── serde ────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn an_empty_document_is_the_defaults_and_the_spec_example_round_trips() {
    let empty: CameraRig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(empty, CameraRig::default());

    let doc = serde_json::json!({
        "rig": {
            "mode": { "track": { "path": "paths/behind-1.path.yaml", "offset": [0.0, 2.2, 4.5], "search_window": 3.0 } },
            "focus_offset": [0.0, 0.5, 0.0],
            "springs": { "position_half_life": 0.15, "rotation_half_life": 0.10 },
            "look_ahead": { "distance": 1.0, "full_speed": 6.0, "half_life": 0.3 },
            "vertical_anchor": { "up_slack": 1.5, "down_slack": 1.0, "half_life": 0.25 },
            "fov_y_deg": 55.0,
            "blend_in": { "seconds": 1.0, "curve": "ease_in_out" }
        },
        "shake": {
            "max_offset": [0.15, 0.15, 0.0],
            "max_angle_deg": [1.5, 1.5, 2.0],
            "frequency": 18.0,
            "decay_per_second": 1.2,
            "exponent": 2.0,
            "seed": 1
        },
        "trauma": 0.0,
        "grounded": true
    });
    let rig: CameraRig = serde_json::from_value(doc).unwrap();
    assert_eq!(rig.rig.fov_y_deg, Some(55.0));
    assert_eq!(rig.rig.blend_in.curve, BlendCurve::EaseInOut);
    let RigMode::Track(track) = &rig.rig.mode;
    assert_eq!(track.path, "paths/behind-1.path.yaml");
    let again: CameraRig = serde_json::from_value(serde_json::to_value(&rig).unwrap()).unwrap();
    assert_eq!(again, rig);
}

#[test]
fn a_restored_rig_resumes_bit_exact_mid_blend() {
    let path = straight();
    let target = |k: u64| [0.0, 0.0, -10.0 - k as f32 * 0.1];
    let mut rig = rig_with(blended_cfg([0.0, 2.2, 4.5], 1.0));
    rig.trauma = 0.6;
    for k in 0..10 {
        tick(&mut rig, &path, target(k), k);
    }
    rig.rig = blended_cfg([6.0, 1.5, 0.0], 1.0);
    for k in 10..25 {
        tick(&mut rig, &path, target(k), k);
    }
    let mut restored: CameraRig =
        serde_json::from_value(serde_json::to_value(&rig).unwrap()).unwrap();
    assert_eq!(restored, rig);
    for k in 25..60 {
        let a = tick(&mut rig, &path, target(k), k);
        let b = tick(&mut restored, &path, target(k), k);
        assert_eq!(a, b, "tick {k}");
    }
}
