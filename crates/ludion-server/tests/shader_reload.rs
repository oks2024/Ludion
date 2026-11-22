//! Wave 5 integration test — dev-only shader hot reload over the wire (`Plan/shader-bindgen.md`
//! decisions 10–11).
//!
//! A dev-flagged host serving a lit scene has an edited renderer shader reloaded through the existing
//! `asset.reload` command (it added no wire method of its own). Three things are asserted
//! end-to-end, over the protocol, exactly as a live tuning session would exercise them:
//!
//! 1. **The edit reaches the pixels.** A tinted `composite.wgsl` (the final fullscreen pass, so every
//!    pixel changes) is reloaded; the next `render.screenshot` differs from the pre-edit one — the
//!    renderer recreated the module + dependent pipeline. The edit → pixels latency is measured.
//! 2. **A bad edit never kills the session.** Syntactically-broken WGSL reloaded to the same path
//!    returns the structured `L2009` error, and the very next screenshot still renders — byte-identical
//!    to the last *good* (tinted) frame, proving the previous pipeline stayed live (keep-last-good).
//! 3. **Without the dev flag it is refused.** A separate, non-dev host returns the structured `L2008`
//!    for the same reload, and no shader is read from disk.
//!
//! A fourth test covers the decision-16 shared *snippets*: editing `snippets/fullscreen_vs.wgsl`
//! rebuilds every dependent pipeline (threshold/blur/composite/blit) transactionally — a valid edit
//! reaches the pixels, and a broken snippet edit is refused with `L2009` while all dependents keep
//! their last-good pipeline.
//!
//! Only a file inside the engine's shader directory stands in for an engine shader, so the tests that
//! edit one copy the directory into the temp project (`shader_tree`) and point the host at the copy
//! (`enable_shader_reload_from`); the real directory is only ever read. Files the renderer compiles
//! once (the GI bakes, the probe relight) are refused whole with `L2009`.
//!
//! Follows the Phase 7/10 reload-suite conventions: bind port 0, tear down at scope end, block on
//! replies rather than sleep, and gracefully skip the GPU-dependent assertions when the host has no
//! adapter.

use std::path::{Path, PathBuf};
use std::time::Instant;

use ludion_engine::Engine;
use ludion_protocol::Command;
use ludion_protocol::command::AssetReloadParams;
use ludion_server::Server;

/// A scene: one camera, one warm point light, and one lit cube filling the view — so both the scene
/// pass and the composite pass have real, non-black pixels to change.
const LIT_SCENE: &str = "\
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
  - name: block
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Mesh:
        shape: cube
";

/// The engine's renderer shader directory, resolved from this test crate's manifest dir
/// (`crates/ludion-server` → `crates/ludion-renderer/src/shaders`). The committed WGSL there is the
/// exact `include_str!` source the shipping renderer compiles.
fn renderer_shaders() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ludion-server lives under crates/")
        .join("ludion-renderer")
        .join("src")
        .join("shaders")
}

/// Copies the engine's shader directory to `root/shaders` (snippets included) and returns it.
fn shader_tree(root: &Path) -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let tree = root.join("shaders");
    copy(&renderer_shaders(), &tree);
    tree
}

/// A fresh, empty temp project directory unique to `label` and this process.
fn temp_project(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-shaderreload-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp project dir");
    dir
}

/// Binds a server serving `engine` on a free localhost port.
fn serve(engine: Engine) -> Server {
    Server::bind(engine, "127.0.0.1:0").expect("server binds")
}

