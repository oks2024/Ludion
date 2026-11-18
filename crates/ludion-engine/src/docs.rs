//! Game data-docs: a game registers a serde `kind` type ([`EngineBuilder::doc_kind`]) and every
//! matching `*.yaml` under the content root loads, validated, into a typed [`DocStore<T>`] resource
//! at each world build (Phase 15 / E4, `Plan/phase-15-data-docs.md`).
//!
//! The typed side of the machinery `ludion-scene` provides generically: [`DocStore<T>`] is the
//! resource game systems read, [`DocKind`] is one registered kind held persistently on the
//! [`Engine`](crate::Engine) (a monomorphized `fn` pointer, like a system, so it survives the
//! `SimDriver` resource wipe), and [`load_doc_stores`] is the content-root walk that rebuilds every
//! store from disk. It is invoked at construction, `scene.load`, and `restore` — the
//! `install_audio_queue` mold — so the store is always a fresh function of the committed, version-
//! controlled content, never a value that lingers across a world reset.
//!
//! A doc kind is `(kind string + schemars schema + a monomorphized loader)`; `ludion-scene` never
//! names `T`, so this module owns the one place `T`'s fields are known — deserialization.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, Severity};
use ludion_core::suggest;
use ludion_ecs::World;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Whether `kind` is an engine-owned document kind a content-root walk leaves to its owner
/// instead of validating it as a game data-doc: every `ludion_scene::KINDS` row that is not
/// `doc_store`. The walk would otherwise raise `L1119` on the engine's own files (the profile
/// save, a map source and the sprite and model assets all live inside the content root). A host
/// summarizing data-docs (the `ludion` facade's `--check-docs`) excludes the same kinds through
/// this one function.
///
/// # Example
/// ```
/// assert!(ludion_engine::is_engine_kind("mesh"));
/// assert!(!ludion_engine::is_engine_kind("pattern")); // a registered doc store
/// assert!(!ludion_engine::is_engine_kind("weapon")); // a game's own kind
/// ```
pub fn is_engine_kind(kind: &str) -> bool {
    ludion_scene::kind_entry(kind).is_some_and(|entry| !entry.doc_store)
}

/// Every loaded game data-doc of one registered kind `T`, keyed by slash-normalized
/// content-root-relative path (e.g. `weapons/crossbow.yaml`).
///
/// A resource, not a component: it never enters the `WorldHash` and trips no drift gate — its
/// numbers reach the hash only through the components a game's systems derive from it, so a reload's
/// effect surfaces exactly at the frame those systems next read the store. The [`BTreeMap`] pins
/// iteration order (the determinism denylist forbids a hashed map anyway).
///
/// # Example
/// ```
/// use ludion_engine::{DocStore, Engine};
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize, JsonSchema, Clone)]
/// struct Weapon {
///     damage: f32,
/// }
///
/// # let dir = std::env::temp_dir().join(format!("ludion-docstore-doc-{}", std::process::id()));
/// # let _ = std::fs::remove_dir_all(&dir);
/// # std::fs::create_dir_all(dir.join("weapons")).unwrap();
/// # std::fs::write(
/// #     dir.join("weapons/crossbow.yaml"),
/// #     "format: 1\nkind: weapon\ndamage: 12.0\n",
/// # )
/// # .unwrap();
/// let mut engine = Engine::builder()
///     .doc_kind::<Weapon>("weapon")
///     .project_root(&dir)
///     .build()
///     .unwrap();
///
/// let store = engine.world().resource::<DocStore<Weapon>>().unwrap();
/// assert_eq!(store.get("weapons/crossbow.yaml").unwrap().damage, 12.0);
/// assert_eq!(store.generation(), 0);
/// # let _ = std::fs::remove_dir_all(&dir);
/// ```
pub struct DocStore<T> {
    docs: BTreeMap<String, T>,
    generation: u64,
}

impl<T> DocStore<T> {
    /// A store of `docs` at generation 0 (a fresh boot; a live reload bumps the generation).
    pub(crate) fn new(docs: BTreeMap<String, T>) -> Self {
        DocStore {
            docs,
            generation: 0,
        }
    }

    /// The doc at `path` (a slash-normalized content-root-relative key), if loaded.
    pub fn get(&self, path: &str) -> Option<&T> {
        self.docs.get(path)
    }

    /// Every `(path, doc)` pair, in pinned key order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &T)> {
        self.docs.iter()
    }

    /// How many docs of this kind are loaded.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// Whether no docs of this kind are loaded.
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// The one doc, when the store holds exactly one; `None` for zero or several. Works on any
    /// store; for a `doc_singleton` kind the engine already refused any other count.
    pub fn single(&self) -> Option<&T> {
        match self.docs.len() {
            1 => self.docs.values().next(),
            _ => None,
        }
    }

    /// The "something changed" signal a system compares against a cached value: 0 at every fresh
    /// boot, incremented by a live reload. A restore or a scene switch re-applies the session's
    /// reload log with each document's reload count, so it reproduces the live count exactly.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Upserts one doc at `key` and bumps the generation — the live-reload primitive
    /// ([`reload_doc`]). Replacing an existing entry or adding a new one both count as one change.
    /// Returns the displaced doc, so a caller can put it back.
    pub(crate) fn upsert(&mut self, key: String, doc: T) -> Option<T> {
        let displaced = self.docs.insert(key, doc);
        self.generation += 1;
        displaced
    }
}

