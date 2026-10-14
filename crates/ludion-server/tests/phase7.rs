//! Phase 7 Wave 3 integration tests — the artist's live asset reload over the wire.
//!
//! Wave 3 gives `asset.reload` something real to do: a CPU [`AssetCache`] `extract()` reads textures
//! through, and a renderer GPU texture cache keyed on content hash. The headline is the *today-
//! impossible* case (`Plan/phase-7-designer-flow.md` decision 5): a **paused** engine whose texture is
//! overwritten on disk re-renders the change. Before Wave 3 a texture edit only showed while the sim
//! ran (accidentally, at per-frame disk-IO cost) and never while paused.
//!
//! These follow the Phase 4 suite's conventions (`phase4.rs`): bind port 0, tear down at scope end,
//! block on replies rather than sleep, and gracefully skip a GPU-dependent assertion when the host has
//! no adapter (an `L2003` prints a `SKIPPED` line and stays green on adapterless CI; the pinned RTX
//! 3090 actually renders and asserts).

use std::path::{Path, PathBuf};
use std::time::Duration;

use ludion_engine::Engine;
use ludion_protocol::command::AssetReloadParams;
use ludion_protocol::event::Event;
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command, SubscribeMode};
use ludion_server::Server;

/// How long to wait for a single streamed frame or event before giving up (never the metric — only a
/// stall bound; flaky timing tests are P0).
const READ: Duration = Duration::from_secs(5);

/// A scene: one camera and one textured sprite filling the view. `tex.png` resolves against the
/// engine's project root (the temp dir each test writes it into).
const SPRITE_SCENE: &str = "\
format: 1
entities:
  - name: cam
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 5.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 60.0
  - name: spr
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Sprite:
        texture: tex.png
        size:
          - 6.0
          - 6.0
";

/// A fresh, empty temp project directory unique to `label` and this process.
fn temp_project(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-phase7-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp project dir");
    dir
}

/// Writes an 8×8 solid-`rgba` PNG to `path`.
fn write_solid_png(path: &Path, rgba: [u8; 4]) {
    let pixels: Vec<u8> = std::iter::repeat_n(rgba, 8 * 8).flatten().collect();
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 8, 8);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&pixels).unwrap();
    }
    std::fs::write(path, &bytes).unwrap();
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

/// The paused engine's texture is overwritten on disk and reloaded over the wire: the `asset.reloaded`
/// event fires with the new content hash, and the next streamed frame differs from the pre-overwrite
/// one — the today-impossible case, now possible.
#[test]
fn needs_gpu_paused_engine_reloads_an_overwritten_texture_over_the_wire() {
    let root = temp_project("paused-reload");
    write_solid_png(&root.join("tex.png"), [220, 30, 30, 255]); // red

    let mut engine = Engine::with_project_root(&root).with_render_size(256, 256);
    engine.load_inline_scene(SPRITE_SCENE).unwrap();
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
            eprintln!("SKIPPED paused_engine_reloads_an_overwritten_texture: no GPU adapter");
            return;
        }
        Err(diag) => panic!("baseline frame read failed: {diag}"),
    };

    // A reload must not perturb sim state.
    let hash_before = client.sim_hash().unwrap();

    // Overwrite the texture on disk with a very different colour, then reload it over the wire (exactly
    // what the host's watcher-forwarder submits).
    let blue = [30, 30, 220, 255];
    write_solid_png(&root.join("tex.png"), blue);
    let new_hash = ludion_assets::content_hash(&std::fs::read(root.join("tex.png")).unwrap());
    client
        .call(Command::AssetReload(AssetReloadParams {
            path: Some("tex.png".into()),
        }))
        .unwrap();

    // The `asset.reloaded` event fires, carrying the new content hash.
    let event = client
        .next_event(READ)
        .unwrap()
        .expect("an asset.reloaded event must arrive");
    match event.event {
        Event::AssetReloaded(data) => {
            assert_eq!(data.path, "tex.png");
            assert_eq!(
                data.hash,
                format!("{new_hash:016x}"),
                "event carries the new hash"
            );
        }
        other => panic!("expected asset.reloaded, got {other:?}"),
    }

    // The next streamed frame differs from the pre-overwrite one — the paused engine re-rendered.
    let changed = next_changed_frame(&mut client, &baseline)
        .expect("a frame reflecting the new texture must arrive while paused");
    assert_ne!(changed, baseline);

    // Sim-invisible: the reload changed no world state.
    assert_eq!(
        client.sim_hash().unwrap().as_u64(),
        hash_before.as_u64(),
        "asset.reload must not change the WorldHash"
    );

    client.unsubscribe(frames).unwrap();
    client.unsubscribe(reloaded).unwrap();
    server.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

