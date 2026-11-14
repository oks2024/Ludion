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
use std::time::SystemTime;

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, Severity};
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

/// A sim system's request to re-freeze the probe world, so probes spawned at runtime (a prefab room
/// swap) capture the world that now holds them.
///
/// The engine installs one into every world. At the start of the next tick it re-freezes the probe
/// world exactly as a load does; with no probe in the world that is a silent no-op that clears the
/// request. It never fires on its own: an edit or a `Static` change alone does not recapture. A plain
/// resource, excluded from the `WorldHash` and from snapshots, registering no component; a replay
/// reproduces it because the system re-issues the request.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, ProbeRefreeze};
/// let mut engine = Engine::new();
/// engine.world_mut().resource_mut::<ProbeRefreeze>().unwrap().request();
/// assert!(engine.world_mut().resource_mut::<ProbeRefreeze>().unwrap().is_requested());
/// ```
#[derive(Default, Debug)]
pub struct ProbeRefreeze {
    requested: bool,
}

impl ProbeRefreeze {
    /// Asks for a re-freeze at the next tick's start.
    pub fn request(&mut self) {
        self.requested = true;
    }

    /// Whether a request is waiting for the next tick.
    pub fn is_requested(&self) -> bool {
        self.requested
    }

    pub(crate) fn take(&mut self) -> bool {
        std::mem::take(&mut self.requested)
    }
}

/// The probe store under the project that holds `project_root`'s `ludion.toml`, or under
/// `project_root` itself when no manifest is above it.
pub(crate) fn store_dir(project_root: &Path) -> PathBuf {
    crate::manifest::find(project_root)
        .unwrap_or_else(|| project_root.to_path_buf())
        .join(".ludion/cooked/probes")
}

/// Bounds the probe store under the project holding `project_root`'s manifest (see
/// [`ludion_renderer::prune_store`]): entries unused for 14 days go, then the oldest past a 4 GiB
/// cap, never one used within 24 hours or named in `keep`. Only that project's store is reached.
///
/// # Example
/// ```
/// use std::time::SystemTime;
/// let dir = std::env::temp_dir().join(format!("ludion-prune-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&dir).unwrap();
/// let report = ludion_engine::prune_probe_store(&dir, &[], SystemTime::now());
/// assert_eq!((report.entries, report.temps), (0, 0));
/// let _ = std::fs::remove_dir_all(&dir);
/// ```
pub fn prune_probe_store(
    project_root: &Path,
    keep: &[u64],
    now: SystemTime,
) -> ludion_renderer::ProbePrune {
    ludion_renderer::prune_store(&store_dir(project_root), keep, now)
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
    /// The hook world's label ([`EngineBuilder::probe_bake_worlds`](crate::EngineBuilder::probe_bake_worlds));
    /// `None` for the loaded scene's world.
    pub world: Option<String>,
}

