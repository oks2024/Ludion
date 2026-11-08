//! `ludion render --dump-probes` — each reflection probe's captured cube and prefiltered chain as
//! PNG strips, plus the manifest that names them.
//!
//! [`render_probe_dump`] is a verification path (`Plan/reflections.md` decision 22): it captures
//! every probe fresh and never reads or writes the probe store, so the strips are a function of the
//! scene alone. [`DumpedProbe::stored`] only reports whether a `Store` host at this quality would
//! find the probe's entry. The strips carry no exposure: a value in [0, 1] reads back as its
//! sRGB-encoded byte.

use std::path::{Path, PathBuf};

use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::Registry;
use ludion_renderer::{FrameSize, ProbeReadback, ProbeVolume, RenderedImage, Renderer};
use ludion_scene::{AppliedOverride, SceneOverride};

use crate::debug_draw::DebugCategories;
use crate::quality::QualitySetting;
use crate::render::extract_scene_file;

/// The result of a probe dump.
#[derive(Debug)]
pub struct ProbeDumpOutcome {
    /// The probe store directory a `Store` host of this scene would read.
    pub store: PathBuf,
    /// The cube edge in texels.
    pub face_size: u32,
    /// The scene's probes in [`probe_rank`](ludion_renderer::probe_rank) order.
    pub probes: Vec<DumpedProbe>,
    /// Whose extract the keys come from: `"store-host"` (a host reading cooked textures, so a key
    /// is the one the store holds) or `"dump"` (this dump's own extract, when `--set` overrides
    /// changed the scene).
    pub key_source: &'static str,
    /// What each `--set` override changed, in the order given.
    pub applied_overrides: Vec<AppliedOverride>,
}

/// One probe of a dump.
#[derive(Debug)]
pub struct DumpedProbe {
    /// The probe's entity.
    pub entity: EntityId,
    /// The probe's position in `probe_rank` order: 0 is shaded first.
    pub rank: u32,
    /// The probe's cube-array index: its position in entity order.
    pub slot: u32,
    /// The probe's influence and parallax volume.
    pub volume: ProbeVolume,
    /// World-space capture point.
    pub position: [f32; 3],
    /// The store key at the dump's quality.
    pub key: u64,
    /// The store holds this probe's entry.
    pub stored: bool,
    /// The raw capture, the six faces side by side (`6f × f`).
    pub captured: Option<RenderedImage>,
    /// The prefiltered chain, one row per mip of the six faces (`6f × Σ edge`).
    pub prefiltered: RenderedImage,
    /// The mean alpha of the capture: the fraction of the view that is sky.
    pub sky_visible: f32,
    /// Wall time to capture and prefilter this probe, in milliseconds.
    pub capture_ms: Option<f64>,
}

/// Captures the reflection probes of the scene at `path` fresh and reads them back.
///
/// # Errors
/// As `extract_scene_file`'s scene errors; `L2003`/`L2004` if no GPU adapter or device is available.
pub fn render_probe_dump(
    registry: &Registry,
    path: &Path,
    size: FrameSize,
    quality: &QualitySetting,
    overrides: &[SceneOverride],
) -> Result<ProbeDumpOutcome, Vec<Diagnostic>> {
    let (scene, applied_overrides, _) = extract_scene_file(
        registry,
        path,
        size,
        overrides,
        0,
        &quality.params,
        &DebugCategories::None,
    )?;
    let mut renderer = Renderer::new().map_err(|d| vec![d])?;
    renderer.set_quality(quality.quality, quality.params);
    let mut clock = crate::profile::SpanClock::default();
    let readbacks = renderer
        .read_probes(&scene, Some(&mut clock))
        .map_err(|d| vec![d])?;
    let (keys, key_source) = if overrides.is_empty() {
        (store_host_keys(path, quality)?, "store-host")
    } else {
        (renderer.probe_keys(&scene), "dump")
    };

    let store = crate::probe_bake::store_dir(path.parent().unwrap_or_else(|| Path::new(".")));
    let face_size = quality.params.ibl_specular_size;
    let mut probes: Vec<DumpedProbe> = readbacks
        .into_iter()
        .map(|readback| {
            let data = &scene.probes[readback.slot as usize];
            let key = keys
                .get(readback.slot as usize)
                .copied()
                .unwrap_or(readback.key);
            let capture_ms = clock
                .spans
                .get(readback.slot as usize)
                .map(|span| span.as_secs_f64() * 1000.0);
            let mut probe = dumped(&store, readback, key, data.volume, data.position.to_array());
            probe.capture_ms = capture_ms;
            probe
        })
        .collect();
    probes.sort_by_key(|probe| probe.rank);
    Ok(ProbeDumpOutcome {
        store,
        face_size,
        probes,
        key_source,
        applied_overrides,
    })
}

