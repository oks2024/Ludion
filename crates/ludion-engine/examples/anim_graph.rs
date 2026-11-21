//! A voxel assembly driven by a Rust `AnimGraph` (`Plan/animation-layers.md` decisions 50-53): three
//! states (`idle`, `carry` with a `carry` layer, a one-shot `wave`), a game system that calls
//! [`AnimGraph::step`] each tick after `animation_events`, and a printout of every transition plus
//! [`AnimGraph::describe`].
//!
//! The graph is a `static` table of states and edges with `fn` conditions: Rust, not an asset. The entity
//! starts in `idle` because the scene spawns `AnimGraphState` beside an `Animation` already playing
//! `idle`; `step` writes `Animation`, `AnimationLayers` and `AnimGraphState` only when an edge fires.
//!
//! The fixture project is written to a temp directory so the example is self-contained.
//!
//! ```text
//! cargo run -p ludion-engine --example anim_graph
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    AnimEdge, AnimGate, AnimGraph, AnimGraphState, AnimState, Animation, AnimationLayers, Engine,
    Mesh, animation_events,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pose {
    Idle,
    Carry,
    Wave,
}

/// What the game wants this tick; a real game reads input, a pickup or a trigger here.
struct Inputs {
    carrying: bool,
    wave: bool,
}

fn inputs_at(frame: u64) -> Inputs {
    Inputs {
        carrying: (10..40).contains(&frame),
        wave: frame == 50,
    }
}

static GRAPH: AnimGraph<Pose, Inputs> = AnimGraph {
    states: &[
        AnimState {
            id: Pose::Idle,
            name: "idle",
            plays: "idle",
            layers: &[],
            one_shot: false,
        },
        AnimState {
            id: Pose::Carry,
            name: "carry",
            plays: "idle",
            layers: &[("carry", "carry", 1.0)],
            one_shot: false,
        },
        AnimState {
            id: Pose::Wave,
            name: "wave",
            plays: "wave",
            layers: &[],
            one_shot: true,
        },
    ],
    edges: &[
        // Any state, even a carry: wave cuts in. A one-shot only leaves early through `Interrupts`.
        AnimEdge {
            from: None,
            to: Pose::Wave,
            when: |i| i.wave,
            fade: 0.0,
            gate: AnimGate::Interrupts,
        },
        AnimEdge {
            from: Some(Pose::Idle),
            to: Pose::Carry,
            when: |i| i.carrying,
            fade: 0.2,
            gate: AnimGate::None,
        },
        AnimEdge {
            from: Some(Pose::Carry),
            to: Pose::Idle,
            when: |i| !i.carrying,
            fade: 0.2,
            gate: AnimGate::None,
        },
        AnimEdge {
            from: Some(Pose::Wave),
            to: Pose::Idle,
            when: |_| true,
            fade: 0.15,
            gate: AnimGate::OnEvent("end"),
        },
    ],
};

/// The game's own system, registered after `animation_events` so `OnEvent("end")` reads this tick's events.
fn graph_system(world: &mut World, ctx: &SystemCtx) {
    let entities: Vec<_> = world
        .query_ids::<(&AnimGraphState,)>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in entities {
        match GRAPH.step(world, entity, &inputs_at(ctx.game_frame.as_u64()), ctx) {
            Ok(Some(to)) => println!("frame {:>3}: entered {to:?}", ctx.game_frame.as_u64()),
            Ok(None) => {}
            Err(diagnostic) => eprintln!("{diagnostic}"),
        }
    }
}

// --- The content (copyable) ----------------------------------------------------------------------

const ASSEMBLY: &str = "\
format: 1
kind: assembly
name: humanoid
size:
  - 3
  - 3
  - 1
palette:
  A:
    color:
      - 0.8
      - 0.5
      - 0.2
      - 1.0
parts:
  - name: torso
    file: parts/cell.part.yaml
    place:
      - 1
      - 1
      - 0
  - name: head
    file: parts/cell.part.yaml
    parent: torso
    place:
      - 1
      - 2
      - 0
  - name: arm
    file: parts/cell.part.yaml
    parent: torso
    place:
      - 2
      - 1
      - 0
clips:
  idle: anims/idle.anim.yaml
  wave: anims/wave.anim.yaml
  carry: anims/carry.anim.yaml
masks:
  arm_only:
    joints:
      - arm
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

/// A one-second clip turning `target` about one axis; `looping` false makes it a one-shot with an `end`.
fn clip(name: &str, looping: bool, target: &str, to: [f64; 3]) -> String {
    let end: String = to.iter().map(|v| format!("          - {v:?}\n")).collect();
    format!(
        "format: 1\nkind: animation\nname: {name}\nduration: 1.0\nloop: {looping}\ntracks:\n  {target}:\n    rotation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n      - t: 1.0\n        v:\n{end}"
    )
}

const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: humanoid
    components:
      Mesh:
        shape:
          assembly: models/humanoid.assembly.yaml
      Animation:
        clip: idle
      AnimationLayers:
        layers:
          - name: carry
            mask: arm_only
      AnimGraphState:
        state: idle
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
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .system("graph_system", graph_system)
        .query::<(&mut Animation,)>()
        .query::<(&mut AnimationLayers,)>()
        .query::<(&mut AnimGraphState,)>()
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

    println!("{}", GRAPH.describe());
    for _ in 0..130 {
        engine.advance_live();
    }
    let _ = std::fs::remove_dir_all(&root);
    ExitCode::SUCCESS
}

fn fixture_project() -> std::io::Result<PathBuf> {
    let root =
        std::env::temp_dir().join(format!("ludion-anim-graph-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "models/humanoid.assembly.yaml", ASSEMBLY)?;
    write(&root, "models/parts/cell.part.yaml", CELL)?;
    write(
        &root,
        "models/anims/idle.anim.yaml",
        &clip("idle", true, "head", [0.0, 10.0, 0.0]),
    )?;
    write(
        &root,
        "models/anims/wave.anim.yaml",
        &clip("wave", false, "arm", [0.0, 0.0, 90.0]),
    )?;
    write(
        &root,
        "models/anims/carry.anim.yaml",
        &clip("carry", true, "arm", [90.0, 0.0, 0.0]),
    )?;
    Ok(root)
}

fn write(root: &Path, rel: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}
