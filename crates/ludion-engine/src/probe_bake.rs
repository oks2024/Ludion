//! The probe store's host axis and the bake report.
//!
//! A host on [`ProbeSource::Store`] reads each reflection probe's prefiltered cube from the
//! content-addressed store `<project>/.ludion/cooked/probes/` and captures (and writes) on a miss.
//! [`ProbeSource::Fresh`], the default every verification path keeps, never touches the store, so a
//! render is a function of the scene alone. [`Engine::bake_probes`] fills the store ahead of time
//! and reports what it did.
//!
//! The axis is render-only: nothing here reaches the world, so `WorldHash`, replays and snapshots
//! do not see it.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use serde::{Deserialize, Serialize};

/// Where a host's reflection-probe cubes come from.
///
/// # Example
/// ```
/// use ludion_engine::ProbeSource;
/// assert_eq!(ProbeSource::default(), ProbeSource::Fresh);
/// assert_eq!(ProbeSource::from_name("store"), Some(ProbeSource::Store));
/// assert_eq!(ProbeSource::Store.name(), "store");
/// // The `--probes` flag of the hosts that serve the store: unset reads it, `rebake` does not.
/// assert_eq!(ProbeSource::from_flag(None), ProbeSource::Store);
/// assert_eq!(ProbeSource::from_flag(Some("rebake")), ProbeSource::Fresh);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ProbeSource {
    /// Capture every probe at load; the store is neither read nor written.
    #[default]
    Fresh,
    /// Read the probe store, capturing and writing on a miss.
    Store,
}

impl ProbeSource {
    /// The axis value's name: `"fresh"` or `"store"`.
    pub fn name(self) -> &'static str {
        match self {
            ProbeSource::Fresh => "fresh",
            ProbeSource::Store => "store",
        }
    }

    /// The value a name spells; the inverse of [`ProbeSource::name`].
    pub fn from_name(name: &str) -> Option<ProbeSource> {
        match name {
            "fresh" => Some(ProbeSource::Fresh),
            "store" => Some(ProbeSource::Store),
            _ => None,
        }
    }

    /// The axis a host's `--probes` flag selects: unset or `store` reads the store, `rebake` does
    /// not. Every host that serves the store maps its flag through this.
    pub fn from_flag(flag: Option<&str>) -> ProbeSource {
        match flag {
            Some("rebake") => ProbeSource::Fresh,
            _ => ProbeSource::Store,
        }
    }
}

/// The probe store under the project that holds `project_root`'s `ludion.toml`, or under
/// `project_root` itself when no manifest is above it.
pub(crate) fn store_dir(project_root: &Path) -> PathBuf {
    crate::manifest::find(project_root)
        .unwrap_or_else(|| project_root.to_path_buf())
        .join(".ludion/cooked/probes")
}

/// How a baked probe's cube was obtained.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeOutcome {
    /// The store already held it.
    Loaded,
    /// It was captured and written.
    Captured,
}

/// One probe of a bake.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ProbeBake {
    /// The probe's entity.
    pub entity: EntityId,
    /// The probe entity's `Name`, if it has one.
    pub name: Option<String>,
    /// The store key; the entry is `<key:016x>.ktx2`.
    pub key: u64,
    /// Whether the entry was already stored or captured now.
    pub outcome: ProbeOutcome,
}

/// What [`Engine::bake_probes`](crate::Engine::bake_probes) did.
///
/// # Example
/// ```
/// use ludion_engine::ProbeBakeReport;
/// let report = ProbeBakeReport { store: "probes".into(), probes: Vec::new() };
/// assert_eq!(report.lines(), vec!["0 probes: 0 captured, 0 loaded".to_string()]);
/// assert_eq!(report.to_json("a.yaml".as_ref(), true, &[])["probes"], serde_json::json!([]));
/// ```
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ProbeBakeReport {
    /// The store directory the entries are in.
    pub store: PathBuf,
    /// The scene's probes, in the renderer's order.
    pub probes: Vec<ProbeBake>,
}

