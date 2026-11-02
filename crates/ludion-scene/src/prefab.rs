//! Prefab documents: `kind: prefab` — a reusable group of named entities a scene or the game spawns
//! as one instance (`Plan/transform-hierarchy-and-prefabs.md` §H).
//!
//! The body is exactly a scene's `entities:` list: the same entity parser, the same schema
//! validator, the same `L####` codes and the same position-based references (here the range is the
//! prefab's own entities). What differs is the envelope and one rule per part:
//!
//! ```yaml
//! format: 1
//! kind: prefab
//! name: goblin          # optional
//! generated_from: ../goblin.gltf   # optional; set by `ludion import`
//! entities:
//!   - name: pelvis      # required on every part, unique within the prefab, no `/`
//!     components: {}
//! ```
//!
//! A prefab carries no `seed` or `physics`: the scene that spawns it does. The
//! singleton-component refusal (`L1229`) names concrete components, so it lives in `ludion-engine`.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::Registry;
use serde_json::Value;

use crate::expand::{merge_partial, ref_targets, remap_positions};
use crate::scalar::scalar_string;
use crate::yaml::{self, NodeValue};
use crate::{AssemblyClip, Scene, SceneEntity, emit as scene_emit, envelope, parse as scene_parse};

/// The prefab document format version this crate reads and writes.
pub const PREFAB_FORMAT_VERSION: u64 = 1;

const PREFAB_KEYS: [&str; 6] = [
    "format",
    "kind",
    "name",
    "generated_from",
    "clips",
    "entities",
];
const SCENE_ONLY_KEYS: [&str; 3] = ["seed", "physics", "preload"];

/// A parsed `kind: prefab` document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PrefabDocument {
    /// Optional prefab name.
    pub name: Option<String>,
    /// The source file this prefab was generated from; a generated prefab is write-only.
    pub generated_from: Option<String>,
    /// Clip bindings (short name → clip file, relative to the prefab), sorted by short name. Empty
    /// means the prefab binds no clips; an authored `clips: {}` and an absent `clips` are the same.
    pub clips: Vec<AssemblyClip>,
    /// The parts, in document order; every one is named.
    pub entities: Vec<SceneEntity>,
}

impl PrefabDocument {
    /// The prefab body as a [`Scene`] — the bridge to [`crate::validate`] and to every pass that
    /// takes a `&Scene`. Entity references in it are positions in `entities`.
    pub fn to_scene(&self) -> Scene {
        Scene {
            entities: self.entities.clone(),
            ..Default::default()
        }
    }

    /// Expands the prefab's own instance entries into a flat scene; see [`crate::expand`].
    pub fn expand(
        &self,
        registry: &Registry,
        resolve: &mut dyn FnMut(&str) -> Result<String, Diagnostic>,
    ) -> Result<crate::ExpandedScene, Vec<Diagnostic>> {
        crate::expand(&self.to_scene(), registry, resolve)
    }

    /// Rewrites every path in the document from relative-to-this-prefab to content-root form, in
    /// place. `key` is the prefab's own content-root path. Covers the asset paths of the parts'
    /// components, of every instance entry's `overrides`, each nested entry's `prefab` and each
    /// clip binding's file; empty
    /// paths (inert defaults) and unknown components are left alone.
    ///
    /// # Example
    /// ```
    /// use schemars::JsonSchema;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Model {
    ///     #[schemars(with = "ludion_ecs::AssetPathSchema")]
    ///     mesh: String,
    /// }
    ///
    /// let mut registry = ludion_ecs::Registry::new();
    /// registry.register::<Model>("Model").unwrap();
    /// let source = "format: 1\nkind: prefab\nentities:\n  - name: body\n    components:\n      Model:\n        mesh: meshes/a.mesh.yaml\n";
    /// let mut doc = ludion_scene::parse_prefab(source).unwrap();
    /// doc.rebase("level/render/model.prefab.yaml", &registry);
    /// assert_eq!(doc.entities[0].components[0].1["mesh"], "level/render/meshes/a.mesh.yaml");
    /// ```
    pub fn rebase(&mut self, key: &str, registry: &Registry) {
        for clip in &mut self.clips {
            clip.file = ludion_core::paths::sibling(key, &clip.file);
        }
        for entity in &mut self.entities {
            rebase_components(&mut entity.components, key, registry);
            if let Some(instance) = entity.instance.as_deref_mut() {
                instance.prefab = ludion_core::paths::sibling(key, &instance.prefab);
                for components in instance.overrides.values_mut() {
                    rebase_components(components, key, registry);
                }
            }
        }
    }

