//! `ludion render --look-at` orbit framing, expanded into ordinary `--set` override specs.
//!
//! The scene's one `Camera` is moved to orbit the target and the scene's `PostProcess` is focused
//! at the orbit distance, all through the override layer — so each change is echoed, `--sweep` and
//! `--frames` work at the framed view, and an explicit `--set` on the same pointer still wins.

use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::{Orbit, euler_xyz_degrees_to_quat, quat_rotate};
use serde_json::Value;

/// The framing flags as given.
pub(crate) struct FramingArgs<'a> {
    pub look_at: Option<&'a str>,
    pub azimuth: Option<f32>,
    pub pitch: Option<f32>,
    pub distance: Option<f32>,
    pub fov: Option<f32>,
}

impl FramingArgs<'_> {
    fn any(&self) -> bool {
        self.look_at.is_some()
            || self.azimuth.is_some()
            || self.pitch.is_some()
            || self.distance.is_some()
            || self.fov.is_some()
    }
}

/// The override specs to apply (framing first, then the user's), and the line that names the framing.
#[derive(Debug)]
pub(crate) struct Expanded {
    pub set: Vec<String>,
    pub summary: Option<String>,
}

fn invalid(message: impl Into<String>, help: impl Into<String>) -> Vec<Diagnostic> {
    vec![Diagnostic::new(code::SCENE_OVERRIDE_INVALID, message).with_help(help)]
}

/// Expands the framing flags into override specs. No flag given returns `user_set` untouched
/// without reading the scene.
pub(crate) fn expand(
    scene_path: &Path,
    args: &FramingArgs,
    user_set: &[String],
) -> Result<Expanded, Vec<Diagnostic>> {
    if !args.any() {
        return Ok(Expanded {
            set: user_set.to_vec(),
            summary: None,
        });
    }
    let target = parse_look_at(args)?;
    for (flag, value) in [("--azimuth", args.azimuth), ("--pitch", args.pitch)] {
        if let Some(value) = value
            && !value.is_finite()
        {
            return Err(invalid(
                format!("{flag} is not a finite number"),
                "pass degrees",
            ));
        }
    }
    if args.pitch.is_some_and(|p| !(-90.0..=90.0).contains(&p)) {
        return Err(invalid(
            "--pitch is outside -90..90 degrees",
            "pitch is degrees looking down: 90 is straight down, -90 straight up",
        ));
    }
    if args.distance.is_some_and(|d| !(d.is_finite() && d > 0.0)) {
        return Err(invalid(
            "--distance must be a positive number of metres",
            "pass the eye-to-target distance, e.g. --distance 12",
        ));
    }
    if args
        .fov
        .is_some_and(|f| !(f.is_finite() && f > 0.0 && f < 180.0))
    {
        return Err(invalid(
            "--fov must be between 0 and 180 degrees",
            "pass the vertical field of view, e.g. --fov 45",
        ));
    }

    let source = std::fs::read_to_string(scene_path).map_err(|error| {
        vec![
            Diagnostic::new(
                code::SCENE_IO,
                format!("could not read `{}`: {error}", scene_path.display()),
            )
            .with_help("check the path (relative to the current directory) and permissions"),
        ]
    })?;
    let scene = ludion_scene::parse(&source)?;
    let component = |entity: &ludion_scene::SceneEntity, name: &str| -> Option<Value> {
        entity
            .components
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };

    let cameras: Vec<usize> = scene
        .entities
        .iter()
        .enumerate()
        .filter(|(_, e)| e.instance.is_none() && component(e, "Camera").is_some())
        .map(|(i, _)| i)
        .collect();
    let [cam] = cameras[..] else {
        return Err(if cameras.is_empty() {
            invalid(
                "--look-at needs a scene camera, found none",
                "add an entity with Camera + Transform",
            )
        } else {
            invalid(
                format!(
                    "--look-at needs exactly one scene camera, found {} (entities {cameras:?})",
                    cameras.len()
                ),
                "keep one Camera entity in the scene",
            )
        });
    };
    let camera = &scene.entities[cam];
    if component(camera, "Parent").is_some() {
        return Err(invalid(
            "the scene camera has a Parent, so its Transform is parent-local",
            "frame a root camera",
        ));
    }
    let Some(transform) = component(camera, "Transform") else {
        return Err(invalid(
            "the scene camera has no Transform to move",
            "give the Camera entity a Transform",
        ));
    };
    let triple = |key: &str| -> [f32; 3] {
        let mut out = [0.0; 3];
        if let Some(values) = transform.get(key).and_then(Value::as_array) {
            for (slot, value) in out.iter_mut().zip(values) {
                *slot = value.as_f64().unwrap_or(0.0) as f32;
            }
        }
        out
    };
    let (eye, rotation) = (triple("translation"), triple("rotation"));
    let forward = quat_rotate(euler_xyz_degrees_to_quat(rotation), [0.0, 0.0, -1.0]);
    let (scene_azimuth, scene_pitch) = Orbit::angles_of(rotation);

    let mut derived = Vec::new();
    let azimuth_deg = args.azimuth.unwrap_or_else(|| {
        derived.push("azimuth");
        scene_azimuth
    });
    let pitch_deg = args.pitch.unwrap_or_else(|| {
        derived.push("pitch");
        scene_pitch
    });
    let distance = match args.distance {
        Some(distance) => distance,
        None => {
            let t = (target[1] - eye[1]) / forward[1];
            if !(forward[1] < 0.0 && t > 0.0) {
                return Err(invalid(
                    "the scene camera does not look down onto the target's height",
                    "pass --distance",
                ));
            }
            derived.push("distance");
            t
        }
    };
    if args.fov.is_some()
        && component(camera, "Camera")
            .and_then(|c| {
                c.get("projection")?
                    .get("type")?
                    .as_str()
                    .map(str::to_owned)
            })
            .is_some_and(|kind| kind == "orthographic")
    {
        return Err(invalid(
            "--fov on an orthographic scene camera",
            "an orthographic camera has no field of view; drop --fov",
        ));
    }

    let orbit = Orbit {
        target,
        azimuth_deg,
        pitch_deg,
        distance,
    };
    let base = format!("/entities/{cam}/components");
    let mut set = vec![
        format!("{base}/Transform/translation={}", json(&orbit.eye())),
        format!("{base}/Transform/rotation={}", json(&orbit.rotation_deg())),
    ];
    if let Some(fov) = args.fov {
        set.push(format!(
            "{base}/Camera/projection={{\"type\":\"perspective\",\"fov_y_deg\":{}}}",
            json(&fov)
        ));
    }
    let post = scene
        .entities
        .iter()
        .position(|e| e.instance.is_none() && component(e, "PostProcess").is_some());
    if let Some(post) = post {
        set.push(format!(
            "/entities/{post}/components/PostProcess/focus_distance={}",
            json(&distance)
        ));
    }
    set.extend(user_set.iter().cloned());

    let note = if derived.is_empty() {
        String::new()
    } else {
        format!(" ({} from the scene camera)", derived.join(", "))
    };
    let summary = format!(
        "framing: look-at {} azimuth {} pitch {} distance {}{note}",
        json(&target),
        shown(azimuth_deg),
        shown(pitch_deg),
        shown(distance),
    );
    Ok(Expanded {
        set,
        summary: Some(summary),
    })
}

