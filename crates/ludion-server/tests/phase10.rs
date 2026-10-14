//! Phase 10 Wave 4 integration test — a voxel model's live reload over the wire.
//!
//! The voxel analog of `phase7.rs`'s texture reload: a **paused** engine serving a scene with a
//! `Mesh.shape: { voxel: … }` has its `.voxel.yaml` overwritten on disk and reloaded over the wire.
//! The `asset.reloaded` event fires with the re-meshed document's new content hash, the next streamed
//! frame reflects the change (the hash-keyed GPU mesh cache re-uploaded), and — because asset bytes
//! are not sim state — the `WorldHash` is untouched. This is the same today-impossible case
//! (`Plan/phase-7-designer-flow.md` decision 5, extended to voxel models by Plan/phase-10 decision 5):
//! live text-editing of a voxel model updates a paused viewer through the existing watcher plumbing,
//! no new wire commands.
//!
//! Follows the Phase 7 suite's conventions: bind port 0, tear down at scope end, block on replies
//! rather than sleep, and gracefully skip the GPU-dependent assertions when the host has no adapter.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ludion_engine::Engine;
use ludion_protocol::command::AssetReloadParams;
use ludion_protocol::event::Event;
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command, SubscribeMode};
use ludion_server::Server;

/// How long to wait for a single streamed frame or event before giving up (a stall bound, never the
/// metric — flaky timing tests are P0).
const READ: Duration = Duration::from_secs(5);

/// A scene: one camera, one warm point light, and one voxel model at the origin filling the view.
/// `voxels/model.voxel.yaml` resolves against the engine's project root (the temp dir each test
/// writes it into).
const VOXEL_SCENE: &str = "\
format: 1
entities:
  - name: cam
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 4.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 60.0
  - name: light
    components:
      Transform:
        translation:
          - 2.0
          - 3.0
          - 4.0
      PointLight:
        intensity: 8.0
        range: 20.0
  - name: model
    components:
      Mesh:
        shape:
          voxel: \"voxels/model.voxel.yaml\"
";

/// A 2×2×2 solid voxel cube whose single palette entry is `rgb` — big enough to fill the view, and a
/// bright color so a palette change is unmistakable in the render.
fn voxel_cube(rgb: [f64; 3]) -> String {
    format!(
        "format: 1
kind: voxel
size:
  - 2
  - 2
  - 2
palette:
  A:
    color:
      - {}
      - {}
      - {}
      - 1.0
slices:
  -
    - AA
    - AA
  -
    - AA
    - AA
",
        rgb[0], rgb[1], rgb[2]
    )
}

/// A fresh, empty temp project directory unique to `label` and this process, with a `voxels/` subdir.
fn temp_project(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-phase10-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("voxels")).expect("create temp project dir");
    dir
}

/// Binds a server serving `engine` on a free localhost port.
fn serve(engine: Engine) -> Server {
    Server::bind(engine, "127.0.0.1:0").expect("server binds")
}

/// Reads the next frame that differs from `baseline`, or `None` if none arrives within `READ` after a
/// few attempts (the streamed readback ring can hand back an in-flight older frame first).
fn next_changed_frame(client: &mut ludion_client::Client, baseline: &[u8]) -> Option<Vec<u8>> {
    for _ in 0..30 {
        match client.next_frame(READ) {
            Ok(Some(frame)) if frame.payload != baseline => return Some(frame.payload),
            Ok(Some(_)) => {}
            Ok(None) => return None,
            Err(diag) => panic!("frame read failed: {diag}"),
        }
    }
    None
}

