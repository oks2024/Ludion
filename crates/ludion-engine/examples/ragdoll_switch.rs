//! The ragdoll switch, end to end: a two-part figure (`torso` + `arm`, jointed by a `hinge`) plays a
//! `wave` clip for 30 ticks, then `switch_to_ragdoll` promotes it into two rigid bodies joined by a
//! `Joint`, and the figure falls under the scene's `physics: {space: free}` gravity for another 120
//! ticks — the part heights printed along the way.
//!
//! With `--limits-from-rest` the switch is `switch_to_ragdoll_with` and `LimitsFrom::Rest`: each
//! joint also carries a `JointZero`, so its limits and any motor target count from the rig's rest
//! pose rather than the pose the puppet was in at the switch. The arm hinge angle is printed at the
//! end of the run. With `--part-velocity` the switch also adds the drawn pose's own motion to each
//! part's `Velocity` (`RagdollSwitch::part_velocity`), and the arm's spawn velocity is printed.
//! With `--recover` the fallen body is folded back into the puppet at the end of the run
//! (`ragdoll_landing`, then `switch_ragdoll_to_assembly`, crossfading into `wave` from the fallen
//! pose) and the landing is printed.
//!
//! The switch is ordinary game code: a real game calls `switch_to_ragdoll` from a system, at
//! `ctx.game_time` (`SystemCtx::game_time`) — a death blow, a stagger, a rope grab. This headless
//! example drives the engine directly (no window, no GPU) and stands in for that system with the
//! engine's own frame clock.
//!
//! The fixture project is written to a temp directory so the example is self-contained; in a real
//! game these four files live in the project tree.
//!
//! Run it headless (no window, no GPU, deterministic):
//!
//! ```text
//! cargo run -p ludion-engine --example ragdoll_switch
//! cargo run -p ludion-engine --example ragdoll_switch -- --limits-from-rest
//! cargo run -p ludion-engine --example ragdoll_switch -- --part-velocity --limits-from-rest
//! cargo run -p ludion-engine --example ragdoll_switch -- --recover --limits-from-rest
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ludion_core::ids::EntityId;
use ludion_core::math::euler_xyz_degrees_to_quat;
use ludion_core::math::glam::Quat;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{
    Engine, LimitsFrom, RagdollSwitch, Transform, load_assembly, ragdoll_landing,
    switch_ragdoll_to_assembly, switch_to_ragdoll, switch_to_ragdoll_with,
};
use ludion_spatial::Velocity;

// --- The content (copyable) ----------------------------------------------------------------------

/// The model of record: a torso and one hinged arm, exactly the shape `Plan/physics.md` W3's
/// ragdoll-authoring plan describes — parts, a `joints:` binding, a clip.
const ASSEMBLY: &str = "\
format: 1
kind: assembly
name: puppet
size:
  - 4
  - 3
  - 1
palette:
  S:
    color:
      - 0.30
      - 0.22
      - 0.18
      - 1.0
parts:
  - name: torso
    file: parts/torso.part.yaml
    place:
      - 0
      - 0
      - 0
  - name: arm
    file: parts/arm.part.yaml
    parent: torso
    place:
      - 2
      - 2
      - 0
    pivot:
      - 0.5
      - 0.5
      - 0.5
joints:
  arm:
    kind: hinge
    axis:
      - 0.0
      - 0.0
      - 1.0
    limits:
      - -60.0
      - 60.0
clips:
  wave: anims/wave.anim.yaml
";

/// A 2x2x1 torso.
const TORSO: &str = "\
format: 1
kind: part
size:
  - 2
  - 2
  - 1
slices:
  -
    - \"SS\"
  -
    - \"SS\"
";

/// A single-cell arm.
const ARM: &str = "\
format: 1
kind: part
size:
  - 1
  - 1
  - 1
slices:
  -
    - \"S\"
";

/// The arm swings −45° → +45° about +Z and back over one looping second.
const WAVE_CLIP: &str = "\
format: 1
kind: animation
name: wave
duration: 1.0
loop: true
tracks:
  arm:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - -45.0
      - t: 0.5
        v:
          - 0.0
          - 0.0
          - 45.0
        ease: smooth
      - t: 1.0
        v:
          - 0.0
          - 0.0
          - -45.0
        ease: smooth
";

/// A `physics: {space: free}` scene: a static floor well out of the way (this example is about the
/// switch and the fall's first couple of seconds, not a landing) and the puppet, starting mid-clip.
const SCENE: &str = "\
format: 1
seed: 1
physics:
  space: free
entities:
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -1000.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 50.0
              - 0.5
              - 50.0
        body: static
  - name: puppet
    components:
      Transform:
        translation:
          - 0.0
          - 5.0
          - 0.0
      Mesh:
        shape:
          assembly: models/puppet.assembly.yaml
      Animation:
        clip: wave
        started: 0
        speed: 1.0
