//! Phase 6 exit-criterion integration test — "Hands", the scripted-designer half.
//!
//! Phase 6 gives a level designer edit tools while keeping every agent guarantee intact. Its exit
//! gate (`Plan/phase-6-editing.md`, Wave 6) is a *scripted designer session*: over the wire, exactly
//! as the editor would, a real edit session is performed and three properties are asserted. This file
//! mechanizes that gate. There is one test — [`scripted_designer_session_meets_the_phase_6_exit_criteria`]
//! — because the three properties are three facts about one shared session, not three separate ones;
//! it drives the session once and checks all three:
//!
//! - **(a) Minimal diff.** The saved scene differs from the source by *exactly* the spawned entity's
//!   block — the drag was undone, the destroy was undone, and every other byte (including the
//!   respawned entity back at its original document position) is identical. Pillar 4's "any client's
//!   edit produces a minimal git diff", mechanized.
//! - **(b) Replay identity.** The recorded session replays to the identical per-frame `WorldHash`
//!   trace (the Phase 3/4 pattern: [`replay_hashes`] of the flushed recording equals
//!   [`drive_hashes`] of the same command list run in-process, and its final hash equals the live
//!   `sim.hash`).
//! - **(c) Byte-identical undo.** Undo of the destroy restored every component value byte-for-byte
//!   (`component.get` after respawn equals the captured pre-destroy values); undo of the drag
//!   restored the `Transform` to its exact value-at-mousedown.
//!
//! This follows the Phase 3/4 suites' conventions (`exit_criteria.rs`, `phase4.rs`): bind port 0 and
//! tear down at scope end; block on replies rather than sleep as a synchronization primitive; arm
//! recording host-side *before* the engine moves onto the sim thread; and pause *before serving* so
//! the paused engine advances only on explicit `sim.step` (the recorded trace is then exactly the
//! commands sent, with no wall-clock free-run ticks slipping in).
//!
//! **No GPU is needed.** The whole session is CPU-only: picking rides `render.bounds` (world AABB +
//! screen rect from extract-side projection math, no render), and nothing screenshots. So there is no
//! graceful-skip path here — the criterion is exercised on every host.
//!
//! # The source scene and the "minimal diff" baseline
//!
//! The session loads the committed gallery `sprites.yaml`. That hand-authored file carries a comment
//! header, which the canonical writer intentionally drops (comments are not part of a scene's one
//! canonical text), and lists components in abbreviated form that the first save materializes to the
//! full canonical form. So the honest baseline for a minimal-diff comparison is the scene's
//! *canonical form* — which is exactly what a designer's editor round-trips and re-saves. We derive
//! it once by a pristine load→save and write it to `source.yaml`; the session's edited output is
//! compared against that. This is faithful to Pillar 4: the gate is "the edit changed only what the
//! designer did", measured against the canonical text an editor actually starts from.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ludion_core::ids::EntityId;
use ludion_engine::{Engine, dispatch, drive_hashes, replay_hashes};
use ludion_protocol::command::{
    ComponentGetParams, ComponentSetParams, EntityDestroyParams, EntitySpawnParams,
    QueryEntitiesParams, RenderBoundsParams, SceneSaveParams, SimStepParams,
};
use ludion_protocol::response::ScreenRect;
use ludion_protocol::{Command, ResponseBody};
use ludion_server::Server;
use serde_json::{Value, json};

// --- Shared helpers ------------------------------------------------------------------------------

/// The `examples/gallery` directory, whose committed `sprites.yaml` the session edits.
fn gallery_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

/// Binds a server serving `engine` on a free localhost port.
fn serve(engine: Engine) -> Server {
    Server::bind(engine, "127.0.0.1:0").expect("server binds")
}

/// The Wave 4 palette's "sprite" archetype: a named, textureless tinted quad. Used verbatim both in
/// the live session and to build the expected diff fixture, so the two must produce byte-identical
/// entity blocks.
fn palette_spawn() -> Command {
    let mut components = BTreeMap::new();
    components.insert(
        "Transform".to_string(),
        json!({ "translation": [0.0, -2.0, 0.0] }),
    );
    components.insert(
        "Sprite".to_string(),
        json!({ "size": [0.5, 0.5], "color": [0.2, 0.6, 0.9, 1.0] }),
    );
    Command::EntitySpawn(EntitySpawnParams {
        name: Some("palette-sprite".to_string()),
        components,
    })
}

/// Writes `engine`'s world through the canonical writer (`Engine::scene_text`, the text `scene.save`
/// writes) to `path` and returns the text.
fn save_scene(engine: &mut Engine, path: &Path) -> String {
    let text = engine.scene_text().expect("the world saves");
    std::fs::write(path, &text).expect("the scene writes");
    text
}