/// A dev-flagged host reloads a tinted `composite.wgsl` and the rendered pixels change; a subsequent
/// broken-WGSL reload is refused with `L2009` while the session keeps rendering the last good frame.
#[test]
fn needs_gpu_dev_host_reloads_a_shader_and_keeps_last_good_on_a_bad_edit() {
    let root = temp_project("dev");
    let tree = shader_tree(&root);

    let mut engine = Engine::with_project_root(&root).with_render_size(256, 256);
    engine.load_inline_scene(LIT_SCENE).unwrap();
    engine.enable_shader_reload_from(&tree); // the dev flag
    ludion_engine::dispatch(&mut engine, Command::SimPause).unwrap();
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    // Baseline render. `L2003` here is the adapterless graceful skip.
    let baseline = match client.screenshot(Some(256), Some(256)) {
        Ok(shot) => shot.data,
        Err(diag) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diag)) => {
            eprintln!(
                "SKIPPED dev_host_reloads_a_shader_and_keeps_last_good_on_a_bad_edit: no GPU adapter"
            );
            return;
        }
        Err(diag) => panic!("baseline screenshot failed: {diag}"),
    };

    // Reloading a shader must never touch sim state.
    let hash_before = client.sim_hash().unwrap();

    // A valid edit that visibly changes every pixel: tint the final composite output pure red. The
    // reload path reads the shader from the given path, so write the modified copy into the tree.
    let original = std::fs::read_to_string(tree.join("composite.wgsl")).unwrap();
    let tinted = original.replace(
        "return vec4<f32>(color,",
        "return vec4<f32>(vec3<f32>(color.r, 0.0, 0.0),",
    );
    assert_ne!(
        tinted, original,
        "the tint edit must actually change the shader source"
    );
    let scratch_name = "shaders/composite.wgsl";
    let scratch = root.join(scratch_name);
    std::fs::write(&scratch, &tinted).unwrap();

    // Edit → pixels latency: reload over the wire, then read the changed frame.
    let started = Instant::now();
    client
        .call(Command::AssetReload(AssetReloadParams {
            path: Some(scratch_name.into()),
        }))
        .expect("a valid shader reload succeeds");
    let tinted_shot = client
        .screenshot(Some(256), Some(256))
        .expect("the session renders after a good reload")
        .data;
    let latency_ms = started.elapsed().as_millis();
    assert_ne!(
        tinted_shot, baseline,
        "the tinted composite must change the rendered pixels"
    );
    eprintln!("shader reload edit->pixels latency: {latency_ms} ms");

    // A *valid* WGSL edit that changes the binding interface — a new uniform at a binding the fixed,
    // size-pinned composite layout does not have. naga accepts the module, but pipeline creation is
    // incompatible with the existing bind group layout; the wgpu error scope catches that (rather than
    // an uncaptured-error crash) and it, too, surfaces as `L2009` with the last good pipeline intact.
    let binding_change = tinted.replace(
        "@group(0) @binding(2) var samp: sampler;",
        "@group(0) @binding(2) var samp: sampler;\n@group(0) @binding(4) var<uniform> extra: vec4<f32>;",
    ).replace(
        "return vec4<f32>(vec3<f32>(color.r, 0.0, 0.0),",
        "return vec4<f32>(vec3<f32>(color.r * extra.r, 0.0, 0.0),",
    );
    assert_ne!(
        binding_change, tinted,
        "the binding-change edit must alter the shader"
    );
    std::fs::write(&scratch, &binding_change).unwrap();
    let err = client
        .call(Command::AssetReload(AssetReloadParams {
            path: Some(scratch_name.into()),
        }))
        .expect_err("a binding-shape change must be rejected, not crash the session");
    assert_eq!(
        err.code.as_str(),
        "L2009",
        "a layout-incompatible reload is L2009: {err}"
    );
    assert_eq!(
        client
            .screenshot(Some(256), Some(256))
            .expect("the session survives a layout-incompatible reload")
            .data,
        tinted_shot,
        "a rejected binding-change reload keeps the last good pipeline"
    );

    // A broken edit to the same shader: the reload is refused with the structured `L2009`, and the
    // previous (tinted) pipeline stays live — the next frame is byte-identical to the last good one.
    std::fs::write(&scratch, "this is not valid wgsl {{{").unwrap();
    let err = client
        .call(Command::AssetReload(AssetReloadParams {
            path: Some(scratch_name.into()),
        }))
        .expect_err("a broken shader reload must be a structured error");
    assert_eq!(err.code.as_str(), "L2009", "bad WGSL is L2009: {err}");

    let after_bad = client
        .screenshot(Some(256), Some(256))
        .expect("the session still renders after a rejected reload")
        .data;
    assert_eq!(
        after_bad, tinted_shot,
        "a rejected reload keeps the last good pipeline — the frame is unchanged"
    );

    // Sim-invisible throughout: no shader reload moved the WorldHash.
    assert_eq!(
        client.sim_hash().unwrap().as_u64(),
        hash_before.as_u64(),
        "shader reload must not change the WorldHash"
    );

    server.shutdown_clean();
    let _ = std::fs::remove_dir_all(&root);
}