";

// --- Headless demo `main` ------------------------------------------------------------------------

fn main() -> ExitCode {
    let root = match fixture_project() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("could not write the fixture project: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut engine = Engine::with_project_root(&root);
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }

    // Let the clip play for 30 ticks — an ordinary "wind up, then die" beat — before the switch.
    for _ in 0..30 {
        engine.advance_live();
    }

    let data = match load_assembly(
        ludion_assets::AssetRoot::content(engine.project_root()),
        "models/puppet.assembly.yaml",
    ) {
        Ok(data) => data,
        Err(diagnostics) => {
            for diagnostic in diagnostics {
                eprintln!("{diagnostic}");
            }
            return ExitCode::FAILURE;
        }
    };

    // `floor` is document position 0, `puppet` is 1 — a freshly loaded scene assigns entity ids by
    // document order at generation 0.
    let puppet = EntityId::new(1, 0);
    let now = SimTime::from_ticks(engine.frame().0, TickRate::DEFAULT);
    let from_rest = std::env::args().any(|arg| arg == "--limits-from-rest");
    let part_velocity = std::env::args().any(|arg| arg == "--part-velocity");
    let recover = std::env::args().any(|arg| arg == "--recover");
    let switched = if from_rest || part_velocity {
        let mut opts = RagdollSwitch::default();
        if from_rest {
            opts.limits_from = LimitsFrom::Rest;
        }
        opts.part_velocity = part_velocity;
        switch_to_ragdoll_with(engine.world_mut(), puppet, &data, now, opts)
    } else {
        switch_to_ragdoll(engine.world_mut(), puppet, &data, now)
    };
    let spawned = match switched {
        Ok(spawned) => spawned,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "switched at tick {}: {} rigid parts ({})",
        engine.frame().0,
        spawned.len(),
        data.parts
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    if part_velocity {
        let velocity = engine.world().get::<Velocity>(spawned[1]).unwrap().unwrap();
        println!(
            "arm spawn velocity: linear {:?}, angular {:?}",
            velocity.linear, velocity.angular
        );
    }

    println!("ticks after switch   torso.y   arm.y");
    let switch_tick = engine.frame().0;
    for elapsed in [0, 30, 60, 90, 120] {
        while engine.frame().0 < switch_tick + elapsed {
            engine.advance_live();
        }
        let torso = engine
            .world()
            .get::<Transform>(spawned[0])
            .unwrap()
            .unwrap();
        let arm = engine
            .world()
            .get::<Transform>(spawned[1])
            .unwrap()
            .unwrap();
        println!(
            "{elapsed:>19}   {:>7.3}   {:>7.3}",
            torso.translation[1], arm.translation[1]
        );
    }

    if from_rest {
        let rotation = |id| {
            let transform = engine.world().get::<Transform>(id).unwrap().unwrap();
            Quat::from_array(euler_xyz_degrees_to_quat(transform.rotation))
        };
        let relative = rotation(spawned[0]).inverse() * rotation(spawned[1]);
        let relative = if relative.w < 0.0 {
            -relative
        } else {
            relative
        };
        let angle = 2.0 * ludion_core::math::atan2(relative.z, relative.w).to_degrees();
        println!("arm hinge angle from rest: {angle:.1} degrees");
    }

    if recover {
        // Fold the fallen body back into the puppet: ask where it lies first (a game picks its
        // get-up clip from `face_up`), then switch, crossfading into `wave` from the fallen pose.
        let now = SimTime::from_ticks(engine.frame().0, TickRate::DEFAULT);
        let landing = match ragdoll_landing(engine.world(), puppet, &spawned, &data) {
            Ok(landing) => landing,
            Err(diagnostic) => {
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
        };
        let folded = switch_ragdoll_to_assembly(
            engine.world_mut(),
            puppet,
            &spawned,
            &data,
            "wave",
            0.25,
            now,
        );
        if let Err(diagnostic) = folded {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
        println!(
            "folded back: face_up {}, root_yaw {:.3} rad, feet {:?}",
            landing.face_up, landing.root_yaw, landing.feet
        );
    }

    let _ = std::fs::remove_dir_all(&root);
    ExitCode::SUCCESS
}

/// Writes the four content files into a fresh temp directory and returns its path.
fn fixture_project() -> std::io::Result<PathBuf> {
    let root = std::env::temp_dir().join(format!("ludion-ragdoll-switch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "models/puppet.assembly.yaml", ASSEMBLY)?;
    write(&root, "models/parts/torso.part.yaml", TORSO)?;
    write(&root, "models/parts/arm.part.yaml", ARM)?;
    write(&root, "models/anims/wave.anim.yaml", WAVE_CLIP)?;
    Ok(root)
}

fn write(root: &Path, rel: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}