impl ProbeBakeReport {
    /// The report as the wire-shaped JSON `ludion bake-probes --json` prints:
    /// `{ok, scene, store, probes: [{entity, name, key, outcome}], diagnostics}`.
    pub fn to_json(&self, scene: &Path, ok: bool, diagnostics: &[Diagnostic]) -> serde_json::Value {
        let probes: Vec<_> = self
            .probes
            .iter()
            .map(|probe| {
                serde_json::json!({
                    "entity": probe.entity,
                    "name": probe.name,
                    "key": format!("{:016x}", probe.key),
                    "outcome": probe.outcome,
                })
            })
            .collect();
        serde_json::json!({
            "ok": ok,
            "scene": scene.to_string_lossy().replace('\\', "/"),
            "store": self.store.to_string_lossy().replace('\\', "/"),
            "probes": probes,
            "diagnostics": diagnostics,
        })
    }

    /// Human output: one line per probe, then a total.
    pub fn lines(&self) -> Vec<String> {
        let captured = self
            .probes
            .iter()
            .filter(|probe| probe.outcome == ProbeOutcome::Captured)
            .count();
        let mut lines: Vec<String> = self
            .probes
            .iter()
            .map(|probe| {
                let outcome = match probe.outcome {
                    ProbeOutcome::Loaded => "loaded",
                    ProbeOutcome::Captured => "captured",
                };
                format!(
                    "{} {:016x} {outcome}",
                    probe.name.as_deref().unwrap_or("(unnamed)"),
                    probe.key
                )
            })
            .collect();
        lines.push(format!(
            "{} probes: {captured} captured, {} loaded",
            self.probes.len(),
            self.probes.len() - captured
        ));
        lines
    }
}

/// How many probes one `L1279` names.
const LISTED: usize = 8;

/// The warning for a resolve that captured probes a `Store` host expected to load; `missing` holds
/// each such probe's label.
pub(crate) fn not_stored_warning(missing: &[String]) -> Diagnostic {
    let more = missing.len().saturating_sub(LISTED);
    let tail = if more > 0 {
        format!(" (and {more} more)")
    } else {
        String::new()
    };
    let mut diagnostic = Diagnostic::new(
        code::REFLECTION_PROBE_NOT_STORED,
        format!(
            "{} reflection probes are not in the probe store; they were captured at load{tail}",
            missing.len()
        ),
    )
    .with_help("run `ludion bake-probes <scene>` (or the game's `--bake-probes`)");
    for label in missing.iter().take(LISTED) {
        diagnostic = diagnostic.with_related(Diagnostic::new(
            code::REFLECTION_PROBE_NOT_STORED,
            format!("{label} has no store entry"),
        ));
    }
    diagnostic
}