    /// Writes an instance's collapsed edits into this prefab, so a fresh instance of it equals the
    /// edited one: `overrides` (instance part path to partial component values) merge onto the
    /// parts' components, and `removed` parts are deleted.
    ///
    /// A path without `/` names a part of this document; `head/rest` names `rest` inside the nested
    /// instance entry `head`, whose own `overrides` and `removed` take the edit. A `removed` path
    /// naming a nested entry itself deletes the whole entry. Deleting a part
    /// shifts every later position, in plain components and in nested entries' override values.
    /// Override values must carry no entity reference: they would be positions in the instance's
    /// document, not this one.
    ///
    /// # Errors
    /// `L1237` for a path naming no part (the message lists the parts), or for a part deleted while
    /// another entity still refers to it. `self` may be partly edited on error: apply to a copy.
    pub fn apply_instance(
        &mut self,
        overrides: &BTreeMap<String, Vec<(String, Value)>>,
        removed: &[String],
        registry: &Registry,
    ) -> Result<(), Diagnostic> {
        for (path, components) in overrides {
            match path.split_once('/') {
                None => {
                    let part = self.plain_part(path)?;
                    for (name, value) in components {
                        merge_component(&mut self.entities[part].components, name, value);
                    }
                }
                Some((head, rest)) => {
                    let entry = self.entry_part(head, path)?;
                    let instance = self.entities[entry].instance.as_mut().expect("an entry");
                    let target = instance.overrides.entry(rest.to_owned()).or_default();
                    for (name, value) in components {
                        merge_component(target, name, value);
                    }
                }
            }
        }
        for path in removed {
            match path.split_once('/') {
                None => {
                    let part = match self.entry_part(path, path) {
                        Ok(entry) => entry,
                        Err(_) => self.plain_part(path)?,
                    };
                    self.delete_part(part, path, registry)?;
                }
                Some((head, rest)) => {
                    let entry = self.entry_part(head, path)?;
                    let instance = self.entities[entry].instance.as_mut().expect("an entry");
                    if !instance.removed.iter().any(|held| held == rest) {
                        instance.removed.push(rest.to_owned());
                    }
                    instance.overrides.remove(rest);
                }
            }
        }
        Ok(())
    }

    fn entry_names(&self) -> String {
        let names: Vec<&str> = self
            .entities
            .iter()
            .filter_map(|entity| entity.name.as_deref())
            .collect();
        names.join(", ")
    }

    fn plain_part(&self, name: &str) -> Result<usize, Diagnostic> {
        self.entities
            .iter()
            .position(|entity| entity.instance.is_none() && entity.name.as_deref() == Some(name))
            .ok_or_else(|| {
                Diagnostic::new(
                    code::PREFAB_APPLY,
                    format!(
                        "the prefab has no part `{name}` (its entries: {})",
                        self.entry_names()
                    ),
                )
                .with_help("apply only parts the prefab has, or a nested part as `<entry>/<part>`")
            })
    }

    fn entry_part(&self, head: &str, path: &str) -> Result<usize, Diagnostic> {
        self.entities
            .iter()
            .position(|entity| entity.instance.is_some() && entity.name.as_deref() == Some(head))
            .ok_or_else(|| {
                Diagnostic::new(
                    code::PREFAB_APPLY,
                    format!(
                        "`{path}` names no nested instance `{head}` in the prefab (its entries: {})",
                        self.entry_names()
                    ),
                )
                .with_help("a path with `/` starts with a nested instance entry's name")
            })
    }