/// A content root the wire `scene.save` can write into: `phase6-root` under the target tmp dir,
/// holding the textures the gallery's `sprites.yaml` names.
fn stage_root(tmp: &Path) -> PathBuf {
    let root = tmp.join("phase6-root");
    std::fs::create_dir_all(root.join("assets")).unwrap();
    for texture in ["checker.png", "brick.png"] {
        std::fs::copy(
            gallery_dir().join("assets").join(texture),
            root.join("assets").join(texture),
        )
        .expect("the gallery texture copies");
    }
    root
}

/// Whether `(px, py)` falls inside `rect` (inclusive) — the editor's point-in-rect pick predicate.
fn rect_contains(rect: &ScreenRect, px: f32, py: f32) -> bool {
    px >= rect.x && px <= rect.x + rect.w && py >= rect.y && py <= rect.y + rect.h
}

/// Unwraps a `component.get` body to its value (a read whose shape the handshake guarantees).
fn get_component(client: &mut ludion_client::Client, entity: EntityId, name: &str) -> Value {
    match client
        .call(Command::ComponentGet(ComponentGetParams {
            entity,
            component: name.to_string(),
        }))
        .unwrap()
        .body
    {
        ResponseBody::ComponentValue { value } => value,
        other => panic!("component.get returned an unexpected body: {other:?}"),
    }
}

// --- The scripted designer session ---------------------------------------------------------------