/// One game-registered semantic pass over the loaded data-docs
/// ([`EngineBuilder::validate_docs`](crate::EngineBuilder::validate_docs)).
///
/// It sees the world *after* every registered kind's [`DocStore`] is installed, so a rule may read
/// across kinds — the cross-reference checks schema validation cannot express, because a `district:`
/// or an `enemy:` is a well-typed string and only the game knows what it must name.
pub type DocValidator = Box<dyn Fn(&World) -> Vec<Diagnostic> + Send + Sync>;

/// Runs the engine's singleton count, then every registered validator in registration order, and
/// concatenates what they return, so a game may split its rules across several passes (one per kind)
/// and still get one report. The engine's own root-aware pass — bullet-kind atlas references — runs
/// last.
pub(crate) fn run_validators(
    doc_kinds: &[DocKind],
    validators: &[DocValidator],
    project_root: &Path,
    world: &World,
) -> Vec<Diagnostic> {
    singleton_counts(doc_kinds, world)
        .into_iter()
        .chain(validators.iter().flat_map(|validate| validate(world)))
        .chain(crate::patterns::validate_bullet_atlases(
            project_root,
            world,
        ))
        .collect()
}

/// One `L1315` per `doc_singleton` kind whose store does not hold exactly one doc. Deterministic
/// (registration order, store key order): the reload gate compares before/after reports with `==`.
fn singleton_counts(doc_kinds: &[DocKind], world: &World) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for doc_kind in doc_kinds.iter().filter(|dk| dk.singleton) {
        let keys = (doc_kind.keys)(world);
        let kind = &doc_kind.kind;
        match keys.len() {
            1 => {}
            0 => out.push(
                Diagnostic::new(
                    code::DOC_SINGLETON_COUNT,
                    format!("`kind: {kind}` is a single-doc kind but no doc declares it"),
                )
                .with_path(kind.clone())
                .with_help(format!(
                    "add one `kind: {kind}` doc under the content root, or register the kind with doc_kind if it may be absent"
                )),
            ),
            n => {
                let listed = keys
                    .iter()
                    .map(|key| format!("`{key}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut diagnostic = Diagnostic::new(
                    code::DOC_SINGLETON_COUNT,
                    format!("`kind: {kind}` is a single-doc kind but {n} docs declare it: {listed}"),
                )
                .with_path(kind.clone())
                .with_help(
                    "delete or rename the stray copy, or register the kind with doc_kind if a set was meant",
                );
                for key in &keys {
                    diagnostic = diagnostic.with_related(
                        Diagnostic::new(
                            code::DOC_SINGLETON_COUNT,
                            format!("`{key}` declares `kind: {kind}`"),
                        )
                        .with_path(key.clone()),
                    );
                }
                out.push(diagnostic);
            }
        }
    }
    out
}

/// One document of the session's reload log: its latest canonical text and how many times it was
/// reloaded. A reload replaces the text in place, so the log holds one entry per `(kind, key)`;
/// `applied` carries the count a re-apply must restore to the store's generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DocReload {
    pub(crate) kind: String,
    pub(crate) key: String,
    pub(crate) document: String,
    pub(crate) applied: u64,
}

/// The session's data-doc reload log: one [`DocReload`] per `(kind, key)`, in first-appearance order.
/// Document state commutes across reloads (a reapply is per document and schema-only), so the order
/// only has to be deterministic; the counts are what keep [`DocStore::generation`] exact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DocReloadLog(Vec<DocReload>);

impl DocReloadLog {
    /// Records one reload: replaces the text of an existing entry and counts it, or appends a new one.
    pub(crate) fn record(&mut self, kind: &str, key: &str, document: &str) {
        match self.0.iter_mut().find(|r| r.kind == kind && r.key == key) {
            Some(entry) => {
                entry.document = document.to_string();
                entry.applied += 1;
            }
            None => self.0.push(DocReload {
                kind: kind.to_string(),
                key: key.to_string(),
                document: document.to_string(),
                applied: 1,
            }),
        }
    }