    /// Deletes the plain part at `part` and shifts every reference above it down by one.
    fn delete_part(
        &mut self,
        part: usize,
        name: &str,
        registry: &Registry,
    ) -> Result<(), Diagnostic> {
        let refers =
            |value: &(String, Value)| ref_targets(registry, &value.0, &value.1).contains(&part);
        for (position, entity) in self.entities.iter().enumerate() {
            let in_overrides = entity
                .instance
                .iter()
                .flat_map(|instance| instance.overrides.values().flatten());
            if position != part && entity.components.iter().chain(in_overrides).any(refers) {
                return Err(Diagnostic::new(
                    code::PREFAB_APPLY,
                    format!(
                        "cannot remove part `{name}`: `{}` still refers to it",
                        entity.name.as_deref().unwrap_or("an unnamed entity")
                    ),
                )
                .with_help("remove or re-point the reference first"));
            }
        }
        self.entities.remove(part);
        let shift = |position: usize| (position > part).then(|| position - 1);
        for entity in &mut self.entities {
            for (component, value) in &mut entity.components {
                remap_positions(registry, component, value, &shift);
            }
            if let Some(instance) = entity.instance.as_mut() {
                for (component, value) in instance.overrides.values_mut().flatten() {
                    remap_positions(registry, component, value, &shift);
                }
            }
        }
        Ok(())
    }
}

fn merge_component(components: &mut Vec<(String, Value)>, name: &str, value: &Value) {
    match components.iter_mut().find(|(held, _)| held == name) {
        Some((_, held)) => merge_partial(held, value.clone()),
        None => components.push((name.to_owned(), value.clone())),
    }
}

