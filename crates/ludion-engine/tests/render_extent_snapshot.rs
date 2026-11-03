//! The snapshot document carries the render extent: a restore lands on the extent the capture had,
//! a document without it leaves the host's extent alone, and a hostile extent is refused before the
//! engine is touched.

use ludion_engine::{Engine, RenderExtent};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::{Value, json};

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(
    engine: &mut Engine,
    snapshot: Value,
) -> Result<(), ludion_core::diagnostics::Diagnostic> {
    engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .map(|_| ())
}

fn world_extent(engine: &Engine) -> (u32, u32) {
    engine.world().resource::<RenderExtent>().unwrap().size()
}

#[test]
fn a_restore_lands_on_the_extent_the_capture_had() {
    let mut source = Engine::new();
    source.set_render_size(960, 540);
    let snapshot = snapshot_of(&mut source);
    assert_eq!(
        snapshot["render_extent"],
        json!({"width": 960, "height": 540})
    );

    let mut target = Engine::new();
    restore(&mut target, snapshot).unwrap();
    assert_eq!(target.render_size(), (960, 540));
    assert_eq!(world_extent(&target), (960, 540));
}

#[test]
fn a_document_without_the_field_leaves_the_extent_alone() {
    let mut source = Engine::new();
    let mut snapshot = snapshot_of(&mut source);
    snapshot.as_object_mut().unwrap().remove("render_extent");

    let mut target = Engine::new();
    target.set_render_size(640, 360);
    restore(&mut target, snapshot).unwrap();
    assert_eq!(target.render_size(), (640, 360));
    assert_eq!(world_extent(&target), (640, 360));
}

#[test]
fn a_hostile_extent_is_refused_with_the_engine_untouched() {
    let mut source = Engine::new();
    for hostile in [
        json!({"width": 0, "height": 540}),
        json!({"width": 960, "height": 99_999_999}),
    ] {
        let mut snapshot = snapshot_of(&mut source);
        snapshot["render_extent"] = hostile;

        let mut target = Engine::new();
        target.set_render_size(640, 360);
        let diagnostic = restore(&mut target, snapshot).unwrap_err();
        assert_eq!(diagnostic.code.as_str(), "L3008");
        assert_eq!(diagnostic.path.as_deref(), Some("/render_extent"));
        assert_eq!(target.render_size(), (640, 360));
        assert_eq!(world_extent(&target), (640, 360));
    }
}