    /// Builds a log from stored entries, folding duplicates: the last text wins and counts add up.
    pub(crate) fn from_entries(entries: impl IntoIterator<Item = DocReload>) -> Self {
        let mut log = DocReloadLog::default();
        for entry in entries {
            match log
                .0
                .iter_mut()
                .find(|r| r.kind == entry.kind && r.key == entry.key)
            {
                Some(existing) => {
                    existing.document = entry.document;
                    existing.applied += entry.applied;
                }
                None => log.0.push(entry),
            }
        }
        log
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &DocReload> {
        self.0.iter()
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

/// What [`prepare_docs`] hands the engine's commit: the walk's skips and the validators' warnings,
/// held back from every engine field until the rebuild they belong to lands.
pub(crate) struct PreparedDocs {
    pub(crate) skips: DocSkips,
    pub(crate) warnings: Vec<Diagnostic>,
}

/// Installs every doc kind's store into `world`, runs the validators over it, then re-applies each
/// entry of `reloads` (one per `(kind, key)`, latest canonical text) off-disk on top, restoring each
/// store's generation by the entry's reload count — the whole
/// fallible half of a world rebuild, run against a world not yet swapped in. Touches no engine state
/// and reports nothing: the warnings ride back in [`PreparedDocs`].
///
/// The validators run before the reapply (the order every rebuild has always had). A game with no
/// registered validator and no `doc_singleton` kind pays nothing, the engine's own bullet-atlas
/// pass included.
///
/// # Errors
/// Every collected walk diagnostic (see [`load_doc_stores`]); else every `Error` the singleton
/// count (`L1315`) or a validator reported, the count first, then registration order; else the first `L1119`/`L1120` a logged reload fails with
/// against the current schema.
///
/// `inline` is the engine's builder-supplied docs, merged over the walk by key.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_docs(
    root: &Path,
    doc_kinds: &[DocKind],
    game_kinds: bool,
    validators: &[DocValidator],
    inline: &BTreeMap<String, InlineDoc>,
    reloads: &DocReloadLog,
    save_dir: Option<&Path>,
    world: &mut World,
) -> Result<PreparedDocs, Vec<Diagnostic>> {
    let skips = load_doc_stores(root, doc_kinds, game_kinds, inline, save_dir, world)?;
    let mut warnings = Vec::new();
    if !validators.is_empty() || doc_kinds.iter().any(|dk| dk.singleton) {
        let (errors, reported): (Vec<_>, Vec<_>) =
            run_validators(doc_kinds, validators, root, world)
                .into_iter()
                .partition(|d| d.severity == Severity::Error);
        if !errors.is_empty() {
            return Err(errors);
        }
        warnings = reported;
    }
    for entry in reloads.iter() {
        reload_doc(doc_kinds, world, &entry.kind, &entry.key, &entry.document)
            .map_err(crate::commands::collapse)
            .map_err(|d| vec![d])?;
        if let Some(dk) = doc_kinds.iter().find(|dk| dk.kind == entry.kind) {
            (dk.advance_generation)(world, entry.applied.saturating_sub(1));
        }
    }
    Ok(PreparedDocs { skips, warnings })
}

/// A doc the builder holds as text ([`doc_text`](crate::EngineBuilder::doc_text)), its kind resolved
/// once at build.
#[derive(Debug, Clone)]
pub(crate) struct InlineDoc {
    pub(crate) kind: String,
    pub(crate) text: String,
}

/// Resolves the builder's `doc_text` entries, in key order: normalizes each key and reads the text's
/// `kind:`. Fails on the first offender.
///
/// # Errors
/// `L1119` for a key that is not a content-root-relative `*.yaml` path, for a text with no `kind:`,
/// or for a kind no registered doc kind owns; the `L100x` error for text the strict-YAML reader
/// rejects.
pub(crate) fn resolve_doc_texts(
    doc_kinds: &[DocKind],
    texts: BTreeMap<String, String>,
) -> Result<BTreeMap<String, InlineDoc>, Diagnostic> {
    let mut out = BTreeMap::new();
    for (raw_key, text) in texts {
        let key = raw_key.replace('\\', "/");
        let bad_key = key.is_empty()
            || key.starts_with('/')
            || key.contains(':')
            || key.split('/').any(|segment| segment == "..")
            || !key.ends_with(".yaml");
        if bad_key {
            return Err(Diagnostic::new(
                code::DOC_ENVELOPE_INVALID,
                format!("doc_text key `{raw_key}` is not a content-root-relative `*.yaml` path"),
            )
            .with_help(
                "a doc_text key is a content-root-relative `*.yaml` path, e.g. `weapons/crossbow.yaml`",
            ));
        }
        let kind = match ludion_scene::peek_kind_checked(&text) {
            Err(diagnostic) => return Err(prefix_file(diagnostic, &key)),
            Ok(None) => {
                return Err(Diagnostic::new(
                    code::DOC_ENVELOPE_INVALID,
                    format!("{key}: doc_text declares no `kind:`"),
                )
                .with_path("/kind")
                .with_help("a data-doc starts with `kind: <registered kind>`"));
            }
            Ok(Some(kind)) => kind,
        };
        if !doc_kinds.iter().any(|dk| dk.kind == kind) {
            return Err(unregistered_kind(&key, &kind, doc_kinds));
        }
        out.insert(key, InlineDoc { kind, text });
    }
    Ok(out)
}

/// Refuses every registered doc kind, and the profile type, whose schema declares a top-level
/// envelope field (`L1257`). Fails on the first offender.
pub(crate) fn validate_registered_types(
    doc_kinds: &[DocKind],
    profile: Option<&crate::player_profile::ProfileKind>,
) -> Result<(), Diagnostic> {
    for doc_kind in doc_kinds {
        ludion_scene::check_doc_type(&doc_kind.kind, &doc_kind.schema)?;
    }
    if let Some(profile) = profile {
        ludion_scene::check_doc_type("profile", profile.schema())?;
    }
    Ok(())
}

/// A kind's monomorphized loader: deserialize the walk's collected field maps into a `DocStore<T>`
/// and insert it as a world resource.
type DocInstaller = fn(&mut World, BTreeMap<String, Value>) -> Result<(), Vec<Diagnostic>>;

/// A kind's monomorphized single-doc reloader: deserialize one already-schema-validated field map and
/// upsert it into the live `DocStore<T>` (bumping generation) — the hot-reload counterpart of
/// [`DocInstaller`]. On success it hands back the [`DocUndo`] that restores the store.
type DocReloader = fn(&mut World, String, Value) -> Result<DocUndo, Vec<Diagnostic>>;

/// Puts a `DocStore` back as it was before one [`DocReloader`] applied: the displaced doc (or its
/// absence) and the previous generation.
pub(crate) type DocUndo = Box<dyn FnOnce(&mut World) + Send>;

/// One registered doc kind, held persistently on the [`Engine`](crate::Engine): the `kind` string,
/// its captured schema, and a monomorphized loader that deserializes the walk's collected field maps
/// into a `DocStore<T>` and inserts it as a world resource.
///
/// The loader is a plain `fn` pointer (`Copy`, like a registered system), so the registration list
/// is a durable source of truth the engine re-runs after every world reset — not a value consumed on
/// the first build.
///
/// A type with a top-level `format` or `kind` field is refused when the engine is built (`L1257`):
/// it would collide with the doc envelope.
///
/// A `singleton` kind must hold exactly one doc: the engine's count rule (`L1315`) runs with the
/// validators, reading the store through the monomorphized `keys`.
pub(crate) struct DocKind {
    pub(crate) kind: String,
    pub(crate) schema: Value,
    pub(crate) singleton: bool,
    install: DocInstaller,
    reload: DocReloader,
    /// Adds to the kind's store generation; a no-op when the store is absent.
    advance_generation: fn(&mut World, u64),
    keys: fn(&World) -> Vec<String>,
}

impl DocKind {
    /// Captures a kind's `(name, schema, monomorphized loader/reloader)` — the value
    /// [`EngineBuilder::doc_kind`](crate::EngineBuilder::doc_kind) (or `doc_singleton`, with
    /// `singleton` set) pushes onto the registration list.
    pub(crate) fn new<T>(kind: &str, singleton: bool) -> Self
    where
        T: DeserializeOwned + Send + Sync + 'static + schemars::JsonSchema,
    {
        DocKind {
            kind: kind.to_string(),
            schema: serde_json::to_value(schemars::schema_for!(T))
                .expect("a schemars JSON Schema always serializes to a JSON value"),
            singleton,
            install: install_doc_store::<T>,
            reload: reload_doc_store::<T>,
            advance_generation: advance_doc_generation::<T>,
            keys: doc_store_keys::<T>,
        }
    }
}

/// Adds `by` to the generation of `DocStore<T>`; nothing when the store is absent. The monomorphized
/// body of every [`DocKind::advance_generation`].
fn advance_doc_generation<T>(world: &mut World, by: u64)
where
    T: Send + Sync + 'static,
{
    if let Some(store) = world.resource_mut::<DocStore<T>>() {
        store.generation += by;
    }
}

/// The keys of `DocStore<T>` in store order; empty when the store is absent. The monomorphized body
/// of every [`DocKind::keys`].
fn doc_store_keys<T>(world: &World) -> Vec<String>
where
    T: Send + Sync + 'static,
{
    world
        .resource::<DocStore<T>>()
        .map(|store| store.docs.keys().cloned().collect())
        .unwrap_or_default()
}

/// Deserializes the collected field maps into `DocStore<T>` and inserts it as a world resource. The
/// monomorphized body of every [`DocKind::install`]; a fresh empty store is inserted even for a kind
/// with no docs, so `world.resource::<DocStore<T>>()` is always present once the kind is registered.
fn install_doc_store<T>(
    world: &mut World,
    field_maps: BTreeMap<String, Value>,
) -> Result<(), Vec<Diagnostic>>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    let mut docs = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (key, value) in field_maps {
        // Should-not-happen after `check_doc`: a schema/serde mismatch, reported rather than silent.
        match ludion_scene::deserialize_doc::<T>(value) {
            Ok(doc) => {
                docs.insert(key, doc);
            }
            Err(diagnostic) => diagnostics.push(prefix_file(diagnostic, &key)),
        }
    }
    if diagnostics.is_empty() {
        world.insert_resource(DocStore::new(docs));
        Ok(())
    } else {
        Err(diagnostics)
    }
}

/// Deserializes one already-schema-validated field map and upserts it into the live `DocStore<T>`,
/// bumping generation. The monomorphized body of every [`DocKind::reload`]; the store is present
/// (every registered kind gets one at world build, even when empty), but a defensive fresh store is
/// created if it is somehow absent so the reload still lands.
fn reload_doc_store<T>(
    world: &mut World,
    key: String,
    field_map: Value,
) -> Result<DocUndo, Vec<Diagnostic>>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    // Should-not-happen after `check_doc`: a schema/serde mismatch. The store is untouched on this
    // path, so a bad reload never wipes prior contents.
    let doc = ludion_scene::deserialize_doc::<T>(field_map)
        .map_err(|diagnostic| vec![prefix_file(diagnostic, &key)])?;
    Ok(match world.resource_mut::<DocStore<T>>() {
        Some(store) => {
            let generation = store.generation;
            let displaced = store.upsert(key.clone(), doc);
            Box::new(move |world: &mut World| {
                if let Some(store) = world.resource_mut::<DocStore<T>>() {
                    match displaced {
                        Some(old) => store.docs.insert(key, old),
                        None => store.docs.remove(&key),
                    };
                    store.generation = generation;
                }
            })
        }
        None => {
            let mut docs = BTreeMap::new();
            docs.insert(key, doc);
            let mut store = DocStore::new(docs);
            store.generation = 1;
            world.insert_resource(store);
            Box::new(|world: &mut World| {
                world.remove_resource::<DocStore<T>>();
            })
        }
    })
}

