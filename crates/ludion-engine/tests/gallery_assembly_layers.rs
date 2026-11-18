//! The gallery humanoid and the `assembly-layers` scene (Neon Shroud request 0007 W2d): the committed
//! assembly and its five clips are canonical, the scene validates, and the two blend figures fire
//! `foot_l` and `foot_r` alternately once their events are registered, so the authored events sit
//! on the strides. The posed look is the `assembly-layers` golden's.

use std::path::{Path, PathBuf};

use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Animation, AnimationEvents, BlendClock, ClipEvent, Engine, Mesh, animation_events, blend_clocks,
};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;

const ASSEMBLY: &str = "voxels/humanoid.assembly.yaml";
const SCENE: &str = "assembly-layers.yaml";
const CLIPS: [&str; 5] = ["carry", "idle", "lean", "run", "walk"];

fn gallery() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(gallery().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn the_gallery_humanoid_and_its_clips_are_canonical() {
    let text = read(ASSEMBLY);
    let doc = ludion_scene::check_assembly(&text).expect("the humanoid checks");
    assert_eq!(ludion_scene::emit_assembly(&doc), text);
    for clip in CLIPS {
        let rel = format!("voxels/anims/humanoid-{clip}.anim.yaml");
        let text = read(&rel);
        let doc = ludion_scene::check_animation(&text).unwrap_or_else(|d| panic!("{rel}: {d:?}"));
        assert_eq!(ludion_scene::emit_animation(&doc), text, "{rel}");
    }
}

#[derive(Default)]
struct EventLog(Vec<ClipEvent>);

fn record(world: &mut World, _ctx: &SystemCtx) {
    let fired: Vec<ClipEvent> = world
        .resource::<AnimationEvents>()
        .map(|events| events.iter().cloned().collect())
        .unwrap_or_default();
    if world.resource::<EventLog>().is_none() {
        world.insert_resource(EventLog::default());
    }
    world
        .resource_mut::<EventLog>()
        .expect("just inserted")
        .0
        .extend(fired);
}

#[test]
fn the_scene_validates_and_its_blend_figures_step_on_alternating_feet() {
    let mut engine = Engine::builder()
        .project_root(gallery())
        .system("blend_clocks", blend_clocks)
        .query::<(&Animation, &Mesh)>()
        .query::<(&mut BlendClock,)>()
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .query::<(&BlendClock,)>()
        .system("record", record)
        .build()
        .expect("the game builds");
    engine
        .load_inline_scene(&read(SCENE))
        .expect("the scene validates");
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 300,
            hash: false,
        }))
        .expect("steps");

    let world = engine.world();
    let log = world.resource::<EventLog>().expect("the log exists");
    let mut figures = 0;
    for id in world.entities() {
        let Some(animation) = world.component::<Animation>(id) else {
            continue;
        };
        if animation.clip != "locomotion" {
            continue;
        }
        figures += 1;
        let names: Vec<&str> = log
            .0
            .iter()
            .filter(|e| e.entity == id && e.layer.is_empty() && e.clip == "locomotion")
            .map(|e| e.name.as_str())
            .collect();
        assert!(names.len() >= 4, "{names:?}");
        assert!(
            names
                .windows(2)
                .all(|pair| pair[0] != pair[1] && matches!(pair[0], "foot_l" | "foot_r")),
            "{names:?}"
        );
    }
    assert_eq!(figures, 2, "the scene has two blend figures");
}
