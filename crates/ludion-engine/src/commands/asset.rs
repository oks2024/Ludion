//! `asset.reload` handler.
//!
//! Reloading drops the CPU [`AssetCache`](crate::asset_cache) entry for the named asset, re-reads and
//! re-decodes it from disk, fires the `asset.reloaded {path, hash}` event through the engine outbox,
//! and lets a *paused* engine re-render it. This is the real reload the Phase-7 asset cache finally
//! gives `asset.reload` something to invalidate (`Plan/phase-7-designer-flow.md` decision 5).
//!
//! **The membership filter is engine-side.** A reload for a path this engine does not cache — an
//! editor temp file, unrelated churn a host forwards — is a cheap no-op: no event, no error, an empty
//! `AssetsReloaded` reply. So a host can forward *everything* under the asset root and the engine
//! ignores what it does not hold; there is no cross-thread sharing of cache membership.
//!
//! **The paused re-render needs no special wiring.** `asset.reload` is an ordinary drained command, so
//! `run_live`'s drain loop trips `frame_dirty` for it exactly as it does for a `component.set` — the
//! next egress pass re-renders even while paused, and because the reload already replaced the cache
//! entry, that render reads the *new* bytes. Asset bytes never enter the `WorldHash`, so a reload is
//! sim-invisible and replay-safe.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::ResponseBody;
use ludion_protocol::command::AssetReloadParams;
use ludion_protocol::event::{AssetReloadedData, Event};

use crate::engine::Engine;

/// `asset.reload` — reload one cached asset by path, or ignore a path this engine does not hold.
pub(crate) fn reload(
    engine: &mut Engine,
    params: AssetReloadParams,
) -> Result<ResponseBody, Diagnostic> {
    let Some(path) = params.path else {
        // No path given: nothing to reload (kept for wire compatibility with the empty-reply shape).
        return Ok(ResponseBody::AssetsReloaded { paths: Vec::new() });
    };

    // A renderer shader (`.wgsl`) is a dev-only reloadable asset (Plan/shader-bindgen.md decisions
    // 10–11). Under the dev flag the renderer recreates the module + dependent pipeline (keeping the
    // last good pipeline on a validation failure); without it, a structured `L2008`. This is checked
    // before the cache/doc branches because a shader is neither a cached CPU asset nor a data-doc — it
    // is render-side only (no `WorldHash`, never recorded), so it takes neither of those paths.
    if ludion_renderer::is_renderer_shader(&path) {
        if !engine.shader_reload_enabled() {
            return Err(Diagnostic::new(
                code::SHADER_RELOAD_DISABLED,
                format!(
                    "`{path}` is a renderer shader, but this host does not have shader hot-reload \
                     enabled"
                ),
            )
            .with_help(
                "start the host with the dev flag (`serve --dev-shaders`, a game run with \
                 `--dev-shaders`, or `ludion dev`); otherwise a shader change needs an engine rebuild",
            ));
        }
        let hash = engine.reload_shader(&path)?;
        engine.push_event(Event::AssetReloaded(AssetReloadedData {
            path: path.clone(),
            hash: format!("{hash:016x}"),
        }));
        return Ok(ResponseBody::AssetsReloaded { paths: vec![path] });
    }

    let root = engine.project_root().to_path_buf();
    if let Some(key) = engine.asset_cache().resolve_key(&root, &path) {
        // A cached CPU asset (texture/mesh/voxel): drop the stale entry and re-read from disk. An
        // `L1101`/`L1102` here (the file went missing or is now unreadable) is reported honestly, not
        // faked as a success. This path is render-only and never recorded.
        let hash = engine.asset_cache_mut().reload(&root, &key)?;
        engine.push_event(Event::AssetReloaded(AssetReloadedData {
            path: path.clone(),
            hash: format!("{hash:016x}"),
        }));
        return Ok(ResponseBody::AssetsReloaded { paths: vec![path] });
    }

    // Not a cached asset — maybe a registered game data-doc. This is sim-visible state (systems derive
    // components from the `DocStore`), so the swap bumps the store's generation and, when recording is
    // armed, is written to the replay as a `DocReload` record (Phase 15 / E4, decision 9). A malformed
    // edit is a diagnosed `L1119`/`L1120` error and leaves the store unchanged; so is an edit that
    // makes a registered validator report a new error (`L1144`, `L1149`–`L1152`).
    if let Some(hash) = engine.reload_doc_from_disk(&path)? {
        engine.push_event(Event::AssetReloaded(AssetReloadedData {
            path: path.clone(),
            hash: format!("{hash:016x}"),
        }));
        return Ok(ResponseBody::AssetsReloaded { paths: vec![path] });
    }

    // A prefab a spawn already loaded: refreshed eagerly, a bad edit refused with the old entry kept.
    // Live instances are untouched; an applied reload is recorded as a `prefab_text` reload record.
    if let Some(hash) = engine.reload_prefab(&path)? {
        engine.push_event(Event::AssetReloaded(AssetReloadedData {
            path: path.clone(),
            hash: format!("{hash:016x}"),
        }));
        return Ok(ResponseBody::AssetsReloaded { paths: vec![path] });
    }

    // A font table or face the library holds: rebuilt eagerly. Refused while recording (`L1255`).
    if let Some(hash) = engine.reload_fonts(&path)? {
        engine.push_event(Event::AssetReloaded(AssetReloadedData {
            path: path.clone(),
            hash: format!("{hash:016x}"),
        }));
        return Ok(ResponseBody::AssetsReloaded { paths: vec![path] });
    }

    // Neither a cached asset nor a registered doc nor a held prefab or font — ignore it (no event, no error).
    Ok(ResponseBody::AssetsReloaded { paths: Vec::new() })
}