/// Reloads one doc of `kind` (at content-key `key`) from `source` into its live [`DocStore`], bumping
/// generation, and returns the doc's new **canonical** text (for recording it inline): [`check_reload`]
/// then [`apply_reload`]. Replay reapply (`source` the recorded inline text) and `prepare_docs` call
/// this directly, ungated — a recording replays what was accepted; the live `asset.reload` runs its
/// own gates between the two halves.
///
/// The store keeps its previous contents on any error — a malformed edit mid-tune must not wipe state:
/// `check_doc` runs (and can fail) before the upsert, and the upsert's own deserialize failure returns
/// before touching the store. The live `asset.reload` may still undo a successful apply
/// ([`apply_reload`]'s [`DocUndo`]) when a validator rejects the result.
///
/// # Errors
/// `L1119`/`L1120` (each prefixed with `key`) if `source` fails envelope or schema validation, or if
/// `kind` is not registered (a should-not-happen guard — the caller resolves the kind first).
pub(crate) fn reload_doc(
    doc_kinds: &[DocKind],
    world: &mut World,
    kind: &str,
    key: &str,
    source: &str,
) -> Result<String, Vec<Diagnostic>> {
    let fields = check_reload(doc_kinds, kind, key, source)?;
    apply_reload(doc_kinds, world, kind, key, fields).map(|(document, _undo)| document)
}