/// The real filesystem watcher (host wiring): overwriting the texture on disk — with no explicit
/// `asset.reload` — makes the server forward it and fire the `asset.reloaded` event.
#[test]
fn needs_gpu_watcher_forwards_a_disk_overwrite_as_a_reload() {
    let root = temp_project("watch-forward");
    write_solid_png(&root.join("tex.png"), [220, 30, 30, 255]);

    let mut engine = Engine::with_project_root(&root).with_render_size(256, 256);
    engine.load_inline_scene(SPRITE_SCENE).unwrap();
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

    // The first frame both populates the CPU cache (so a reload has something to invalidate) and is the
    // adapterless graceful-skip probe.
    if let Ok(None) = client.next_frame(READ) {
        ludion_core::require_gpu::skip_no_adapter(true);
        eprintln!("SKIPPED watcher_forwards_a_disk_overwrite_as_a_reload: no GPU adapter");
        client.unsubscribe(frames).unwrap();
        client.unsubscribe(reloaded).unwrap();
        server.shutdown();
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    // Start the watcher (short debounce for a snappy test), then overwrite the texture on disk.
    let _watch = server
        .watch_assets(&root, Duration::from_millis(50))
        .expect("watcher starts");
    write_solid_png(&root.join("tex.png"), [30, 30, 220, 255]);

    // The watcher forwards the change as an asset.reload; the event fires (generous timeout — a flaky
    // watch would be a P0 bug). Other unrelated batches (e.g. the temp dir itself) are ignored by the
    // engine, so keep reading until the asset.reloaded for tex.png arrives.
    let mut saw = false;
    for _ in 0..10 {
        match client.next_event(READ).unwrap() {
            Some(msg) => {
                if let Event::AssetReloaded(data) = msg.event
                    && data.path.ends_with("tex.png")
                {
                    saw = true;
                    break;
                }
            }
            None => break,
        }
    }
    assert!(
        saw,
        "the disk overwrite must surface as an asset.reloaded event"
    );

    client.unsubscribe(frames).unwrap();
    client.unsubscribe(reloaded).unwrap();
    server.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

/// A reload dropped mid-run leaves the per-step `WorldHash` trace byte-identical to a run without it —
/// asset bytes are not sim state (replay-safe, determinism-safe).
#[test]
fn asset_reload_leaves_the_worldhash_trace_unchanged() {
    // Control: step a paused engine and collect a hash trace, no reload.
    let control_root = temp_project("hash-control");
    write_solid_png(&control_root.join("tex.png"), [220, 30, 30, 255]);
    let control = {
        let mut engine = Engine::with_project_root(&control_root).with_render_size(256, 256);
        engine.load_inline_scene(SPRITE_SCENE).unwrap();
        ludion_engine::dispatch(&mut engine, Command::SimPause).unwrap();
        let server = serve(engine);
        let mut client = ludion_client::Client::connect(&server.url()).unwrap();
        let h0 = client.sim_hash().unwrap().as_u64();
        client.sim_step(3, false).unwrap();
        let h1 = client.sim_hash().unwrap().as_u64();
        client.sim_step(3, false).unwrap();
        let h2 = client.sim_hash().unwrap().as_u64();
        server.shutdown();
        (h0, h1, h2)
    };

    // With a reload injected between the two step batches.
    let root = temp_project("hash-reload");
    write_solid_png(&root.join("tex.png"), [220, 30, 30, 255]);
    let mut engine = Engine::with_project_root(&root).with_render_size(256, 256);
    engine.load_inline_scene(SPRITE_SCENE).unwrap();
    ludion_engine::dispatch(&mut engine, Command::SimPause).unwrap();
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();
    // Populate the CPU cache so the reload genuinely invalidates (needs a GPU; degrades to a no-op
    // reload without one, which is still WorldHash-neutral — the invariant under test holds either way).
    let _ = client.screenshot(Some(64), Some(64));

    let h0 = client.sim_hash().unwrap().as_u64();
    client.sim_step(3, false).unwrap();
    let h1 = client.sim_hash().unwrap().as_u64();

    write_solid_png(&root.join("tex.png"), [30, 30, 220, 255]);
    client
        .call(Command::AssetReload(AssetReloadParams {
            path: Some("tex.png".into()),
        }))
        .unwrap();

    client.sim_step(3, false).unwrap();
    let h2 = client.sim_hash().unwrap().as_u64();
    server.shutdown();

    assert_eq!(
        (h0, h1, h2),
        control,
        "a reload must not move the WorldHash trace"
    );

    let _ = std::fs::remove_dir_all(&control_root);
    let _ = std::fs::remove_dir_all(&root);
}
