//! The rig pipeline, tick by tick, on hand-built paths.

use ludion_camera::{
    Blend, BlendCurve, BlendIn, CameraRig, ChaseRig, FirstPersonRig, FollowRig, Framing,
    KickConfig, LookAhead, Recenter, RigCollision, RigCollisionConfig, RigConfig, RigInput,
    RigMode, RigOutput, Springs, TrackRig, add_fov_kick, add_kick, add_look, add_look_counts,
    add_trauma, camera_relative, camera_relative_yaw, look_from_counts, orbit_eye, resolved_camera,
    set_look, snap_rig, step_rig,
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
        collision: None,
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
            collision: None,
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
        collision: None,
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
        collision: None,
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

fn out_bits(o: &RigOutput) -> Vec<u32> {
    o.translation
        .iter()
        .chain(&o.rotation_deg)
        .chain(&o.fov_y_deg)
        .map(|v| v.to_bits())
        .collect()
}

#[test]
fn resolved_camera_with_no_extra_look_is_the_ticks_camera_bit_for_bit() {
    let mut rig = fp_rig();
    rig.rig.fov_y_deg = Some(70.0);
    rig.trauma = 0.6;
    for k in 0..40u64 {
        let kf = k as f32;
        add_look(&mut rig, ludion_core::math::sin(kf * 0.3) * 3.0, 0.7);
        add_kick(&mut rig, 0.2, -1.0, 0.5);
        add_fov_kick(&mut rig, 1.5);
        let target = [kf * 0.05, 0.0, -kf * 0.03];
        let out = fp_tick(&mut rig, target, k);
        let resolved = resolved_camera(&rig, target, [0.0; 2]).unwrap();
        assert_eq!(out_bits(&resolved.view), out_bits(&out), "tick {k}");
        let aim = rig.aim().unwrap();
        assert_eq!(resolved.aim, aim);
    }
    let path = straight();
    let mut track = rig_with(rigid([0.0, 2.2, 4.5]));
    track.trauma = 0.5;
    for k in 0..20 {
        let target = [0.0, 0.0, -10.0 - k as f32];
        let out = tick(&mut track, &path, target, k);
        let resolved = resolved_camera(&track, target, [0.0; 2]).unwrap();
        assert_eq!(out_bits(&resolved.view), out_bits(&out), "track tick {k}");
    }
}

#[test]
fn resolved_camera_is_none_before_the_first_pose() {
    assert!(resolved_camera(&fp_rig(), [0.0; 3], [0.0; 2]).is_none());
}

#[test]
fn extra_look_equals_the_look_applied_on_a_held_tick() {
    let mut rig = fp_rig();
    rig.trauma = 0.4;
    add_kick(&mut rig, 0.3, -2.0, 1.0);
    let target = [1.0, 0.0, -2.0];
    for k in 0..10u64 {
        add_look(&mut rig, 1.5, 0.5);
        fp_tick(&mut rig, target, k);
    }
    for extra in [[10.0, 500.0], [-30.0, -20.0], [200.0, 0.0]] {
        let latched = resolved_camera(&rig, target, extra).unwrap();
        let mut reference = rig.clone();
        add_look(&mut reference, extra[0], extra[1]);
        let out = fp_tick_dt(&mut reference, target, 10, 0.0);
        assert_eq!(out_bits(&latched.view), out_bits(&out), "{extra:?}");
        assert_eq!(Some(latched.aim), reference.aim());
    }
}

#[test]
fn extra_look_is_ignored_on_track_rigs_and_during_a_blend() {
    let path = straight();
    let mut track = rig_with(rigid([0.0, 2.2, 4.5]));
    tick(&mut track, &path, [0.0, 0.0, -10.0], 0);
    let base = resolved_camera(&track, [0.0, 0.0, -10.0], [0.0; 2]).unwrap();
    let looked = resolved_camera(&track, [0.0, 0.0, -10.0], [30.0, 10.0]).unwrap();
    assert_eq!(base, looked);

    let mut rig = fp_rig();
    fp_tick(&mut rig, [0.0; 3], 0);
    rig.rig.fov_y_deg = Some(50.0);
    fp_tick(&mut rig, [0.0; 3], 1);
    assert!(rig.state.blend.is_none());
    rig.rig = fp_cfg(1.0);
    rig.rig.fov_y_deg = Some(60.0);
    fp_tick(&mut rig, [0.0; 3], 2);
    assert!(rig.state.blend.is_some());
    let base = resolved_camera(&rig, [0.0; 3], [0.0; 2]).unwrap();
    let looked = resolved_camera(&rig, [0.0; 3], [30.0, 10.0]).unwrap();
    assert_eq!(base, looked);
}

#[test]
fn counts_convert_through_the_rigs_one_sensitivity() {
    let mut rig = fp_rig();
    assert_eq!(CameraRig::DEFAULT_LOOK_DEG_PER_COUNT, 0.1);
    rig.look_deg_per_count = 0.25;
    add_look_counts(&mut rig, 8.0, 4.0);
    assert_eq!(rig.look_deg, [-2.0, 1.0]);
    assert_eq!(look_from_counts(&rig, 8.0, 4.0), [-2.0, 1.0]);
    add_look_counts(&mut rig, f32::NAN, 4.0);
    assert_eq!(rig.look_deg, [-2.0, 2.0]);
}

#[test]
fn sensitivity_is_serialized_only_off_the_default() {
    let mut rig = fp_rig();
    assert!(
        serde_json::to_value(&rig)
            .unwrap()
            .get("look_deg_per_count")
            .is_none()
    );
    rig.look_deg_per_count = 0.05;
    let value = serde_json::to_value(&rig).unwrap();
    assert_eq!(value["look_deg_per_count"], 0.05f32);
    let back: CameraRig = serde_json::from_value(value).unwrap();
    assert_eq!(back.look_deg_per_count, 0.05);
    rig.look_deg_per_count = f32::NAN;
    assert_eq!(
        ludion_camera::invalid_field(&rig)
            .map(|(p, _)| p)
            .as_deref(),
        Some("/look_deg_per_count")
    );
}

// ── chase and follow ─────────────────────────────────────────────────────────────────────────────

fn boom_cfg(mode: RigMode, blend_seconds: f32) -> RigConfig {
    RigConfig {
        mode,
        blend_in: BlendIn {
            seconds: blend_seconds,
            curve: BlendCurve::Linear,
        },
        look_ahead: LookAhead {
            distance: 0.0,
            ..LookAhead::default()
        },
        ..RigConfig::default()
    }
}

fn chase_rig(chase: ChaseRig) -> CameraRig {
    rig_with(boom_cfg(RigMode::Chase(chase), 0.0))
}

fn follow_rig() -> CameraRig {
    rig_with(boom_cfg(RigMode::Follow(FollowRig::default()), 0.0))
}

#[test]
fn chase_pivot_springs_to_the_orbit_eye_at_rest() {
    let mut rig = chase_rig(ChaseRig::default());
    fp_tick(&mut rig, [0.0; 3], 0);
    assert_eq!(rig.state.pivot, Some([0.0; 3]));
    let target = [3.0, 0.0, -5.0];
    let mut out = fp_tick(&mut rig, target, 1);
    // One tick behind the target: the spring, not the boom, lags.
    assert!(dist(out.translation, orbit_eye(target, 0.0, 20.0, 6.0)) > 0.5);
    for k in 2..400 {
        out = fp_tick(&mut rig, target, k);
    }
    let want = orbit_eye(target, 0.0, 20.0, 6.0);
    assert!(
        near(out.translation, want, 1e-3),
        "{:?} {want:?}",
        out.translation
    );
    let aim = rig.aim().unwrap();
    assert_eq!((aim.yaw_deg, aim.pitch_deg), (0.0, 20.0));
}

#[test]
fn chase_yaw_is_continuous_across_the_seam_under_steady_look() {
    let mut rig = chase_rig(ChaseRig::default());
    set_look(&mut rig, 170.0, 20.0);
    let mut last = fp_tick(&mut rig, [0.0; 3], 0).translation;
    let mut crossed = false;
    for k in 1..40 {
        add_look(&mut rig, 3.0, 0.0);
        let eye = fp_tick(&mut rig, [0.0; 3], k).translation;
        // A 3 degree turn moves a 6 m boom at most ~0.32 m.
        assert!(dist(eye, last) < 0.4, "tick {k}: {last:?} -> {eye:?}");
        crossed |= rig.state.yaw_deg < 0.0;
        last = eye;
    }
    assert!(crossed, "the yaw wrapped through the seam");
}

#[test]
fn equal_pitch_limits_fix_the_pitch_against_look() {
    let mut rig = chase_rig(ChaseRig {
        pitch_min_deg: 35.0,
        pitch_max_deg: 35.0,
        pitch_deg: 35.0,
        ..ChaseRig::default()
    });
    for k in 0..5 {
        add_look(&mut rig, 1.0, 20.0 * (k as f32 - 2.0));
        fp_tick(&mut rig, [0.0; 3], k);
        assert_eq!(rig.state.pitch_deg, 35.0);
    }
    assert!((rig.state.yaw_deg - 5.0).abs() < 1e-4, "yaw still turns");
}

#[test]
fn follow_sits_exactly_on_its_boom_and_consumes_look() {
    let mut rig = follow_rig();
    let target = [3.0, 1.0, -5.0];
    let out = fp_tick(&mut rig, target, 0);
    assert_eq!(rig.state.eye, orbit_eye(target, 0.0, 45.0, 10.0));
    assert_eq!(out.translation, orbit_eye(target, 0.0, 45.0, 10.0));
    add_look(&mut rig, 30.0, 10.0);
    fp_tick(&mut rig, target, 1);
    assert_eq!(rig.look_deg, [0.0; 2], "look is consumed");
    assert_eq!(rig.state.eye, orbit_eye(target, 0.0, 45.0, 10.0));
    assert_eq!(rig.aim().unwrap().yaw_deg, 0.0);
}

#[test]
fn chase_to_first_person_keeps_the_yaw_and_enters_at_the_pitch() {
    let mut rig = chase_rig(ChaseRig::default());
    set_look(&mut rig, 70.0, 20.0);
    fp_tick(&mut rig, [0.0; 3], 0);
    rig.rig = RigConfig {
        mode: RigMode::FirstPerson(FirstPersonRig {
            enter_pitch_deg: Some(-5.0),
            ..FirstPersonRig::default()
        }),
        ..fp_cfg(0.0)
    };
    fp_tick(&mut rig, [0.0; 3], 1);
    assert_eq!(rig.state.yaw_deg, 70.0);
    assert_eq!(rig.state.pitch_deg, -5.0);
    assert_eq!(rig.state.pivot, None);
    // Without `enter_pitch_deg` the pitch is carried.
    let mut rig = chase_rig(ChaseRig::default());
    fp_tick(&mut rig, [0.0; 3], 0);
    rig.rig = fp_cfg(0.0);
    fp_tick(&mut rig, [0.0; 3], 1);
    assert_eq!(rig.state.pitch_deg, 20.0);
}

#[test]
fn first_person_to_chase_keeps_the_yaw_and_resets_the_pitch() {
    let mut rig = fp_rig();
    set_look(&mut rig, -120.0, 60.0);
    fp_tick(&mut rig, [0.0; 3], 0);
    rig.rig = boom_cfg(RigMode::Chase(ChaseRig::default()), 0.0);
    fp_tick(&mut rig, [0.0; 3], 1);
    assert_eq!(rig.state.yaw_deg, -120.0);
    assert_eq!(rig.state.pitch_deg, 20.0);
    // An authored yaw wins over the carried one.
    rig.rig = boom_cfg(
        RigMode::Chase(ChaseRig {
            yaw_deg: Some(30.0),
            ..ChaseRig::default()
        }),
        0.0,
    );
    fp_tick(&mut rig, [0.0; 3], 2);
    assert_eq!(rig.state.yaw_deg, 30.0);
}

#[test]
fn a_held_tick_applies_chase_look_with_a_frozen_pivot() {
    let mut rig = chase_rig(ChaseRig::default());
    fp_tick(&mut rig, [0.0; 3], 0);
    fp_tick(&mut rig, [4.0, 0.0, 0.0], 1);
    let pivot = rig.state.pivot;
    add_look(&mut rig, 12.0, 0.0);
    let out = fp_tick_dt(&mut rig, [4.0, 0.0, 0.0], 1, 0.0);
    assert_eq!(rig.state.pivot, pivot);
    assert_eq!(rig.state.yaw_deg, 12.0);
    assert_eq!(out.translation, orbit_eye(pivot.unwrap(), 12.0, 20.0, 6.0));
}

#[test]
fn resolved_camera_on_chase_equals_look_then_a_held_tick() {
    let mut rig = chase_rig(ChaseRig::default());
    for k in 0..30 {
        fp_tick(&mut rig, [k as f32 * 0.1, 0.0, -2.0], k);
    }
    let target = [2.9, 0.0, -2.0];
    let latched = resolved_camera(&rig, target, [25.0, -7.0]).unwrap();
    let mut held = rig.clone();
    add_look(&mut held, 25.0, -7.0);
    let out = fp_tick_dt(&mut held, target, 30, 0.0);
    assert_eq!(latched.view, out);
    assert_eq!(latched.aim, held.aim().unwrap());
    assert_eq!(
        resolved_camera(&rig, target, [0.0; 2]).unwrap().view,
        fp_tick_dt(&mut rig.clone(), target, 30, 0.0)
    );
}

#[test]
fn resolved_camera_ignores_the_look_mid_blend_and_on_follow() {
    let mut rig = fp_rig();
    fp_tick(&mut rig, [0.0; 3], 0);
    rig.rig = boom_cfg(RigMode::Chase(ChaseRig::default()), 1.0);
    fp_tick(&mut rig, [0.0; 3], 1);
    assert!(rig.state.blend.is_some());
    let base = resolved_camera(&rig, [0.0; 3], [0.0; 2]).unwrap();
    assert_eq!(base, resolved_camera(&rig, [0.0; 3], [30.0, 10.0]).unwrap());
    let mut rig = follow_rig();
    fp_tick(&mut rig, [0.0; 3], 0);
    let base = resolved_camera(&rig, [0.0; 3], [0.0; 2]).unwrap();
    assert_eq!(base, resolved_camera(&rig, [0.0; 3], [30.0, 10.0]).unwrap());
}

#[test]
fn the_boom_modes_serialize_and_default_compactly() {
    let value = serde_json::to_value(FirstPersonRig::default()).unwrap();
    assert!(value.get("enter_pitch_deg").is_none());
    let back: FirstPersonRig = serde_json::from_value(value).unwrap();
    assert_eq!(back, FirstPersonRig::default());
    let rig: CameraRig =
        serde_json::from_value(serde_json::json!({"rig": {"mode": {"chase": {"distance": 4.0}}}}))
            .unwrap();
    let RigMode::Chase(chase) = &rig.rig.mode else {
        panic!("chase");
    };
    assert_eq!(
        (chase.distance, chase.pitch_deg, chase.yaw_deg),
        (4.0, 20.0, None)
    );
    let value = serde_json::to_value(fp_rig()).unwrap();
    assert!(value["state"].get("pivot").is_none());
    assert!(value["state"].get("pivot_velocity").is_none());
}

// ── collision, framing and recenter ──────────────────────────────────────────────────────────────

/// An infinite slab of solid between `z0` and `z1`: analytic sphere casts along any direction.
#[derive(Debug)]
struct Slab {
    z0: f32,
    z1: f32,
}

impl RigCollision for Slab {
    fn cast(&self, from: [f32; 3], dir: [f32; 3], max: f32, radius: f32) -> Option<f32> {
        let (lo, hi) = (self.z0 - radius, self.z1 + radius);
        if from[2] > lo && from[2] < hi {
            return Some(0.0);
        }
        let entry = if dir[2] > 0.0 && from[2] <= lo {
            lo
        } else if dir[2] < 0.0 && from[2] >= hi {
            hi
        } else {
            return None;
        };
        let d = (entry - from[2]) / dir[2];
        (d <= max).then_some(d)
    }
}

fn collide_cfg(collision: RigCollisionConfig) -> RigConfig {
    RigConfig {
        collision,
        ..boom_cfg(RigMode::Chase(ChaseRig::default()), 0.0)
    }
}

fn on() -> RigCollisionConfig {
    RigCollisionConfig {
        enabled: true,
        ..RigCollisionConfig::default()
    }
}

fn tick_with(
    rig: &mut CameraRig,
    target: [f32; 3],
    frame: u64,
    game_dt: f32,
    collision: Option<&dyn RigCollision>,
) -> RigOutput {
    let input = RigInput {
        target: Some(target),
        path: None,
        game_dt,
        dt: DT,
        game_frame: FrameIndex::new(frame),
        camera_fov_y_deg: None,
        collision,
    };
    step_rig(rig, &input).expect("target present")
}

fn full_boom() -> f32 {
    6.0
}

#[test]
fn a_hit_pulls_the_boom_in_on_the_same_tick() {
    let wall = Slab { z0: 3.0, z1: 50.0 };
    let mut rig = rig_with(collide_cfg(on()));
    let out = tick_with(&mut rig, [0.0; 3], 0, DT, Some(&wall));
    let boom = rig.state.boom.expect("pulled in");
    assert!(boom < full_boom() - 2.0, "{boom}");
    assert!((dist(out.translation, [0.0; 3]) - boom).abs() < 1e-4);
    // The eye sphere stays clear of the wall.
    assert!(out.translation[2] + 0.3 <= 3.0 + 1e-4);
    assert_eq!(rig.state.boom_hold, 0.15);
    assert!(!out.stuck);
    // Aim reports the collided eye; the uncollided one stays in the state.
    assert_eq!(rig.aim().unwrap().eye, out.translation);
    assert!(rig.state.eye[2] > 3.0);
}

#[test]
fn the_boom_holds_then_eases_back_and_a_held_clock_freezes_the_hold() {
    let wall = Slab { z0: 3.0, z1: 50.0 };
    let far = Slab { z0: 90.0, z1: 91.0 };
    let mut rig = rig_with(collide_cfg(on()));
    tick_with(&mut rig, [0.0; 3], 0, DT, Some(&wall));
    let pulled = rig.state.boom.unwrap();
    // A held clock neither spends the hold nor eases.
    for _ in 0..30 {
        tick_with(&mut rig, [0.0; 3], 0, 0.0, Some(&far));
    }
    assert_eq!((rig.state.boom, rig.state.boom_hold), (Some(pulled), 0.15));
    // The hold spends on gameplay time with the boom unchanged...
    for k in 1..=8 {
        tick_with(&mut rig, [0.0; 3], k, DT, Some(&far));
        assert_eq!(rig.state.boom, Some(pulled), "tick {k}");
    }
    // ...then it eases back out to the full boom and the state returns to default.
    let mut last = pulled;
    let mut grew = false;
    for k in 9..600 {
        tick_with(&mut rig, [0.0; 3], k, DT, Some(&far));
        match rig.state.boom {
            Some(boom) => {
                assert!(boom >= last, "monotone recovery");
                grew |= boom > pulled;
                last = boom;
            }
            None => break,
        }
    }
    assert!(grew);
    assert_eq!((rig.state.boom, rig.state.boom_hold), (None, 0.0));
}

#[test]
fn a_deeper_hit_during_recovery_pulls_in_at_once_and_restarts_the_hold() {
    let near_wall = Slab { z0: 4.0, z1: 50.0 };
    let nearer = Slab { z0: 2.0, z1: 50.0 };
    let mut rig = rig_with(collide_cfg(on()));
    tick_with(&mut rig, [0.0; 3], 0, DT, Some(&near_wall));
    let first = rig.state.boom.unwrap();
    for k in 1..4 {
        tick_with(&mut rig, [0.0; 3], k, DT, Some(&near_wall));
    }
    assert!(rig.state.boom_hold < 0.15);
    tick_with(&mut rig, [0.0; 3], 4, DT, Some(&nearer));
    assert!(rig.state.boom.unwrap() < first);
    assert_eq!(rig.state.boom_hold, 0.15);
}

#[test]
fn starting_inside_retries_from_the_target_then_falls_back_and_reports_stuck() {
    // The pivot sits inside the slab but the target does not: the retry from the target finds the
    // wall between it and the eye.
    let slab = Slab { z0: 2.5, z1: 3.5 };
    let mut rig = rig_with(RigConfig {
        focus_offset: [0.0, 0.0, 3.0],
        ..collide_cfg(on())
    });
    let out = tick_with(&mut rig, [0.0; 3], 0, DT, Some(&slab));
    assert!(!out.stuck);
    let boom = rig.state.boom.expect("pulled in");
    assert!(boom >= 0.6 && boom < full_boom());
    // Both inside: the boom holds at `min_distance` and the tick reports stuck.
    let all = Slab {
        z0: -100.0,
        z1: 100.0,
    };
    let mut rig = rig_with(collide_cfg(on()));
    let out = tick_with(&mut rig, [0.0; 3], 0, DT, Some(&all));
    assert!(out.stuck);
    assert_eq!(rig.state.boom, Some(0.6));
    assert!((dist(out.translation, [0.0; 3]) - 0.6).abs() < 1e-4);
}

#[test]
fn disabled_collision_is_bit_identical_to_no_collision() {
    let wall = Slab { z0: 3.0, z1: 50.0 };
    let mut plain = rig_with(collide_cfg(RigCollisionConfig::default()));
    let mut disabled = plain.clone();
    let mut absent = rig_with(collide_cfg(on()));
    for k in 0..60 {
        for rig in [&mut plain, &mut disabled, &mut absent] {
            add_look(rig, 1.5, 0.2);
        }
        let target = [k as f32 * 0.05, 0.0, -1.0];
        let a = tick_with(&mut plain, target, k, DT, None);
        let b = tick_with(&mut disabled, target, k, DT, Some(&wall));
        let c = tick_with(&mut absent, target, k, DT, None);
        assert_eq!((a, a), (b, c));
    }
    // `last_applied` carries the authored config, which differs by the enabled flag alone.
    let json = |rig: &CameraRig| {
        let mut state = serde_json::to_value(&rig.state).unwrap();
        state.as_object_mut().unwrap().remove("last_applied");
        state
    };
    assert_eq!(json(&plain), json(&disabled));
    assert_eq!(json(&plain), json(&absent));
    assert!(json(&plain).get("boom").is_none());
}

#[test]
fn track_collision_casts_from_the_focus() {
    let path = straight();
    let wall = Slab {
        z0: -17.0,
        z1: -10.0,
    };
    let mut rig = rig_with(RigConfig {
        collision: on(),
        ..rigid([0.0, 0.0, 4.5])
    });
    // Target at z = -20: the focus is on it and the eye 4.5 m behind at z = -15.5, inside the wall.
    let out = tick(&mut rig, &path, [0.0, 1.0, -20.0], 0);
    let _ = out;
    let input = RigInput {
        target: Some([0.0, 1.0, -20.0]),
        path: Some(&path),
        game_dt: DT,
        dt: DT,
        game_frame: FrameIndex::new(1),
        camera_fov_y_deg: None,
        collision: Some(&wall),
    };
    let out = step_rig(&mut rig, &input).unwrap();
    let boom = rig.state.boom.expect("pulled in");
    assert!(boom < 4.5);
    assert!(out.translation[2] + 0.3 <= -17.0 + 1e-3 || out.translation[2] - 0.3 >= -10.0);
}

fn framed(weight: f32) -> Framing {
    Framing {
        weight,
        pivot_offset: [1.0, 0.5, 0.0],
        yaw_offset_deg: 20.0,
        pitch_offset_deg: 5.0,
        distance: Some(4.0),
        half_life: 0.12,
    }
}

#[test]
fn framing_at_weight_zero_is_bit_identical_to_no_framing() {
    let mut plain = chase_rig(ChaseRig::default());
    let mut zero = chase_rig(ChaseRig::default());
    zero.framing = framed(0.0);
    for k in 0..40 {
        add_look(&mut plain, 2.0, 0.5);
        add_look(&mut zero, 2.0, 0.5);
        let target = [k as f32 * 0.1, 0.0, -2.0];
        assert_eq!(
            fp_tick(&mut plain, target, k),
            fp_tick(&mut zero, target, k)
        );
    }
    assert_eq!(plain.state, zero.state);
}

#[test]
fn framing_reaches_full_weight_and_returns_to_exact_rest() {
    let mut rig = chase_rig(ChaseRig::default());
    fp_tick(&mut rig, [0.0; 3], 0);
    rig.framing = framed(1.0);
    for k in 1..400 {
        fp_tick(&mut rig, [0.0; 3], k);
    }
    assert_eq!(rig.state.framing_weight, 1.0);
    let aim = rig.aim().unwrap();
    assert!((aim.yaw_deg - 20.0).abs() < 1e-4);
    assert!((aim.pitch_deg - 25.0).abs() < 1e-4);
    assert_eq!(rig.state.yaw_deg, 0.0, "the unframed orbit yaw stays");
    assert!(near(
        aim.eye,
        orbit_eye([1.0, 0.5, 0.0], 20.0, 25.0, 4.0),
        1e-3
    ));
    // A framed pivot offset rotates with the orbit yaw.
    set_look(&mut rig, 90.0, 20.0);
    fp_tick(&mut rig, [0.0; 3], 401);
    let aim = rig.aim().unwrap();
    assert!(near(
        aim.eye,
        orbit_eye([0.0, 0.5, -1.0], 110.0, 25.0, 4.0),
        1e-3
    ));
    rig.framing.weight = 0.0;
    for k in 402..800 {
        fp_tick(&mut rig, [0.0; 3], k);
    }
    assert_eq!(rig.state.framing_weight, 0.0);
    assert!(
        serde_json::to_value(&rig.state)
            .unwrap()
            .get("framing_weight")
            .is_none()
    );
}

#[test]
fn a_snap_takes_the_framing_weight_at_once() {
    let mut rig = chase_rig(ChaseRig::default());
    rig.framing = framed(1.0);
    fp_tick(&mut rig, [0.0; 3], 0);
    assert_eq!(rig.state.framing_weight, 1.0);
}

#[test]
fn the_latch_with_framing_equals_look_then_a_held_tick() {
    let mut rig = chase_rig(ChaseRig::default());
    fp_tick(&mut rig, [0.0; 3], 0);
    rig.framing = framed(1.0);
    for k in 1..30 {
        fp_tick(&mut rig, [k as f32 * 0.1, 0.0, -2.0], k);
    }
    assert!(rig.state.framing_weight > 0.0 && rig.state.framing_weight < 1.0);
    let target = [2.9, 0.0, -2.0];
    let latched = resolved_camera(&rig, target, [25.0, -7.0]).unwrap();
    let mut held = rig.clone();
    add_look(&mut held, 25.0, -7.0);
    let out = fp_tick_dt(&mut held, target, 30, 0.0);
    assert_eq!(latched.view, out);
    assert_eq!(latched.aim, held.aim().unwrap());
}

fn recenter_rig(min_speed: f32) -> CameraRig {
    chase_rig(ChaseRig {
        recenter: Recenter {
            enabled: true,
            delay: 0.5,
            half_life: 0.2,
            min_speed,
        },
        ..ChaseRig::default()
    })
}

#[test]
fn recenter_turns_behind_the_motion_only_after_the_delay_without_look() {
    let mut rig = recenter_rig(1.0);
    let mut x = 0.0;
    fp_tick(&mut rig, [x, 0.0, 0.0], 0);
    for k in 1..=25 {
        x += 3.0 * DT;
        fp_tick(&mut rig, [x, 0.0, 0.0], k);
        assert_eq!(rig.state.yaw_deg, 0.0, "tick {k}: still inside the delay");
    }
    for k in 26..=120 {
        x += 3.0 * DT;
        fp_tick(&mut rig, [x, 0.0, 0.0], k);
    }
    assert!(
        (rig.state.yaw_deg + 90.0).abs() < 0.5,
        "{}",
        rig.state.yaw_deg
    );
    // A nonzero look restarts the wait.
    add_look(&mut rig, 30.0, 0.0);
    x += 3.0 * DT;
    fp_tick(&mut rig, [x, 0.0, 0.0], 121);
    assert!(rig.state.look_idle < 2.0 * DT);
    let yaw = rig.state.yaw_deg;
    for k in 122..=140 {
        x += 3.0 * DT;
        fp_tick(&mut rig, [x, 0.0, 0.0], k);
    }
    assert_eq!(rig.state.yaw_deg, yaw);
}

#[test]
fn recenter_never_turns_below_min_speed_and_stays_off_by_default() {
    let mut slow = recenter_rig(1.0);
    let mut off = chase_rig(ChaseRig::default());
    let mut x = 0.0;
    for k in 0..200 {
        x += 0.5 * DT;
        fp_tick(&mut slow, [x, 0.0, 0.0], k);
        fp_tick(&mut off, [x * 6.0, 0.0, 0.0], k);
    }
    assert_eq!(slow.state.yaw_deg, 0.0);
    assert_eq!(off.state.yaw_deg, 0.0);
    assert_eq!(off.state.look_idle, 0.0);
}