/// The validating half of [`reload_doc`]: resolves `kind`, then checks `source` against its envelope
/// and schema, returning the document's field map. Touches no world state.
///
/// # Errors
/// As [`reload_doc`]'s validation errors.
pub(crate) fn check_reload(
    doc_kinds: &[DocKind],
    kind: &str,
    key: &str,
    source: &str,
) -> Result<Value, Vec<Diagnostic>> {
    let dk = doc_kinds
        .iter()
        .find(|dk| dk.kind == kind)
        .ok_or_else(|| vec![unregistered_kind(key, kind, doc_kinds)])?;
    ludion_scene::check_doc(source, kind, &dk.schema).map_err(|errors| {
        errors
            .into_iter()
            .map(|d| prefix_file(d, key))
            .collect::<Vec<_>>()
    })
}

/// The applying half of [`reload_doc`]: upserts `fields` (from [`check_reload`]) into the kind's
/// store, bumping generation, and returns the canonical text with the [`DocUndo`] that restores the
/// store as it was.
///
/// # Errors
/// `L1120` if the schema-valid `fields` still fail to deserialize; the store is untouched.
pub(crate) fn apply_reload(
    doc_kinds: &[DocKind],
    world: &mut World,
    kind: &str,
    key: &str,
    fields: Value,
) -> Result<(String, DocUndo), Vec<Diagnostic>> {
    let dk = doc_kinds
        .iter()
        .find(|dk| dk.kind == kind)
        .ok_or_else(|| vec![unregistered_kind(key, kind, doc_kinds)])?;
    let undo = (dk.reload)(world, key.to_string(), fields.clone())?;
    Ok((ludion_scene::emit_doc(kind, &fields, &dk.schema), undo))
}

/// Peeks `source`'s top-level `kind:` for the live-reload path (`Engine::reload_doc_from_disk`),
/// prefixing a strict-YAML parse failure with `key` the way every other doc diagnostic collected by
/// this module is. `Ok(None)` when `source` has no `kind` (a scene) or is empty — not our path, the
/// caller falls through to the existing silent no-op; `Err` when the reader rejects `source` outright,
/// which the caller reports rather than treating as "not our path" (`a_malformed_reload_is_rejected_and_preserves_the_store`).
///
/// # Errors
/// `L100x` (prefixed with `key`) if `source` does not parse as strict-subset YAML at all.
pub(crate) fn peek_reload_kind(key: &str, source: &str) -> Result<Option<String>, Diagnostic> {
    ludion_scene::peek_kind_checked(source).map_err(|d| prefix_file(d, key))
}

/// Resolves an `asset.reload` path (relative to `root`, or absolute as a host watcher forwards it) to
/// a `(content-root-relative key, file source)` pair **iff** it names an existing `*.yaml` under the
/// content root — the gate the doc-reload branch checks before peeking `kind:`. `Ok(None)` for a
/// non-YAML path, an absolute file outside the root, or one that cannot be read (the caller then
/// leaves it a no-op).
///
/// The key matches the boot walk's ([`content_root_yaml`]), so a reload upserts the same `DocStore`
/// entry the boot load created rather than a divergent one. The walk never yields a key with `..`,
/// so a path that climbs — relative, or absolute with a `..` segment — is refused before any read.
///
/// # Errors
/// `L1321` for a `*.yaml` path that climbs with `..`.
pub(crate) fn resolve_doc_path(
    root: &Path,
    incoming: &str,
) -> Result<Option<(String, String)>, Diagnostic> {
    let incoming_path = Path::new(incoming);
    let abs = if incoming_path.is_absolute() {
        incoming_path.to_path_buf()
    } else {
        root.join(incoming_path)
    };
    if abs.extension().and_then(|e| e.to_str()) != Some("yaml") {
        return Ok(None);
    }
    if abs
        .strip_prefix(root)
        .is_ok_and(|rel| rel.components().any(|c| matches!(c, Component::ParentDir)))
    {
        return Err(Diagnostic::new(
            ludion_core::codes::code::CONTENT_PATH_OUTSIDE_ROOT,
            format!("asset path `{incoming}` climbs out of the content root with `..`"),
        )
        .with_help("name the document relative to the content root, without `..` segments"));
    }
    let Some(key) = doc_key(root, &abs) else {
        return Ok(None);
    };
    Ok(std::fs::read_to_string(&abs)
        .ok()
        .map(|source| (key, source)))
}

