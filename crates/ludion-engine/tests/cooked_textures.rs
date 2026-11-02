//! `Engine::set_textures` end to end (`Plan/cooked-assets.md` Wave 1): a cooked mesh map renders
//! like its PNG, an uncooked one falls back with one `L1271`, and the default reads no store. Every
//! test needs a GPU adapter with BC7 or ASTC and graceful-skips without one.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded};
use ludion_assets::cooked::{ColorSpace, CookSettings, CookTarget, cook_key, cooked_file_name};
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{Egress, EgressChannels, Engine, TextureSource};
use ludion_protocol::command::{
    AssetReloadParams, RenderScreenshotParams, SceneLoadParams, SimStepParams,
    StreamSubscribeParams, SubscribeMode,
};
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command};
use ludion_renderer::Renderer;

const MESH: &str = "assets/kit/k/meshes/m.mesh.yaml";
const BASE: &str = "assets/kit/k/textures/b.png";
const NORMAL: &str = "assets/kit/k/textures/n.png";

const SCENE: &str = "\
format: 1
entities:
  - components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 2.2
      Camera:
        near: 0.1
        far: 100.0
  - components:
      Mesh:
        shape:
          asset: assets/kit/k/meshes/m.mesh.yaml
";

fn list(name: &str, values: &[f64]) -> String {
    let items: String = values.iter().map(|v| format!("  - {v:?}\n")).collect();
    format!("{name}:\n{items}")
}

/// A unit quad facing +Z with UVs over the unit square, a base-colour and a normal map.
fn mesh_doc() -> String {
    let mut doc = String::from("format: 1\nkind: mesh\n");
    doc += &list(
        "positions",
        &[
            -1.0, -1.0, 0.0, 1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0,
        ],
    );
    doc += &list("uvs", &[0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0]);
    doc += &list("tangents", &[1.0, 0.0, 0.0, 1.0].repeat(4));
    doc += "base_color_texture:\n  path: ../textures/b.png\n";
    doc += "normal_texture:\n  path: ../textures/n.png\n";
    doc += "indices:\n";
    for index in [0, 1, 2, 0, 2, 3] {
        doc += &format!("  - {index}\n");
    }
    doc
}

/// An 8×8 map with a red/green gradient `seed` shifts, so a wrongly sampled map shows.
fn png(seed: u8, flat_blue: bool) -> Vec<u8> {
    let mut rgba = Vec::new();
    for y in 0..8u8 {
        for x in 0..8u8 {
            let blue = if flat_blue { 255 } else { 128 };
            rgba.extend_from_slice(&[
                (x * 30).wrapping_add(seed),
                (y * 30).wrapping_add(seed),
                blue,
                255,
            ]);
        }
    }
    ludion_assets::encode_png(8, 8, &rgba).expect("an 8x8 PNG encodes")
}

/// A scratch project with one `[[import]]` table over the fixture mesh and its two maps.
fn project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-cooked-textures-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("assets/kit/k/meshes")).unwrap();
    std::fs::create_dir_all(dir.join("assets/kit/k/textures")).unwrap();
    std::fs::write(
        dir.join("ludion.toml"),
        "name = \"fixture\"\ndefault_scene = \"scenes/main.yaml\"\n\n[[import]]\nfrom = \"art/*.gltf\"\nto = \"assets/kit\"\n",
    )
    .unwrap();
    std::fs::write(dir.join(MESH), mesh_doc()).unwrap();
    std::fs::write(dir.join(BASE), png(20, false)).unwrap();
    std::fs::write(dir.join(NORMAL), png(100, true)).unwrap();
    dir
}

/// Writes the cooked copy of the map at `png` under its key in `dir`'s store for `target`;
/// `alpha_cutoff` is the mask cutoff a mask mesh's base-colour map is cooked at.
fn cook_map(
    dir: &Path,
    png: &str,
    target: CookTarget,
    space: ColorSpace,
    alpha_cutoff: Option<f32>,
) {
    let source = ludion_assets::load_png(ludion_assets::AssetRoot::content(dir), png).unwrap();
    let settings = CookSettings {
        target,
        color_space: space,
        max_texture_size: None,
        alpha_cutoff,
    };
    let cooked = ludion_cook::cook(&source, &settings).unwrap();
    let store = dir.join(".ludion/cooked").join(target.name());
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(
        store.join(cooked_file_name(cook_key(source.hash, &settings))),
        ludion_cook::write_ktx2(&cooked),
    )
    .unwrap();
}