/// The paused engine's voxel model is overwritten on disk and reloaded over the wire: the
/// `asset.reloaded` event fires with the re-meshed model's new content hash, the next streamed frame
/// differs from the pre-overwrite one, and the `WorldHash` is unchanged — the today-impossible case,
/// now possible for voxel models too.
#[test]
fn needs_gpu_paused_engine_reloads_an_overwritten_voxel_model_over_the_wire() {
    let root = temp_project("paused-voxel-reload");
    let voxel = root.join("voxels/model.voxel.yaml");
    std::fs::write(&voxel, voxel_cube([0.9, 0.15, 0.1])).unwrap(); // red

    let mut engine = Engine::with_project_root(&root).with_render_size(256, 256);
    engine.load_inline_scene(VOXEL_SCENE).unwrap();
    // Paused: the whole point is that a reload shows *without* the sim running.
    ludion_engine::dispatch(&mut engine, Command::SimPause).unwrap();
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    let frames = client
        .subscribe(
            Channel::Frames,
            SubscribeMode::LatestWins { max_fps: None },
            FrameFormat::Qoi,
        )
        .unwrap();
    let reloaded = client
        .subscribe(
            Channel::AssetReloaded,
            SubscribeMode::default(),
            FrameFormat::Qoi,
        )
        .unwrap();

    // Baseline frame (also the lazy GPU renderer's first-frame cost). A `None` here is the adapterless
    // graceful skip: the egress loop latches `L2003` and renders nothing.
    let baseline = match client.next_frame(READ) {
        Ok(Some(frame)) => frame.payload,
        Ok(None) => {
            ludion_core::require_gpu::skip_no_adapter(true);
            eprintln!(
                "SKIPPED paused_engine_reloads_an_overwritten_voxel_model_over_the_wire: no GPU \
                 adapter"
            );
            return;
        }
        Err(diag) => panic!("baseline frame read failed: {diag}"),
    };

    // A reload must not perturb sim state.
    let hash_before = client.sim_hash().unwrap();

    // Overwrite the model on disk with a very different colour, then reload it over the wire (exactly
    // what the host's watcher-forwarder submits).
    std::fs::write(&voxel, voxel_cube([0.1, 0.2, 0.95])).unwrap(); // blue
    let new_hash = ludion_assets::content_hash(&std::fs::read(&voxel).unwrap());
    client
        .call(Command::AssetReload(AssetReloadParams {
            path: Some("voxels/model.voxel.yaml".into()),
        }))
        .unwrap();

    // The `asset.reloaded` event fires, carrying the re-meshed model's new content hash.
    let event = client
        .next_event(READ)
        .unwrap()
        .expect("an asset.reloaded event must arrive");
    match event.event {
        Event::AssetReloaded(data) => {
            assert_eq!(data.path, "voxels/model.voxel.yaml");
            assert_eq!(
                data.hash,
                format!("{new_hash:016x}"),
                "event carries the new content hash"
            );
        }
        other => panic!("expected asset.reloaded, got {other:?}"),
    }

    // The next streamed frame differs from the pre-overwrite one — the paused engine re-rendered.
    let changed = next_changed_frame(&mut client, &baseline)
        .expect("a frame reflecting the re-meshed model must arrive while paused");
    assert_ne!(changed, baseline);

    // Sim-invisible: the reload changed no world state.
    assert_eq!(
        client.sim_hash().unwrap().as_u64(),
        hash_before.as_u64(),
        "asset.reload of a voxel model must not change the WorldHash"
    );

    client.unsubscribe(frames).unwrap();
    client.unsubscribe(reloaded).unwrap();
    server.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

/// A voxel-model reload dropped mid-run leaves the per-step `WorldHash` trace byte-identical to a run
/// without it — asset bytes are not sim state (replay-safe, determinism-safe). The voxel analog of
/// `phase7.rs`'s `asset_reload_leaves_the_worldhash_trace_unchanged`.
#[test]
fn voxel_reload_leaves_the_worldhash_trace_unchanged() {
    fn step_trace(root: &Path, reload_between: bool) -> (u64, u64, u64) {
        let mut engine = Engine::with_project_root(root).with_render_size(256, 256);
        engine.load_inline_scene(VOXEL_SCENE).unwrap();
        ludion_engine::dispatch(&mut engine, Command::SimPause).unwrap();
        let server = serve(engine);
        let mut client = ludion_client::Client::connect(&server.url()).unwrap();
        // Populate the CPU cache so a reload genuinely invalidates (needs a GPU; degrades to a no-op
        // reload without one, which is still WorldHash-neutral — the invariant holds either way).
        let _ = client.screenshot(Some(64), Some(64));

        let h0 = client.sim_hash().unwrap().as_u64();
        client.sim_step(3, false).unwrap();
        let h1 = client.sim_hash().unwrap().as_u64();

        if reload_between {
            std::fs::write(
                root.join("voxels/model.voxel.yaml"),
                voxel_cube([0.1, 0.2, 0.95]),
            )
            .unwrap();
            client
                .call(Command::AssetReload(AssetReloadParams {
                    path: Some("voxels/model.voxel.yaml".into()),
                }))
                .unwrap();
        }

        client.sim_step(3, false).unwrap();
        let h2 = client.sim_hash().unwrap().as_u64();
        server.shutdown();
        (h0, h1, h2)
    }

    let control_root = temp_project("voxel-hash-control");
    std::fs::write(
        control_root.join("voxels/model.voxel.yaml"),
        voxel_cube([0.9, 0.15, 0.1]),
    )
    .unwrap();
    let control = step_trace(&control_root, false);

    let root = temp_project("voxel-hash-reload");
    std::fs::write(
        root.join("voxels/model.voxel.yaml"),
        voxel_cube([0.9, 0.15, 0.1]),
    )
    .unwrap();
    let with_reload = step_trace(&root, true);

    assert_eq!(
        with_reload, control,
        "a voxel-model reload must not move the WorldHash trace"
    );

    let _ = std::fs::remove_dir_all(&control_root);
    let _ = std::fs::remove_dir_all(&root);
}
