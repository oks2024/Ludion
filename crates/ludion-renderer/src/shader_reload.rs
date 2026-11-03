//! Dev shader hot reload: which files can be reloaded, and the pieces of a reload that live outside
//! `Renderer`'s own pipeline table.
//!
//! A reload key is the file's path inside the engine's shader directory, `/`-joined: `mesh.wgsl` or
//! `snippets/camera.wgsl`. Matching is exact, so `snippets/mesh.wgsl` and a root-level `camera.wgsl`
//! name no engine shader. A reload either applies everywhere its source is compiled or is refused
//! whole:
//!
//! - any render shader body or snippet: its pipelines are rebuilt, and when `blit.wgsl` or a snippet
//!   it composes changes, the mip-chain blit ([`MipGen`]) with them;
//! - `ibl.wgsl` and the snippets only it composes (`dfg`, `sky_gradient`): the [`Ibl`] is rebuilt, its
//!   LUT recomputed and its sky cubes dropped;
//! - the other compute shaders and the snippets they compose (`gi_rays`, and `math`, which feeds
//!   them): refused with `L2009`, because they are compiled once per renderer and replacing a bake
//!   would drop its volumes.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_render_graph::Gpu;

use crate::compute_shader::ComputeShader;
use crate::ibl::Ibl;
use crate::mipgen::MipGen;
use crate::render::{ShaderId, Snippet};

/// What a reload key names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReloadTarget {
    Body(ShaderId),
    Snippet(Snippet),
    IblBody,
}

fn invalid(message: String) -> Diagnostic {
    Diagnostic::new(code::SHADER_RELOAD_INVALID, message)
}

fn refused(key: &str, shader: ComputeShader) -> Diagnostic {
    invalid(format!(
        "`{key}` is compiled once into {}'s compute pipeline, so a hot reload cannot apply it \
         everywhere; nothing was changed. Rebuild and restart the host to pick up the edit",
        shader.file_name()
    ))
    .with_help(
        "shaders are compiled into the binary, and `ludion dev` does not rebuild on a shader edit: \
         stop the host, rebuild and rerun",
    )
}

/// Maps a reload key to the shader it names.
///
/// # Errors
/// `L2009` if no engine shader or snippet has that key, or if it is one the renderer compiles once.
pub(crate) fn classify(key: &str) -> Result<ReloadTarget, Diagnostic> {
    let unknown = || {
        invalid(format!(
            "`{key}` is not a renderer shader or snippet, so it cannot be hot-reloaded"
        ))
    };
    if let Some(name) = key.strip_prefix("snippets/") {
        let snippet = Snippet::ALL
            .into_iter()
            .find(|s| s.file_name() == name)
            .ok_or_else(unknown)?;
        let compiled_once = ComputeShader::ALL
            .into_iter()
            .find(|c| *c != ComputeShader::Ibl && c.snippets().contains(&snippet));
        return match compiled_once {
            Some(shader) => Err(refused(key, shader)),
            None => Ok(ReloadTarget::Snippet(snippet)),
        };
    }
    if let Some(id) = ShaderId::all().find(|id| id.file_name() == key) {
        return Ok(ReloadTarget::Body(id));
    }
    match ComputeShader::ALL
        .into_iter()
        .find(|c| c.file_name() == key)
    {
        Some(ComputeShader::Ibl) => Ok(ReloadTarget::IblBody),
        Some(shader) => Err(refused(key, shader)),
        None => Err(unknown()),
    }
}

/// Whether `key` could be hot-reloaded, with the refusal a host reports when it could not. Needs no
/// GPU, so a host can refuse before it reads the file.
///
/// # Errors
/// `L2009`, as [`classify`].
pub fn check_shader_reload(key: &str) -> Result<(), Diagnostic> {
    classify(key).map(|_| ())
}

/// Whether `path`'s file name is one of the renderer's `.wgsl` files — render shader, compute shader
/// or snippet. The membership test an `asset.reload` handler uses to route a `.wgsl` to the shader
/// branch (or to `L2008`) instead of the texture, mesh and document branches; the reload key itself
/// is [`classify`]'s.
pub fn is_renderer_shader(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    ShaderId::all().any(|id| id.file_name() == name)
        || ComputeShader::ALL
            .into_iter()
            .any(|c| c.file_name() == name)
        || Snippet::ALL.into_iter().any(|s| s.file_name() == name)
}

/// Validates reloaded WGSL with naga — the exact front-end and validator wgpu runs at
/// `create_shader_module` — so a parse/type error becomes a readable `L2009` and never reaches the
/// GPU (keep-last-good: the caller returns before touching anything live).
pub(crate) fn validate_wgsl(name: &str, source: &str) -> Result<(), Diagnostic> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| {
        invalid(format!(
            "{name}: reloaded WGSL failed to parse — the previous shader stays live\n{}",
            e.emit_to_string(source),
        ))
    })?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::default(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| {
        invalid(format!(
            "{name}: reloaded WGSL failed validation — the previous shader stays live\n{}",
            e.emit_to_string(source),
        ))
    })?;
    Ok(())
}