/// `L1280`: a forced bake named no probe.
pub(crate) fn unknown_probe(name: &str, names: &[String]) -> Diagnostic {
    let help = if names.is_empty() {
        "no probe in the scene has a `Name`; give the probe one".to_string()
    } else {
        format!("the scene's named probes are: {}", names.join(", "))
    };
    Diagnostic::new(
        code::REFLECTION_PROBE_UNKNOWN,
        format!("no reflection probe is named `{name}`"),
    )
    .with_help(help)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, TextureSource};

    #[test]
    fn the_store_dir_follows_the_manifest() {
        let root = std::env::temp_dir().join(format!("ludion-probe-dir-{}", std::process::id()));
        let scenes = root.join("scenes");
        std::fs::create_dir_all(&scenes).unwrap();
        assert_eq!(store_dir(&scenes), scenes.join(".ludion/cooked/probes"));
        std::fs::write(root.join("ludion.toml"), "").unwrap();
        assert_eq!(store_dir(&scenes), root.join(".ludion/cooked/probes"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_names_round_trip() {
        for source in [ProbeSource::Fresh, ProbeSource::Store] {
            assert_eq!(ProbeSource::from_name(source.name()), Some(source));
        }
        assert_eq!(ProbeSource::from_name("other"), None);
    }

    fn scene() -> &'static str {
        "format: 1
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
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - -0.5
          - 0.0
        scale:
          - 10.0
          - 1.0
          - 10.0
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
      ReflectionProbe: {}
"
    }

    fn project(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-probe-bake-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn engine(dir: &Path, scene: &str) -> Engine {
        let mut engine = Engine::builder().project_root(dir).build().expect("builds");
        engine.load_inline_scene(scene).expect("the scene loads");
        engine
    }

    fn codes(engine: &mut Engine) -> Vec<String> {
        let sink = engine.world_mut().resource_mut::<crate::Diagnostics>();
        sink.map(|sink| {
            sink.take()
                .iter()
                .map(|d| d.code.as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
    }

    fn no_gpu(engine: &mut Engine) -> bool {
        match engine.renderer() {
            Ok(_) => false,
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED probe bake: no GPU adapter");
                true
            }
            Err(d) => panic!("renderer init failed: {d:?}"),
        }
    }

    #[test]
    fn an_engine_is_fresh_with_no_store() {
        let engine = Engine::new();
        assert_eq!(engine.probes(), ProbeSource::Fresh);
        assert_eq!(engine.probe_store_dir(), None);
        let dir = project("fresh");
        let engine = engine_in(&dir);
        assert_eq!(engine.probes(), ProbeSource::Fresh);
        assert_eq!(engine.probe_store_dir(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn engine_in(dir: &Path) -> Engine {
        Engine::builder().project_root(dir).build().expect("builds")
    }

    #[test]
    fn set_probes_resolves_the_store_dir() {
        let dir = project("dir");
        let mut engine = engine_in(&dir);
        engine.set_probes(ProbeSource::Store);
        assert_eq!(
            engine.probe_store_dir(),
            Some(dir.join(".ludion/cooked/probes").as_path())
        );
        engine.set_probes(ProbeSource::Fresh);
        assert_eq!(engine.probe_store_dir(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_probeless_scene_bakes_an_empty_report_without_a_renderer() {
        let dir = project("empty");
        let mut engine = engine_in(&dir);
        engine
            .load_inline_scene("format: 1\nentities:\n  - name: a\n")
            .unwrap();
        let report = engine.bake_probes(None).unwrap();
        assert!(report.probes.is_empty());
        assert!(!engine.has_renderer());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_gpu_bake_then_a_store_host_hits() {
        let dir = project("gpu");
        let mut a = engine(&dir, scene());
        if no_gpu(&mut a) {
            return;
        }
        let first = a.bake_probes(None).unwrap();
        assert_eq!(first.probes.len(), 2);
        assert!(
            first
                .probes
                .iter()
                .all(|p| p.outcome == ProbeOutcome::Captured)
        );
        let store = dir.join(".ludion/cooked/probes");
        assert_eq!(std::fs::read_dir(&store).unwrap().count(), 2);
        assert!(codes(&mut a).is_empty(), "a bake warns of no miss");

        let mut b = engine(&dir, scene());
        b.set_textures(TextureSource::Cooked);
        b.set_probes(ProbeSource::Store);
        b.render_image(64, 64, false)
            .map_err(|d| d[0].clone())
            .unwrap();
        let loaded = b.renderer().unwrap().probe_resolve();
        assert!(loaded.probes.iter().all(|p| p.loaded), "{loaded:?}");
        assert!(codes(&mut b).is_empty());

        let forced = b.bake_probes(Some("left")).unwrap();
        let outcomes: Vec<_> = forced
            .probes
            .iter()
            .map(|p| (p.name.as_deref().unwrap(), p.outcome))
            .collect();
        assert_eq!(
            outcomes,
            [
                ("left", ProbeOutcome::Captured),
                ("right", ProbeOutcome::Loaded)
            ]
        );
        let unknown = b.bake_probes(Some("nowhere")).unwrap_err();
        assert_eq!(unknown.code.as_str(), "L1280");

        std::fs::remove_dir_all(&store).unwrap();
        let mut c = engine(&dir, scene());
        c.set_textures(TextureSource::Cooked);
        c.set_probes(ProbeSource::Store);
        c.render_image(64, 64, false)
            .map_err(|d| d[0].clone())
            .unwrap();
        assert_eq!(codes(&mut c), ["L1279"]);

        let mut d = engine(&dir, scene());
        d.render_image(64, 64, false)
            .map_err(|d| d[0].clone())
            .unwrap();
        let fresh = d.renderer().unwrap().probe_resolve();
        assert!(
            fresh.probes.iter().all(|p| p.key.is_none() && !p.loaded),
            "{fresh:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
