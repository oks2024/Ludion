//! `ReflectionProbe`: place probes, let the scene check catch a copy-paste, read the ranked order.
//!
//! A probe is a box or sphere volume that captures the scene around its entity for reflections.
//! `scene_engine_diagnostics` (what `ludion check-scene` and the wire's `scene.check` run) refuses
//! two probes with the same shape, size and pose (`L1277`) and warns about overlaps that only the
//! entity id would break (`L1275`) and probes buried in a higher-ranked one (`L1276`). The renderer
//! orders probes by `ludion_renderer::probe_rank`: higher `priority`, then boxes before spheres,
//! then the smaller volume, then entity id.
//!
//! Run it:
//!
//! ```text
//! cargo run -p ludion-engine --example reflection_probes
//! ```

use std::process::ExitCode;

use ludion_engine::{builtin_registry, extract, scene_engine_diagnostics};
use ludion_renderer::{ProbeVolume, probe_rank};

const CAMERA: &str = "\
format: 1
entities:
  - components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 5.0
      Camera:
        near: 0.1
        far: 100.0
";

/// A probe entity: a box of the given half-extent at `x`, or a sphere when `radius` is set.
fn probe(name: &str, x: f32, shape: &str) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n          - {x:?}\n          \
         - 0.0\n          - 0.0\n      ReflectionProbe:\n        shape:\n          {shape}\n"
    )
}

fn main() -> ExitCode {
    let room =
        "box:\n            extent:\n              - 4.0\n              - 2.0\n              - 3.0";
    let orb = "sphere:\n            radius: 1.5";

    // The second room was pasted and never moved.
    let pasted = format!(
        "{CAMERA}{}{}",
        probe("room", 0.0, room),
        probe("room copy", 0.0, room)
    );
    // Fixed: it moved next door, and a sphere probe covers the alcove beyond.
    let fixed = format!(
        "{CAMERA}{}{}{}",
        probe("room", 0.0, room),
        probe("next room", 9.0, room),
        probe("alcove", 14.0, orb)
    );

    let root =
        std::env::temp_dir().join(format!("ludion-reflection-probes-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create temp project root");
    let registry = builtin_registry();

    let scene = ludion_scene::check(&pasted, &registry).expect("pasted scene parses");
    let diagnostics = scene_engine_diagnostics(&root, &scene);
    for diagnostic in &diagnostics {
        println!("{} {}", diagnostic.code, diagnostic.message);
    }
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code.as_str(), "L1277");

    let scene = ludion_scene::check(&fixed, &registry).expect("fixed scene parses");
    let diagnostics = scene_engine_diagnostics(&root, &scene);
    println!("fixed scene: {} diagnostics", diagnostics.len());
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let world = ludion_scene::instantiate(&scene, &registry).expect("fixed scene instantiates");
    let mut probes = extract(&world, (320, 180), ludion_assets::AssetRoot::content(&root))
        .expect("extract")
        .probes;
    probes.sort_by(probe_rank);
    for probe in &probes {
        let kind = match probe.volume {
            ProbeVolume::Box { .. } => "box",
            ProbeVolume::Sphere { .. } => "sphere",
        };
        println!(
            "{} {kind} at x={} size {:.1}",
            probe.entity,
            probe.position.x,
            probe.size()
        );
    }
    // Equal priority: boxes first, the smaller box ahead of the larger, and the sphere last.
    let kinds: Vec<bool> = probes.iter().map(|p| p.volume.is_sphere()).collect();
    assert_eq!(kinds, [false, false, true]);
    assert_eq!(probes[0].position.x, 0.0);

    std::fs::remove_dir_all(&root).ok();
    ExitCode::SUCCESS
}
