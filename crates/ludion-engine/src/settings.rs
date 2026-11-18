//! The per-machine player settings: the [`Settings`] request resource, the [`SettingsView`] a game
//! reads, and the lenient `save/settings.yaml` they persist to (`Plan/settings.md`).
//!
//! A game's options menu writes [`Settings`] (bus volumes, window mode, quality preset,
//! anti-aliasing, `save()`); the engine drains it at the frame boundary onto the
//! [`Engine`](crate::Engine), where it outlives world rebuilds, and applies it: volumes to the
//! `User` mix layer, anti-aliasing and quality to the next render, the window mode to a host
//! channel. Persistence is opt-in ([`EngineBuilder::persist_settings`](crate::EngineBuilder::persist_settings)).
//!
//! The file is the shared per-machine save shape: a canonical `format: 1` / `kind: settings`
//! envelope, loaded leniently. A file that cannot be honored is backed up and replaced by defaults
//! with one `L4340`; an unknown key is dropped (`L4341`); an out-of-range or unknown value is replaced
//! (`L4342`). One bad field never resets the rest. Emit writes every present value in schema order
//! (unset optional fields are omitted), so an untouched file stays a minimal diff.

use std::collections::BTreeMap;
use std::path::Path;

use ludion_core::audio::Bus;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_renderer::AntiAliasing;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The settings document `format:` this build reads and writes; a newer file is backed up and
/// defaulted, so an older build never silently downgrades it.
const SETTINGS_FORMAT: u64 = 1;

/// Where the file lives, relative to the save root.
pub(crate) const SETTINGS_PATH: &str = "save/settings.yaml";

/// The `kind:` of the file, and its key in a replay header's `save_docs`.
pub(crate) const SETTINGS_KIND: &str = "settings";

/// The window modes a game can ask the window host for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    /// A normal window.
    Windowed,
    /// A borderless window covering the monitor.
    BorderlessFullscreen,
}

impl WindowMode {
    const ALL: [WindowMode; 2] = [WindowMode::Windowed, WindowMode::BorderlessFullscreen];

    /// The spelling used in the file.
    pub fn name(self) -> &'static str {
        match self {
            WindowMode::Windowed => "windowed",
            WindowMode::BorderlessFullscreen => "borderless_fullscreen",
        }
    }
}

/// The quality presets a player can pick (`ludion docs quality`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QualityPreset {
    /// The cheapest preset.
    Low,
    /// A middle preset.
    Medium,
    /// A high preset.
    High,
    /// Everything the scene authors.
    Full,
}

impl QualityPreset {
    const ALL: [QualityPreset; 4] = [
        QualityPreset::Low,
        QualityPreset::Medium,
        QualityPreset::High,
        QualityPreset::Full,
    ];

    /// The preset's name, as the file and `--quality` spell it.
    pub fn name(self) -> &'static str {
        match self {
            QualityPreset::Low => "low",
            QualityPreset::Medium => "medium",
            QualityPreset::High => "high",
            QualityPreset::Full => "full",
        }
    }
}

/// The settings file's fields. Every field defaults, so an older or partial file loads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub(crate) struct SettingsDoc {
    pub audio: AudioSettings,
    #[serde(skip_serializing_if = "DisplaySettings::is_default")]
    pub display: DisplaySettings,
    #[serde(skip_serializing_if = "GraphicsSettings::is_default")]
    pub graphics: GraphicsSettings,
}

/// `audio:` — the master volume and the other buses' volumes by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub(crate) struct AudioSettings {
    pub master: f32,
    /// Bus name → volume. Unknown names are kept so a game's own buses survive a round trip.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub buses: BTreeMap<String, f32>,
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings {
            master: 1.0,
            buses: BTreeMap::new(),
        }
    }
}

/// `display:` — the window mode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub(crate) struct DisplaySettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowMode>,
}

impl DisplaySettings {
    fn is_default(&self) -> bool {
        self.window.is_none()
    }
}

/// `graphics:` — the quality preset and anti-aliasing mode, each unset until chosen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(default)]
pub(crate) struct GraphicsSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<QualityPreset>,
    /// An [`AntiAliasing`] name, parsed with `AntiAliasing::from_name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anti_aliasing: Option<String>,
}