/// The probe keys a `Store` host loading the scene at `path` computes: its extract reads cooked
/// textures, as [`Engine::bake_probes`](crate::Engine::bake_probes) forces.
fn store_host_keys(path: &Path, quality: &QualitySetting) -> Result<Vec<u64>, Vec<Diagnostic>> {
    let mut engine = crate::Engine::builder()
        .patterns()
        .load_scene_file(path)
        .map_err(|d| vec![d])?;
    engine.set_quality(quality.clone());
    engine.set_textures(crate::TextureSource::Cooked);
    let scene = engine.extract_render_scene(64, 64)?;
    let renderer = engine.renderer().map_err(|d| vec![d])?;
    Ok(renderer.probe_keys(&scene))
}

fn dumped(
    store: &Path,
    readback: ProbeReadback,
    key: u64,
    volume: ProbeVolume,
    position: [f32; 3],
) -> DumpedProbe {
    let ProbeReadback {
        entity,
        slot,
        rank,
        key: _,
        face_size,
        captured,
        levels,
    } = readback;
    let alpha_of = |texels: &[f32]| {
        let alphas = texels.chunks_exact(4).map(|t| t[3]);
        alphas.clone().sum::<f32>() / alphas.count().max(1) as f32
    };
    let sky_visible = match (&captured, levels.first()) {
        (Some(captured), _) => alpha_of(captured),
        (None, Some(mip0)) => alpha_of(mip0),
        (None, None) => 0.0,
    };
    DumpedProbe {
        entity,
        rank,
        slot,
        volume,
        position,
        key,
        stored: store
            .join(ludion_assets::cooked::cooked_file_name(key))
            .is_file(),
        captured: captured.map(|faces| strip(&[(faces, face_size)])),
        prefiltered: strip(
            &levels
                .into_iter()
                .enumerate()
                .map(|(mip, level)| (level, (face_size >> mip).max(1)))
                .collect::<Vec<_>>(),
        ),
        sky_visible,
        capture_ms: None,
    }
}

/// Rows of six faces side by side, one row per `(texels, edge)` level, left-aligned on black.
fn strip(levels: &[(Vec<f32>, u32)]) -> RenderedImage {
    let first = levels.first().map_or(1, |(_, edge)| *edge);
    let width = 6 * first;
    let height: u32 = levels.iter().map(|(_, edge)| edge).sum();
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for texel in rgba.chunks_exact_mut(4) {
        texel[3] = 255;
    }
    let mut top = 0;
    for (texels, edge) in levels {
        let edge = *edge as usize;
        for face in 0..6 {
            for y in 0..edge {
                for x in 0..edge {
                    let from = ((face * edge + y) * edge + x) * 4;
                    let to = (((top + y) * width as usize) + face * edge + x) * 4;
                    for c in 0..3 {
                        rgba[to + c] = ludion_assets::encode_srgb(texels[from + c].clamp(0.0, 1.0));
                    }
                }
            }
        }
        top += edge;
    }
    RenderedImage {
        width,
        height,
        rgba,
    }
}

impl ProbeDumpOutcome {
    /// The dump's `probes.json` document, probes in rank order. `rank` is the probe's position in
    /// `probe_rank` order (the `probe_index` view's colour); `slot` is its entity-order index into
    /// the cube array. `scene` and `quality` are the names the caller ran with.
    pub fn manifest(&self, scene: &Path, quality: &str) -> serde_json::Value {
        serde_json::json!({
            "scene": scene.display().to_string(),
            "store": self.store.display().to_string(),
            "quality": quality,
            "face_size": self.face_size,
            "key_source": self.key_source,
            "probes": self.probes.iter().map(DumpedProbe::manifest).collect::<Vec<_>>(),
        })
    }
}

impl DumpedProbe {
    /// The file name of this probe's captured strip. Files are named by rank, not slot.
    pub fn captured_file(&self) -> String {
        format!("probe-{}-captured.png", self.rank)
    }

    /// The file name of this probe's prefiltered strip.
    pub fn prefiltered_file(&self) -> String {
        format!("probe-{}-prefiltered.png", self.rank)
    }

