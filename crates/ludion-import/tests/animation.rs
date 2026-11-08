//! The importer's animation mapping on synthetic documents: a three-joint chain
//! (`root` → `spine` → `head`, node indices 1–3) under a skinned one-triangle mesh (node 0).

use ludion_core::diagnostics::Diagnostic;
use ludion_import::{ClipArtifact, ExternalFiles, Import, ImportOptions, import};
use ludion_scene::{Ease, check_animation, emit_animation, sample_rotation};

const ROOT: usize = 1;
const SPINE: usize = 2;
const BODY: usize = 0;

struct Channel {
    node: usize,
    path: &'static str,
    interpolation: &'static str,
    times: Vec<f32>,
    /// The flat output values.
    values: Vec<f32>,
    /// Components per output element (3 for vectors, 4 for rotations, 1 for weights).
    components: usize,
}

struct Animation {
    name: Option<&'static str>,
    extras: Option<&'static str>,
    channels: Vec<Channel>,
}

fn animation(channels: Vec<Channel>) -> Animation {
    Animation {
        name: Some("clip"),
        extras: None,
        channels,
    }
}

fn with_extras(mut animation: Animation, extras: &'static str) -> Animation {
    animation.extras = Some(extras);
    animation
}

/// `degrees` about the unit `axis` as an `[x, y, z, w]` quaternion.
fn quat(axis: [f32; 3], degrees: f32) -> [f32; 4] {
    let half = degrees.to_radians() * 0.5;
    [
        axis[0] * half.sin(),
        axis[1] * half.sin(),
        axis[2] * half.sin(),
        half.cos(),
    ]
}

fn rotation(node: usize, times: Vec<f32>, quats: &[[f32; 4]]) -> Channel {
    Channel {
        node,
        path: "rotation",
        interpolation: "LINEAR",
        times,
        values: quats.iter().flatten().copied().collect(),
        components: 4,
    }
}

fn translation(node: usize, times: Vec<f32>, points: &[[f32; 3]]) -> Channel {
    Channel {
        node,
        path: "translation",
        interpolation: "LINEAR",
        times,
        values: points.iter().flatten().copied().collect(),
        components: 3,
    }
}

fn frames(fps: f32, count: usize) -> Vec<f32> {
    (0..count).map(|k| k as f32 / fps).collect()
}

const NODES: &str = r#"{"name":"body","mesh":0,"skin":0},{"name":"root","children":[2]},{"name":"spine","translation":[0,1,0],"children":[3]},{"name":"head","translation":[0,0.5,0]}"#;
const SKINS: &str = r#"{"joints":[1,2,3]}"#;

fn gltf(animations: &[Animation]) -> String {
    gltf_with_nodes(animations, NODES, SKINS)
}