/// Parses strict-subset YAML into a [`PrefabDocument`], collecting structure diagnostics and the
/// part-name rules (`L1226`–`L1228`) in one pass.
pub(crate) fn parse(source: &str) -> Result<PrefabDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::PREFAB_DOCUMENT_STRUCTURE,
                "the prefab document is empty; a prefab needs at least `format: 1`, `kind: prefab` \
                 and `entities`",
            )
            .with_path("")
            .with_help(
                "a minimal prefab is:\n  format: 1\n  kind: prefab\n  entities:\n    - name: root\n      components: {}",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::PREFAB_DOCUMENT_STRUCTURE,
                format!(
                    "the prefab document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: prefab`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = PrefabDocument::default();
    let mut parts: Vec<(usize, SceneEntity)> = Vec::new();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut entities_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::PREFAB_DOCUMENT_STRUCTURE,
                    code::PREFAB_DOCUMENT_STRUCTURE,
                    PREFAB_FORMAT_VERSION,
                    "the prefab document",
                    "prefab",
                    "prefab",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::PREFAB_DOCUMENT_STRUCTURE,
                    "prefab",
                    "a prefab document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the prefab name",
                    code::PREFAB_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "generated_from" => {
                doc.generated_from = envelope::expect_string(
                    &entry.node,
                    "/generated_from",
                    "the generated-from provenance marker",
                    code::PREFAB_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "clips" => {
                doc.clips = envelope::parse_clips(
                    entry,
                    code::PREFAB_DOCUMENT_STRUCTURE,
                    "clips/open.anim.yaml",
                    &mut diagnostics,
                );
            }
            "entities" => {
                entities_seen = true;
                match &entry.node.value {
                    NodeValue::Sequence(items) if items.is_empty() => diagnostics.push(
                        Diagnostic::new(
                            code::PREFAB_EMPTY,
                            format!(
                                "the prefab has no parts: `entities` is empty (line {})",
                                entry.node.line
                            ),
                        )
                        .with_path("/entities")
                        .with_help(
                            "name at least one part:\n  entities:\n    - name: root\n      components: {}",
                        ),
                    ),
                    NodeValue::Sequence(items) => {
                        for (index, item) in items.iter().enumerate() {
                            if let Some(entity) =
                                scene_parse::parse_entity(item, index, &mut diagnostics)
                            {
                                parts.push((index, entity));
                            }
                        }
                    }
                    other => diagnostics.push(
                        Diagnostic::new(
                            code::PREFAB_DOCUMENT_STRUCTURE,
                            format!(
                                "`entities` must be a sequence, found {} (line {})",
                                other.type_phrase(),
                                entry.node.line
                            ),
                        )
                        .with_path("/entities")
                        .with_help("write one part per `- ` item"),
                    ),
                }
            }
            scene_only if SCENE_ONLY_KEYS.contains(&scene_only) => diagnostics.push(
                Diagnostic::new(
                    code::PREFAB_DOCUMENT_STRUCTURE,
                    format!(
                        "a prefab document does not take `{scene_only}` (line {})",
                        entry.key_line
                    ),
                )
                .with_path(format!("/{scene_only}"))
                .with_help(
                    "a prefab carries no seed/physics/preload: the scene that spawns it does; remove the key",
                ),
            ),
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a prefab document",
                PREFAB_KEYS,
                code::PREFAB_DOCUMENT_STRUCTURE,
            )),
        }
    }

    for (key, seen, help) in [
        ("format", format_seen, "add `format: 1` as the first line"),
        ("kind", kind_seen, "add `kind: prefab`"),
        (
            "entities",
            entities_seen,
            "add `entities:` with one named part per `- ` item",
        ),
    ] {
        if !seen {
            diagnostics.push(envelope::missing_key(
                key,
                help,
                code::PREFAB_DOCUMENT_STRUCTURE,
                "prefab document",
            ));
        }
    }

    check_part_names(&parts, &mut diagnostics);
    if parts
        .first()
        .is_some_and(|(_, entity)| entity.instance.is_none())
    {
        let plain: Vec<&SceneEntity> = parts
            .iter()
            .map(|(_, entity)| entity)
            .filter(|entity| entity.instance.is_none())
            .collect();
        if let Some(found) = crate::expand::mixed_root(&plain) {
            diagnostics.push(found.with_path("/entities/0/presentation"));
        }
    }
    doc.entities = parts.into_iter().map(|(_, entity)| entity).collect();

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// `L1227` for a part with no usable name, `L1228` at the later of two parts sharing one. `parts`
/// pairs each parsed entity with its document position.
fn check_part_names(parts: &[(usize, SceneEntity)], diagnostics: &mut Vec<Diagnostic>) {
    let mut first_seen: Vec<(&str, usize)> = Vec::new();
    for (index, entity) in parts {
        let Some(name) = entity.name.as_deref() else {
            diagnostics.push(
                Diagnostic::new(
                    code::PREFAB_PART_NAME,
                    format!("prefab part {index} has no `name`"),
                )
                .with_path(format!("/entities/{index}"))
                .with_help("every part of a prefab is named: add `name: <part>`"),
            );
            continue;
        };
        if name.is_empty() || name.contains('/') {
            diagnostics.push(
                Diagnostic::new(
                    code::PREFAB_PART_NAME,
                    format!("prefab part {index} has the name `{name}`, which is empty or contains `/`"),
                )
                .with_path(format!("/entities/{index}/name"))
                .with_help(
                    "use a non-empty name without `/`: an instance's entities are named `<instance>/<part>`",
                ),
            );
            continue;
        }
        match first_seen.iter().find(|(seen, _)| *seen == name) {
            Some((_, first)) => diagnostics.push(
                Diagnostic::new(
                    code::PREFAB_PART_DUPLICATE,
                    format!("prefab parts {first} and {index} are both named `{name}`"),
                )
                .with_path(format!("/entities/{index}/name"))
                .with_help("rename one of the parts; part names are unique within a prefab"),
            ),
            None => first_seen.push((name, *index)),
        }
    }
}