    /// `box` or `sphere`.
    pub fn shape_name(&self) -> &'static str {
        match self.volume {
            ProbeVolume::Box { .. } => "box",
            ProbeVolume::Sphere { .. } => "sphere",
        }
    }

    /// This probe's `probes.json` entry; the one place its fields are listed.
    fn manifest(&self) -> serde_json::Value {
        let shape = match self.volume {
            ProbeVolume::Box { extent } => {
                serde_json::json!({ "box": { "extent": extent.to_array() } })
            }
            ProbeVolume::Sphere { radius } => serde_json::json!({ "sphere": { "radius": radius } }),
        };
        serde_json::json!({
            "rank": self.rank,
            "slot": self.slot,
            "entity": self.entity.to_string(),
            "shape": shape,
            "position": self.position,
            "key": format!("{:016x}", self.key),
            "stored": self.stored,
            "sky_visible": self.sky_visible,
            "capture_ms": self.capture_ms,
            "captured": self.captured.as_ref().map(|_| self.captured_file()),
            "prefiltered": self.prefiltered_file(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;

    const SCENE: &str = "format: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 1.5
          - 6.0
      Camera: {}
  - name: environment
    components:
      Environment: {}
  - name: block
    components:
      Transform:
        translation:
          - 0.0
          - 0.5
          - 0.0
      Mesh:
        shape: cube
  - name: left
    components:
      Transform:
        translation:
          - -3.0
          - 1.0
          - 0.0
      ReflectionProbe: {}
  - name: right
    components:
      Transform:
        translation:
          - 3.0
          - 1.0
          - 0.0
      ReflectionProbe:
        shape:
          sphere:
            radius: 4.0
";

    fn scene_file(tag: &str, source: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-probe-dump-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scene.yaml");
        std::fs::write(&path, source).unwrap();
        path
    }

    fn dump(path: &Path) -> Option<ProbeDumpOutcome> {
        let registry = crate::components::builtin_registry();
        match render_probe_dump(
            &registry,
            path,
            FrameSize::native(64, 64),
            &QualitySetting::full(),
            &[],
        ) {
            Ok(outcome) => Some(outcome),
            Err(d) if ludion_core::require_gpu::is_no_adapter(&d) => {
                eprintln!("SKIPPED probe dump: no GPU adapter");
                None
            }
            Err(d) => panic!("probe dump failed: {d:?}"),
        }
    }

    #[test]
    fn needs_gpu_a_dump_reads_every_probe_and_flags_the_store() {
        let path = scene_file("two", SCENE);
        let Some(first) = dump(&path) else {
            return;
        };
        assert_eq!(first.probes.len(), 2);
        let f = first.face_size;
        // the chain stops at an 8-texel face
        let edges: u32 = (0..)
            .map(|mip| f >> mip)
            .take_while(|&edge| edge >= 8)
            .sum();
        for probe in &first.probes {
            let captured = probe.captured.as_ref().expect("a fresh capture");
            assert_eq!((captured.width, captured.height), (6 * f, f));
            assert_eq!(probe.prefiltered.width, 6 * f);
            assert_eq!(probe.prefiltered.height, edges);
            assert!(!probe.stored);
            assert!(probe.capture_ms.is_some_and(|ms| ms > 0.0), "{probe:?}");
        }
        assert_eq!(first.probes[0].shape_name(), "box");
        assert_eq!(first.probes[1].shape_name(), "sphere");

        std::fs::create_dir_all(&first.store).unwrap();
        std::fs::write(
            first
                .store
                .join(ludion_assets::cooked::cooked_file_name(first.probes[0].key)),
            [],
        )
        .unwrap();
        let second = dump(&path).expect("the adapter is still there");
        assert_eq!(
            second.probes.iter().map(|p| p.stored).collect::<Vec<_>>(),
            [true, false]
        );
        assert_eq!(second.probes[0].key, first.probes[0].key);

        let manifest = second.manifest(&path, "full");
        for (entry, probe) in manifest["probes"]
            .as_array()
            .unwrap()
            .iter()
            .zip(&second.probes)
        {
            let key = entry["key"].as_str().unwrap();
            assert_eq!(key.len(), 16);
            assert_eq!(u64::from_str_radix(key, 16).unwrap(), probe.key);
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn needs_gpu_rank_is_the_static_order_and_slot_the_entity_order() {
        let source = SCENE.replace(
            "      ReflectionProbe:\n        shape:",
            "      ReflectionProbe:\n        priority: 5\n        shape:",
        );
        let path = scene_file("rank", &source);
        let Some(outcome) = dump(&path) else {
            return;
        };
        let ranked: Vec<_> = outcome.probes.iter().map(|p| (p.rank, p.slot)).collect();
        assert_eq!(ranked, [(0, 1), (1, 0)]);
        assert_eq!(outcome.probes[0].shape_name(), "sphere");
        let manifest = outcome.manifest(&path, "full");
        assert_eq!(manifest["probes"][0]["rank"], 0);
        assert_eq!(manifest["probes"][0]["slot"], 1);
        assert_eq!(outcome.probes[0].captured_file(), "probe-0-captured.png");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn needs_gpu_a_probeless_scene_dumps_nothing() {
        let source = SCENE.split("  - name: left").next().unwrap();
        let path = scene_file("none", source);
        let Some(outcome) = dump(&path) else {
            return;
        };
        assert!(outcome.probes.is_empty());
        assert_eq!(
            outcome.manifest(&path, "full")["probes"],
            serde_json::json!([])
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A textured scene's keys are the ones a bake stored: the dump reads them through a
    /// cooked-texture host, whatever its own pixels are drawn from.
    #[test]
    fn needs_gpu_a_textured_scene_is_stored_after_a_bake() {
        use ludion_assets::cooked::{ColorSpace, CookSettings, cook_key, cooked_file_name};
        let Ok(renderer) = Renderer::new() else {
            eprintln!("SKIPPED textured probe dump: no GPU adapter");
            return;
        };
        let Some(target) = renderer.cook_target() else {
            eprintln!("SKIPPED textured probe dump: no BC7 or ASTC support");
            return;
        };
        let dir =
            std::env::temp_dir().join(format!("ludion-probe-dump-tex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets/kit/k/meshes")).unwrap();
        std::fs::create_dir_all(dir.join("assets/kit/k/textures")).unwrap();
        std::fs::write(
            dir.join("ludion.toml"),
            "name = \"fixture\"\ndefault_scene = \"scenes/main.yaml\"\n\n[[import]]\nfrom = \"art/*.gltf\"\nto = \"assets/kit\"\n",
        )
        .unwrap();
        let mut mesh = String::from("format: 1\nkind: mesh\n");
        let list = |name: &str, values: &[f64]| {
            let items: String = values.iter().map(|v| format!("  - {v:?}\n")).collect();
            format!("{name}:\n{items}")
        };
        mesh += &list(
            "positions",
            &[-1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 2.0, 0.0, -1.0, 2.0, 0.0],
        );
        mesh += &list("uvs", &[0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0]);
        mesh += "base_color_texture:\n  path: ../textures/b.png\nindices:\n";
        for index in [0, 1, 2, 0, 2, 3] {
            mesh += &format!("  - {index}\n");
        }
        std::fs::write(dir.join("assets/kit/k/meshes/m.mesh.yaml"), mesh).unwrap();
        let rgba: Vec<u8> = (0..64u8)
            .flat_map(|i| [i * 4, 255 - i * 4, 90, 255])
            .collect();
        std::fs::write(
            dir.join("assets/kit/k/textures/b.png"),
            ludion_assets::encode_png(8, 8, &rgba).unwrap(),
        )
        .unwrap();
        let source = ludion_assets::load_png(
            ludion_assets::AssetRoot::content(&dir),
            "assets/kit/k/textures/b.png",
        )
        .unwrap();
        let settings = CookSettings {
            target,
            color_space: ColorSpace::Srgb,
            max_texture_size: None,
            alpha_cutoff: None,
        };
        let store = dir.join(".ludion/cooked").join(target.name());
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(
            store.join(cooked_file_name(cook_key(source.hash, &settings))),
            ludion_cook::write_ktx2(&ludion_cook::cook(&source, &settings).unwrap()),
        )
        .unwrap();
        let scene = dir.join("main.yaml");
        std::fs::write(
            &scene,
            SCENE.replace(
                "  - name: block\n    components:\n",
                "  - name: block\n    components:\n      Mesh:\n        shape:\n          asset: assets/kit/k/meshes/m.mesh.yaml\n",
            )
            .replace("      Mesh:\n        shape: cube\n", ""),
        )
        .unwrap();

        let mut engine = Engine::builder()
            .patterns()
            .load_scene_file(&scene)
            .unwrap();
        let baked = engine.bake_probes(None).unwrap();
        assert_eq!(baked.probes.len(), 2);

        let outcome = dump(&scene).expect("the adapter is still there");
        assert_eq!(outcome.key_source, "store-host");
        assert!(outcome.probes.iter().all(|p| p.stored), "{outcome:?}");
        for (dumped, bake) in outcome.probes.iter().zip(&baked.probes) {
            assert_eq!(dumped.key, bake.key);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
