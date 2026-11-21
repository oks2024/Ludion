//! A two-role paired action, end to end: a guard and a player (both `CharacterMotor`s drawing the
//! gallery humanoid) are fit-tested, started, warped onto the choke's aligned pose and played to
//! the clip end. Every `paired:choke` event is printed with its tick, then the final relative pose.
//!
//! The game's three systems are registered in order: the driver (start, then `paired_motion` feeding
//! the motor), `animation_events`, and `paired_update`, which must come after `animation_events` and
//! before whatever reads `AnimationEvents`.
//!
//! The gallery's `voxels/` directory is copied to a temp directory so the run is self-contained.
//!
//! ```text
//! cargo run -p ludion-engine --example paired_action
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::components::{Name, PairedAction, Transform};
use ludion_engine::{
    AnimationEvents, Engine, Warp, animation_events, move_character, paired_fit, paired_motion,
    paired_update, start_paired, turn_character,
};

const DOC: &str = "voxels/choke.paired.yaml";
const START: u64 = 5;

const SCENE: &str = "\
format: 1
seed: 5
physics:
  space: free
entities:
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -0.5
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 20.0
              - 0.5
              - 20.0
        body: static
  - name: player
    components:
      Transform:
        translation:
          - 3.4
          - 0.0
          - 1.5
        rotation:
          - 0.0
          - 160.0
          - 0.0
        scale:
          - 0.125
          - 0.125
          - 0.125
      Mesh:
        shape:
          assembly: voxels/humanoid.assembly.yaml
      CharacterMotor:
        radius: 0.35
        height: 1.7
  - name: guard
    components:
      Transform:
        translation:
          - 3.0
          - 0.0
          - 2.0
        scale:
          - 0.125
          - 0.125
          - 0.125
      Mesh:
        shape:
          assembly: voxels/humanoid.assembly.yaml
      CharacterMotor:
        radius: 0.35
        height: 1.7
";

fn by_name(world: &World, name: &str) -> Option<EntityId> {
    world.entities().find(|id| {
        world
            .get_unrecorded::<Name>(*id)
            .ok()
            .flatten()
            .is_some_and(|n| n.0 == name)
    })
}

/// The game's driver: starts the action on tick `START` if it fits, then feeds each participant's
/// warp step to the motor every tick.
fn driver(world: &mut World, ctx: &SystemCtx) {
    let (Some(player), Some(guard)) = (by_name(world, "player"), by_name(world, "guard")) else {
        return;
    };
    if ctx.frame.as_u64() == START {
        let cast = [("instigator", player), ("target", guard)];
        let warp = Warp {
            distance: 0.6,
            yaw_deg: 45.0,
        };
        match paired_fit(world, DOC, &cast, warp) {
            Ok(fit) if fit.fits => {
                start_paired(world, DOC, &cast, 0.1, ctx.game_instant)
                    .expect("a fitting cast starts");
                println!("tick {}: started", ctx.frame.as_u64());
            }
            other => println!("tick {}: does not fit: {other:?}", ctx.frame.as_u64()),
        }
    }
    for entity in [player, guard] {
        if let Some(step) = paired_motion(world, entity, ctx) {
            let _ = move_character(world, entity, step.displacement);
            let _ = turn_character(world, entity, step.yaw);
        }
    }
}

/// Prints what `paired_update` appended this tick.
fn printer(world: &mut World, ctx: &SystemCtx) {
    let Some(events) = world.resource::<AnimationEvents>() else {
        return;
    };
    for event in events
        .iter()
        .filter(|event| event.clip.starts_with("paired:"))
    {
        println!(
            "tick {}: {} on {:?}: {}",
            ctx.frame.as_u64(),
            event.clip,
            event.entity,
            event.name
        );
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/voxels");
    let root: PathBuf =
        std::env::temp_dir().join(format!("ludion-paired-action-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    if let Err(error) = copy_dir(&gallery, &root.join("voxels")) {
        eprintln!("cannot copy the gallery voxels: {error}");
        return ExitCode::FAILURE;
    }
    let mut engine = Engine::builder()
        .project_root(&root)
        .system("driver", driver)
        .system("animation_events", animation_events)
        .system("paired_update", paired_update)
        .system("printer", printer)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");

    let mut ended = false;
    for _ in 0..(START + 90) {
        engine.advance_live();
        let world = engine.world();
        let player = by_name(world, "player").expect("player");
        if engine.frame().as_u64() > START + 1 && !world.has::<PairedAction>(player) {
            ended = true;
            break;
        }
    }
    let world = engine.world();
    let (player, guard) = (
        by_name(world, "player").unwrap(),
        by_name(world, "guard").unwrap(),
    );
    let feet = |id| world.get::<Transform>(id).unwrap().unwrap().translation;
    let (p, g) = (feet(player), feet(guard));
    println!(
        "final: the player is {:.3} m {:.3} m from the guard (dx, dz); action ended: {ended}",
        p[0] - g[0],
        p[2] - g[2]
    );
    let _ = std::fs::remove_dir_all(&root);
    if ended {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
