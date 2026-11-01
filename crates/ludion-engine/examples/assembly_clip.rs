//! An animated multi-part voxel model, end to end: a `kind: assembly` with two parts, a
//! `kind: animation` clip bound under the short name `walk`, an entity carrying `Mesh.shape:
//! assembly` plus [`Animation`], and the posed part transforms printed at four instants of the clip.
//!
//! This is the whole runtime shape `Plan/voxel-animation.md` decisions 5-8 describe:
//!
//! - one entity, one `Transform`, one `Mesh` — the parts are **not** entities and nothing sim-side
//!   can see them;
//! - a game system picks the clip by writing `Animation { clip, started, speed }` (here the scene
//!   authors it, which is the same write);
//! - the pose is derived at the render boundary from `(clip, sim time)` alone, so the printed
//!   matrices are a pure function of the tick — pausing freezes them, and a replay reproduces them.
//!
//! The fixture project is written to a temp directory so the example is self-contained; in a real
//! game these four files live in the project tree and the model is authored with
//! `ludion render --model`.
//!
//! Run it headless (no window, no GPU, deterministic):
//!
//! ```text
//! cargo run -p ludion-engine --example assembly_clip
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ludion_core::time::{SimTime, TickRate};
use ludion_engine::{Engine, QualityParams, extract_at};

// --- The content (copyable) ----------------------------------------------------------------------

/// The model of record: one palette, two placed parts, one clip binding. `arm` hangs off `body` and
/// turns about its own `pivot`, expressed in the part's own voxel coordinates.
const ASSEMBLY: &str = "\
format: 1
kind: assembly
name: waver
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
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
  - name: arm
    file: parts/arm.part.yaml
    parent: body
    place:
      - 2
      - 2
      - 0
    pivot:
      - 0.5
      - 0.5
      - 0.5
clips:
  wave: anims/wave.anim.yaml
";

/// A 2x2x1 torso.
const BODY: &str = "\
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

/// The clip: the arm swings −60° → +60° about +Z and back over one looping second, easing at the
/// extremes. Keyframes are block style — the strict-YAML subset rejects flow collections.
const CLIP: &str = "\
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
          - -60.0
      - t: 0.5
        v:
          - 0.0
          - 0.0
          - 60.0
        ease: smooth
      - t: 1.0
        v:
          - 0.0
          - 0.0
          - -60.0
        ease: smooth
";

/// The scene: one entity playing `wave` from frame 0, and one un-animated copy standing beside it as
/// the visible rest-pose reference.
const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: waver
    components:
      Transform:
        translation:
          - -3.0
          - 0.0
          - 0.0
      Mesh:
        shape:
          assembly: models/waver.assembly.yaml
      Animation:
        clip: wave
        started: 0
        speed: 1.0
  - name: statue
    components:
      Transform:
        translation:
          - 3.0
          - 0.0
          - 0.0
      Mesh:
        shape:
          assembly: models/waver.assembly.yaml
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

    println!("tick   arm origin (animated)        arm origin (rest)");
    for tick in [0, 15, 30, 45] {
        // `extract_at` is the render boundary: the world is not touched, only read. A host passes
        // its driver's own `SimTime`; nothing here consults a clock.
        let scene = match extract_at(
            engine.world(),
            (320, 180),
            ludion_assets::AssetRoot::content(engine.project_root()),
            SimTime::from_ticks(tick, TickRate::DEFAULT),
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
        // Ascending entity order, then the assembly's authored part order: body, arm, body, arm.
        let animated = scene.meshes[1].model.w_axis;
        let rest = scene.meshes[3].model.w_axis;
        println!(
            "{tick:>4}   ({:>6.3}, {:>6.3}, {:>6.3})      ({:>6.3}, {:>6.3}, {:>6.3})",
            animated.x, animated.y, animated.z, rest.x, rest.y, rest.z
        );
    }

    let _ = std::fs::remove_dir_all(&root);
    ExitCode::SUCCESS
}

/// Writes the four content files into a fresh temp directory and returns its path.
fn fixture_project() -> std::io::Result<PathBuf> {
    let root = std::env::temp_dir().join(format!("ludion-assembly-clip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "models/waver.assembly.yaml", ASSEMBLY)?;
    write(&root, "models/parts/body.part.yaml", BODY)?;
    write(&root, "models/parts/arm.part.yaml", ARM)?;
    write(&root, "models/anims/wave.anim.yaml", CLIP)?;
    Ok(root)
}

fn write(root: &Path, rel: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}