/// This machine's cook target, or `None` (after a skip line) when it has no adapter or no format.
fn target_or_skip(test: &str) -> Option<CookTarget> {
    match Renderer::new() {
        Ok(renderer) => {
            let target = renderer.cook_target();
            if target.is_none() {
                eprintln!("SKIPPED {test}: no BC7 or ASTC support");
            }
            target
        }
        Err(diagnostic)
            if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diagnostic)) =>
        {
            eprintln!("SKIPPED {test}: no GPU adapter");
            None
        }
        Err(diagnostic) => panic!("renderer init failed: {diagnostic:?}"),
    }
}

fn engine(dir: &Path, textures: TextureSource) -> Engine {
    let mut engine = Engine::builder().project_root(dir).build().expect("builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine.set_textures(textures);
    engine
}

fn l1271(lines: &[LogEvent]) -> Vec<&LogEvent> {
    lines
        .iter()
        .filter(|log| {
            log.diagnostic
                .as_ref()
                .is_some_and(|d| d.code.as_str() == "L1271")
        })
        .collect()
}

/// Renders `rounds` screenshots on a live sim thread, stepping once after each, and returns the log
/// lines seen after every round.
fn live_logs(engine: Engine, rounds: usize) -> Vec<Vec<LogEvent>> {
    live_logs_with(engine, rounds, |_, _| {})
}

/// [`live_logs`], calling `between(round, send)` before each round's screenshot, with `send` running
/// a command on the sim thread.
fn live_logs_with(
    engine: Engine,
    rounds: usize,
    mut between: impl FnMut(usize, &dyn Fn(Command)),
) -> Vec<Vec<LogEvent>> {
    let mut engine = engine;
    engine.execute(Command::SimPause).expect("pauses");
    let (intake, intake_rx) = unbounded();
    let (shutdown, shutdown_rx) = shutdown_channel();
    let (frames, _frames_rx) = bounded::<Egress>(1);
    let (events, events_rx) = unbounded::<Egress>();
    let handle = std::thread::spawn(move || {
        run_live(
            engine,
            intake_rx,
            shutdown_rx,
            Some(EgressChannels { frames, events }),
        )
    });
    let send = |command: Command| {
        let (stamped, reply) = StampedCommand::new(command);
        intake.send(stamped).unwrap();
        reply
            .recv_timeout(Duration::from_secs(30))
            .expect("the sim thread replies")
            .expect("the command succeeds");
    };
    send(Command::StreamSubscribe(StreamSubscribeParams {
        channel: Channel::Log,
        mode: SubscribeMode::LatestWins { max_fps: None },
        format: FrameFormat::Qoi,
    }));
    let mut seen = Vec::new();
    for round in 0..rounds {
        between(round, &send);
        send(Command::RenderScreenshot(RenderScreenshotParams {
            width: Some(64),
            height: Some(64),
            ..RenderScreenshotParams::default()
        }));
        send(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }));
        let mut lines = Vec::new();
        let mut wait = Duration::from_secs(5);
        while let Ok(Egress::Event(event)) = events_rx.recv_timeout(wait) {
            if let Event::Log(log) = event.message.event {
                lines.push(log);
            }
            wait = Duration::from_millis(300);
        }
        seen.push(lines);
    }
    shutdown.signal();
    let _ = handle.join();
    seen
}