impl GraphicsSettings {
    fn is_default(&self) -> bool {
        self.quality.is_none() && self.anti_aliasing.is_none()
    }
}

/// The file's name for a bus's volume key under `audio.buses` (`None` for the master, which has its
/// own `audio.master` key).
fn bus_key(bus: Bus) -> Option<&'static str> {
    match bus {
        Bus::Master => None,
        Bus::Sfx => Some("sfx"),
        Bus::Ambient => Some("ambient"),
        Bus::Voice => Some("voice"),
        Bus::Ui => Some("ui"),
        Bus::Music => Some("music"),
    }
}

impl SettingsDoc {
    /// A bus's volume in the document, `1.0` when unset.
    pub(crate) fn volume(&self, bus: Bus) -> f32 {
        match bus_key(bus) {
            None => self.audio.master,
            Some(key) => self.audio.buses.get(key).copied().unwrap_or(1.0),
        }
    }

    /// Sets a bus's volume.
    pub(crate) fn set_volume(&mut self, bus: Bus, volume: f32) {
        match bus_key(bus) {
            None => self.audio.master = volume,
            Some(key) => {
                self.audio.buses.insert(key.to_string(), volume);
            }
        }
    }

    /// The anti-aliasing mode the file names, if any.
    pub(crate) fn anti_aliasing(&self) -> Option<AntiAliasing> {
        self.graphics
            .anti_aliasing
            .as_deref()
            .and_then(AntiAliasing::from_name)
    }

    /// The canonical `kind: settings` text.
    pub(crate) fn emit(&self) -> String {
        let schema = serde_json::to_value(schemars::schema_for!(SettingsDoc))
            .expect("the settings schema is JSON");
        let value = serde_json::to_value(self).expect("settings serialize");
        ludion_scene::emit_doc("settings", &value, &schema)
    }
}

/// Reads `path` leniently. A missing file is the defaults and no warning; a file that cannot be
/// honored is renamed aside (`settings.yaml.bak.<hash>`) and defaults start with one `L4340`; every
/// other problem is repaired field by field with an `L4341`/`L4342` warning each.
pub(crate) fn load(path: &Path) -> (SettingsDoc, Vec<Diagnostic>) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (SettingsDoc::default(), Vec::new());
        }
        Err(error) => {
            let reason = format!("it could not be read: {error}");
            return (SettingsDoc::default(), vec![reset(path, &reason, false)]);
        }
    };
    let mut diagnostics = Vec::new();
    match parse(&text, &mut diagnostics) {
        Ok(doc) => (doc, diagnostics),
        Err(reason) => (SettingsDoc::default(), vec![reset(path, &reason, true)]),
    }
}

/// Parses embedded text (a replay header's `save_docs` entry) the way [`load`] parses the file, but
/// never touches disk: unparseable text is the defaults with one `L4340`.
pub(crate) fn parse_text(text: &str) -> (SettingsDoc, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    match parse(text, &mut diagnostics) {
        Ok(doc) => (doc, diagnostics),
        Err(reason) => (
            SettingsDoc::default(),
            vec![reset_diagnostic(
                &reason,
                "the recorded settings were not installed",
            )],
        ),
    }
}

/// The window mode the file under `save_root` names, if it parses. A missing or unloadable file is
/// `None`: this peek never backs up and never warns (the engine's own build reports once).
pub fn settings_window_mode_at(save_root: &Path) -> Option<WindowMode> {
    let text = std::fs::read_to_string(save_root.join(SETTINGS_PATH)).ok()?;
    parse(&text, &mut Vec::new()).ok()?.display.window
}

/// Backs the file up (when asked) and builds the one `L4340`.
fn reset(path: &Path, reason: &str, backup: bool) -> Diagnostic {
    let kept = if backup {
        match crate::player_profile::backup_file(path) {
            Ok(name) => format!("the old file was kept as `{name}`"),
            Err(error) => {
                format!("the old file could not be backed up ({error}) and stays in place")
            }
        }
    } else {
        "the file is untouched".to_string()
    };
    reset_diagnostic(reason, &kept)
}