/// The `L2009` for a reloaded shader that parsed and validated but whose rebuilt pipeline wgpu
/// rejected (a binding-shape change against the size-pinned layouts, or an entry point that no
/// longer exists). The wgpu error scope caught it, so the previous pipeline is untouched.
pub(crate) fn pipeline_incompatible(name: &str, error: wgpu::Error) -> Diagnostic {
    invalid(format!(
        "{name}: the reloaded shader's pipeline is incompatible with the existing bind group \
         layouts (a binding-shape change) — the previous shader stays live\n{error}",
    ))
    .with_help(
        "a hot-reloaded shader may change only its body, not its interface: keep uniform/struct \
         sizes and @group/@binding slots as they were, or rebuild the engine to change them",
    )
}

/// Builds a fresh [`Ibl`] from the candidate composition `composed`: naga first, then the pipelines
/// and the LUT under error scopes. The live `Ibl` is untouched either way.
pub(crate) fn build_ibl(gpu: &Gpu, composed: &str) -> Result<Ibl, Diagnostic> {
    let name = ComputeShader::Ibl.file_name();
    validate_wgsl(name, composed)?;
    Ibl::from_source(gpu, composed).map_err(|error| pipeline_incompatible(name, error))
}

/// Builds a fresh [`MipGen`] from the candidate composition of `blit.wgsl`, as [`build_ibl`].
pub(crate) fn build_mipgen(
    gpu: &Gpu,
    layout: &wgpu::BindGroupLayout,
    composed: &str,
) -> Result<MipGen, Diagnostic> {
    let name = ShaderId::Blit.file_name();
    validate_wgsl(name, composed)?;
    let device = gpu.device();
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mipgen = MipGen::new(device, layout, composed);
    match pollster::block_on(scope.pop()) {
        Some(error) => Err(pipeline_incompatible(name, error)),
        None => Ok(mipgen),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(result: Result<ReloadTarget, Diagnostic>) -> String {
        result.expect_err("refused").code.to_string()
    }

    #[test]
    fn keys_match_exactly() {
        assert_eq!(
            classify("mesh.wgsl").unwrap(),
            ReloadTarget::Body(ShaderId::Mesh)
        );
        assert_eq!(
            classify("snippets/camera.wgsl").unwrap(),
            ReloadTarget::Snippet(Snippet::Camera)
        );
        assert_eq!(classify("ibl.wgsl").unwrap(), ReloadTarget::IblBody);
        for key in [
            "camera.wgsl",
            "snippets/mesh.wgsl",
            "a/mesh.wgsl",
            "snippets/snippets/camera.wgsl",
            "",
        ] {
            assert_eq!(code_of(classify(key)), "L2009", "{key}");
        }
    }

    #[test]
    fn compile_once_shaders_and_their_snippets_are_refused() {
        for key in [
            "gi_bake.wgsl",
            "gi_inject.wgsl",
            "probe_relight.wgsl",
            "snippets/gi_rays.wgsl",
            "snippets/math.wgsl",
        ] {
            let diag = classify(key).expect_err(key);
            assert_eq!(diag.code.to_string(), "L2009", "{key}");
            assert!(diag.message.contains("compiled once"), "{key}: {diag}");
            assert!(
                diag.message.contains("Rebuild and restart"),
                "{key}: {diag}"
            );
        }
        for key in ["snippets/dfg.wgsl", "snippets/sky_gradient.wgsl"] {
            assert!(classify(key).is_ok(), "{key}");
        }
    }

    /// Every `.wgsl` the crate ships either reloads or is refused with a reason, so a new shader
    /// must take a side.
    #[test]
    fn every_shipped_wgsl_takes_a_side() {
        let dir = std::path::Path::new(crate::render::SHADER_SOURCE_DIR);
        for (sub, prefix) in [(dir.to_owned(), ""), (dir.join("snippets"), "snippets/")] {
            for entry in std::fs::read_dir(&sub).expect("shader dir") {
                let path = entry.expect("entry").path();
                if path.extension().is_none_or(|e| e != "wgsl") {
                    continue;
                }
                let key = format!("{prefix}{}", path.file_name().unwrap().to_string_lossy());
                match classify(&key) {
                    Ok(_) => {}
                    Err(diag) => assert!(
                        diag.message.contains("compiled once"),
                        "{key} is neither reloadable nor deliberately refused: {diag}"
                    ),
                }
                assert!(is_renderer_shader(&key), "{key}");
            }
        }
    }

    #[test]
    fn the_membership_test_is_by_file_name() {
        assert!(is_renderer_shader("a/b/ibl.wgsl"));
        assert!(is_renderer_shader("C:\\x\\gi_rays.wgsl"));
        assert!(!is_renderer_shader("notes.wgsl"));
    }
}