/// A cooked map renders like its PNG, and a full store raises no `L1271`.
#[test]
fn needs_gpu_cooked_render_matches_source_within_ssim() {
    let Some(target) = target_or_skip("cooked_render_matches_source") else {
        return;
    };
    let dir = project("ssim");
    cook_map(&dir, BASE, target, ColorSpace::Srgb, None);
    cook_map(&dir, NORMAL, target, ColorSpace::Linear, None);

    let source = engine(&dir, TextureSource::Source)
        .render_png(64, 64)
        .expect("source render");
    let cooked = engine(&dir, TextureSource::Cooked)
        .render_png(64, 64)
        .expect("cooked render");
    let report = ludion_testkit::golden::compare_png_bytes(
        &source,
        &cooked,
        ludion_testkit::golden::DEFAULT_SSIM_THRESHOLD,
    )
    .expect("both decode");
    assert!(
        report.pass,
        "the cooked render drifted from the source: ssim {:?}",
        report.ssim
    );

    let rounds = live_logs(engine(&dir, TextureSource::Cooked), 1);
    assert!(l1271(&rounds[0]).is_empty(), "{:?}", rounds[0]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// With nothing cooked the render still succeeds from the PNGs, and one `L1271` lists both maps.
#[test]
fn needs_gpu_uncooked_maps_warn_once_and_render() {
    if target_or_skip("uncooked_maps_warn_once").is_none() {
        return;
    }
    let dir = project("warn");
    let rounds = live_logs(engine(&dir, TextureSource::Cooked), 2);
    let first = l1271(&rounds[0]);
    assert_eq!(first.len(), 1, "{:?}", rounds[0]);
    let diagnostic = first[0].diagnostic.as_ref().unwrap();
    assert!(diagnostic.related.len() >= 2, "{diagnostic:?}");
    assert!(l1271(&rounds[1]).is_empty(), "{:?}", rounds[1]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A host that never calls `set_textures` is on `Source` and reads no store: an empty one raises
/// nothing.
#[test]
fn needs_gpu_default_is_source() {
    if target_or_skip("default_is_source").is_none() {
        return;
    }
    let dir = project("default");
    let engine = {
        let mut engine = Engine::builder()
            .project_root(&dir)
            .build()
            .expect("builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        engine
    };
    assert_eq!(engine.textures(), TextureSource::Source);
    let rounds = live_logs(engine, 1);
    assert!(l1271(&rounds[0]).is_empty(), "{:?}", rounds[0]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mask mesh's base-colour map is read from the entry cooked at its cutoff: that store raises no
/// `L1271`, and one cooked only without the cutoff is a miss.
#[test]
fn needs_gpu_mask_mesh_reads_the_cutoff_keyed_entry() {
    let Some(target) = target_or_skip("mask_mesh_reads_the_cutoff_keyed_entry") else {
        return;
    };
    let mask_doc = mesh_doc()
        + "alpha_mode: mask
alpha_cutoff: 0.5
";

    let dir = project("mask-hit");
    std::fs::write(dir.join(MESH), &mask_doc).unwrap();
    cook_map(&dir, BASE, target, ColorSpace::Srgb, Some(0.5));
    cook_map(&dir, NORMAL, target, ColorSpace::Linear, None);
    let rounds = live_logs(engine(&dir, TextureSource::Cooked), 1);
    assert!(l1271(&rounds[0]).is_empty(), "{:?}", rounds[0]);
    let _ = std::fs::remove_dir_all(&dir);

    let dir = project("mask-miss");
    std::fs::write(dir.join(MESH), &mask_doc).unwrap();
    cook_map(&dir, BASE, target, ColorSpace::Srgb, None);
    cook_map(&dir, NORMAL, target, ColorSpace::Linear, None);
    let rounds = live_logs(engine(&dir, TextureSource::Cooked), 1);
    assert_eq!(l1271(&rounds[0]).len(), 1, "{:?}", rounds[0]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A map cooked after the first render is picked up by an `asset.reload` of a store file, with no
/// restart: the second `L1271` lists only the map still uncooked.
#[test]
fn needs_gpu_a_sync_mid_session_takes_effect() {
    let Some(target) = target_or_skip("a_sync_mid_session_takes_effect") else {
        return;
    };
    let dir = project("sync-mid");
    let root = dir.clone();
    let rounds = live_logs_with(engine(&dir, TextureSource::Cooked), 2, |round, send| {
        if round == 1 {
            cook_map(&root, BASE, target, ColorSpace::Srgb, None);
            send(Command::AssetReload(AssetReloadParams {
                path: Some(".ludion/cooked/store-file.ktx2".to_string()),
            }));
        }
    });
    assert_eq!(
        l1271(&rounds[0])[0]
            .diagnostic
            .as_ref()
            .unwrap()
            .related
            .len(),
        2
    );
    let second = l1271(&rounds[1]);
    assert_eq!(second.len(), 1, "{:?}", rounds[1]);
    assert_eq!(second[0].diagnostic.as_ref().unwrap().related.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A scene load re-tests the cook fallbacks, so a store filled since the last load is read.
#[test]
fn needs_gpu_a_scene_load_retries_a_cook_fallback() {
    let Some(target) = target_or_skip("a_scene_load_retries_a_cook_fallback") else {
        return;
    };
    let dir = project("load-retry");
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::write(dir.join("scenes/main.yaml"), SCENE).unwrap();
    let root = dir.clone();
    let rounds = live_logs_with(engine(&dir, TextureSource::Cooked), 2, |round, send| {
        if round == 1 {
            cook_map(&root, BASE, target, ColorSpace::Srgb, None);
            send(Command::SceneLoad(SceneLoadParams {
                path: "scenes/main.yaml".to_string(),
            }));
        }
    });
    assert_eq!(
        l1271(&rounds[0])[0]
            .diagnostic
            .as_ref()
            .unwrap()
            .related
            .len(),
        2
    );
    let second = l1271(&rounds[1]);
    assert_eq!(second.len(), 1, "{:?}", rounds[1]);
    assert_eq!(second[0].diagnostic.as_ref().unwrap().related.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