/// Validates the prefab body against `registry` exactly as a scene's entities are: the same codes,
/// with entity references ranged over the prefab's own parts.
///
/// On top of that, every asset path (and nested `prefab:`) must be a relative path a prefab-local
/// document may write: `L1311` otherwise, since [`PrefabDocument::rebase`] would resolve it against
/// the prefab's directory.
pub(crate) fn validate(doc: &PrefabDocument, registry: &Registry) -> Vec<Diagnostic> {
    let mut diagnostics = crate::validate::validate(&doc.to_scene(), registry);
    for clip in &doc.clips {
        let pointer = format!("/clips/{}", yaml::pointer_token(&clip.name));
        if clip.name.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::PREFAB_DOCUMENT_STRUCTURE,
                    "a clip binding has an empty short name",
                )
                .with_path(pointer.clone())
                .with_help(
                    "name the clip the way game code selects it — `open`, `snap` — or remove the \
                     binding",
                ),
            );
        }
        if clip.file.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::PREFAB_DOCUMENT_STRUCTURE,
                    format!("the clip binding `{}` has an empty file path", clip.name),
                )
                .with_path(pointer)
                .with_help("point the binding at the clip document, relative to this prefab file"),
            );
        } else {
            check_path("clip", &clip.file, pointer, &mut diagnostics);
        }
    }
    for (index, entity) in doc.entities.iter().enumerate() {
        let base = format!("/entities/{index}");
        check_component_paths(
            &entity.components,
            &format!("{base}/components"),
            registry,
            &mut diagnostics,
        );
        let Some(instance) = entity.instance.as_deref() else {
            continue;
        };
        check_path(
            "prefab",
            &instance.prefab,
            format!("{base}/prefab"),
            &mut diagnostics,
        );
        for (part, components) in &instance.overrides {
            let overrides = format!("{base}/overrides/{}", yaml::pointer_token(part));
            check_component_paths(components, &overrides, registry, &mut diagnostics);
        }
    }
    diagnostics
}

fn rebase_components(components: &mut [(String, Value)], key: &str, registry: &Registry) {
    for (name, value) in components {
        if let Some(info) = registry.get(name) {
            info.for_each_asset_path(value, &mut |_, path| {
                if !path.is_empty() {
                    *path = ludion_core::paths::sibling(key, path);
                }
            });
        }
    }
}

fn check_component_paths(
    components: &[(String, Value)],
    base: &str,
    registry: &Registry,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (name, value) in components {
        let Some(info) = registry.get(name) else {
            continue;
        };
        let mut value = value.clone();
        info.for_each_asset_path(&mut value, &mut |pointer, path| {
            if !path.is_empty() {
                let at = format!("{base}/{}{pointer}", yaml::pointer_token(name));
                check_path(name, path, at, diagnostics);
            }
        });
    }
}

fn check_path(what: &str, path: &str, pointer: String, diagnostics: &mut Vec<Diagnostic>) {
    if let Some(refusal) = ludion_core::paths::sibling_refusal(path) {
        diagnostics.push(
            Diagnostic::new(
                code::PREFAB_ASSET_PATH,
                format!("`{what}` path `{path}` {}", refusal.describe()),
            )
            .with_path(pointer)
            .with_help(
                "a path in a prefab is relative to the prefab file's directory; `..` may climb",
            ),
        );
    }
}

/// Writes `doc` in canonical form: `format`, `kind`, `name`, `generated_from`, `clips`, then the
/// entities as a scene writes them.
pub(crate) fn emit(doc: &PrefabDocument, registry: &Registry) -> String {
    let mut out = String::from("format: 1\nkind: prefab\n");
    if let Some(name) = &doc.name {
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }
    if let Some(generated_from) = &doc.generated_from {
        out.push_str("generated_from: ");
        out.push_str(&scalar_string(generated_from));
        out.push('\n');
    }
    if !doc.clips.is_empty() {
        out.push_str("clips:\n");
        let mut clips = doc.clips.clone();
        clips.sort_by(|a, b| a.name.cmp(&b.name));
        for clip in &clips {
            out.push_str("  ");
            out.push_str(&scalar_string(&clip.name));
            out.push_str(": ");
            out.push_str(&scalar_string(&clip.file));
            out.push('\n');
        }
    }
    if doc.entities.is_empty() {
        out.push_str("entities: []\n");
    } else {
        out.push_str("entities:\n");
        for entity in &doc.entities {
            scene_emit::emit_entity(&mut out, entity, registry);
        }
    }
    out
}