fn parse_look_at(args: &FramingArgs) -> Result<[f32; 3], Vec<Diagnostic>> {
    let Some(value) = args.look_at else {
        return Err(invalid(
            "--azimuth/--pitch/--distance/--fov frame a shot around a target, and --look-at is missing",
            "add --look-at x,y,z",
        ));
    };
    let bad = || {
        invalid(
            format!("--look-at expects `x,y,z` (a single comma-joined token), got `{value}`"),
            "three numbers, e.g. --look-at 0,1,-4 or --look-at -2,1,0",
        )
    };
    let parts: Vec<&str> = value.split(',').collect();
    let [x, y, z] = parts[..] else {
        return Err(bad());
    };
    let mut out = [0.0f32; 3];
    for (slot, part) in out.iter_mut().zip([x, y, z]) {
        *slot = part
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(bad)?;
    }
    Ok(out)
}

/// A readable rendering for the summary line: three decimals, trailing zeros trimmed.
fn shown(value: f32) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// `serde_json`'s shortest round-trip rendering, for the values that become override specs.
fn json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).expect("finite numbers serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAB: &str = "format: 1
seed: 0
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 26.44
          - 11.9
        rotation:
          - -58.0
          - 0.0
          - 0.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 40.0
  - name: grade
    components:
      PostProcess:
        focus_distance: 5.0
";

    fn scene(name: &str, text: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-framing-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scene.yaml");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn args(look_at: Option<&'static str>) -> FramingArgs<'static> {
        FramingArgs {
            look_at,
            azimuth: None,
            pitch: None,
            distance: None,
            fov: None,
        }
    }

    fn value(spec: &str) -> Vec<f32> {
        let (_, v) = spec.split_once('=').unwrap();
        serde_json::from_str(v).unwrap()
    }

    fn message(diagnostics: &[Diagnostic]) -> String {
        assert_eq!(diagnostics[0].code, code::SCENE_OVERRIDE_INVALID);
        diagnostics[0].to_string()
    }

    #[test]
    fn no_flag_reads_no_file() {
        let user = vec!["/a=1".to_string()];
        let expanded = expand(Path::new("does/not/exist.yaml"), &args(None), &user).unwrap();
        assert_eq!(expanded.set, user);
        assert!(expanded.summary.is_none());
    }

    #[test]
    fn look_at_alone_reproduces_the_game_camera() {
        let path = scene("lab", LAB);
        let user = vec!["/entities/1/components/PostProcess/focus_distance=9".to_string()];
        let expanded = expand(&path, &args(Some("0,1,0")), &user).unwrap();
        assert_eq!(expanded.set.len(), 4);
        let eye = value(&expanded.set[0]);
        assert!(
            (eye[1] - 26.44).abs() < 1e-2 && (eye[2] - 15.9).abs() < 1e-2,
            "{eye:?}"
        );
        let rotation = value(&expanded.set[1]);
        assert!(
            (rotation[0] + 58.0).abs() < 1e-2 && rotation[1].abs() < 1e-2,
            "{rotation:?}"
        );
        let focus = value(&format!(
            "x=[{}]",
            expanded.set[2].split_once('=').unwrap().1
        ))[0];
        assert!((focus - 30.0).abs() < 0.05, "{focus}");
        assert_eq!(expanded.set[3], user[0], "the user's --set stays last");
        let summary = expanded.summary.unwrap();
        assert!(
            summary.contains("azimuth 0 pitch 58 distance 29.99")
                && summary.contains("(azimuth, pitch, distance from the scene camera)"),
            "{summary}"
        );
    }

    #[test]
    fn explicit_flags_override_every_derived_value() {
        let path = scene("explicit", LAB);
        let a = FramingArgs {
            look_at: Some("-2,1,-15"),
            azimuth: Some(-15.0),
            pitch: Some(30.0),
            distance: Some(12.0),
            fov: Some(45.0),
        };
        let expanded = expand(&path, &a, &[]).unwrap();
        assert_eq!(expanded.set.len(), 4);
        assert!(
            expanded.set[2].ends_with("{\"type\":\"perspective\",\"fov_y_deg\":45.0}"),
            "{}",
            expanded.set[2]
        );
        assert!(
            expanded.set[3].ends_with("focus_distance=12.0"),
            "{}",
            expanded.set[3]
        );
        let summary = expanded.summary.unwrap();
        assert!(
            summary.contains("look-at [-2.0,1.0,-15.0] azimuth -15 pitch 30 distance 12")
                && !summary.contains("scene camera"),
            "{summary}"
        );
    }

    #[test]
    fn a_scene_without_post_process_sets_no_focus() {
        let text = LAB.split("  - name: grade").next().unwrap();
        let expanded = expand(&scene("nopost", text), &args(Some("0,1,0")), &[]).unwrap();
        assert_eq!(expanded.set.len(), 2);
    }

    #[test]
    fn bad_input_is_l4008() {
        let path = scene("errors", LAB);
        let with = |f: fn(&mut FramingArgs)| {
            let mut a = args(Some("0,1,0"));
            f(&mut a);
            message(&expand(&path, &a, &[]).unwrap_err())
        };
        let mut lone = args(None);
        lone.azimuth = Some(10.0);
        assert!(message(&expand(&path, &lone, &[]).unwrap_err()).contains("add --look-at x,y,z"));
        assert!(message(&expand(&path, &args(Some("1,2")), &[]).unwrap_err()).contains("x,y,z"));
        assert!(message(&expand(&path, &args(Some("1,2,x")), &[]).unwrap_err()).contains("x,y,z"));
        assert!(with(|a| a.pitch = Some(91.0)).contains("-90..90"));
        assert!(with(|a| a.distance = Some(0.0)).contains("--distance"));
        assert!(with(|a| a.fov = Some(180.0)).contains("--fov"));
    }

    #[test]
    fn unusable_cameras_are_refused() {
        let none = scene("nocam", "format: 1\nseed: 0\nentities: []\n");
        assert!(
            message(&expand(&none, &args(Some("0,0,0")), &[]).unwrap_err()).contains("found none")
        );

        let two = scene(
            "twocam",
            &format!("{LAB}  - components:\n      Camera: {{}}\n"),
        );
        assert!(message(&expand(&two, &args(Some("0,0,0")), &[]).unwrap_err()).contains("found 2"));

        let level = scene(
            "level",
            &LAB.replace(
                "- -58.0
", "- 0.0
",
            ),
        );
        assert!(
            message(&expand(&level, &args(Some("0,0,0")), &[]).unwrap_err())
                .contains("pass --distance")
        );
        let mut explicit = args(Some("0,0,0"));
        explicit.distance = Some(5.0);
        assert!(expand(&level, &explicit, &[]).is_ok());

        let ortho = scene("ortho", &LAB.replace("perspective", "orthographic"));
        let mut fov = args(Some("0,1,0"));
        fov.fov = Some(40.0);
        assert!(message(&expand(&ortho, &fov, &[]).unwrap_err()).contains("orthographic"));
    }
}