/// What [`Engine::bake_probes`](crate::Engine::bake_probes) did.
///
/// # Example
/// ```
/// use ludion_engine::{ProbeBake, ProbeBakeReport, ProbeOutcome};
/// let report = ProbeBakeReport { store: "probes".into(), probes: Vec::new() };
/// assert_eq!(report.lines(), vec!["0 probes: 0 captured, 0 loaded".to_string()]);
/// assert_eq!(report.to_json("a.yaml".as_ref(), true, &[])["probes"], serde_json::json!([]));
///
/// let probe = ProbeBake {
///     entity: ludion_core::ids::EntityId::new(0, 0),
///     name: None,
///     key: 1,
///     outcome: ProbeOutcome::Captured,
///     world: None,
/// };
/// let report = ProbeBakeReport { store: "probes".into(), probes: vec![probe] };
/// let json = report.to_json("a.yaml".as_ref(), true, &[]);
/// assert!(json["probes"][0].get("world").is_none());
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
    /// `{ok, scene, store, probes: [{entity, name, key, outcome, world?}], diagnostics}`; `world`
    /// appears only on a hook world's probes.
    pub fn to_json(&self, scene: &Path, ok: bool, diagnostics: &[Diagnostic]) -> serde_json::Value {
        let probes: Vec<_> = self
            .probes
            .iter()
            .map(|probe| {
                let mut json = serde_json::json!({
                    "entity": probe.entity,
                    "name": probe.name,
                    "key": format!("{:016x}", probe.key),
                    "outcome": probe.outcome,
                });
                if let Some(world) = &probe.world {
                    json["world"] = serde_json::json!(world);
                }
                json
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
                let world = probe
                    .world
                    .as_ref()
                    .map(|world| format!("{world}: "))
                    .unwrap_or_default();
                format!(
                    "{world}{} {:016x} {outcome}",
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

/// The error for a coverage check that found probes with no store entry; `missing` holds each such
/// probe's label.
pub(crate) fn not_stored_error(missing: &[String]) -> Diagnostic {
    let mut diagnostic = Diagnostic::new(
        code::REFLECTION_PROBE_NOT_STORED,
        format!(
            "{} reflection probes of this world have no probe-store entry{}",
            missing.len(),
            match missing.len().saturating_sub(LISTED) {
                0 => String::new(),
                more => format!(" (and {more} more)"),
            }
        ),
    )
    .with_severity(Severity::Error)
    .with_help("bake with the game's `--bake-probes`");
    for label in missing.iter().take(LISTED) {
        diagnostic = diagnostic.with_related(Diagnostic::new(
            code::REFLECTION_PROBE_NOT_STORED,
            format!("{label} has no store entry"),
        ));
    }
    diagnostic
}

/// One probe of a [`ProbeCoverage`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProbeCoverageEntry {
    /// The probe's entity.
    pub entity: EntityId,
    /// The probe entity's `Name`, if it has one.
    pub name: Option<String>,
    /// The store key a `Store` host computes for this world.
    pub key: u64,
    /// Whether the store holds the entry.
    pub stored: bool,
}

/// Which of the frozen world's probes the probe store holds
/// ([`Engine::probe_coverage`](crate::Engine::probe_coverage)).
///
/// # Example
/// ```
/// use ludion_engine::ProbeCoverage;
/// let coverage = ProbeCoverage { store: "probes".into(), probes: Vec::new() };
/// assert_eq!(coverage.missing().count(), 0);
/// assert!(coverage.check().is_ok());
/// ```
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ProbeCoverage {
    /// The store directory the entries are looked up in.
    pub store: PathBuf,
    /// The world's probes, in the renderer's slot order.
    pub probes: Vec<ProbeCoverageEntry>,
}

impl ProbeCoverage {
    /// The probes the store does not hold.
    pub fn missing(&self) -> impl Iterator<Item = &ProbeCoverageEntry> {
        self.probes.iter().filter(|probe| !probe.stored)
    }

    /// Fails with an `L1279` error naming every probe the store does not hold.
    ///
    /// # Errors
    /// `L1279` when any probe is missing.
    pub fn check(&self) -> Result<(), Diagnostic> {
        let labels: Vec<String> = self
            .missing()
            .map(|probe| {
                probe
                    .name
                    .clone()
                    .unwrap_or_else(|| probe.entity.to_string())
            })
            .collect();
        if labels.is_empty() {
            Ok(())
        } else {
            Err(not_stored_error(&labels))
        }
    }
}

/// The game hook behind [`EngineBuilder::probe_bake_worlds`](crate::EngineBuilder::probe_bake_worlds).
pub(crate) type ProbeBakeWorlds =
    Box<dyn FnMut(&mut crate::Engine, usize) -> Result<Option<String>, Diagnostic> + Send>;

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
    use crate::components::{Name, Transform};
    use crate::{Engine, TextureSource};
    use ludion_ecs::{SystemCtx, World};
    use ludion_protocol::perf::ProbeProfile;

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
  - name: box
    components:
      Transform:
        translation:
          - 0.0
          - 0.5
          - -3.0
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

    fn hooked_engine(
        dir: &Path,
        scene: &str,
        hook: impl FnMut(&mut Engine, usize) -> Result<Option<String>, Diagnostic> + Send + 'static,
    ) -> Engine {
        let mut engine = Engine::builder()
            .project_root(dir)
            .probe_bake_worlds(hook)
            .build()
            .expect("builds");
        engine.load_inline_scene(scene).expect("the scene loads");
        engine
    }

    const PROBELESS: &str = "format: 1\nentities:\n  - name: a\n";

    #[test]
    fn a_hook_walks_its_worlds_in_order() {
        let dir = project("hook-walk");
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = calls.clone();
        let mut engine = hooked_engine(&dir, PROBELESS, move |_, i| {
            seen.lock().unwrap().push(i);
            Ok((i < 2).then(|| format!("world-{i}")))
        });
        let report = engine.bake_probes(None).unwrap();
        assert!(report.probes.is_empty());
        assert_eq!(*calls.lock().unwrap(), [0, 1, 2]);
        assert!(!engine.has_renderer());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hook_error_fails_the_bake_and_keeps_the_hook() {
        let dir = project("hook-error");
        let calls = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let seen = calls.clone();
        let mut engine = hooked_engine(&dir, PROBELESS, move |_, _| {
            *seen.lock().unwrap() += 1;
            Err(Diagnostic::new(
                code::REFLECTION_PROBE_UNKNOWN,
                "the room failed",
            ))
        });
        let first = engine.bake_probes(None).unwrap_err();
        assert_eq!(first.message, "the room failed");
        let second = engine.bake_probes(None).unwrap_err();
        assert_eq!(second.message, "the room failed");
        assert_eq!(*calls.lock().unwrap(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_probeless_world_has_empty_coverage() {
        let dir = project("coverage-empty");
        let mut engine = engine(&dir, PROBELESS);
        let coverage = engine.probe_coverage().unwrap();
        assert!(coverage.probes.is_empty());
        assert!(coverage.check().is_ok());
        assert!(!engine.has_renderer());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_gpu_a_presentation_probe_is_baked() {
        let dir = project("presentation");
        let mut engine = engine(
            &dir,
            "format: 1
entities:
  - name: floor
    components:
      Transform:
        scale:
          - 10.0
          - 1.0
          - 10.0
      Mesh:
        shape: cube
  - name: seen
    presentation: true
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      ReflectionProbe: {}
",
        );
        if no_gpu(&mut engine) {
            return;
        }
        let report = engine.bake_probes(None).unwrap();
        let outcomes: Vec<_> = report
            .probes
            .iter()
            .map(|p| (p.name.as_deref().unwrap(), p.outcome))
            .collect();
        assert_eq!(outcomes, [("seen", ProbeOutcome::Captured)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn needs_gpu_a_static_scene_relights_its_probes_by_exactly_one() {
        let dir = project("static-relight");
        let mut engine = engine(
            &dir,
            "format: 1
entities:
  - name: environment
    components:
      Environment:
        sun_disc_size: 5.0
        sun_disc_intensity: 8.0
        ibl_intensity: 1.2
  - name: sun
    components:
      Transform:
        translation:
          - 0.0
          - 10.0
          - 0.0
      DirectionalLight:
        direction:
          - 0.3
          - -0.8
          - -0.5
        intensity: 1.0
  - name: gi
    components:
      GiVolume:
        cell_size: 0.5
        probe_spacing: 1.0
        sky_visibility: 0.8
        bounce: 0.6
  - name: floor
    components:
      Transform:
        scale:
          - 10.0
          - 1.0
          - 10.0
      Mesh:
        shape: cube
  - name: wall
    components:
      Transform:
        translation:
          - 0.0
          - 2.0
          - -4.0
        scale:
          - 8.0
          - 4.0
          - 1.0
      Mesh:
        shape: cube
  - name: probe
    components:
      Transform:
        translation:
          - 0.0
          - 2.0
          - 0.0
      ReflectionProbe: {}
",
        );
        if no_gpu(&mut engine) {
            return;
        }
        let scene = engine.extract_render_scene(64, 64).unwrap();
        let renderer = engine.renderer().unwrap();
        assert!(scene.gi.is_some(), "the GiVolume bakes");
        assert!(scene.environment.is_some_and(|env| env.sun_disc_size > 0.0));
        assert_eq!(scene.probes.len(), 1);
        let irradiance = renderer.read_probe_irradiance(&scene).unwrap();
        assert_eq!(irradiance.len(), 1);
        assert!(irradiance[0].baked.iter().flatten().any(|&e| e > 0.0));
        assert_eq!(irradiance[0].live, irradiance[0].baked);
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
        assert_eq!(
            b.probe_profile(),
            Some(ProbeProfile {
                hits: 2,
                captures: 0
            })
        );
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
        assert_eq!(
            d.probe_profile(),
            Some(ProbeProfile {
                hits: 0,
                captures: 2
            })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Moves the entity named `name` by `dx` along x.
    fn shift(world: &mut World, name: &str, dx: f32) {
        let ids: Vec<_> = world.entities().collect();
        for id in ids {
            if world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|n| n.0 == name)
                && let Ok(Some(transform)) = world.get_mut::<Transform>(id)
            {
                transform.translation[0] += dx;
            }
        }
    }

    fn slide(world: &mut World, _ctx: &SystemCtx) {
        shift(world, "box", 0.5);
    }

    fn sliding_engine(dir: &Path) -> Engine {
        let mut engine = Engine::builder()
            .system("slide", slide)
            .project_root(dir)
            .build()
            .expect("builds");
        engine.load_inline_scene(scene()).expect("the scene loads");
        engine
    }

    /// A host that ticks before it first renders captures the world as loaded, so it hits the entries
    /// a bake (frame 0, no ticks) wrote.
    #[test]
    fn needs_gpu_a_ticked_store_host_hits_the_bake() {
        let dir = project("ticked");
        let mut a = sliding_engine(&dir);
        if no_gpu(&mut a) {
            return;
        }
        let baked = a.bake_probes(None).unwrap();
        assert!(codes(&mut a).is_empty());

        let mut b = sliding_engine(&dir);
        b.set_textures(TextureSource::Cooked);
        b.set_probes(ProbeSource::Store);
        b.step_ticks(5);
        b.render_image(64, 64, false)
            .map_err(|d| d[0].clone())
            .unwrap();
        let resolved = b.renderer().unwrap().probe_resolve();
        assert!(resolved.probes.iter().all(|p| p.loaded), "{resolved:?}");
        let keys: Vec<_> = resolved.probes.iter().map(|p| p.key).collect();
        let baked_keys: Vec<_> = baked.probes.iter().map(|p| Some(p.key)).collect();
        assert_eq!(keys, baked_keys);
        assert!(codes(&mut b).is_empty(), "a ticked host reported a miss");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only a load or a `ProbeRefreeze` request recaptures: an in-session edit of the world leaves the
    /// probes alone; a reload of the same text with other geometry, or another scene with the same
    /// probes, recaptures.
    #[test]
    fn needs_gpu_a_reload_recaptures_and_an_edit_does_not() {
        let dir = project("reload");
        let mut engine = engine(&dir, scene());
        if no_gpu(&mut engine) {
            return;
        }
        engine.set_textures(TextureSource::Cooked);
        engine.set_probes(ProbeSource::Store);
        let render = |engine: &mut Engine| {
            engine
                .render_image(64, 64, false)
                .map_err(|d| d[0].clone())
                .unwrap();
            engine.renderer().unwrap().probe_resolve()
        };
        let first = render(&mut engine);
        assert_eq!(first.resolve, 1);

        shift(engine.world_mut(), "floor", 1.0);
        let edited = render(&mut engine);
        assert_eq!(edited.resolve, first.resolve, "an edit recaptured");

        engine
            .world_mut()
            .resource_mut::<ProbeRefreeze>()
            .unwrap()
            .request();
        engine.step_ticks(1);
        let refrozen = render(&mut engine);
        assert_eq!(refrozen.resolve, first.resolve + 1);
        assert_ne!(refrozen.probes[0].key, first.probes[0].key);

        engine
            .load_inline_scene(&scene().replace("- -0.5", "- -0.6"))
            .unwrap();
        let moved = render(&mut engine);
        assert_eq!(moved.resolve, refrozen.resolve + 1);
        assert_ne!(moved.probes[0].key, refrozen.probes[0].key);

        let other = format!(
            "{}  - name: extra\n    components:\n      Mesh:\n        shape: cube\n",
            scene()
        );
        engine.load_inline_scene(&other).unwrap();
        let swapped = render(&mut engine);
        assert_eq!(swapped.resolve, moved.resolve + 1);
        assert_ne!(swapped.probes[0].key, moved.probes[0].key);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn request_refreeze(engine: &mut Engine) {
        engine
            .world_mut()
            .resource_mut::<ProbeRefreeze>()
            .unwrap()
            .request();
    }

    fn is_requested(engine: &mut Engine) -> bool {
        engine
            .world_mut()
            .resource_mut::<ProbeRefreeze>()
            .unwrap()
            .is_requested()
    }

    fn frozen_meshes(engine: &mut Engine) -> Vec<ludion_renderer::MeshInstance> {
        let scene = engine.extract_render_scene(64, 64).expect("extract");
        scene.probe_world.as_ref().unwrap().scene().meshes.clone()
    }

    #[test]
    fn a_refreeze_applies_at_the_next_tick() {
        let dir = project("refreeze-tick");
        let mut engine = engine(&dir, scene());
        let before = frozen_meshes(&mut engine);
        shift(engine.world_mut(), "box", 1.0);
        assert_eq!(frozen_meshes(&mut engine), before, "an edit recaptured");
        request_refreeze(&mut engine);
        assert_eq!(frozen_meshes(&mut engine), before, "a request froze early");
        assert!(is_requested(&mut engine));
        engine.step_ticks(1);
        assert_ne!(frozen_meshes(&mut engine), before, "the box did not move");
        assert!(!is_requested(&mut engine));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refreeze_without_probes_is_a_no_op() {
        let dir = project("refreeze-none");
        let mut engine = engine(
            &dir,
            "format: 1\nentities:\n  - name: a\n    components:\n      Mesh:\n        shape: cube\n",
        );
        request_refreeze(&mut engine);
        engine.step_ticks(1);
        assert!(!is_requested(&mut engine));
        assert!(codes(&mut engine).is_empty());
        let scene = engine.extract_render_scene(64, 64).expect("extract");
        assert!(scene.probe_world.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hidden_mesh_is_not_in_the_probe_world() {
        let dir = project("refreeze-hidden");
        let mut plain = engine(&dir, scene());
        let expected = frozen_meshes(&mut plain).len();
        let hidden = format!(
            "{}  - name: warm\n    components:\n      Mesh:\n        shape: cube\n      Visibility:\n        visible: false\n      GiDynamic: {{}}\n",
            scene()
        );
        let mut engine = engine(&dir, &hidden);
        assert_eq!(frozen_meshes(&mut engine).len(), expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_refreeze_resource_survives_a_reload() {
        let dir = project("refreeze-reload");
        let mut engine = engine(&dir, scene());
        engine.load_inline_scene(scene()).unwrap();
        assert!(engine.world_mut().resource_mut::<ProbeRefreeze>().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    const ROOM_BASE: &str = "format: 1
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
  - name: room
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      ReflectionProbe: {}
";

    const ROOM_A: &str = "format: 1
kind: prefab
name: room-a
entities:
  - name: cube-a
    components:
      Transform:
        translation:
          - -3.0
          - 0.5
          - 0.0
      Mesh:
        shape: cube
";

    const ROOM_B: &str = "format: 1
kind: prefab
name: room-b
entities:
  - name: ball-b
    components:
      Transform:
        translation:
          - 2.0
          - 1.0
          - -2.0
      Mesh:
        shape: sphere
  - name: cube-b
    components:
      Transform:
        translation:
          - 3.0
          - 0.5
          - 3.0
      Mesh:
        shape: cube
";

    /// A store engine on `dir` holding the base room, with both prefabs on disk.
    fn room_engine(dir: &Path) -> Engine {
        std::fs::write(dir.join("a.prefab.yaml"), ROOM_A).unwrap();
        std::fs::write(dir.join("b.prefab.yaml"), ROOM_B).unwrap();
        let mut engine = engine(dir, ROOM_BASE);
        engine.set_textures(TextureSource::Cooked);
        engine.set_probes(ProbeSource::Store);
        engine
    }

    /// Spawns the prefab, asks for a refreeze and ticks once, as a system swapping a room would.
    fn swap_in(engine: &mut Engine, path: &str) -> Vec<ludion_core::ids::EntityId> {
        let ids = crate::prefab::spawn_prefab(engine.world_mut(), path, None, Some("room"), false)
            .expect("the prefab spawns");
        request_refreeze(engine);
        engine.step_ticks(1);
        ids
    }

    fn render_resolve(engine: &mut Engine) -> ludion_renderer::ProbeResolve {
        engine
            .render_image(64, 64, false)
            .map_err(|d| d[0].clone())
            .unwrap();
        engine.renderer().unwrap().probe_resolve()
    }

    /// A runtime room swap captures the world that holds it: the swapped engine's key and faces equal
    /// those of a fresh engine that spawned room B the same way (spawn, request, tick) on a project
    /// of its own.
    #[test]
    fn needs_gpu_a_refreeze_captures_the_swapped_room() {
        let dir = project("swap");
        let mut engine = room_engine(&dir);
        if no_gpu(&mut engine) {
            return;
        }
        let a_ids = swap_in(&mut engine, "a.prefab.yaml");
        let a = render_resolve(&mut engine);
        for id in a_ids {
            engine.world_mut().despawn(id).unwrap();
        }
        swap_in(&mut engine, "b.prefab.yaml");
        let b = render_resolve(&mut engine);
        assert_eq!(b.resolve, a.resolve + 1, "the swap did not recapture");
        assert_ne!(b.probes[0].key, a.probes[0].key);

        let fresh_dir = project("swap-fresh");
        let mut fresh = room_engine(&fresh_dir);
        swap_in(&mut fresh, "b.prefab.yaml");
        let fresh_b = render_resolve(&mut fresh);
        assert_eq!(
            b.probes[0].key, fresh_b.probes[0].key,
            "a runtime swap and a fresh world keyed differently"
        );

        let swapped_scene = engine.extract_render_scene(64, 64).expect("extract");
        let fresh_scene = fresh.extract_render_scene(64, 64).expect("extract");
        let swapped_faces = engine
            .renderer()
            .unwrap()
            .read_probes(&swapped_scene, None)
            .expect("read_probes");
        let fresh_faces = fresh
            .renderer()
            .unwrap()
            .read_probes(&fresh_scene, None)
            .expect("read_probes");
        assert_eq!(swapped_faces.len(), fresh_faces.len());
        for (s, f) in swapped_faces.iter().zip(&fresh_faces) {
            assert_eq!(s.key, f.key);
            assert!(s.captured == f.captured, "captured faces differ");
            assert!(s.levels == f.levels, "prefiltered levels differ");
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&fresh_dir);
    }

    /// The bake hook fills the store for runtime-spawned rooms under the keys a `Store` host computes
    /// after the game's own swap, coverage reports a miss before a bake and a hit after, and `force`
    /// reaches every world.
    #[test]
    fn needs_gpu_a_bake_hook_fills_the_store_for_runtime_worlds() {
        let dir = project("hook-gpu");
        std::fs::write(dir.join("a.prefab.yaml"), ROOM_A).unwrap();
        std::fs::write(dir.join("b.prefab.yaml"), ROOM_B).unwrap();
        let mut ids: Vec<ludion_core::ids::EntityId> = Vec::new();
        let mut h = hooked_engine(&dir, ROOM_BASE, move |engine, i| {
            let spawn = |engine: &mut Engine, path: &str| {
                crate::prefab::spawn_prefab(engine.world_mut(), path, None, Some("room"), false)
                    .map_err(|mut d| d.swap_remove(0))
            };
            match i {
                0 => ids = spawn(engine, "a.prefab.yaml")?,
                1 => {
                    for id in ids.drain(..) {
                        engine.world_mut().despawn(id).expect("the room despawns");
                    }
                    ids = spawn(engine, "b.prefab.yaml")?;
                }
                _ => {
                    for id in ids.drain(..) {
                        engine.world_mut().despawn(id).expect("the room despawns");
                    }
                    return Ok(None);
                }
            }
            Ok(Some(["a", "b"][i].to_string()))
        });
        if no_gpu(&mut h) {
            return;
        }
        let report = h.bake_probes(None).unwrap();
        let worlds: Vec<_> = report.probes.iter().map(|p| p.world.as_deref()).collect();
        assert_eq!(worlds, [None, Some("a"), Some("b")]);
        assert!(
            report
                .probes
                .iter()
                .all(|p| p.outcome == ProbeOutcome::Captured)
        );
        let mut keys: Vec<_> = report.probes.iter().map(|p| p.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 3, "the three worlds key apart");
        let store = dir.join(".ludion/cooked/probes");
        assert_eq!(std::fs::read_dir(&store).unwrap().count(), 3);
        let json = report.to_json("s.yaml".as_ref(), true, &[]);
        assert_eq!(json["probes"][1]["world"], "a");
        assert!(json["probes"][0].get("world").is_none());
        let key_b = report.probes[2].key;

        let mut s = room_engine(&dir);
        assert!(s.probe_coverage().unwrap().missing().next().is_none());
        let a_ids = swap_in(&mut s, "a.prefab.yaml");
        let coverage = s.probe_coverage().unwrap();
        assert!(coverage.probes.iter().all(|p| p.stored), "{coverage:?}");
        assert!(render_resolve(&mut s).probes.iter().all(|p| p.loaded));
        for id in a_ids {
            s.world_mut().despawn(id).unwrap();
        }
        swap_in(&mut s, "b.prefab.yaml");
        let coverage = s.probe_coverage().unwrap();
        assert!(coverage.probes.iter().all(|p| p.stored), "{coverage:?}");
        assert_eq!(coverage.probes[0].key, key_b);
        assert!(render_resolve(&mut s).probes.iter().all(|p| p.loaded));
        assert!(!codes(&mut s).contains(&"L1279".to_string()));

        let fresh_dir = project("hook-gpu-fresh");
        let mut f = room_engine(&fresh_dir);
        swap_in(&mut f, "b.prefab.yaml");
        let miss = f.probe_coverage().unwrap();
        assert!(miss.probes.iter().all(|p| !p.stored), "{miss:?}");
        assert_eq!(miss.check().unwrap_err().code.as_str(), "L1279");
        f.bake_probes(None).unwrap();
        let hit = f.probe_coverage().unwrap();
        assert!(hit.probes.iter().all(|p| p.stored), "{hit:?}");
        assert!(hit.check().is_ok());
        assert_eq!(hit.probes[0].key, key_b);

        let forced = h.bake_probes(Some("room")).unwrap();
        assert!(
            forced
                .probes
                .iter()
                .all(|p| p.outcome == ProbeOutcome::Captured)
        );
        let identity = |report: &ProbeBakeReport| {
            report
                .probes
                .iter()
                .map(|p| (p.key, p.world.clone()))
                .collect::<Vec<_>>()
        };
        // World b's key is not compared: the key is slot-order dependent, and the second walk
        // reuses freed slots in another order.
        assert_eq!(forced.probes.len(), 3);
        assert_eq!(identity(&forced)[..2], identity(&report)[..2]);
        assert_eq!(forced.probes[2].world.as_deref(), Some("b"));
        assert_eq!(
            h.bake_probes(Some("nowhere")).unwrap_err().code.as_str(),
            "L1280"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&fresh_dir);
    }

    /// An observed offline profile times the probe captures as their own load phase and reports the
    /// resolve; a scene without probes reports neither.
    #[test]
    fn needs_gpu_profile_reports_probe_bake_time_and_captures() {
        let dir = project("profile");
        let mut engine = engine(&dir, scene());
        engine.observe_load();
        let report = match crate::profile_engine(&mut engine, "probes".into(), 2, 1) {
            Ok(report) => report,
            Err(d) if ludion_core::require_gpu::is_no_adapter(&d) => {
                eprintln!("SKIPPED probe profile: no GPU adapter");
                return;
            }
            Err(d) => panic!("profile failed: {d:?}"),
        };
        let load = report.load.expect("an observed load");
        let bake = load.probe_bake_ms.expect("two probes were captured");
        assert!(bake > 0.0 && bake <= load.total_ms, "{load:?}");
        assert_eq!(
            report.probes,
            Some(ProbeProfile {
                hits: 0,
                captures: 2
            })
        );

        let mut bare = Engine::builder().project_root(&dir).build().unwrap();
        bare.observe_load();
        bare.load_inline_scene("format: 1\nentities:\n  - name: a\n")
            .unwrap();
        let report = crate::profile_engine(&mut bare, "bare".into(), 2, 1).unwrap();
        assert_eq!(report.probes, None);
        assert_eq!(report.load.and_then(|l| l.probe_bake_ms), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