/// A shared *snippet* reload rebuilds **every** dependent pipeline transactionally
/// (`Plan/shader-bindgen.md` decision 16). Editing `snippets/fullscreen_vs.wgsl` — the fullscreen
/// vertex stage shared by the bloom threshold, blur, composite, and overlay-blit passes — changes the
/// composed source of all of them at once. A valid edit (mirror the sampled UV horizontally) visibly
/// changes the rendered pixels through the composite pass; a broken snippet edit is then refused with
/// `L2009` and *all* dependents keep their last-good pipeline (no half-applied snippet edit).
#[test]
fn needs_gpu_dev_host_reloads_a_snippet_and_rebuilds_all_dependents() {
    let root = temp_project("snippet");
    let tree = shader_tree(&root);

    let mut engine = Engine::with_project_root(&root).with_render_size(256, 256);
    engine.load_inline_scene(LIT_SCENE).unwrap();
    engine.enable_shader_reload_from(&tree);
    ludion_engine::dispatch(&mut engine, Command::SimPause).unwrap();
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    let baseline = match client.screenshot(Some(256), Some(256)) {
        Ok(shot) => shot.data,
        Err(diag) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diag)) => {
            eprintln!(
                "SKIPPED dev_host_reloads_a_snippet_and_rebuilds_all_dependents: no GPU adapter"
            );
            return;
        }
        Err(diag) => panic!("baseline screenshot failed: {diag}"),
    };
    let hash_before = client.sim_hash().unwrap();

    // The snippet lives under `shaders/snippets/`; its reload key is `snippets/fullscreen_vs.wgsl`,
    // so the edit is written into the copy of the tree and that path is reloaded.
    let snippet_path = tree.join("snippets").join("fullscreen_vs.wgsl");
    let original = std::fs::read_to_string(&snippet_path).unwrap();
    // A valid, binding-preserving edit: mirror the sampled UV in X. Every fullscreen pass then samples
    // the horizontally-flipped source, so the composite output (the final image) is visibly mirrored.
    let mirrored = original.replace(
        "out.uv = vec2<f32>(xy.x * 0.5 + 0.5, 1.0 - (xy.y * 0.5 + 0.5));",
        "out.uv = vec2<f32>(1.0 - (xy.x * 0.5 + 0.5), 1.0 - (xy.y * 0.5 + 0.5));",
    );
    assert_ne!(
        mirrored, original,
        "the UV-mirror edit must change the snippet"
    );
    let scratch_name = "shaders/snippets/fullscreen_vs.wgsl";
    let scratch = root.join(scratch_name);
    std::fs::write(&scratch, &mirrored).unwrap();

    let started = Instant::now();
    client
        .call(Command::AssetReload(AssetReloadParams {
            path: Some(scratch_name.into()),
        }))
        .expect("a valid snippet reload succeeds (all dependents rebuilt)");
    let mirrored_shot = client
        .screenshot(Some(256), Some(256))
        .expect("the session renders after a good snippet reload")
        .data;
    eprintln!(
        "snippet reload edit->pixels latency: {} ms",
        started.elapsed().as_millis()
    );
    assert_ne!(
        mirrored_shot, baseline,
        "the mirrored fullscreen VS must change the rendered pixels through its dependent pipelines"
    );

    // A broken snippet edit: every dependent's composed source now fails to parse. The reload is
    // refused with `L2009` and *all* dependents stay on their last-good pipeline — the next frame is
    // byte-identical to the last good (mirrored) one, proving the reload was transactional.
    std::fs::write(&scratch, "this is not valid wgsl {{{").unwrap();
    let err = client
        .call(Command::AssetReload(AssetReloadParams {
            path: Some(scratch_name.into()),
        }))
        .expect_err("a broken snippet reload must be a structured error");
    assert_eq!(
        err.code.as_str(),
        "L2009",
        "bad snippet WGSL is L2009: {err}"
    );
    assert_eq!(
        client
            .screenshot(Some(256), Some(256))
            .expect("the session still renders after a rejected snippet reload")
            .data,
        mirrored_shot,
        "a rejected snippet reload keeps every dependent's last-good pipeline"
    );

    assert_eq!(
        client.sim_hash().unwrap().as_u64(),
        hash_before.as_u64(),
        "snippet reload must not change the WorldHash"
    );

    server.shutdown_clean();
    let _ = std::fs::remove_dir_all(&root);
}