/// The Phase 6 exit gate, mechanized end to end. See the module docs for the three properties.
#[test]
fn scripted_designer_session_meets_the_phase_6_exit_criteria() {
    let gallery = gallery_dir();
    let sprites_yaml = std::fs::read_to_string(gallery.join("sprites.yaml")).unwrap();
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));

    // --- Baselines, derived from the engine (no hand-authored YAML fixture) ---------------------
    // `source_canonical`: the gallery scene in canonical form — the designer's on-disk starting
    // point. `expected_edited`: the same scene with only the palette entity spawned. Because
    // `world_to_scene` emits entities in ascending slot order and the palette lands in the highest
    // slot, the palette entity is appended last, so `expected_edited` is exactly `source_canonical`
    // followed by the palette block — that suffix is the whole expected diff.
    let mut base_engine = Engine::with_project_root(&gallery);
    base_engine.load_inline_scene(&sprites_yaml).unwrap();
    let source_canonical = save_scene(&mut base_engine, &tmp.join("phase6_source.yaml"));

    let mut palette_engine = Engine::with_project_root(&gallery);
    palette_engine.load_inline_scene(&sprites_yaml).unwrap();
    dispatch(&mut palette_engine, palette_spawn()).unwrap();
    let expected_edited = save_scene(&mut palette_engine, &tmp.join("phase6_expected.yaml"));

    let palette_block = expected_edited
        .strip_prefix(&source_canonical)
        .expect("the palette entity must append as a suffix of the canonical scene")
        .to_string();
    assert!(
        palette_block.contains("name: palette-sprite"),
        "the expected diff fixture must be the palette entity's block, got:\n{palette_block}"
    );

    // --- Serve the scene, paused, with recording armed ------------------------------------------
    // Pause *before* arming recording so the pause is not itself a recorded command; arm before
    // serving so the header captures the starting scene + seed. The paused engine advances only on
    // the explicit `sim.step`s the session sends.
    let record_path = tmp.join("phase6_session.lrp");
    let root = stage_root(&tmp);
    let mut engine = Engine::with_project_root(&root);
    engine.load_inline_scene(&sprites_yaml).unwrap();
    dispatch(&mut engine, Command::SimPause).unwrap();
    let sink = std::fs::File::create(&record_path).expect("recording file creates");
    engine.set_recording(sink).expect("recording arms");
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    // The recorded command list, accumulated in wire order as the session runs, then reused verbatim
    // to reproduce the trace in-process (the Phase 3/4 pattern). Pure reads (query, bounds, get,
    // save) are never pushed — they are not recorded either, so the two lists stay aligned.
    let mut recorded: Vec<Command> = Vec::new();

    // --- Map entity names to ids (a read) -------------------------------------------------------
    // The editor knows entities by name in its hierarchy; we resolve the two the session touches.
    let sprites = match client
        .call(Command::QueryEntities(QueryEntitiesParams {
            filter: vec!["Sprite".to_string()],
            include: vec!["Name".to_string()],
            world: false,
            entities: None,
        }))
        .unwrap()
        .body
    {
        ResponseBody::QueryResult { entities } => entities,
        other => panic!("query.entities returned an unexpected body: {other:?}"),
    };
    let id_of = |name: &str| -> EntityId {
        sprites
            .iter()
            .find(|m| m.components.get("Name") == Some(&json!(name)))
            .unwrap_or_else(|| panic!("scene should carry a sprite named {name}"))
            .entity
    };
    // The drag target and a *different* destroy target — the session must never confuse the two.
    let drag_target = id_of("tinted-sprite");
    let destroy_target = id_of("brick-sprite");
    assert_ne!(drag_target, destroy_target);

    // --- (1) Pick via render.bounds — a genuine point-in-rect hit test --------------------------
    let bounds = match client
        .call(Command::RenderBounds(RenderBoundsParams::default()))
        .unwrap()
        .body
    {
        ResponseBody::RenderBounds { entities, .. } => entities,
        other => panic!("render.bounds returned an unexpected body: {other:?}"),
    };
    // Aim the "click" at the centre of the intended sprite's screen rect...
    let target_rect = bounds
        .iter()
        .find(|b| b.entity == drag_target)
        .and_then(|b| b.screen_rect)
        .expect("the drag target must have a screen rect (it is in front of the camera)");
    let (click_x, click_y) = (
        target_rect.x + target_rect.w / 2.0,
        target_rect.y + target_rect.h / 2.0,
    );
    // ...then pick exactly as the editor does: the front-most (nearest, i.e. largest AABB-centre z)
    // renderable whose rect contains the click. The pick must land on the intended entity.
    let picked = bounds
        .iter()
        .filter(|b| {
            b.on_screen
                && b.screen_rect
                    .is_some_and(|r| rect_contains(&r, click_x, click_y))
        })
        .max_by(|a, b| {
            let za = (a.world_aabb.min[2] + a.world_aabb.max[2]) / 2.0;
            let zb = (b.world_aabb.min[2] + b.world_aabb.max[2]) / 2.0;
            za.partial_cmp(&zb).unwrap()
        })
        .expect("the click must hit at least one renderable")
        .entity;
    assert_eq!(
        picked, drag_target,
        "the point-in-rect pick must select the intended entity"
    );

    // --- (2) Drag: a burst of throttled preview sets, then the release --------------------------
    // Every intermediate set is kept — the recording's truth is what happened, not the undo stack's
    // abstraction (plan decision 7). The FIRST set's `previous` is the value-at-mousedown: the single
    // undo entry for the whole drag.
    let drag_values = [
        json!({ "translation": [2.5, 0.10, 0.0] }),
        json!({ "translation": [2.6, 0.20, 0.0] }),
        json!({ "translation": [2.7, 0.30, 0.0] }),
        json!({ "translation": [2.8, 0.40, 0.0] }),
        json!({ "translation": [2.9, 0.45, 0.0] }),
        json!({ "translation": [3.0, 0.50, 0.0] }), // value-at-release
    ];
    let mut value_at_mousedown: Option<Value> = None;
    for value in drag_values {
        let cmd = Command::ComponentSet(ComponentSetParams {
            entity: drag_target,
            component: "Transform".to_string(),
            value,
        });
        let previous = match client.call(cmd.clone()).unwrap().body {
            ResponseBody::ComponentSet { previous } => previous,
            other => panic!("component.set returned an unexpected body: {other:?}"),
        };
        value_at_mousedown.get_or_insert_with(|| {
            previous.expect("the drag target already had a Transform, so previous is Some")
        });
        recorded.push(cmd);
    }
    let value_at_mousedown = value_at_mousedown.unwrap();

    // Advance a couple of frames so the recorded trace is genuinely multi-frame (a static scene: the
    // steps do not change world content, only the frame — the trace is per-frame all the same).
    client.sim_step(2, false).unwrap();
    recorded.push(Command::SimStep(SimStepParams {
        ticks: 2,
        hash: false,
    }));

    // --- (3) Spawn from the palette -------------------------------------------------------------
    match client.call(palette_spawn()).unwrap().body {
        ResponseBody::Spawned { .. } => {}
        other => panic!("entity.spawn returned an unexpected body: {other:?}"),
    }
    recorded.push(palette_spawn());

    // --- (4) Destroy a different pre-existing entity; capture the inverse -----------------------
    let destroy_cmd = Command::EntityDestroy(EntityDestroyParams {
        entity: destroy_target,
    });
    let destroyed_components = match client.call(destroy_cmd.clone()).unwrap().body {
        ResponseBody::EntityDestroyed { components, .. } => components,
        other => panic!("entity.destroy returned an unexpected body: {other:?}"),
    };
    recorded.push(destroy_cmd);
    // The inverse payload must be well-shaped: the full serialized state, `Name` included.
    for expected in ["Name", "Transform", "Sprite"] {
        assert!(
            destroyed_components.contains_key(expected),
            "the destroy inverse must carry `{expected}`, got keys {:?}",
            destroyed_components.keys().collect::<Vec<_>>()
        );
    }
    assert_eq!(
        destroyed_components.get("Name"),
        Some(&json!("brick-sprite")),
        "the destroy inverse must carry the entity's name"
    );

    // --- (5) Undo twice, editor-style (undo is ordinary commands) -------------------------------
    // Undo #1 = the destroy → respawn with the captured components (Name included). The deterministic
    // free-list reuses the freed slot, so document order on save is preserved.
    let respawn_cmd = Command::EntitySpawn(EntitySpawnParams {
        name: None,
        components: destroyed_components.clone(),
    });
    let respawned = match client.call(respawn_cmd.clone()).unwrap().body {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("respawn entity.spawn returned an unexpected body: {other:?}"),
    };
    recorded.push(respawn_cmd);

    // (c) Byte-identical undo of the destroy: every restored component equals what was captured.
    for (name, expected) in &destroyed_components {
        let restored = get_component(&mut client, respawned, name);
        assert_eq!(
            &restored, expected,
            "undo of destroy must restore `{name}` byte-for-byte"
        );
    }

    // Undo #2 = the drag → one set back to the value-at-mousedown.
    let undo_drag = Command::ComponentSet(ComponentSetParams {
        entity: drag_target,
        component: "Transform".to_string(),
        value: value_at_mousedown.clone(),
    });
    client.call(undo_drag.clone()).unwrap();
    recorded.push(undo_drag);

    // (c) Byte-identical undo of the drag: the Transform is exactly its value-at-mousedown again.
    let after_undo = get_component(&mut client, drag_target, "Transform");
    assert_eq!(
        after_undo, value_at_mousedown,
        "undo of the drag must restore the Transform to its value-at-mousedown"
    );

    // Advance a few more frames so the trace samples more tick boundaries.
    client.sim_step(3, false).unwrap();
    recorded.push(Command::SimStep(SimStepParams {
        ticks: 3,
        hash: false,
    }));

    // The live world hash at the end of the session (a read).
    let wire_final_hash = client.sim_hash().unwrap();

    // --- (6) Save -------------------------------------------------------------------------------
    let edited_path = root.join("phase6_edited.yaml");
    match client
        .call(Command::SceneSave(SceneSaveParams {
            path: "phase6_edited.yaml".to_string(),
        }))
        .unwrap()
        .body
    {
        ResponseBody::SceneSaved { .. } => {}
        other => panic!("scene.save returned an unexpected body: {other:?}"),
    }
    let edited = std::fs::read_to_string(&edited_path).expect("edited scene reads back");

    // Shut down cleanly so the (unbuffered) recording flushes fully before we read it back.
    drop(client);
    server.shutdown();
    let recorded_bytes = std::fs::read(&record_path).expect("recording reads back");

    // --- (a) Minimal diff, mechanized -----------------------------------------------------------
    // The whole-file identity first (the strongest single statement of the gate):
    assert_eq!(
        edited, expected_edited,
        "the edited scene must equal the source scene with exactly the palette entity added \
         (drag undone, destroy undone, everything else byte-identical)"
    );
    // ...then the same claim as an explicit line diff, so a failure localizes: every source line is
    // preserved in place (0 removed), and the only added lines are the palette block.
    let source_lines: Vec<&str> = source_canonical.lines().collect();
    let edited_lines: Vec<&str> = edited.lines().collect();
    let palette_lines: Vec<&str> = palette_block.lines().collect();
    assert_eq!(
        edited_lines.len(),
        source_lines.len() + palette_lines.len(),
        "the edit must add exactly the palette block and remove nothing"
    );
    assert_eq!(
        &edited_lines[..source_lines.len()],
        source_lines.as_slice(),
        "every source line must survive the session unchanged and in place"
    );
    assert_eq!(
        &edited_lines[source_lines.len()..],
        palette_lines.as_slice(),
        "the only added lines must be the palette entity block"
    );

    // --- (b) Replay identity --------------------------------------------------------------------
    // The recording replays to the same full per-frame trace the identical command list produces
    // in-process, and its final hash equals the live `sim.hash`.
    let replay_trace = replay_hashes(&recorded_bytes, ".").expect("recording replays");

    let mut in_process = Engine::new();
    in_process.load_inline_scene(&sprites_yaml).unwrap();
    let in_process_trace = drive_hashes(&mut in_process, recorded.clone()).unwrap();

    assert_eq!(
        replay_trace.len(),
        in_process_trace.len(),
        "the replay and the in-process drive must produce the same number of frames"
    );
    for (i, (replayed, in_proc)) in replay_trace.iter().zip(&in_process_trace).enumerate() {
        assert_eq!(
            (replayed.0.as_u64(), replayed.1.as_u64()),
            (in_proc.0.as_u64(), in_proc.1.as_u64()),
            "frame/hash divergence at trace index {i}"
        );
    }
    let (_, replay_final) = *replay_trace.last().expect("a non-empty hash trace");
    assert_eq!(
        replay_final.as_u64(),
        wire_final_hash.as_u64(),
        "the replayed recording must end on the same WorldHash the wire session hashed"
    );
}