fn gltf_with_nodes(animations: &[Animation], nodes: &str, skins: &str) -> String {
    let mut bytes = Vec::new();
    for value in [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend([0_u8, 1, 0, 0].repeat(3));
    for value in [0.2_f32, 0.6, 0.0, 0.0].repeat(3) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let mut accessors = vec![
        r#"{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]}"#.to_string(),
        r#"{"bufferView":1,"componentType":5121,"count":3,"type":"VEC4"}"#.to_string(),
        r#"{"bufferView":2,"componentType":5126,"count":3,"type":"VEC4"}"#.to_string(),
    ];
    let mut views = vec![
        r#"{"buffer":0,"byteOffset":0,"byteLength":36}"#.to_string(),
        r#"{"buffer":0,"byteOffset":36,"byteLength":12}"#.to_string(),
        r#"{"buffer":0,"byteOffset":48,"byteLength":48}"#.to_string(),
    ];
    let mut animation_json = Vec::new();
    for animation in animations {
        let mut channels = Vec::new();
        let mut samplers = Vec::new();
        for (index, channel) in animation.channels.iter().enumerate() {
            let mut push = |data: &[f32], accessor: String| {
                views.push(format!(
                    r#"{{"buffer":0,"byteOffset":{},"byteLength":{}}}"#,
                    bytes.len(),
                    data.len() * 4
                ));
                for value in data {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                accessors.push(accessor.replace("VIEW", &(views.len() - 1).to_string()));
                accessors.len() - 1
            };
            let input = push(
                &channel.times,
                format!(
                    r#"{{"bufferView":VIEW,"componentType":5126,"count":{},"type":"SCALAR"}}"#,
                    channel.times.len()
                ),
            );
            let kind = ["SCALAR", "SCALAR", "VEC2", "VEC3", "VEC4"][channel.components];
            let output = push(
                &channel.values,
                format!(
                    r#"{{"bufferView":VIEW,"componentType":5126,"count":{},"type":"{kind}"}}"#,
                    channel.values.len() / channel.components
                ),
            );
            samplers.push(format!(
                r#"{{"input":{input},"output":{output},"interpolation":"{}"}}"#,
                channel.interpolation
            ));
            channels.push(format!(
                r#"{{"sampler":{index},"target":{{"node":{},"path":"{}"}}}}"#,
                channel.node, channel.path
            ));
        }
        let name = animation
            .name
            .map_or(String::new(), |n| format!(r#""name":"{n}","#));
        let extras = animation
            .extras
            .map_or(String::new(), |e| format!(r#","extras":{e}"#));
        animation_json.push(format!(
            r#"{{{name}"channels":[{}],"samplers":[{}]{extras}}}"#,
            channels.join(","),
            samplers.join(",")
        ));
    }
    let buffer = base64::encode(&bytes);
    format!(
        r#"{{"asset":{{"version":"2.0"}},"nodes":[{nodes}],"skins":[{skins}],"meshes":[{{"name":"tri","primitives":[{{"attributes":{{"POSITION":0,"JOINTS_0":1,"WEIGHTS_0":2}}}}]}}],"accessors":[{}],"bufferViews":[{}],"buffers":[{{"byteLength":{},"uri":"data:application/octet-stream;base64,{buffer}"}}],"animations":[{}]}}"#,
        accessors.join(","),
        views.join(","),
        bytes.len(),
        animation_json.join(","),
    )
}

fn run(animations: &[Animation], lossy: bool) -> Result<Import, Vec<Diagnostic>> {
    import(
        gltf(animations).as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(lossy),
    )
}

fn clip_of(animations: &[Animation]) -> ClipArtifact {
    let mut import = run(animations, false).expect("the animation imports");
    assert_eq!(import.clips.len(), animations.len());
    import.clips.remove(0)
}

fn codes(errors: &[Diagnostic]) -> Vec<&str> {
    errors.iter().map(|d| d.code.as_str()).collect()
}

/// A full turn about Y in 24 frames (0.4 s at 60 fps): 15 degrees a frame, every key a real
/// keyframe, so reduction alone decides what survives.
#[test]
fn a_fast_full_turn_imports_with_no_euler_flip() {
    let quats: Vec<[f32; 4]> = (0..=24)
        .map(|k| quat([0.0, 1.0, 0.0], 15.0 * k as f32))
        .collect();
    let clip = clip_of(&[animation(vec![rotation(ROOT, frames(60.0, 25), &quats)])]);
    let keys = &clip.doc.tracks[0].rotation;
    assert_eq!(clip.doc.tracks[0].target, "root");
    assert!(keys.len() < 25, "reduction dropped keys: {}", keys.len());
    for pair in keys.windows(2) {
        assert!(
            pair[1].v[1] > pair[0].v[1],
            "Euler Y runs monotonically: {:?} then {:?}",
            pair[0].v,
            pair[1].v
        );
        assert!((pair[1].v[1] - pair[0].v[1]).abs() < 180.5, "no flip");
    }
    let last = keys.last().unwrap();
    assert!((last.v[1] - 360.0).abs() < 1e-3, "{:?}", last.v);
    assert!(
        last.v[0].abs() < 1e-3 && last.v[2].abs() < 1e-3,
        "{:?}",
        last.v
    );
}

/// Two frames an exact half turn apart: the Euler unwrap ties at 180°, which the validator refuses
/// and which would play the opposite way. The importer keys the source's midpoint instead.
#[test]
fn an_exact_half_turn_between_two_keys_imports_and_turns_the_source_way() {
    for axis in 0..3 {
        let mut flip = [0.0, 0.0, 0.0, 0.0];
        flip[axis] = 1.0;
        let animations = [animation(vec![rotation(
            ROOT,
            vec![0.0, 1.0],
            &[[0.0, 0.0, 0.0, 1.0], flip],
        )])];
        for lossy in [false, true] {
            let import = run(&animations, lossy).expect("the half turn imports");
            assert!(import.warnings.is_empty(), "{:?}", import.warnings);
            let clip = &import.clips[0];
            check_animation(&emit_animation(&clip.doc)).expect("the clip validates");
            let keys = &clip.doc.tracks[0].rotation;
            for k in 0..=200 {
                let u = k as f64 / 200.0;
                let got = sample_rotation(keys, u).expect("keys");
                let half = (90.0 * u).to_radians();
                let mut want = [0.0, 0.0, 0.0, half.cos()];
                want[axis] = half.sin();
                let dot: f64 = (0..4).map(|c| got[c] * want[c]).sum();
                let degrees = 2.0 * dot.abs().min(1.0).acos().to_degrees();
                assert!(degrees < 0.1, "axis {axis} u {u}: {degrees}° off, {got:?}");
            }
        }
    }
}

/// A split key's time follows the same short-times rule as every source time: the shortest `f32`
/// decimal, never `0.15000000000000002`.
#[test]
fn a_half_turn_split_time_is_the_shortest_f32_decimal() {
    let animations = [animation(vec![rotation(
        ROOT,
        vec![0.1, 0.2],
        &[[0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 0.0]],
    )])];
    let import = run(&animations, false).expect("the half turn imports");
    let keys = &import.clips[0].doc.tracks[0].rotation;
    assert!(keys.len() > 2, "the flip is split: {keys:?}");
    for key in keys {
        let short = (key.t as f32).to_string().parse::<f64>().unwrap();
        assert_eq!(key.t, short, "{keys:?}");
    }
}

/// A squash and stretch on the root shears its children's model matrices; a child turning 150°
/// under it still measures against the source without an error from the shear.
#[test]
fn a_child_turning_under_a_non_uniform_parent_scale_stays_in_bound() {
    let times = frames(30.0, 25);
    let ease = |k: usize| {
        let t = k as f32 / 24.0;
        t * t * (3.0 - 2.0 * t)
    };
    let scales: Vec<f32> = (0..25)
        .flat_map(|k| {
            let e = ease(k);
            [1.0 + 0.08 * e, 1.0 - 0.15 * e, 1.0 + 0.08 * e]
        })
        .collect();
    let quats: Vec<[f32; 4]> = (0..25)
        .map(|k| quat([1.0, 0.0, 0.0], 150.0 * ease(k)))
        .collect();
    let clip = clip_of(&[animation(vec![
        Channel {
            node: ROOT,
            path: "scale",
            interpolation: "LINEAR",
            times: times.clone(),
            values: scales,
            components: 3,
        },
        rotation(SPINE, times, &quats),
    ])]);
    let spine = clip
        .report
        .joints
        .iter()
        .find(|j| j.joint == "spine")
        .unwrap();
    assert!(spine.rotation_deg <= 0.5, "{spine:?}");
}

/// A joint whose exported channels never leave its rest pose gets no track, and is absent from the
/// report's track counts; the clip stays inside its bound with the channel sampled as rest.
#[test]
fn channels_constant_at_rest_are_dropped() {
    let times = frames(30.0, 13);
    let turn: Vec<[f32; 4]> = (0..13)
        .map(|k| quat([0.0, 1.0, 0.0], 10.0 * k as f32))
        .collect();
    let still_rotation = vec![quat([0.0, 0.0, 1.0], 0.0); 13];
    let still_translation = vec![[0.0_f32, 1.0, 0.0]; 13];
    let clip = clip_of(&[animation(vec![
        rotation(ROOT, times.clone(), &turn),
        rotation(SPINE, times.clone(), &still_rotation),
        translation(SPINE, times, &still_translation),
    ])]);
    let targets: Vec<&str> = clip.doc.tracks.iter().map(|t| t.target.as_str()).collect();
    assert_eq!(targets, ["root"]);
    assert_eq!(clip.report.tracks.len(), 1);
    assert_eq!(clip.report.tracks[0].target, "root");
    assert!(clip.report.joints.iter().all(|j| j.rotation_deg <= 0.5));
}

/// An animation whose every channel stays at rest would reduce to zero tracks: `L1208`, in both
/// lossy modes.
#[test]
fn clip_entirely_at_rest_is_refused() {
    let times = frames(30.0, 13);
    let still_rotation = vec![quat([0.0, 0.0, 1.0], 0.0); 13];
    let animations = [animation(vec![rotation(SPINE, times, &still_rotation)])];
    for lossy in [false, true] {
        let errors = run(&animations, lossy).expect_err("a trackless clip is refused");
        assert_eq!(codes(&errors), ["L1208"], "lossy = {lossy}");
        assert!(
            errors[0].message.contains("animation 0"),
            "{}",
            errors[0].message
        );
    }
}

/// A ±0.3° / ±1 mm sine on the spine survives reduction and stays inside the bound.
#[test]
fn tiny_breathing_survives_reduction() {
    let times = frames(30.0, 61);
    let phase = |k: usize| (std::f32::consts::TAU * k as f32 / 60.0).sin();
    let quats: Vec<[f32; 4]> = (0..61)
        .map(|k| quat([1.0, 0.0, 0.0], 0.3 * phase(k)))
        .collect();
    let points: Vec<[f32; 3]> = (0..61)
        .map(|k| [0.0, 1.0 + 0.001 * phase(k), 0.0])
        .collect();
    let clip = clip_of(&[animation(vec![
        rotation(SPINE, times.clone(), &quats),
        translation(SPINE, times, &points),
    ])]);
    let track = &clip.doc.tracks[0];
    assert_eq!(track.target, "spine");
    assert!(track.rotation.len() > 2, "{}", track.rotation.len());
    assert!(track.translation.len() > 2, "{}", track.translation.len());
    let spine = clip
        .report
        .joints
        .iter()
        .find(|j| j.joint == "spine")
        .unwrap();
    assert!(
        spine.rotation_deg <= 0.5 && spine.translation <= 0.002,
        "{spine:?}"
    );
    let counts = &clip.report.tracks[0];
    assert_eq!(counts.rotation.before, 61);
    assert_eq!(counts.rotation.after, track.rotation.len());
    assert_eq!(clip.report.source_frames, 61);
}

/// Keys that cross ±180° about X read as continuous Euler values, not a 350° jump.
#[test]
fn euler_values_are_continuous_across_180() {
    let angles = [170.0_f32, 178.0, 190.0, 181.0, 172.0];
    let quats: Vec<[f32; 4]> = angles.iter().map(|&a| quat([1.0, 0.0, 0.0], a)).collect();
    let clip = clip_of(&[animation(vec![rotation(SPINE, frames(30.0, 5), &quats)])]);
    let keys = &clip.doc.tracks[0].rotation;
    for pair in keys.windows(2) {
        assert!((pair[1].v[0] - pair[0].v[0]).abs() < 180.0, "{keys:?}");
    }
    assert!(
        keys.iter().any(|k| (k.v[0] - 190.0).abs() < 1e-3),
        "the peak reads as 190, not -170: {keys:?}"
    );
}

/// A bound tighter than the reduction leaves is refused with the joint and the frame named.
#[test]
fn a_clip_over_its_bound_is_refused() {
    let times = frames(30.0, 61);
    let quats: Vec<[f32; 4]> = (0..61)
        .map(|k| {
            quat(
                [1.0, 0.0, 0.0],
                0.3 * (std::f32::consts::TAU * k as f32 / 60.0).sin(),
            )
        })
        .collect();
    let refused = with_extras(
        animation(vec![rotation(SPINE, times, &quats)]),
        r#"{"bound_rotation_deg":0.0001,"reduce_rotation_deg":1.0}"#,
    );
    for lossy in [false, true] {
        let errors = run(std::slice::from_ref(&refused), lossy).expect_err("refused");
        assert_eq!(codes(&errors), ["L1206"], "lossy = {lossy}");
        let message = &errors[0].message;
        assert!(message.contains("animation 0"), "{message}");
        assert!(message.contains("joint `"), "{message}");
        assert!(message.contains("frame "), "{message}");
        assert!(message.contains("rotation"), "{message}");
        assert!(!message.contains("  "), "no double spaces: {message}");
    }
}

/// STEP keys map to `step` ease; a CUBICSPLINE channel is sampled at four steps per interval (more
/// where it curves) and reduced.
#[test]
fn step_and_cubic_spline_channels() {
    let step = Channel {
        interpolation: "STEP",
        ..translation(
            ROOT,
            vec![0.0, 1.0, 2.0],
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
        )
    };
    let clip = clip_of(&[animation(vec![step])]);
    let keys = &clip.doc.tracks[0].translation;
    assert_eq!(keys.len(), 3);
    assert_eq!(keys[0].ease, Ease::Linear, "the first key's ease is inert");
    assert!(keys[1..].iter().all(|k| k.ease == Ease::Step), "{keys:?}");

    // Values 0, 1, 2 at t = 0, 1, 2 with tangent 1: the spline is the straight line x = t.
    let mut values = Vec::new();
    for x in [0.0_f32, 1.0, 2.0] {
        values.extend([1.0, 0.0, 0.0, x, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }
    let cubic = Channel {
        interpolation: "CUBICSPLINE",
        values,
        ..translation(ROOT, vec![0.0, 1.0, 2.0], &[])
    };
    let clip = clip_of(&[animation(vec![cubic])]);
    let counts = &clip.report.tracks[0].translation;
    assert_eq!(clip.report.source_frames, 9);
    assert_eq!(counts.before, 9);
    assert_eq!(counts.after, 2, "a straight line reduces to its endpoints");
}

/// One CUBICSPLINE translation and one rotation channel on `root`, keyed at t = 0 and 1 with value
/// zero/identity and both tangents `tangent` (a Hermite arc that bulges well between its keys).
fn bulging_cubic(tangent: f32) -> Animation {
    let mut translation_values = Vec::new();
    let mut rotation_values = Vec::new();
    for key in 0..2 {
        // [in, value, out]; only key 0's out and key 1's in tangents matter.
        let (in_tangent, out_tangent) = if key == 0 {
            (0.0, tangent)
        } else {
            (tangent, 0.0)
        };
        translation_values.extend([in_tangent, 0.0, 0.0, 0.0, 0.0, 0.0, out_tangent, 0.0, 0.0]);
        rotation_values.extend([
            0.0,
            0.0,
            in_tangent,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            out_tangent,
            0.0,
        ]);
    }
    animation(vec![
        Channel {
            interpolation: "CUBICSPLINE",
            values: translation_values,
            ..translation(ROOT, vec![0.0, 1.0], &[])
        },
        Channel {
            interpolation: "CUBICSPLINE",
            values: rotation_values,
            ..rotation(ROOT, vec![0.0, 1.0], &[])
        },
    ])
}

/// The glTF-spec cubic Hermite of `bulging_cubic(tangent)` at `u` in [0, 1] (dt = 1): the
/// translation x, and the rotation as a normalized `[x, y, z, w]`.
fn hermite(tangent: f64, u: f64) -> (f64, [f64; 4]) {
    let (u2, u3) = (u * u, u * u * u);
    let (h10, h11) = (u3 - 2.0 * u2 + u, u3 - u2);
    let bulge = h10 * tangent + h11 * tangent;
    // h00 + h01 = 1 for the unit value at both keys.
    let q = [0.0, 0.0, bulge, 1.0];
    let len = (bulge * bulge + 1.0).sqrt();
    (bulge, q.map(|c| c / len))
}

/// The worst distance and angle between the emitted clip and the spec Hermite on a dense grid.
fn dense_error(clip: &ClipArtifact, tangent: f64) -> (f64, f64) {
    let track = clip.doc.tracks.iter().find(|t| t.target == "root").unwrap();
    let (mut distance, mut angle) = (0.0_f64, 0.0_f64);
    for k in 0..=2000 {
        let u = f64::from(k) / 2000.0;
        let (x, q) = hermite(tangent, u);
        let sampled = ludion_scene::sample_channel(&track.translation, u).unwrap();
        distance = distance
            .max(((sampled[0] - x).powi(2) + sampled[1].powi(2) + sampled[2].powi(2)).sqrt());
        let r = ludion_scene::sample_rotation(&track.rotation, u).unwrap();
        let dot = (r[0] * q[0] + r[1] * q[1] + r[2] * q[2] + r[3] * q[3]).abs();
        angle = angle.max((2.0 * dot.min(1.0).acos()).to_degrees());
    }
    (distance, angle)
}

/// Between its frames the runtime interpolates the reduced keys linearly, so a curved cubic
/// interval must be sampled densely enough that the reported error is the error everywhere.
#[test]
fn a_curved_cubic_interval_is_sampled_until_the_reported_error_holds_between_frames() {
    // The rotating root swings `head` 1.5 units away, so the default translation bound is too tight.
    let clip = clip_of(&[with_extras(
        bulging_cubic(1.0),
        r#"{"bound_translation":0.01}"#,
    )]);
    let root = clip
        .report
        .joints
        .iter()
        .find(|j| j.joint == "root")
        .unwrap();
    let (distance, angle) = dense_error(&clip, 1.0);
    // The default reduce tolerances: 0.0005 units and 0.1 degrees.
    assert!(
        distance <= root.translation + 2.0 * 0.0005,
        "translation: dense {distance} vs reported {} (frames {})",
        root.translation,
        clip.report.source_frames
    );
    assert!(
        angle <= root.rotation_deg + 2.0 * 0.1,
        "rotation: dense {angle}° vs reported {}° (frames {})",
        root.rotation_deg,
        clip.report.source_frames
    );
}

/// A tangent so steep that 64 steps still miss is refused, naming the joint, channel and interval.
#[test]
fn a_cubic_interval_too_curved_for_the_cap_is_l1206() {
    for lossy in [false, true] {
        let mut steep = bulging_cubic(100.0);
        steep.channels.remove(1);
        let errors = run(&[steep], lossy).expect_err("refused");
        assert_eq!(codes(&errors), ["L1206"], "lossy = {lossy}");
        let message = &errors[0].message;
        assert!(message.contains("`root`"), "{message}");
        assert!(message.contains("translation"), "{message}");
        assert!(message.contains("CUBICSPLINE"), "{message}");
        assert!(!message.contains("  "), "no double spaces: {message}");
    }
}

#[test]
fn loop_extras_set_the_flag_and_the_seam() {
    let channels = || {
        vec![translation(
            ROOT,
            vec![0.0, 1.0],
            &[[0.0, 0.0, 0.0], [0.5, 0.0, 0.0]],
        )]
    };
    let one_shot = clip_of(&[animation(channels())]);
    assert!(!one_shot.doc.looped);
    assert!(one_shot.report.seam.is_none());

    let looping = clip_of(&[with_extras(animation(channels()), r#"{"loop":true}"#)]);
    assert!(looping.doc.looped);
    let seam = looping.report.seam.expect("a loop reports its seam");
    assert!((seam.translation - 0.5).abs() < 1e-9, "{seam:?}");
    assert_eq!(seam.rotation_deg, 0.0);
}

/// Morph weights and a skin joint mixed into an animation of parts are `L1202`; the part channel
/// still imports under `--allow-lossy`.
#[test]
fn unmappable_channels_are_l1202() {
    let weights = Channel {
        node: BODY,
        path: "weights",
        interpolation: "LINEAR",
        times: vec![0.0, 1.0],
        values: vec![0.0, 1.0],
        components: 1,
    };
    let on_body = translation(BODY, vec![0.0, 1.0], &[[0.0; 3], [1.0, 0.0, 0.0]]);
    let joint = translation(ROOT, vec![0.0, 1.0], &[[0.0; 3], [1.0, 0.0, 0.0]]);
    let animations = [animation(vec![weights, on_body, joint])];

    let errors = run(&animations, false).expect_err("strict refuses");
    assert_eq!(codes(&errors), ["L1202", "L1202"]);
    assert!(errors[0].message.contains("morph-weights"), "{errors:?}");
    assert!(
        errors[1]
            .message
            .contains("skin joint in an animation of prefab parts"),
        "{errors:?}"
    );

    let plan = run(&animations, true).expect("lossy imports the part channel");
    assert_eq!(plan.clips.len(), 1);
    assert!(plan.clips[0].binds_parts);
    assert_eq!(plan.warnings.len(), 2);
    assert!(plan.warnings.iter().all(|w| w.code.as_str() == "L1202"));

    let none = [animation(vec![Channel {
        node: BODY,
        path: "weights",
        interpolation: "LINEAR",
        times: vec![0.0, 1.0],
        values: vec![0.0, 1.0],
        components: 1,
    }])];
    let plan = run(&none, true).expect("lossy");
    assert!(plan.clips.is_empty());
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.message.contains("no importable channel")),
        "{:?}",
        plan.warnings
    );
}

/// The skin joints first, then a part: the part channel is the one dropped.
#[test]
fn a_part_channel_in_an_animation_of_skin_joints_is_l1202() {
    let joint = translation(ROOT, vec![0.0, 1.0], &[[0.0; 3], [1.0, 0.0, 0.0]]);
    let on_body = translation(BODY, vec![0.0, 1.0], &[[0.0; 3], [1.0, 0.0, 0.0]]);
    let animations = [animation(vec![joint, on_body])];

    let errors = run(&animations, false).expect_err("strict refuses");
    assert_eq!(codes(&errors), ["L1202"]);
    assert!(
        errors[0]
            .message
            .contains("prefab part in an animation of skin joints"),
        "{errors:?}"
    );
    let plan = run(&animations, true).expect("lossy imports the joint channel");
    assert!(!plan.clips[0].binds_parts);
    assert!(plan.prefab.clips.is_empty());
}

/// A part clip on a node with no mesh (a pivot): the track names the part and the prefab lists it.
#[test]
fn a_rotation_on_a_pivot_node_imports_as_a_part_clip() {
    let nodes = format!(r#"{NODES},{{"name":"pivot"}}"#);
    let spin = rotation(
        4,
        vec![0.0, 1.0],
        &[quat([1.0, 0.0, 0.0], 0.0), quat([1.0, 0.0, 0.0], -100.0)],
    );
    let json = gltf_with_nodes(&[animation(vec![spin])], &nodes, SKINS);
    let plan = import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(false),
    )
    .expect("imports");

    let clip = &plan.clips[0];
    assert!(clip.binds_parts);
    assert_eq!(clip.doc.tracks[0].target, "pivot");
    let end = clip.doc.tracks[0].rotation.last().unwrap();
    assert!((end.v[0] + 100.0).abs() < 1e-3, "{:?}", end.v);
    assert_eq!(plan.prefab.clips.len(), 1);
    assert_eq!(plan.prefab.clips[0].name, "clip");
    assert_eq!(plan.prefab.clips[0].file, "clips/clip.anim.yaml");
}

/// A STEP sampler keeps `ease: step` on every key after the first, and reduction keeps the steps.
#[test]
fn a_step_channel_on_a_part_keeps_its_steps() {
    let step = Channel {
        interpolation: "STEP",
        ..translation(
            BODY,
            vec![0.0, 0.5, 1.0],
            &[[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
        )
    };
    let clip = clip_of(&[animation(vec![step])]);
    assert!(clip.binds_parts);
    let keys = &clip.doc.tracks[0].translation;
    assert_eq!(keys.len(), 3, "{keys:?}");
    assert!(keys[1..].iter().all(|k| k.ease == Ease::Step), "{keys:?}");
    assert!(emit_animation(&clip.doc).contains("ease: step"));
}

/// A CUBICSPLINE part channel is resampled into frames within the reduce tolerance.
#[test]
fn a_cubicspline_part_channel_resamples_within_bound() {
    // In-tangent, value, out-tangent per key; zero tangents make a smoothstep.
    let cubic = Channel {
        interpolation: "CUBICSPLINE",
        ..translation(
            BODY,
            vec![0.0, 1.0],
            &[
                [0.0; 3],
                [0.0; 3],
                [0.0; 3],
                [0.0; 3],
                [1.0, 0.0, 0.0],
                [0.0; 3],
            ],
        )
    };
    let clip = clip_of(&[animation(vec![cubic])]);
    assert!(clip.binds_parts);
    assert!(
        clip.report.source_frames > 2,
        "{}",
        clip.report.source_frames
    );
    assert!(clip.report.joints[0].translation <= 0.002);
}

/// A child listed before its parent still yields parent-first tracks, and the child measures in
/// model space under a rotated parent.
#[test]
fn part_tracks_are_parent_first_whatever_the_document_order() {
    let nodes = format!(
        r#"{NODES},{{"name":"hand","translation":[1,0,0]}},{{"name":"arm","rotation":[0,0.7071068,0,0.7071068],"children":[4]}}"#
    );
    let turn = rotation(
        5,
        vec![0.0, 0.5, 1.0],
        &[
            quat([0.0, 1.0, 0.0], 90.0),
            quat([0.0, 1.0, 0.0], 135.0),
            quat([0.0, 1.0, 0.0], 180.0),
        ],
    );
    let reach = translation(4, vec![0.0, 1.0], &[[1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]);
    let json = gltf_with_nodes(&[animation(vec![reach, turn])], &nodes, SKINS);
    let plan = import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(false),
    )
    .expect("imports");

    let targets: Vec<&str> = plan.clips[0]
        .doc
        .tracks
        .iter()
        .map(|t| t.target.as_str())
        .collect();
    assert_eq!(targets, ["arm", "hand"]);
    let names: Vec<&str> = plan.clips[0]
        .report
        .joints
        .iter()
        .map(|j| j.joint.as_str())
        .collect();
    assert_eq!(names, ["body", "arm", "hand"]);
}

/// A node under a skin joint is no part, so a channel on it has nothing to drive.
#[test]
fn a_channel_on_a_node_under_a_skin_joint_is_l1202() {
    let nodes = NODES.replace(r#""name":"head","#, r#""name":"head","children":[4],"#);
    let nodes = format!(r#"{nodes},{{"name":"hat"}}"#);
    let on_hat = translation(4, vec![0.0, 1.0], &[[0.0; 3], [1.0, 0.0, 0.0]]);
    let json = gltf_with_nodes(&[animation(vec![on_hat])], &nodes, SKINS);
    let errors = import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(false),
    )
    .expect_err("refused");
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("node that has no part")),
        "{errors:?}"
    );
}

/// A channel on a joint whose rest folds in a helper node above it is dropped and named; a channel
/// on the joint below it (whose own rest is untouched) still imports.
#[test]
fn a_channel_on_a_folded_joint_is_l1202() {
    let nodes = r#"{"name":"body","mesh":0,"skin":0},{"name":"j0","children":[2]},{"name":"helper","translation":[0.5,0,0],"scale":[2,2,2],"children":[3]},{"name":"j1","translation":[0,1,0],"children":[4]},{"name":"j2","translation":[0,0.5,0]}"#;
    let skins = r#"{"joints":[1,3,4]}"#;
    let moving = |node| translation(node, vec![0.0, 1.0], &[[0.0; 3], [1.0, 0.0, 0.0]]);
    let animations = [animation(vec![moving(3), moving(4)])];
    let json = gltf_with_nodes(&animations, nodes, skins);

    let errors = import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(false),
    )
    .expect_err("strict refuses");
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("whose rest folds in the transform")
                && e.message.contains("j1")
                && e.help.as_deref().is_some_and(|h| h.contains("helper node"))),
        "{errors:?}"
    );

    let import = import(
        json.as_bytes(),
        ExternalFiles::Refused,
        &ImportOptions::lossy(true),
    )
    .expect("lossy imports");
    assert!(
        import.warnings.iter().any(|w| w.code.as_str() == "L1202"
            && w.message.contains("j1")
            && w.message.contains("folds")),
        "{:?}",
        import.warnings
    );
    let clip = &import.clips[0];
    assert_eq!(clip.doc.tracks.len(), 1, "only j2's channel imports");
    assert_eq!(clip.doc.tracks[0].target, "j2");
}

#[test]
fn bad_extras_are_l1207() {
    let base = || {
        vec![translation(
            ROOT,
            vec![0.0, 1.0],
            &[[0.0; 3], [1.0, 0.0, 0.0]],
        )]
    };
    for extras in [
        r#"{"reduce_rotation_deg":-1.0}"#,
        r#"{"bound_scale":0}"#,
        r#"{"bound_translation":"tight"}"#,
        r#"{"loop":"yes"}"#,
        r#"{"speed":2.0}"#,
        r#"[1]"#,
    ] {
        for lossy in [false, true] {
            let errors = run(&[with_extras(animation(base()), extras)], lossy).expect_err(extras);
            assert_eq!(codes(&errors), ["L1207"], "{extras} (lossy = {lossy})");
        }
    }
}

/// Clips whose names differ only in case would share one file on Windows and macOS, so they get
/// distinct names the way equal names do.
#[test]
fn clip_names_differing_only_in_case_get_distinct_files() {
    let quats: Vec<[f32; 4]> = (0..=24)
        .map(|k| quat([0.0, 1.0, 0.0], 15.0 * k as f32))
        .collect();
    let channel = || vec![rotation(ROOT, frames(60.0, 25), &quats)];
    let animations = [
        Animation {
            name: Some("Walk"),
            ..animation(channel())
        },
        Animation {
            name: Some("walk"),
            ..animation(channel())
        },
    ];
    let import = run(&animations, false).unwrap();
    let paths: Vec<&str> = import.clips.iter().map(|c| c.rel_path.as_str()).collect();
    assert_eq!(paths, ["clips/Walk.anim.yaml", "clips/walk-1.anim.yaml"]);
    assert_eq!(import.clips[1].doc.name.as_deref(), Some("walk-1"));
}

/// Two imports give byte-identical text, and each clip is valid canonical text.
#[test]
fn the_output_is_deterministic_and_valid() {
    let quats: Vec<[f32; 4]> = (0..=24)
        .map(|k| quat([0.0, 1.0, 0.0], 15.0 * k as f32))
        .collect();
    let animations = [
        animation(vec![rotation(ROOT, frames(60.0, 25), &quats)]),
        Animation {
            name: Some("clip"),
            ..animation(vec![translation(
                SPINE,
                vec![0.0, 1.0],
                &[[0.0, 1.0, 0.0], [0.0, 1.2, 0.0]],
            )])
        },
    ];
    let first = run(&animations, false).unwrap();
    let second = run(&animations, false).unwrap();
    assert_eq!(first.clips.len(), 2);
    let paths: Vec<&str> = first.clips.iter().map(|c| c.rel_path.as_str()).collect();
    assert_eq!(paths, ["clips/clip.anim.yaml", "clips/clip-1.anim.yaml"]);
    for (a, b) in first.clips.iter().zip(&second.clips) {
        let text = emit_animation(&a.doc);
        assert_eq!(text, emit_animation(&b.doc));
        assert_eq!(a.report, b.report);
        let parsed = check_animation(&text).expect("the clip is valid");
        assert_eq!(emit_animation(&parsed), text, "a fixed point of the writer");
        assert!(
            a.doc.generated_from.is_none(),
            "the CLI sets `generated_from`"
        );
    }
}