/// A non-dev host (no dev flag) refuses an `asset.reload` on a renderer shader path with the
/// structured `L2008` — the reload never reads disk, and the session is untouched. No GPU needed:
/// the not-enabled check precedes any renderer access.
#[test]
fn non_dev_host_refuses_a_shader_reload_with_l2008() {
    let root = temp_project("nondev");

    let mut engine = Engine::with_project_root(&root).with_render_size(64, 64);
    engine.load_inline_scene(LIT_SCENE).unwrap();
    // Deliberately NOT enabling shader reload.
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    let err = client
        .call(Command::AssetReload(AssetReloadParams {
            path: Some("crates/ludion-renderer/src/shaders/mesh.wgsl".into()),
        }))
        .expect_err("a shader reload on a non-dev host must be a structured error");
    assert_eq!(
        err.code.as_str(),
        "L2008",
        "a shader reload without the dev flag is L2008: {err}"
    );

    server.shutdown_clean();
    let _ = std::fs::remove_dir_all(&root);
}

/// An absolute `.wgsl` outside both the project root and the renderer's shader dir is refused with
/// `L1321` before any read. No GPU needed.
#[test]
fn an_absolute_shader_path_outside_both_roots_is_refused() {
    let root = temp_project("escape");
    let outside = temp_project("escape-outside");
    let stray = outside.join("composite.wgsl");
    std::fs::write(&stray, "this is not valid wgsl {{{").unwrap();

    let mut engine = Engine::with_project_root(&root).with_render_size(64, 64);
    engine.load_inline_scene(LIT_SCENE).unwrap();
    engine.enable_shader_reload();
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    for path in [
        stray.to_string_lossy().into_owned(),
        "../composite.wgsl".to_owned(),
    ] {
        let err = client
            .call(Command::AssetReload(AssetReloadParams { path: Some(path) }))
            .expect_err("a shader path outside the roots must be refused");
        // A read of the stray file would have been L2009 (invalid WGSL), never L1321.
        assert_eq!(err.code.as_str(), "L1321", "{err}");
        assert!(
            err.help.is_some(),
            "every L1321 here says where shaders live: {err}"
        );
    }

    server.shutdown_clean();
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

/// A project file named like an engine shader is not that shader: only a file inside the engine's
/// shader directory stands in for one. A relative path is joined to the project root, so
/// `<root>/composite.wgsl` is refused with `L1321` before any read. No GPU needed.
#[test]
fn a_project_file_named_like_an_engine_shader_is_refused() {
    let root = temp_project("lookalike");
    std::fs::write(root.join("composite.wgsl"), "this is not valid wgsl {{{").unwrap();

    let mut engine = Engine::with_project_root(&root).with_render_size(64, 64);
    engine.load_inline_scene(LIT_SCENE).unwrap();
    engine.enable_shader_reload();
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    let err = client
        .call(Command::AssetReload(AssetReloadParams {
            path: Some("composite.wgsl".into()),
        }))
        .expect_err("a project file is not an engine shader");
    assert_eq!(err.code.as_str(), "L1321", "{err}");

    server.shutdown_clean();
    let _ = std::fs::remove_dir_all(&root);
}

/// The shaders the renderer compiles once are refused whole with `L2009` and a rebuild message, with
/// nothing applied. No GPU needed: the refusal precedes the file read.
#[test]
fn compile_once_shaders_are_refused_with_a_rebuild_message() {
    let root = temp_project("compile-once");
    let tree = shader_tree(&root);

    let mut engine = Engine::with_project_root(&root).with_render_size(64, 64);
    engine.load_inline_scene(LIT_SCENE).unwrap();
    engine.enable_shader_reload_from(&tree);
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    for path in [
        "shaders/gi_bake.wgsl",
        "shaders/gi_inject.wgsl",
        "shaders/probe_relight.wgsl",
        "shaders/snippets/gi_rays.wgsl",
        "shaders/snippets/math.wgsl",
    ] {
        let err = client
            .call(Command::AssetReload(AssetReloadParams {
                path: Some(path.into()),
            }))
            .expect_err(path);
        assert_eq!(err.code.as_str(), "L2009", "{path}: {err}");
        assert!(
            err.message.to_lowercase().contains("rebuild"),
            "{path}: {err}"
        );
    }

    server.shutdown_clean();
    let _ = std::fs::remove_dir_all(&root);
}

/// Without the dev flag a compute shader is a renderer shader too: `L2008`, not a silent success.
#[test]
fn a_non_dev_host_refuses_a_compute_shader_with_l2008() {
    let root = temp_project("nondev-compute");

    let mut engine = Engine::with_project_root(&root).with_render_size(64, 64);
    engine.load_inline_scene(LIT_SCENE).unwrap();
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    let err = client
        .call(Command::AssetReload(AssetReloadParams {
            path: Some("crates/ludion-renderer/src/shaders/ibl.wgsl".into()),
        }))
        .expect_err("a compute shader reload on a non-dev host must be refused");
    assert_eq!(err.code.as_str(), "L2008", "{err}");

    server.shutdown_clean();
    let _ = std::fs::remove_dir_all(&root);
}

/// Inside the shader directory is not enough: the file's path there must be an engine shader's. A
/// root-level `camera.wgsl` and `snippets/mesh.wgsl` are neither. No GPU needed.
#[test]
fn a_stray_file_inside_the_dir_is_not_an_engine_shader() {
    let root = temp_project("stray");
    let tree = shader_tree(&root);
    std::fs::write(tree.join("camera.wgsl"), "// stray").unwrap();
    std::fs::write(tree.join("snippets").join("mesh.wgsl"), "// stray").unwrap();

    let mut engine = Engine::with_project_root(&root).with_render_size(64, 64);
    engine.load_inline_scene(LIT_SCENE).unwrap();
    engine.enable_shader_reload_from(&tree);
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    for path in ["shaders/camera.wgsl", "shaders/snippets/mesh.wgsl"] {
        let err = client
            .call(Command::AssetReload(AssetReloadParams {
                path: Some(path.into()),
            }))
            .expect_err(path);
        assert_eq!(err.code.as_str(), "L2009", "{path}: {err}");
    }

    server.shutdown_clean();
    let _ = std::fs::remove_dir_all(&root);
}

/// A host enabled with the compiled-in directory reloads a real shader by its absolute path. The file
/// is only read; the render after it is the same shader, so the frame is unchanged.
#[test]
fn needs_gpu_the_compiled_in_dir_reloads_a_real_shader() {
    let root = temp_project("real-dir");

    let mut engine = Engine::with_project_root(&root).with_render_size(128, 128);
    engine.load_inline_scene(LIT_SCENE).unwrap();
    engine.enable_shader_reload();
    ludion_engine::dispatch(&mut engine, Command::SimPause).unwrap();
    let server = serve(engine);
    let mut client = ludion_client::Client::connect(&server.url()).unwrap();

    let baseline = match client.screenshot(Some(128), Some(128)) {
        Ok(shot) => shot.data,
        Err(diag) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diag)) => {
            eprintln!("SKIPPED the_compiled_in_dir_reloads_a_real_shader: no GPU adapter");
            return;
        }
        Err(diag) => panic!("baseline screenshot failed: {diag}"),
    };
    let real = renderer_shaders().join("composite.wgsl");
    client
        .call(Command::AssetReload(AssetReloadParams {
            path: Some(real.to_string_lossy().into_owned()),
        }))
        .expect("the compiled-in directory's own shader reloads");
    assert_eq!(
        client.screenshot(Some(128), Some(128)).unwrap().data,
        baseline,
        "reloading a shader with its own source leaves the frame unchanged"
    );

    server.shutdown_clean();
    let _ = std::fs::remove_dir_all(&root);
}