/// The content-root-relative, slash-normalized key for `abs`, or `None` if `abs` is not under `root`.
/// A lexical strip first (the common case); a canonical-identity strip second, so a host-forwarded
/// absolute path whose textual form differs from the root's (e.g. macOS `/var`→`/private/var`) still
/// resolves to the same key the boot walk produced.
fn doc_key(root: &Path, abs: &Path) -> Option<String> {
    if let Ok(rel) = abs.strip_prefix(root) {
        return Some(rel.to_string_lossy().replace('\\', "/"));
    }
    let canon_target = std::fs::canonicalize(abs).ok()?;
    let canon_root = std::fs::canonicalize(root).ok()?;
    let rel = canon_target.strip_prefix(&canon_root).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Walks the content `root` recursively and rebuilds every registered kind's [`DocStore<T>`] from
/// the docs it finds — the engine's boot/reset doc-load step.
///
/// **When no kinds are registered this does nothing — no walk, no IO** — so a game that registers no
/// doc kinds boots byte-identically to before this facility existed. Otherwise every `*.yaml` under
/// `root` is read and sniffed textually (`ludion_scene::sniff_kind`): a file with no line that could be a
/// top-level `kind` key is a scene, and one whose plainly decoded `kind` is an engine kind is a
/// loader's asset; both are skipped unparsed. Every other file is peeked
/// (via `ludion_scene::peek_kind_checked`) for its top-level `kind:` and routed: no `kind` (a scene)
/// or an engine kind ([`is_engine_kind`]) is skipped; a registered kind is validated with
/// `ludion_scene::check_doc` against the captured schema and collected into that kind's store; an
/// unregistered `kind` is an L1119 (or, per `game_kinds` below, a skipped L1155); a file the
/// strict-YAML subset rejects outright is an L100x — reported, never silently skipped the way a
/// `kind`-less scene is. Walk order is pinned by sorting on the normalized key, so two boots from
/// the same tree produce identical stores.
///
/// Diagnostics are collected across every file (fail-fast at the end with everything reported, not
/// first-error-wins) so an agent tuning content sees all its broken docs in one pass. On success,
/// every registered kind has a resource in `world` (empty if it matched no file).
///
/// `game_kinds` says whether any of `doc_kinds` came from a game's own
/// [`doc_kind`](crate::EngineBuilder::doc_kind). When it is false — the built-in engine behind
/// `ludion serve`/`ludion sim-hash`, which registers only engine-owned kinds and hosts anyone's
/// scene by construction — an unregistered `kind:` cannot be a typo in content this engine owns, so
/// the file is skipped with an `L1155` warning (returned on the `Ok` path) instead of refusing the
/// boot.
///
/// # Errors
/// Every collected diagnostic: `L1101` for an unreadable file, `L100x` for a file the strict-YAML
/// reader rejects, `L1119`/`L1120` for a malformed doc (each prefixed with the offending file's
/// path), `L1119` for an unregistered `kind` on an engine a game registered a kind on.
///
/// `inline` (the builder's [`doc_text`](crate::EngineBuilder::doc_text) docs, keyed like the walk)
/// replaces the file at the same key and is checked the same way after the walk's files.
pub(crate) fn load_doc_stores(
    root: &Path,
    doc_kinds: &[DocKind],
    game_kinds: bool,
    inline: &BTreeMap<String, InlineDoc>,
    save_dir: Option<&Path>,
    world: &mut World,
) -> Result<DocSkips, Vec<Diagnostic>> {
    if doc_kinds.is_empty() && inline.is_empty() {
        return Ok(DocSkips::default());
    }

    // A shadowed file is not even read, so an inline doc also wins over a malformed file.
    let mut files = content_root_yaml(root, save_dir);
    files.retain(|(key, _)| !inline.contains_key(key));

    let mut per_kind: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut warnings = Vec::new();
    // Counted per kind, not per file: what a host prints is the summary, and what it needs to say is
    // which game kinds went unread and how many files that cost — never the file list.
    let mut skipped_per_kind: BTreeMap<String, usize> = BTreeMap::new();
    for (key, path) in &files {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) => {
                diagnostics.push(unreadable_doc(key, path, &error));
                continue;
            }
        };
        // Full-parsing every scene and engine asset only to route it dominated boot time; a file
        // skipped here that the reader would reject is reported by its own loader, not this walk.
        match ludion_scene::sniff_kind(&source) {
            ludion_scene::KindSniff::Absent => continue,
            ludion_scene::KindSniff::Kind(kind) if is_engine_kind(kind) => continue,
            _ => {}
        }
        match ludion_scene::peek_kind_checked(&source) {
            // No `kind` key is a scene; the scene loader owns those.
            Ok(None) => continue,
            // A file the strict-YAML subset rejects outright: reported like any other malformed
            // doc, never silently absent from its `DocStore` (the defect this walk used to have).
            Err(diagnostic) => diagnostics.push(prefix_file(diagnostic, key)),
            Ok(Some(kind)) if is_engine_kind(&kind) => continue,
            Ok(Some(kind)) => match doc_kinds.iter().find(|dk| dk.kind == kind) {
                Some(dk) => match ludion_scene::check_doc(&source, &kind, &dk.schema) {
                    Ok(fields) => {
                        per_kind
                            .entry(dk.kind.as_str())
                            .or_default()
                            .insert(key.clone(), fields);
                    }
                    Err(errors) => {
                        diagnostics.extend(errors.into_iter().map(|d| prefix_file(d, key)));
                    }
                },
                None if game_kinds => diagnostics.push(unregistered_kind(key, &kind, doc_kinds)),
                None => {
                    *skipped_per_kind.entry(kind.clone()).or_default() += 1;
                    warnings.push(skipped_kind(key, &kind));
                }
            },
        }
    }
    for (key, doc) in inline {
        let Some(dk) = doc_kinds.iter().find(|dk| dk.kind == doc.kind) else {
            continue;
        };
        match ludion_scene::check_doc(&doc.text, &doc.kind, &dk.schema) {
            Ok(fields) => {
                per_kind
                    .entry(dk.kind.as_str())
                    .or_default()
                    .insert(key.clone(), fields);
            }
            Err(errors) => diagnostics.extend(errors.into_iter().map(|d| prefix_file(d, key))),
        }
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    // Insert every kind's store (empty if it matched nothing), collecting any deserialization
    // mismatch across all kinds.
    let mut install_diagnostics = Vec::new();
    for dk in doc_kinds {
        let maps = per_kind.remove(dk.kind.as_str()).unwrap_or_default();
        if let Err(errors) = (dk.install)(world, maps) {
            install_diagnostics.extend(errors);
        }
    }
    if install_diagnostics.is_empty() {
        Ok(DocSkips {
            summary: skipped_kinds_summary(&skipped_per_kind),
            warnings,
        })
    } else {
        Err(install_diagnostics)
    }
}

