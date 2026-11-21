//! An assembly's arm that follows an entity: an `ik:` chain aimed by an `IkTargets` target that names
//! a `ball` by `entity`, with no per-tick write.
//!
//! - the `ik:` key on the assembly names a two-part chain (`hand`) and its effector;
//! - `IkTargets.targets.hand.entity` is an entity reference, so a scene save and a carry remap it, and
//!   a despawned ball nulls it (the hand then poses as the clip, silently);
//! - extract reads the ball's world position each frame, so moving the ball's `Transform` is all a game
//!   does.
//!
//! The fixture project is written to a temp directory so the example is self-contained. The example
//! moves the ball, prints the hand and ball positions per tick and exits non-zero if the hand is not
//! within `1e-3` of the ball.
//!
//! ```text
//! cargo run -p ludion-engine --example ik_entity_target
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ludion_core::ids::EntityId;
use ludion_core::math::glam::Vec3;
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, Transform, extract_at};

// --- The content (copyable) ----------------------------------------------------------------------

/// A torso with a two-part arm hanging down `-Y`; the `hand` chain reaches with the lower part's origin
/// plus the `effector` offset.
const ASSEMBLY: &str = "\
format: 1
kind: assembly
name: reacher
size:
  - 3
  - 5
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: torso
    file: parts/cell.part.yaml
    place:
      - 1
      - 3
      - 0
  - name: upper-arm-r
    file: parts/cell.part.yaml
    parent: torso
    place:
      - 1
      - 2
      - 0
    pivot:
      - 0.5
      - 0.5
      - 0.5
  - name: lower-arm-r
    file: parts/cell.part.yaml
    parent: upper-arm-r
    place:
      - 1
      - 1
      - 0
    pivot:
      - 0.5
      - 0.5
      - 0.5
ik:
  hand:
    parts:
      - upper-arm-r
      - lower-arm-r
    effector:
      - 0.5
      - 0.0
      - 0.5
";

/// A single-cell part.
const CELL: &str = "\
format: 1
kind: part
size:
  - 1
  - 1
  - 1
slices:
  -
    - \"A\"
";

/// The scene: the reacher at the origin, and a `ball` (entity index 1) its hand follows. The target
/// names the ball by `index`; the scene loader remaps it to the ball's real id.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: reacher
    components:
      Transform: {}
      Mesh:
        shape:
          assembly: models/reacher.assembly.yaml
      IkTargets:
        targets:
          hand:
            entity:
              index: 1
              generation: 0
            weight: 1.0
  - name: ball
    components:
      Transform:
        translation:
          - 0.6
          - 1.9
          - 0.9
";

// --- Headless demo `main` ------------------------------------------------------------------------

/// The ball's positions, one per printed tick, all inside the arm's reach of 1.5 from its shoulder.
const BALL: [[f32; 3]; 4] = [
    [0.6, 1.9, 0.9],
    [0.8, 1.9, 0.9],
    [0.6, 2.1, 0.9],
    [0.4, 1.7, 0.8],
];

/// The lower arm part's mesh: the third part of the first entity.
const LOWER_ARM: usize = 2;

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

    let ball = EntityId::new(1, 0);
    let mut failed = false;
    println!("tick   ball                         hand");
    for (tick, position) in BALL.iter().enumerate() {
        // The only per-tick write is the ball's own motion; `IkTargets` is never touched.
        if let Ok(Some(transform)) = engine.world_mut().get_mut::<Transform>(ball) {
            transform.translation = *position;
        }
        let scene = match extract_at(
            engine.world(),
            (320, 180),
            ludion_assets::AssetRoot::content(engine.project_root()),
            SimTime::from_ticks(tick as u64, TickRate::DEFAULT),
            QualityParams::FULL.particle_density,
        ) {
            Ok(scene) => scene,
            Err(diagnostics) => {
                for diagnostic in diagnostics {
                    eprintln!("{diagnostic}");
                }
                return ExitCode::FAILURE;
            }
        };
        let hand = scene.meshes[LOWER_ARM].model.transform_point3(Vec3::ZERO);
        let ball_at = Vec3::from(*position);
        println!(
            "{tick:>4}   ({:>5.2}, {:>5.2}, {:>5.2})      ({:>5.2}, {:>5.2}, {:>5.2})",
            ball_at.x, ball_at.y, ball_at.z, hand.x, hand.y, hand.z
        );
        failed |= (hand - ball_at).length() > 1e-3;
    }

    let _ = std::fs::remove_dir_all(&root);
    if failed {
        eprintln!("the hand did not reach the ball");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Writes the content files into a fresh temp directory and returns its path.
fn fixture_project() -> std::io::Result<PathBuf> {
    let root = std::env::temp_dir().join(format!("ludion-ik-entity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "models/reacher.assembly.yaml", ASSEMBLY)?;
    write(&root, "models/parts/cell.part.yaml", CELL)?;
    Ok(root)
}

fn write(root: &Path, rel: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}
