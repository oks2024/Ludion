//! The rig pipeline, tick by tick, on hand-built paths.

use ludion_camera::{
    Blend, BlendCurve, BlendIn, CameraRig, FirstPersonRig, KickConfig, LookAhead, RigConfig,
    RigInput, RigMode, RigOutput, Springs, TrackRig, add_fov_kick, add_kick, add_look, add_trauma,
    camera_relative, camera_relative_yaw, set_look, snap_rig, step_rig,
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
fn an_unusable_blend_snaps_instead_of_freezing() {
    let path = straight();
    let target = [0.0, 1.0, -20.0];
    type Corrupt = fn(&mut Blend);
    let corruptions: [(&str, Corrupt); 4] = [
        ("negative seconds", |b| b.seconds = -1.0),
        ("zero seconds", |b| b.seconds = 0.0),
        ("NaN seconds", |b| b.seconds = f32::NAN),
        ("infinite from.eye", |b| b.from.eye[0] = f32::INFINITY),
    ];
    for (name, corrupt) in corruptions {
        let mut rig = rig_with(blended_cfg([0.0, 2.2, 4.5], 1.0));
        tick(&mut rig, &path, target, 0);
        rig.rig = blended_cfg([6.0, 1.5, 0.0], 1.0);
        for frame in 1..4 {
            tick(&mut rig, &path, target, frame);
        }
        corrupt(rig.state.blend.as_mut().expect("a blend is running"));
        let out = tick(&mut rig, &path, target, 4);
        assert!(rig.state.blend.is_none(), "{name}: the blend is dropped");
        assert!(
            near(out.translation, [6.0, 1.5, -20.0], 1e-3),
            "{name}: {out:?}"
        );
    }
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
    let RigMode::Track(track) = &rig.rig.mode else {
        panic!("track mode")
    };
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

// ── first person ─────────────────────────────────────────────────────────────────────────────────

fn fp_cfg(blend_seconds: f32) -> RigConfig {
    RigConfig {
        mode: RigMode::FirstPerson(FirstPersonRig::default()),
        blend_in: BlendIn {
            seconds: blend_seconds,
            curve: BlendCurve::Linear,
        },
        ..RigConfig::default()
    }
}

fn fp_tick_dt(rig: &mut CameraRig, target: [f32; 3], frame: u64, game_dt: f32) -> RigOutput {
    let input = RigInput {
        target: Some(target),
        path: None,
        game_dt,
        dt: DT,
        game_frame: FrameIndex::new(frame),
        camera_fov_y_deg: None,
    };
    step_rig(rig, &input).expect("target present")
}

fn fp_tick(rig: &mut CameraRig, target: [f32; 3], frame: u64) -> RigOutput {
    fp_tick_dt(rig, target, frame, DT)
}

fn fp_rig() -> CameraRig {
    rig_with(fp_cfg(0.0))
}

#[test]
fn first_person_pitch_clamps_and_yaw_wraps() {
    let mut rig = fp_rig();
    add_look(&mut rig, 0.0, 500.0);
    fp_tick(&mut rig, [0.0; 3], 0);
    assert_eq!(rig.state.pitch_deg, 89.0);
    add_look(&mut rig, 0.0, -1000.0);
    fp_tick(&mut rig, [0.0; 3], 1);
    assert_eq!(rig.state.pitch_deg, -89.0);
    set_look(&mut rig, 170.0, 0.0);
    add_look(&mut rig, 20.0, 0.0);
    fp_tick(&mut rig, [0.0; 3], 2);
    assert!((rig.state.yaw_deg + 170.0).abs() < 1e-4);
}

#[test]
fn first_person_look_is_consumed_and_empty_look_is_not_serialized() {
    let mut rig = fp_rig();
    assert!(
        serde_json::to_value(&rig)
            .unwrap()
            .get("look_deg")
            .is_none()
    );
    add_look(&mut rig, 5.0, 3.0);
    add_look(&mut rig, f32::NAN, f32::INFINITY);
    assert_eq!(rig.look_deg, [5.0, 3.0]);
    assert!(
        serde_json::to_value(&rig)
            .unwrap()
            .get("look_deg")
            .is_some()
    );
    fp_tick(&mut rig, [0.0; 3], 0);
    assert_eq!(rig.look_deg, [0.0; 2]);
    assert!(
        serde_json::to_value(&rig)
            .unwrap()
            .get("look_deg")
            .is_none()
    );
}

#[test]
fn first_person_eye_follows_the_target_in_the_yaw_frame() {
    let mut rig = rig_with(RigConfig {
        mode: RigMode::FirstPerson(FirstPersonRig {
            eye_offset: [1.0, 1.6, 0.0],
            ..FirstPersonRig::default()
        }),
        ..fp_cfg(0.0)
    });
    set_look(&mut rig, 90.0, 20.0);
    let out = fp_tick(&mut rig, [3.0, 0.0, -2.0], 0);
    // Yaw 90: the right vector points to -Z; pitch does not tilt the offset.
    assert!(
        near(out.translation, [3.0, 1.6, -3.0], 1e-5),
        "{:?}",
        out.translation
    );
    let out = fp_tick(&mut rig, [4.0, 1.0, -2.0], 1);
    assert!(near(out.translation, [4.0, 2.6, -3.0], 1e-5));
}

#[test]
fn first_person_runs_are_bit_identical_and_aim_matches_the_pose() {
    let run = || {
        let mut rig = fp_rig();
        let mut outs = Vec::new();
        for k in 0..300u64 {
            let kf = k as f32;
            add_look(
                &mut rig,
                ludion_core::math::sin(kf * 0.37) * 4.0,
                ludion_core::math::cos(kf * 0.11) * 2.5,
            );
            let out = fp_tick(&mut rig, [kf * 0.01, 0.0, -kf * 0.02], k);
            let aim = rig.aim().expect("posed");
            assert_eq!(aim.rotation_deg(), out.rotation_deg, "tick {k}");
            outs.push(out);
        }
        outs
    };
    let (a, b) = (run(), run());
    for (x, y) in a.iter().zip(&b) {
        let bits = |o: &RigOutput| {
            o.translation
                .iter()
                .chain(&o.rotation_deg)
                .map(|v| v.to_bits())
                .collect::<Vec<_>>()
        };
        assert_eq!(bits(x), bits(y));
    }
}

#[test]
fn aim_forward_is_unit_and_matches_yaw_and_pitch() {
    let mut rig = fp_rig();
    set_look(&mut rig, 90.0, 0.0);
    fp_tick(&mut rig, [0.0; 3], 0);
    let f = rig.aim().unwrap().forward();
    assert!(near(f, [-1.0, 0.0, 0.0], 1e-5), "{f:?}");
    set_look(&mut rig, 0.0, 90.0 - 1e-3);
    fp_tick(&mut rig, [0.0; 3], 1);
    assert!(rig.aim().unwrap().forward()[1] < -0.99);
}

#[test]
fn switching_from_track_blends_and_keeps_the_yaw() {
    let path = straight();
    let mut rig = rig_with(rigid([0.0, 2.2, 4.5]));
    for k in 0..5 {
        tick(&mut rig, &path, [0.0, 0.0, -10.0], k);
    }
    let yaw = rig.state.yaw_deg;
    rig.rig = fp_cfg(1.0);
    let out = fp_tick(&mut rig, [0.0, 0.0, -10.0], 5);
    assert_eq!(rig.state.yaw_deg, yaw);
    assert!(rig.state.blend.is_some());
    let first_eye = [0.0, 1.6, -10.0];
    assert!(
        dist(out.translation, first_eye) > 0.5,
        "starts at the old pose"
    );
    for k in 6..70 {
        fp_tick(&mut rig, [0.0, 0.0, -10.0], k);
    }
    let out = fp_tick(&mut rig, [0.0, 0.0, -10.0], 70);
    assert!(rig.state.blend.is_none());
    assert!(near(out.translation, first_eye, 1e-5));
}

#[test]
fn snap_keeps_the_look() {
    let mut rig = fp_rig();
    set_look(&mut rig, 40.0, 10.0);
    fp_tick(&mut rig, [0.0; 3], 0);
    snap_rig(&mut rig);
    fp_tick(&mut rig, [5.0, 0.0, 0.0], 1);
    assert_eq!((rig.state.yaw_deg, rig.state.pitch_deg), (40.0, 10.0));
    assert!(near(rig.state.eye, [5.0, 1.6, 0.0], 1e-5));
}

#[test]
fn a_held_clock_still_looks_and_freezes_trauma() {
    let mut rig = fp_rig();
    rig.trauma = 0.5;
    fp_tick(&mut rig, [0.0; 3], 0);
    let shake_time = rig.state.shake_time;
    add_look(&mut rig, 10.0, 5.0);
    let out = fp_tick_dt(&mut rig, [2.0, 0.0, 0.0], 0, 0.0);
    assert_eq!((rig.state.yaw_deg, rig.state.pitch_deg), (10.0, 5.0));
    assert_eq!(rig.trauma, 0.5);
    assert_eq!(rig.state.shake_time, shake_time);
    assert!(
        (out.translation[1] - 1.6).abs() < 0.3,
        "eye rides the target while held"
    );
    assert_eq!(rig.aim().unwrap().eye[0], 2.0);
}

#[test]
fn camera_relative_follows_the_look_yaw() {
    let mut rig = fp_rig();
    set_look(&mut rig, 90.0, 0.0);
    fp_tick(&mut rig, [0.0; 3], 0);
    let forward = camera_relative([0.0, 1.0], &rig);
    assert!(near(forward, [-1.0, 0.0, 0.0], 1e-5), "{forward:?}");
}

#[test]
fn shake_never_moves_the_aim() {
    let mut shaken = fp_rig();
    shaken.trauma = 1.0;
    let mut calm = fp_rig();
    let mut moved = false;
    for k in 0..30u64 {
        for rig in [&mut shaken, &mut calm] {
            add_look(rig, 1.0, 0.5);
        }
        let a = fp_tick(&mut shaken, [0.0; 3], k);
        let b = fp_tick(&mut calm, [0.0; 3], k);
        moved |= a.rotation_deg != b.rotation_deg;
        assert_eq!(shaken.aim(), calm.aim(), "tick {k}");
    }
    assert!(moved, "the shake did move the output");
}

#[test]
fn first_person_needs_no_path_and_a_document_round_trips() {
    let rig: CameraRig = serde_json::from_value(serde_json::json!({
        "rig": { "mode": { "first_person": { "pitch_limit_deg": 80.0 } } },
        "look_deg": [1.0, 2.0],
    }))
    .unwrap();
    let RigMode::FirstPerson(fp) = &rig.rig.mode else {
        panic!("first_person mode")
    };
    assert_eq!((fp.pitch_limit_deg, fp.eye_offset), (80.0, [0.0, 1.6, 0.0]));
    let again: CameraRig = serde_json::from_value(serde_json::to_value(&rig).unwrap()).unwrap();
    assert_eq!(again, rig);
}

// ── kick and roll ────────────────────────────────────────────────────────────────────────────────

fn bits3(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

#[test]
fn a_pitch_kick_moves_the_output_but_never_the_aim() {
    let mut rig = fp_rig();
    let mut twin = fp_rig();
    add_look(&mut rig, 30.0, 10.0);
    add_look(&mut twin, 30.0, 10.0);
    add_kick(&mut rig, 0.0, -3.0, 0.0);
    let out = fp_tick(&mut rig, [0.0; 3], 0);
    let base = fp_tick(&mut twin, [0.0; 3], 0);
    assert_ne!(out.rotation_deg, base.rotation_deg);
    assert_eq!(rig.aim(), twin.aim());
    assert_eq!(rig.yaw_deg().to_bits(), twin.yaw_deg().to_bits());
    assert_eq!(rig.state.pose, twin.state.pose);
    assert_eq!(
        camera_relative([0.0, 1.0], &rig).map(f32::to_bits),
        camera_relative([0.0, 1.0], &twin).map(f32::to_bits)
    );
}

#[test]
fn a_kick_decays_to_exactly_zero_and_rejoins_its_twin() {
    let mut rig = fp_rig();
    let mut twin = fp_rig();
    add_kick(&mut rig, 1.0, -2.0, 0.5);
    add_fov_kick(&mut rig, 3.0);
    let mut frame = 0;
    while rig.kick_deg != [0.0; 3] || rig.fov_kick_deg != 0.0 {
        fp_tick(&mut rig, [0.0; 3], frame);
        fp_tick(&mut twin, [0.0; 3], frame);
        frame += 1;
        assert!(frame < 600, "a kick rests within a bounded tick count");
    }
    let a = fp_tick(&mut rig, [0.0; 3], frame);
    let b = fp_tick(&mut twin, [0.0; 3], frame);
    assert_eq!(bits3(a.rotation_deg), bits3(b.rotation_deg));
    assert_eq!(rig, twin, "at rest the whole component is the twin's");
}

#[test]
fn a_scripted_look_and_kick_run_is_bit_identical() {
    let run = || {
        let mut rig = fp_rig();
        (0..90)
            .map(|k| {
                add_look(&mut rig, 0.7, -0.2);
                if k % 17 == 0 {
                    add_kick(&mut rig, 0.3, -1.5, 0.4);
                    add_fov_kick(&mut rig, 2.0);
                }
                let out = fp_tick(&mut rig, [0.0, 0.0, -(k as f32)], k);
                (bits3(out.rotation_deg), bits3(out.translation))
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}

#[test]
fn a_held_clock_freezes_the_decay_and_still_shows_a_new_kick() {
    let mut rig = fp_rig();
    add_kick(&mut rig, 0.0, -4.0, 0.0);
    fp_tick(&mut rig, [0.0; 3], 0);
    fp_tick(&mut rig, [0.0; 3], 1);
    let frozen = rig.kick_deg;
    assert!(frozen[1] < 0.0 && frozen[1] > -4.0, "it has decayed some");
    let before = fp_tick_dt(&mut rig, [0.0; 3], 1, 0.0).rotation_deg;
    fp_tick_dt(&mut rig, [0.0; 3], 1, 0.0);
    assert_eq!(bits3(rig.kick_deg), bits3(frozen));
    add_kick(&mut rig, 0.0, -1.0, 0.0);
    let during = fp_tick_dt(&mut rig, [0.0; 3], 1, 0.0).rotation_deg;
    assert_ne!(during, before, "a kick added in a hold shows at once");
    // Resuming decays from the held value exactly as a rig holding that value would.
    let mut twin = rig.clone();
    fp_tick(&mut rig, [0.0; 3], 2);
    fp_tick(&mut twin, [0.0; 3], 2);
    assert_eq!(bits3(rig.kick_deg), bits3(twin.kick_deg));
    assert!(rig.kick_deg[1].abs() < (frozen[1] - 1.0).abs());
}

#[test]
fn positive_roll_raises_the_right_vector_and_persists() {
    let mut rig = fp_rig();
    let right =
        |out: &RigOutput| quat_rotate(euler_xyz_degrees_to_quat(out.rotation_deg), [1.0, 0.0, 0.0]);
    let level = fp_tick(&mut rig, [0.0; 3], 0);
    assert!(right(&level)[1].abs() < 1e-6);
    rig.roll_deg = 10.0;
    for k in 1..5 {
        let out = fp_tick(&mut rig, [0.0; 3], k);
        assert!(right(&out)[1] > 0.1, "tick {k}");
        assert_eq!(rig.roll_deg, 10.0, "the rig never consumes roll");
    }
    let mut twin = fp_rig();
    for k in 0..5 {
        fp_tick(&mut twin, [0.0; 3], k);
    }
    assert_eq!(rig.aim(), twin.aim());
    let mut kicked = fp_rig();
    add_kick(&mut kicked, 0.0, 0.0, 10.0);
    assert!(right(&fp_tick(&mut kicked, [0.0; 3], 0))[1] > 0.1);
}

#[test]
fn fov_channels_add_clamp_and_decay() {
    let cfg = RigConfig {
        fov_y_deg: Some(90.0),
        ..fp_cfg(0.0)
    };
    let mut rig = rig_with(cfg);
    rig.fov_offset_deg = 8.0;
    add_fov_kick(&mut rig, 4.0);
    let first = fp_tick(&mut rig, [0.0; 3], 0).fov_y_deg.unwrap();
    assert_eq!(first, 102.0, "a kick shows on the tick it is added");
    let mut last = first;
    for k in 1..200 {
        last = fp_tick(&mut rig, [0.0; 3], k).fov_y_deg.unwrap();
    }
    assert_eq!(last, 98.0);
    rig.fov_offset_deg = 500.0;
    assert_eq!(fp_tick(&mut rig, [0.0; 3], 200).fov_y_deg, Some(179.0));
    rig.fov_offset_deg = -500.0;
    assert_eq!(fp_tick(&mut rig, [0.0; 3], 201).fov_y_deg, Some(1.0));

    let mut inert = fp_rig();
    inert.fov_offset_deg = 8.0;
    add_fov_kick(&mut inert, 4.0);
    assert_eq!(fp_tick(&mut inert, [0.0; 3], 0).fov_y_deg, None);
}

#[test]
fn a_first_person_kick_clamps_at_the_pitch_limit() {
    let mut rig = fp_rig();
    add_look(&mut rig, 0.0, 85.0);
    add_kick(&mut rig, 0.0, 30.0, 0.0);
    let out = fp_tick(&mut rig, [0.0; 3], 0);
    let mut twin = fp_rig();
    add_look(&mut twin, 0.0, 89.0);
    let limit = fp_tick(&mut twin, [0.0; 3], 0);
    assert_eq!(bits3(out.rotation_deg), bits3(limit.rotation_deg));
}

#[test]
fn a_track_rig_is_bit_identical_at_zero_and_rolls_when_asked() {
    let path = straight();
    let run = |roll: f32| {
        let mut rig = rig_with(blended_cfg([0.0, 2.2, 4.5], 1.0));
        rig.roll_deg = roll;
        (0..30)
            .map(|k| tick(&mut rig, &path, [0.0, 0.0, -10.0 - k as f32 * 0.1], k))
            .collect::<Vec<_>>()
    };
    let plain = run(0.0);
    assert_eq!(plain, run(0.0));
    assert_ne!(run(8.0)[29].rotation_deg, plain[29].rotation_deg);
}

#[test]
fn a_switch_mid_kick_blends_from_the_unkicked_pose() {
    let mut rig = rig_with(fp_cfg(1.0));
    add_kick(&mut rig, 5.0, -5.0, 5.0);
    fp_tick(&mut rig, [0.0; 3], 0);
    fp_tick(&mut rig, [0.0; 3], 1);
    rig.rig = RigConfig {
        mode: RigMode::FirstPerson(FirstPersonRig {
            eye_offset: [0.0, 1.0, 0.0],
            ..FirstPersonRig::default()
        }),
        ..fp_cfg(1.0)
    };
    fp_tick(&mut rig, [0.0; 3], 2);
    let blend = rig.state.blend.as_ref().expect("a blend started");
    assert_eq!(blend.from.yaw_deg, 0.0);
    assert_eq!(blend.from.pitch_deg, 0.0);
    assert_ne!(rig.kick_deg, [0.0; 3], "a switch leaves the kick alone");
}

#[test]
fn snap_keeps_the_kick() {
    let mut rig = fp_rig();
    add_kick(&mut rig, 1.0, 2.0, 3.0);
    add_fov_kick(&mut rig, 4.0);
    fp_tick(&mut rig, [0.0; 3], 0);
    let kept = (rig.kick_deg, rig.fov_kick_deg);
    snap_rig(&mut rig);
    fp_tick_dt(&mut rig, [0.0; 3], 1, 0.0);
    assert_eq!((rig.kick_deg, rig.fov_kick_deg), kept);
}

#[test]
fn the_new_channels_serialize_only_when_off_rest() {
    let keys = [
        "kick",
        "kick_deg",
        "fov_kick_deg",
        "roll_deg",
        "fov_offset_deg",
    ];
    let at_rest = serde_json::to_value(CameraRig::default()).unwrap();
    for key in keys {
        assert!(at_rest.get(key).is_none(), "{key}");
    }
    let mut rig = CameraRig {
        roll_deg: 3.0,
        fov_offset_deg: 8.0,
        kick: KickConfig {
            half_life: 0.2,
            fov_half_life: 0.3,
        },
        ..CameraRig::default()
    };
    add_kick(&mut rig, 1.0, -2.0, 0.5);
    add_fov_kick(&mut rig, 4.0);
    let value = serde_json::to_value(&rig).unwrap();
    for key in keys {
        assert!(value.get(key).is_some(), "{key}");
    }
    let back: CameraRig = serde_json::from_value(value).unwrap();
    assert_eq!(back, rig);
    add_kick(&mut rig, f32::NAN, f32::INFINITY, 0.0);
    add_fov_kick(&mut rig, f32::NAN);
    assert_eq!((rig.kick_deg, rig.fov_kick_deg), ([1.0, -2.0, 0.5], 4.0));
}
