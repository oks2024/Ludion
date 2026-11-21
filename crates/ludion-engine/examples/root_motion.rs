//! Root motion on a voxel assembly (`Plan/animation-layers.md` decisions 55-60): a one-shot `lunge`
//! clip carries a `$root` track (eight units forward over one second), a game system reads
//! [`root_motion_delta`] each tick and applies it to `Transform`, and the example prints the per-tick
//! delta and the final position.
//!
//! The engine never applies root motion: the game chooses what to do with the delta (a motor's
//! `move_character` for a character with collision, as here a plain `Transform` write).
//!
//! The fixture project is written to a temp directory so the example is self-contained.
//!
//! ```text
//! cargo run -p ludion-engine --example root_motion
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Animation, Engine, Mesh, Transform, root_motion_delta};

/// The game's own system: reads this tick's delta and moves the entity by it.
fn apply_root_motion(world: &mut World, ctx: &SystemCtx) {
    let entities: Vec<_> = world
        .query_ids::<(&Animation, &Mesh, &Transform)>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in entities {
        let Some(delta) = root_motion_delta(&*world, entity, ctx) else {
            continue;
        };
        if delta.translation != [0.0; 3] {
            println!(
                "frame {:>3}: delta {:?} weight {}",
                ctx.game_frame.as_u64(),
                delta.translation,
                delta.weight
            );
        }
        if let Ok(Some(transform)) = world.get_mut::<Transform>(entity) {
            for axis in 0..3 {
                transform.translation[axis] += delta.translation[axis];
            }
            transform.rotation[1] += delta.yaw.to_degrees();
        }
    }
}

const ASSEMBLY: &str = "\
format: 1
kind: assembly
name: body
size:
  - 1
  - 1
  - 1
palette:
  A:
    color:
      - 0.8
      - 0.5
      - 0.2
      - 1.0
parts:
  - name: body
    file: parts/cell.part.yaml
    place:
      - 0
      - 0
      - 0
clips:
  lunge: anims/lunge.anim.yaml
";

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

/// The `$root` track is quoted by the canonical writer; it is never drawn, so the `body` track is
/// only there to give the clip something to pose.
const LUNGE: &str = "\
format: 1
kind: animation
name: lunge
duration: 1.0
loop: false
tracks:
  \"$root\":
    translation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 0.0
          - 0.0
          - -8.0
  body:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 0.0
          - 20.0
          - 0.0
";

const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: lunger
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Mesh:
        shape:
          assembly: models/body.assembly.yaml
      Animation:
        clip: lunge
";

fn main() -> ExitCode {
    let root = match fixture_project() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("could not write the fixture project: {error}");
            return ExitCode::FAILURE;
        }
    };
    let built = Engine::builder()
        .project_root(&root)
        .system("apply_root_motion", apply_root_motion)
        .query::<(&Animation, &Mesh, &mut Transform)>()
        .build();
    let mut engine = match built {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("{error:?}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    for _ in 0..90 {
        engine.advance_live();
    }
    let world = engine.world();
    let position = world
        .entities()
        .find_map(|id| world.component::<Transform>(id).map(|t| t.translation));
    println!("final position: {position:?}");
    let _ = std::fs::remove_dir_all(&root);
    ExitCode::SUCCESS
}

fn fixture_project() -> std::io::Result<PathBuf> {
    let root =
        std::env::temp_dir().join(format!("ludion-root-motion-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "models/body.assembly.yaml", ASSEMBLY)?;
    write(&root, "models/parts/cell.part.yaml", CELL)?;
    write(&root, "models/anims/lunge.anim.yaml", LUNGE)?;
    Ok(root)
}

fn write(root: &Path, rel: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}
