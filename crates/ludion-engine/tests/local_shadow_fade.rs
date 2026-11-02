//! A light past its `shadow_distance` is not a shadow candidate: it costs no atlas views, is not
//! counted as requested, and a scene whose casting lights are all past theirs schedules no
//! `local_shadow` pass. Both read the real extract, through the profiler's local-shadow panel and the
//! graph dump.

use std::path::{Path, PathBuf};

use ludion_engine::QualitySetting;
use ludion_renderer::{AntiAliasing, FrameSize};

const CAMERA: &str = "\
format: 1
name: local-shadow-fade
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 5.0
          - 12.0
        rotation:
          - -20.0
          - 0.0
          - 0.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 45.0
  - name: ground
    components:
      Transform:
        scale:
          - 22.0
          - 1.0
          - 22.0
      Mesh:
        shape: plane
";

/// A casting point light at `x`, with `shadow_distance` `distance` (footprint distance from the
/// camera is about 10 m for these lights).
fn light(name: &str, x: f32, distance: f32) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n          - {x}\n          - 2.0\n          - 0.0\n      \
         PointLight:\n        intensity: 6.0\n        range: 2.0\n        cast_shadows: true\n        \
         shadow_distance: {distance}\n        shadow_fade: 1.0\n"
    )
}

/// A per-test scratch directory, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("ludion-shadow-fade-{}-{test}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn scene(&self, name: &str, lights: &[(f32, f32)]) -> PathBuf {
        let mut text = CAMERA.to_string();
        for (i, (x, distance)) in lights.iter().enumerate() {
            text.push_str(&light(&format!("lamp{i}"), *x, *distance));
        }
        let path = self.0.join(format!("{name}.yaml"));
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn requested_and_views(path: &Path) -> (u64, u64, u64, u64) {
    let report = ludion_engine::profile_scene(path, 1, 0, None, None, QualitySetting::full())
        .expect("the scene profiles");
    let panel = report.local_shadows.expect("a local-shadow panel");
    (panel.requested, panel.granted, panel.skipped, panel.views)
}

fn passes(path: &Path) -> Vec<String> {
    let json = ludion_engine::dump_scene_graph_json(
        path,
        FrameSize::native(64, 64),
        &QualitySetting::full(),
        AntiAliasing::Off,
    )
    .expect("the scene schedules");
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value["schedule"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn lights_past_their_distance_are_not_requested_and_cost_no_views() {
    // Two lights inside a 40 m limit, three past a 5 m one.
    let scratch = Scratch::new("mixed");
    let path = scratch.scene(
        "mixed",
        &[
            (-3.0, 40.0),
            (3.0, 40.0),
            (-1.0, 5.0),
            (0.0, 5.0),
            (1.0, 5.0),
        ],
    );
    assert_eq!(requested_and_views(&path), (2, 2, 0, 12));
}

#[test]
fn a_scene_with_every_light_past_its_distance_schedules_no_local_shadow_pass() {
    let scratch = Scratch::new("past");
    let past = scratch.scene("past", &[(-1.0, 5.0), (1.0, 5.0)]);
    assert_eq!(requested_and_views(&past), (0, 0, 0, 0));
    assert!(!passes(&past).contains(&"local_shadow".to_string()));

    let inside = scratch.scene("inside", &[(-1.0, 40.0), (1.0, 5.0)]);
    assert!(passes(&inside).contains(&"local_shadow".to_string()));
}