/// Every `*.yaml` file the content-root walk reads, keyed by slash-normalized path relative to `root`
/// and sorted by key. Dot-directories, `target/` and `node_modules/` are skipped: build output and
/// engine scratch are not content. So is `save_dir` (the engine's `<save_root>/save`, see
/// `Engine::save_dir`) when it lies under `root`: saved player state is not content, and the lenient
/// save loads own it. The boot walk and the facade's `--check-docs` both use this, so the files a
/// host counts are exactly the files the engine validated.
///
/// # Example
/// ```
/// let root = std::env::temp_dir().join(format!("ludion-content-root-yaml-{}", std::process::id()));
/// for rel in ["a.yaml", "target/b.yaml", ".ludion/c.yaml", "save/x.yaml"] {
///     let path = root.join(rel);
///     std::fs::create_dir_all(path.parent().unwrap()).unwrap();
///     std::fs::write(path, "format: 1\n").unwrap();
/// }
/// let keys = |skip: Option<&std::path::Path>| -> Vec<String> {
///     ludion_engine::content_root_yaml(&root, skip)
///         .into_iter()
///         .map(|(key, _)| key)
///         .collect()
/// };
/// assert_eq!(keys(None), ["a.yaml", "save/x.yaml"]);
/// assert_eq!(keys(Some(&root.join("save"))), ["a.yaml"]);
/// std::fs::remove_dir_all(&root).unwrap();
/// ```
pub fn content_root_yaml(root: &Path, save_dir: Option<&Path>) -> Vec<(String, PathBuf)> {
    let skip = save_dir.and_then(|dir| save_dir_key(root, dir));
    let mut files = Vec::new();
    collect_yaml(root, root, skip.as_deref(), &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// The slash-normalized key of `save_dir` relative to `root`, or `None` when it lies outside the
/// content root (a `save_root` set elsewhere). Falls back to canonical paths when one side is
/// relative and the other absolute. The walk and the doc reload share this rule.
pub(crate) fn save_dir_key(root: &Path, save_dir: &Path) -> Option<String> {
    let rel = match save_dir.strip_prefix(root) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => save_dir
            .canonicalize()
            .ok()?
            .strip_prefix(root.canonicalize().ok()?)
            .ok()?
            .to_path_buf(),
    };
    let key = rel.to_string_lossy().replace('\\', "/");
    (!key.is_empty()).then_some(key)
}

/// Whether the doc `key` lies at or under the save directory's relative key.
pub(crate) fn key_under_save_dir(key: &str, save_key: &str) -> bool {
    key == save_key
        || key
            .strip_prefix(save_key)
            .is_some_and(|r| r.starts_with('/'))
}

/// Recursively collects every `*.yaml` file under `dir`, keyed by its slash-normalized path relative
/// to `root`. Unreadable directories are skipped (they hold no reachable docs), and so are
/// dot-directories — `.ludion/` is the engine's own scratch/output convention and never holds
/// content, so a stray scratch YAML there must not fail a game's boot. The caller sorts, so the
/// arbitrary `read_dir` order does not leak.
fn collect_yaml(root: &Path, dir: &Path, skip: Option<&str>, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            // Build output, not content: a default `project_root(".")` — every
            // `GameplayDriver::new` test's root — sits above `target/`, which after a few builds
            // holds hundreds of thousands of files.
            if name.to_string_lossy().starts_with('.') || name == "target" || name == "node_modules"
            {
                continue;
            }
            if let Some(skip) = skip
                && path
                    .strip_prefix(root)
                    .is_ok_and(|rel| rel.to_string_lossy().replace('\\', "/") == skip)
            {
                continue;
            }
            collect_yaml(root, &path, skip, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("yaml")
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push((rel.to_string_lossy().replace('\\', "/"), path));
        }
    }
}