fn reset_diagnostic(reason: &str, kept: &str) -> Diagnostic {
    Diagnostic::new(
        code::SETTINGS_RESET,
        format!("`settings.yaml` was not loaded: {reason}; defaults apply and {kept}"),
    )
    .with_path("settings.yaml")
    .with_help(
        "nothing is required: the next `Settings::save()` writes a fresh file; to recover old values, \
         reconcile the backup by hand",
    )
}

fn parse(text: &str, diagnostics: &mut Vec<Diagnostic>) -> Result<SettingsDoc, String> {
    let envelope = ludion_scene::open_settings(text).map_err(|errors| {
        errors
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    if envelope.format > SETTINGS_FORMAT {
        return Err(format!(
            "its `format` is {}, newer than the {SETTINGS_FORMAT} this engine reads",
            envelope.format
        ));
    }
    let Value::Object(fields) = envelope.fields else {
        return Err("it is not a mapping".to_string());
    };
    let cleaned = sanitize(fields, diagnostics);
    serde_json::from_value(Value::Object(cleaned))
        .map_err(|error| format!("a value has the wrong shape: {error}"))
}

fn warn(
    code: ludion_core::diagnostics::DiagnosticCode,
    pointer: &str,
    message: String,
    help: &str,
) -> Diagnostic {
    Diagnostic::new(code, format!("`settings.yaml` {pointer}: {message}"))
        .with_path(format!("settings.yaml{pointer}"))
        .with_help(help.to_string())
}

/// Drops unknown keys, clamps volumes and clears unknown names, reporting each, so the typed
/// deserialize that follows cannot fail on a field the player can edit.
fn sanitize(
    mut fields: Map<String, Value>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Map<String, Value> {
    keep_known(
        &mut fields,
        "",
        &["audio", "display", "graphics"],
        diagnostics,
    );
    for section in ["audio", "display", "graphics"] {
        let pointer = format!("/{section}");
        match fields.remove(section) {
            None => {}
            Some(Value::Object(map)) => {
                let map = match section {
                    "audio" => sanitize_audio(map, diagnostics),
                    "display" => sanitize_display(map, diagnostics),
                    _ => sanitize_graphics(map, diagnostics),
                };
                fields.insert(section.to_string(), Value::Object(map));
            }
            Some(Value::Null) => {}
            Some(_) => diagnostics.push(warn(
                code::SETTINGS_VALUE_CLAMPED,
                &pointer,
                "the section is not a mapping and is ignored".to_string(),
                "write the section as a mapping of keys, or delete it",
            )),
        }
    }
    fields
}

fn keep_known(
    map: &mut Map<String, Value>,
    parent: &str,
    known: &[&str],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let unknown: Vec<String> = map
        .keys()
        .filter(|key| !known.contains(&key.as_str()))
        .cloned()
        .collect();
    for key in unknown {
        map.remove(&key);
        diagnostics.push(warn(
            code::SETTINGS_KEY_DROPPED,
            &format!("{parent}/{key}"),
            format!(
                "unknown key `{key}` dropped (known here: {})",
                known.join(", ")
            ),
            "correct the key or delete the line",
        ));
    }
}

/// A volume value: clamped into `0..=1`; anything that is not a finite number is `1.0`.
fn volume(value: &Value, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> f32 {
    match value.as_f64() {
        Some(number) if number.is_finite() => {
            let clamped = number.clamp(0.0, 1.0);
            if clamped != number {
                diagnostics.push(warn(
                    code::SETTINGS_VALUE_CLAMPED,
                    pointer,
                    format!("volume {number} is outside 0..=1 and was clamped to {clamped}"),
                    "write a volume between 0 and 1",
                ));
            }
            clamped as f32
        }
        _ => {
            diagnostics.push(warn(
                code::SETTINGS_VALUE_CLAMPED,
                pointer,
                "the volume is not a finite number and was replaced by 1.0".to_string(),
                "write a volume between 0 and 1",
            ));
            1.0
        }
    }
}

fn sanitize_audio(
    mut map: Map<String, Value>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Map<String, Value> {
    keep_known(&mut map, "/audio", &["master", "buses"], diagnostics);
    if let Some(master) = map.get("master").cloned() {
        let value = volume(&master, "/audio/master", diagnostics);
        map.insert("master".to_string(), Value::from(f64::from(value)));
    }
    match map.remove("buses") {
        Some(Value::Object(buses)) => {
            let mut kept = Map::new();
            for (name, value) in buses {
                let pointer = format!("/audio/buses/{name}");
                if name == "master" {
                    diagnostics.push(warn(
                        code::SETTINGS_KEY_DROPPED,
                        &pointer,
                        "the master volume is `audio.master`, not a bus entry; dropped".to_string(),
                        "move the value to `audio: master:`",
                    ));
                    continue;
                }
                if bus_names().all(|known| known != name) {
                    diagnostics.push(warn(
                        code::SETTINGS_KEY_DROPPED,
                        &pointer,
                        format!("`{name}` is not an engine bus; kept for the game that owns it"),
                        "ignore this if the game defines the bus, else correct the name",
                    ));
                }
                let level = volume(&value, &pointer, diagnostics);
                kept.insert(name, Value::from(f64::from(level)));
            }
            map.insert("buses".to_string(), Value::Object(kept));
        }
        Some(Value::Null) | None => {}
        Some(_) => diagnostics.push(warn(
            code::SETTINGS_VALUE_CLAMPED,
            "/audio/buses",
            "`buses` is not a mapping and is ignored".to_string(),
            "write `buses:` as bus names to volumes",
        )),
    }
    map
}

fn bus_names() -> impl Iterator<Item = &'static str> {
    Bus::ALL.into_iter().filter_map(bus_key)
}

/// Keeps a string field only if `known` accepts it, else drops it with an `L4342`.
fn named(
    map: &mut Map<String, Value>,
    key: &str,
    pointer: &str,
    expected: &str,
    known: impl Fn(&str) -> Option<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match map.remove(key) {
        None | Some(Value::Null) => {}
        Some(value) => match value.as_str().and_then(known) {
            Some(name) => {
                map.insert(key.to_string(), Value::String(name));
            }
            None => diagnostics.push(warn(
                code::SETTINGS_VALUE_CLAMPED,
                pointer,
                format!("{} is not a known name and is ignored", show(&value)),
                &format!("write one of: {expected}"),
            )),
        },
    }
}

fn show(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), |text| format!("`{text}`"))
}

fn sanitize_display(
    mut map: Map<String, Value>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Map<String, Value> {
    keep_known(&mut map, "/display", &["window"], diagnostics);
    let expected = WindowMode::ALL.map(WindowMode::name).join(", ");
    named(
        &mut map,
        "window",
        "/display/window",
        &expected,
        |name| {
            WindowMode::ALL
                .iter()
                .find(|mode| mode.name() == name)
                .map(|mode| mode.name().to_string())
        },
        diagnostics,
    );
    map
}

fn sanitize_graphics(
    mut map: Map<String, Value>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Map<String, Value> {
    keep_known(
        &mut map,
        "/graphics",
        &["quality", "anti_aliasing"],
        diagnostics,
    );
    let expected = QualityPreset::ALL.map(QualityPreset::name).join(", ");
    named(
        &mut map,
        "quality",
        "/graphics/quality",
        &expected,
        |name| {
            QualityPreset::ALL
                .iter()
                .find(|preset| preset.name() == name.to_ascii_lowercase())
                .map(|preset| preset.name().to_string())
        },
        diagnostics,
    );
    let expected = AntiAliasing::ALL.map(AntiAliasing::name).join(", ");
    named(
        &mut map,
        "anti_aliasing",
        "/graphics/anti_aliasing",
        &expected,
        |name| AntiAliasing::from_name(name).map(|mode| mode.name().to_string()),
        diagnostics,
    );
    map
}

/// An ECS resource: the player settings a game system requests, honored at the next frame boundary.
///
/// The engine installs one (empty) into every world it builds. The last request per field in a tick
/// wins. Volumes go to the `User` mix layer ([`MixLayer::User`](ludion_core::audio::MixLayer));
/// anti-aliasing and quality apply from the next render (a quality a host pinned with
/// `--quality` persists but does not apply); a window mode is sent to the window host. Nothing is
/// written to disk until [`save`](Settings::save), and only when the engine was built with
/// [`persist_settings`](crate::EngineBuilder::persist_settings).
///
/// **Host state:** the requests live on the [`Engine`](crate::Engine), survive world rebuilds, and are
/// never hashed, snapshotted, recorded or on the wire. Read the result back through
/// [`SettingsView`].
///
/// # Example
/// ```
/// use ludion_engine::{Bus, Engine, QualityPreset, Settings, SettingsView};
///
/// fn menu(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
///     if let Some(settings) = world.resource_mut::<Settings>() {
///         settings.request_bus_volume(Bus::Music, 0.6);
///         settings.request_quality(QualityPreset::Medium);
///     }
/// }
///
/// let mut engine = Engine::builder().system("menu", menu).build().unwrap();
/// engine.advance_live();
/// let view = engine.world().resource::<SettingsView>().unwrap();
/// assert_eq!(view.volume(Bus::Music), 0.6);
/// assert_eq!(view.quality(), Some(QualityPreset::Medium));
/// ```
#[derive(Default, Debug)]
pub struct Settings {
    requests: SettingsRequests,
}

/// What one drain takes from [`Settings`]: the last value per field plus the save flag.
#[derive(Default, Debug, PartialEq)]
pub(crate) struct SettingsRequests {
    /// Indexed by `bus as usize`.
    pub volumes: [Option<f32>; 6],
    pub window: Option<WindowMode>,
    pub quality: Option<QualityPreset>,
    pub anti_aliasing: Option<AntiAliasing>,
    pub save: bool,
}

impl SettingsRequests {
    pub(crate) fn is_empty(&self) -> bool {
        *self == SettingsRequests::default()
    }
}

impl Settings {
    /// Requests a bus volume, clamped silently into `0..=1`; NaN is ignored.
    pub fn request_bus_volume(&mut self, bus: Bus, volume: f32) {
        if !volume.is_nan() {
            self.requests.volumes[bus as usize] = Some(volume.clamp(0.0, 1.0));
        }
    }

    /// Requests the master volume, `request_bus_volume(Bus::Master, ..)`.
    pub fn request_master_volume(&mut self, volume: f32) {
        self.request_bus_volume(Bus::Master, volume);
    }

    /// Requests a window mode; the window host applies it, a headless engine ignores it.
    pub fn request_window_mode(&mut self, mode: WindowMode) {
        self.requests.window = Some(mode);
    }

    /// Requests a quality preset from the next render on (unless a host pinned the quality).
    pub fn request_quality(&mut self, preset: QualityPreset) {
        self.requests.quality = Some(preset);
    }

    /// Requests an anti-aliasing mode from the next render on. Below a game's
    /// [`RenderSettings`](crate::RenderSettings) request and the host's `--aa` flag.
    pub fn request_anti_aliasing(&mut self, mode: AntiAliasing) {
        self.requests.anti_aliasing = Some(mode);
    }

    /// Asks the engine to write the settings file at the frame boundary. Several calls in a frame
    /// are one write; without `persist_settings` it does nothing.
    pub fn save(&mut self) {
        self.requests.save = true;
    }

    /// Whether a request is waiting for the next frame boundary.
    pub fn is_pending(&self) -> bool {
        !self.requests.is_empty()
    }

    /// Takes the pending requests, leaving none (the engine's drain path).
    pub(crate) fn take(&mut self) -> SettingsRequests {
        std::mem::take(&mut self.requests)
    }
}

/// An ECS resource: the player settings in force, read-only.
///
/// **Deterministic:** it is a pure function of the settings file at start and the sim's own
/// [`Settings`] calls, never of a host flag, so a system may branch on it. It lives on the engine
/// like the requests do, so a world rebuild or `engine.restore` does not roll it back. It shows every
/// field, set or not; `None` means the player never chose one.
///
/// # Example
/// ```
/// use ludion_engine::{Bus, Engine, SettingsView};
///
/// let engine = Engine::new();
/// let view = engine.world().resource::<SettingsView>().unwrap();
/// assert_eq!(view.volume(Bus::Master), 1.0);
/// assert_eq!(view.window_mode(), None);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsView {
    doc: SettingsDoc,
}

impl SettingsView {
    pub(crate) fn from_doc(doc: &SettingsDoc) -> Self {
        SettingsView { doc: doc.clone() }
    }

    /// A bus's volume, `1.0` when unset (the master included).
    pub fn volume(&self, bus: Bus) -> f32 {
        self.doc.volume(bus)
    }

    /// The window mode the player chose, if any.
    pub fn window_mode(&self) -> Option<WindowMode> {
        self.doc.display.window
    }

    /// The quality preset the player chose, if any.
    pub fn quality(&self) -> Option<QualityPreset> {
        self.doc.graphics.quality
    }

    /// The anti-aliasing mode the player chose, if any.
    pub fn anti_aliasing(&self) -> Option<AntiAliasing> {
        self.doc.anti_aliasing()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.yaml")
    }

    fn load_text(name: &str, text: &str) -> (SettingsDoc, Vec<Diagnostic>, std::path::PathBuf) {
        let path = scratch(name);
        std::fs::write(&path, text).unwrap();
        let (doc, diagnostics) = load(&path);
        (doc, diagnostics, path)
    }

    fn codes(diagnostics: &[Diagnostic]) -> Vec<&str> {
        diagnostics.iter().map(|d| d.code.as_str()).collect()
    }

    #[test]
    fn a_missing_file_is_the_defaults_without_a_warning() {
        let (doc, diagnostics) = load(&scratch("missing"));
        assert_eq!(doc, SettingsDoc::default());
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn a_malformed_file_is_backed_up_and_defaulted_once() {
        for (name, text) in [
            ("garbage", "format: 1\nkind: settings\n  audio: [\n"),
            ("no-kind", "format: 1\naudio:\n  master: 0.5\n"),
            ("newer", "format: 9\nkind: settings\n"),
            ("nan", "format: 1\nkind: settings\naudio:\n  master: .nan\n"),
        ] {
            let (doc, diagnostics, path) = load_text(name, text);
            assert_eq!(doc, SettingsDoc::default(), "{name}");
            assert_eq!(codes(&diagnostics), ["L4340"], "{name}");
            assert!(!path.exists(), "{name}: the bad file moved aside");
            let backups = std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("settings.yaml.bak.")
                })
                .count();
            assert_eq!(backups, 1, "{name}");
        }
    }

    #[test]
    fn unknown_keys_are_dropped_and_unknown_buses_kept() {
        let text = "format: 1\nkind: settings\nvolume: 3\naudio:\n  master: 0.5\n  loudness: 2\n  buses:\n    music: 0.4\n    foley: 0.7\n";
        let (doc, diagnostics, _) = load_text("unknown", text);
        assert_eq!(doc.audio.master, 0.5);
        assert_eq!(doc.audio.buses.get("music"), Some(&0.4));
        assert_eq!(doc.audio.buses.get("foley"), Some(&0.7));
        assert_eq!(codes(&diagnostics), ["L4341", "L4341", "L4341"]);
        let paths: Vec<_> = diagnostics
            .iter()
            .filter_map(|d| d.path.as_deref())
            .collect();
        assert!(paths.contains(&"settings.yaml/volume"));
        assert!(paths.contains(&"settings.yaml/audio/loudness"));
        assert!(paths.contains(&"settings.yaml/audio/buses/foley"));
        assert!(
            doc.emit().contains("foley: 0.7"),
            "the unknown bus survives a round trip"
        );
    }

    #[test]
    fn volumes_clamp_and_non_numbers_default() {
        let text = "format: 1\nkind: settings\naudio:\n  master: 4\n  buses:\n    sfx: -1\n    music: loud\n    ui: ~\n";
        let (doc, diagnostics, _) = load_text("clamp", text);
        assert_eq!(doc.audio.master, 1.0);
        assert_eq!(doc.audio.buses.get("sfx"), Some(&0.0));
        assert_eq!(doc.audio.buses.get("music"), Some(&1.0));
        assert_eq!(doc.audio.buses.get("ui"), Some(&1.0));
        assert_eq!(codes(&diagnostics), ["L4342"; 4]);
    }

    #[test]
    fn a_wrong_shaped_section_never_resets_the_file() {
        let text = "format: 1\nkind: settings\ngraphics: 3\naudio:\n  master: 0.5\n";
        let (doc, diagnostics, _) = load_text("shape", text);
        assert_eq!(doc.audio.master, 0.5);
        assert_eq!(codes(&diagnostics), ["L4342"]);
    }

    #[test]
    fn a_bad_name_clears_its_field_and_keeps_the_rest() {
        let text = "format: 1\nkind: settings\ndisplay:\n  window: stretched\ngraphics:\n  quality: ultra\n  anti_aliasing: fxaa\naudio:\n  master: 0.5\n";
        let (doc, diagnostics, _) = load_text("names", text);
        assert_eq!(doc.display.window, None);
        assert_eq!(doc.graphics, GraphicsSettings::default());
        assert_eq!(doc.audio.master, 0.5);
        assert_eq!(codes(&diagnostics), ["L4342"; 3]);
    }

    #[test]
    fn emit_and_load_round_trip() {
        let mut doc = SettingsDoc::default();
        doc.set_volume(Bus::Master, 0.8);
        doc.set_volume(Bus::Music, 0.25);
        doc.display.window = Some(WindowMode::BorderlessFullscreen);
        doc.graphics.quality = Some(QualityPreset::High);
        doc.graphics.anti_aliasing = Some("smaa".to_string());
        let text = doc.emit();
        assert!(text.starts_with("format: 1\nkind: settings\n"), "{text}");
        let (back, diagnostics, _) = load_text("round-trip", &text);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(back, doc);
        assert_eq!(back.emit(), text, "emit is idempotent");
        assert_eq!(
            SettingsDoc::default().emit(),
            "format: 1\nkind: settings\naudio:\n  master: 1.0\n"
        );
    }

    #[test]
    fn parse_text_round_trips_emit_and_never_touches_disk() {
        let mut doc = SettingsDoc::default();
        doc.set_volume(Bus::Music, 0.25);
        doc.graphics.quality = Some(QualityPreset::Low);
        let (back, diagnostics) = parse_text(&doc.emit());
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(back, doc);

        let (doc, diagnostics) = parse_text("format: 1\nkind: settings\n  audio: [\n");
        assert_eq!(doc, SettingsDoc::default());
        assert_eq!(codes(&diagnostics), ["L4340"]);
    }

    #[test]
    fn the_window_peek_reads_a_valid_file_and_never_backs_up_a_bad_one() {
        let root = scratch("peek").parent().unwrap().to_path_buf();
        assert_eq!(settings_window_mode_at(&root), None, "no file");
        let path = root.join(SETTINGS_PATH);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "format: 1\nkind: settings\ndisplay:\n  window: borderless_fullscreen\n",
        )
        .unwrap();
        assert_eq!(
            settings_window_mode_at(&root),
            Some(WindowMode::BorderlessFullscreen)
        );
        let garbage = "format: 1\nkind: settings\n  audio: [\n";
        std::fs::write(&path, garbage).unwrap();
        assert_eq!(settings_window_mode_at(&root), None, "garbage");
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["settings.yaml"], "no backup was made");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), garbage);
    }

    #[test]
    fn requests_clamp_ignore_nan_and_take_empties() {
        let mut settings = Settings::default();
        assert!(!settings.is_pending());
        settings.request_bus_volume(Bus::Music, 3.0);
        settings.request_bus_volume(Bus::Sfx, f32::NAN);
        settings.request_master_volume(0.5);
        settings.request_quality(QualityPreset::Low);
        settings.request_quality(QualityPreset::High);
        assert!(settings.is_pending());
        let taken = settings.take();
        assert_eq!(taken.volumes[Bus::Music as usize], Some(1.0));
        assert_eq!(taken.volumes[Bus::Sfx as usize], None);
        assert_eq!(taken.volumes[Bus::Master as usize], Some(0.5));
        assert_eq!(taken.quality, Some(QualityPreset::High));
        assert!(!settings.is_pending());
    }

    #[test]
    fn anti_aliasing_precedence_holds_for_every_combination() {
        use crate::resolve_anti_aliasing;
        let optional = [None, Some(AntiAliasing::Off), Some(AntiAliasing::Smaa)];
        for game in optional {
            for host in optional {
                for file in optional {
                    for builder in AntiAliasing::ALL {
                        // The engine resolves `game → host → file → builder`.
                        let resolved = resolve_anti_aliasing(game, host, file.unwrap_or(builder));
                        let expected = game.or(host).or(file).unwrap_or(builder);
                        assert_eq!(resolved, expected);
                    }
                }
            }
        }
    }
}