/// Prefixes a doc diagnostic's message with the file it came from, so a multi-file walk's collected
/// diagnostics each name their source (the file is the context `check_doc`'s in-doc JSON Pointer
/// lacks). The extract path annotates `L1101` with the offending component path the same way.
fn prefix_file(mut diagnostic: Diagnostic, key: &str) -> Diagnostic {
    diagnostic.message = format!("{key}: {}", diagnostic.message);
    diagnostic
}

/// An `L1101` for a `*.yaml` the walk found but could not read as UTF-8 text (reusing the asset IO
/// code rather than minting a doc-specific one).
fn unreadable_doc(key: &str, path: &Path, error: &std::io::Error) -> Diagnostic {
    Diagnostic::new(
        code::ASSET_FILE_IO,
        format!(
            "could not read data-doc `{key}` (at `{}`): {error}",
            path.display()
        ),
    )
    .with_help("check the file exists and is readable UTF-8 text")
}

/// An `L1155` for a doc whose `kind:` this engine does not know, on an engine no *game* registered a
/// kind on — the built-in engine behind `ludion serve`/`ludion sim-hash`. It hosts anyone's scene, so
/// a foreign kind is a fact about the content root rather than a typo in content it owns: the file is
/// skipped and said so, never a boot failure. No nearest-match suggestion — there is nothing to have
/// meant.
fn skipped_kind(key: &str, kind: &str) -> Diagnostic {
    Diagnostic::new(
        code::DOC_KIND_NOT_REGISTERED,
        format!("{key}: declares `kind: {kind}`, which this engine does not register — skipped"),
    )
    .with_path("/kind")
    .with_help("host the scene through the game's own binary, whose builder registers the kind")
}

/// What a content-root walk skipped for declaring a `kind:` this engine does not register: the
/// per-file `L1155` warnings in walk order, and the one aggregated `L1155` a host prints in their
/// place. Both describe the same files — the pair exists because the precise answer and the readable
/// one are different sizes: a game's scene directory holds tens of its own docs, and one three-line
/// block each buries whatever the host says next (an unbindable port, most sharply).
#[derive(Debug, Default)]
pub(crate) struct DocSkips {
    /// One warning per skipped file, in walk order. The programmatic answer.
    pub(crate) warnings: Vec<Diagnostic>,
    /// The single warning a host prints instead of [`warnings`](DocSkips::warnings). `None` when
    /// nothing was skipped.
    pub(crate) summary: Option<Diagnostic>,
}

/// The one `L1155` a host prints in place of [`skipped_kind`]'s per-file warnings: how many files
/// were skipped and, in kind order, which kinds cost what. The kinds are the actionable part — they
/// name what a game binary would have to register — and the file list is not, so it is dropped
/// rather than folded in. `None` when nothing was skipped.
fn skipped_kinds_summary(skipped_per_kind: &BTreeMap<String, usize>) -> Option<Diagnostic> {
    let total: usize = skipped_per_kind.values().sum();
    if total == 0 {
        return None;
    }
    let kinds = skipped_per_kind
        .iter()
        .map(|(kind, count)| format!("{kind} ({count})"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(
        Diagnostic::new(
            code::DOC_KIND_NOT_REGISTERED,
            format!(
                "skipped {total} document{} under the content root declaring a `kind:` this engine \
                 does not register: {kinds}",
                if total == 1 { "" } else { "s" }
            ),
        )
        .with_help(
            "host the scene through the game's own binary, whose builder registers the kinds",
        ),
    )
}

/// An `L1119` for a doc whose `kind:` is neither an engine kind nor a registered doc kind: naming
/// the file and suggesting the nearest registered (or engine) kind. A game that registers zero kinds
/// never walks, so this only bites a game that opted in — a typo'd `kind:` fails the boot loudly
/// rather than tuning content blind (the fail-fast posture, decision 4). An engine carrying only
/// engine-owned kinds takes [`skipped_kind`] instead.
fn unregistered_kind(key: &str, kind: &str, doc_kinds: &[DocKind]) -> Diagnostic {
    let registered: Vec<&str> = doc_kinds.iter().map(|dk| dk.kind.as_str()).collect();
    let diagnostic = Diagnostic::new(
        code::DOC_ENVELOPE_INVALID,
        format!("{key}: declares `kind: {kind}`, which is not a registered data-doc kind"),
    )
    .with_path("/kind");
    // Suggest over registered kinds *and* the engine kinds, so a typo of either is caught.
    let candidates = registered.iter().copied().chain(
        ludion_scene::KINDS
            .iter()
            .filter(|entry| !entry.doc_store)
            .map(|entry| entry.name),
    );
    match suggest::nearest(kind, candidates) {
        Some(candidate) => diagnostic.with_help(format!("nearest kind is `{candidate}`")),
        None => diagnostic.with_help(format!(
            "registered kinds: {}; or `mesh`/`voxel` for an engine asset, `profile` for a player save, \
             or remove `kind:` for a scene",
            registered.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    /// `pattern` is the one engine kind a game registers as a doc store, so the walk must not skip it.
    #[test]
    fn the_registered_pattern_kind_is_not_skipped() {
        assert!(
            ludion_scene::kind_entry(crate::builder::PATTERN_KIND).is_some_and(|k| k.doc_store)
        );
        assert!(!super::is_engine_kind(crate::builder::PATTERN_KIND));
    }
}
