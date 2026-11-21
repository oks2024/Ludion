//! The two data-doc kinds localization owns — `kind: strings` and `kind: locales` — and the
//! validation pass over them (`L1711`, `L1716`, `L1718`, `L1722`, `L1723`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::World;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::strings::placeholders;
use super::{LocalizationEnabled, default_locale};
use crate::docs::DocStore;

/// The `kind:` of a string table.
pub(crate) const STRINGS_KIND: &str = "strings";
/// The `kind:` of the locale list.
pub(crate) const LOCALES_KIND: &str = "locales";

/// One locale's flat key → text map (`kind: strings`). Several files may share a locale; a key
/// appears once per locale (`L1711`).
///
/// ```yaml
/// format: 1
/// kind: strings
/// locale: en
/// strings:
///   radio.intro: "Operator to Neon, do you copy?"
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct StringsDoc {
    /// The locale tag these strings are written in (`en`, `fr`, ...).
    pub locale: String,
    /// Key → text. `{name}` is a placeholder; `{{` and `}}` are literal braces.
    #[serde(default)]
    pub strings: BTreeMap<String, String>,
}

/// The locales a game ships and which one is the default (`kind: locales`, at most one doc). Without
/// the doc the default is `en` and any locale tag is accepted.
///
/// ```yaml
/// format: 1
/// kind: locales
/// default: en
/// locales:
///   en:
///     name: English
///   fr:
///     name: Français
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct LocalesDoc {
    /// The default locale: the fallback for every other, and the one every key must exist in. It
    /// must be listed under `locales`.
    pub default: String,
    /// The shipped locales by tag.
    #[serde(default)]
    pub locales: BTreeMap<String, LocaleInfo>,
}

/// One shipped locale.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct LocaleInfo {
    /// The name a language menu shows, in its own language.
    pub name: String,
}

fn diagnostic(
    code: ludion_core::diagnostics::DiagnosticCode,
    message: String,
    path: String,
) -> Diagnostic {
    Diagnostic::new(code, message)
        .with_path(path)
        .with_help("see `ludion docs localization`")
}

/// locale → the keys it defines, over every loaded `kind: strings` doc (an empty locale is skipped;
/// `L1718` reports it). The tables dialogue validation checks its keys against.
pub(crate) fn key_tables(world: &World) -> BTreeMap<&str, BTreeSet<&str>> {
    let mut tables: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    if let Some(store) = world.resource::<DocStore<StringsDoc>>() {
        for (_, doc) in store.iter() {
            if doc.locale.trim().is_empty() {
                continue;
            }
            tables
                .entry(doc.locale.as_str())
                .or_default()
                .extend(doc.strings.keys().map(String::as_str));
        }
    }
    tables
}

/// The text of `key` in `locale`'s tables, the first writer when several docs share the locale; no
/// default-locale fallback.
pub(crate) fn string_in<'w>(world: &'w World, locale: &str, key: &str) -> Option<&'w str> {
    world
        .resource::<DocStore<StringsDoc>>()?
        .iter()
        .filter(|(_, doc)| doc.locale == locale)
        .find_map(|(_, doc)| doc.strings.get(key).map(String::as_str))
}

/// Reads every `kind: strings` and `kind: locales` doc under the content root `root` into `world`'s
/// two `DocStore`s and marks the world localization-enabled: what the builder's doc walk does for an
/// engine, for a host without one (check-scene, a bare render host). A file that fails its schema is
/// skipped — the game's own boot reports it. Returns whether any strings doc was found.
pub(crate) fn install_content_docs(world: &mut World, root: &Path) -> bool {
    let schema = |schema: schemars::Schema| {
        serde_json::to_value(schema).expect("a schemars JSON Schema always serializes")
    };
    let strings_schema = schema(schemars::schema_for!(StringsDoc));
    let locales_schema = schema(schemars::schema_for!(LocalesDoc));
    let mut strings: BTreeMap<String, StringsDoc> = BTreeMap::new();
    let mut locales: BTreeMap<String, LocalesDoc> = BTreeMap::new();
    for (key, path) in crate::content_root_yaml(root, None) {
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let kind = match ludion_scene::sniff_kind(&source) {
            ludion_scene::KindSniff::Absent => continue,
            ludion_scene::KindSniff::Kind(kind) => kind.to_owned(),
            ludion_scene::KindSniff::Undecided => match ludion_scene::peek_kind_checked(&source) {
                Ok(Some(kind)) => kind,
                _ => continue,
            },
        };
        match kind.as_str() {
            STRINGS_KIND => {
                if let Some(doc) = read_doc(&source, &kind, &strings_schema) {
                    strings.insert(key, doc);
                }
            }
            LOCALES_KIND => {
                if let Some(doc) = read_doc(&source, &kind, &locales_schema) {
                    locales.insert(key, doc);
                }
            }
            _ => {}
        }
    }
    let found = !strings.is_empty();
    world.insert_resource(DocStore::new(strings));
    world.insert_resource(DocStore::new(locales));
    world.insert_resource(LocalizationEnabled);
    found
}

fn read_doc<T: serde::de::DeserializeOwned>(
    source: &str,
    kind: &str,
    schema: &serde_json::Value,
) -> Option<T> {
    let fields = ludion_scene::check_doc(source, kind, schema).ok()?;
    ludion_scene::deserialize_doc::<T>(fields).ok()
}

/// `L1711`, `L1716`, `L1718`, `L1722` and `L1723` over every loaded `kind: strings` and
/// `kind: locales` doc, in store order (a duplicate is reported at the later path).
pub(crate) fn validate_localization_docs(world: &World) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let default = default_locale(world);
    let locales = world.resource::<DocStore<LocalesDoc>>();
    if let Some(store) = locales {
        for (index, (path, doc)) in store.iter().enumerate() {
            if index > 0 {
                out.push(diagnostic(
                    code::LOCALES_INVALID,
                    format!("`{path}` is a second `kind: locales` doc; only one may exist"),
                    format!("locales/{path}"),
                ));
            }
            if doc.default.trim().is_empty() || !doc.locales.contains_key(&doc.default) {
                out.push(diagnostic(
                    code::LOCALES_INVALID,
                    format!(
                        "`{path}` has `default: {}`, which is not one of its listed `locales`",
                        doc.default
                    ),
                    format!("locales/{path}/default"),
                ));
            }
        }
    }
    let listed = locales.and_then(|store| store.iter().next().map(|(_, doc)| doc));
    // locale -> key -> (doc path, text); the first writer is kept, a later one is the duplicate.
    let mut tables: BTreeMap<&str, BTreeMap<&str, (&str, &str)>> = BTreeMap::new();
    if let Some(store) = world.resource::<DocStore<StringsDoc>>() {
        for (path, doc) in store.iter() {
            if doc.locale.trim().is_empty() {
                out.push(diagnostic(
                    code::STRINGS_LOCALE_INVALID,
                    format!("`{path}` has an empty `locale`"),
                    format!("strings/{path}/locale"),
                ));
                continue;
            }
            if let Some(listed) = listed
                && !listed.locales.contains_key(&doc.locale)
            {
                out.push(diagnostic(
                    code::LOCALES_STRINGS_UNLISTED,
                    format!(
                        "`{path}` is written in locale `{}`, which the `kind: locales` doc does not list",
                        doc.locale
                    ),
                    format!("strings/{path}/locale"),
                ));
            }
            let table = tables.entry(doc.locale.as_str()).or_default();
            for (key, text) in &doc.strings {
                if table.contains_key(key.as_str()) {
                    out.push(diagnostic(
                        code::STRINGS_DUPLICATE_KEY,
                        format!(
                            "string key `{key}` is already defined for locale `{}` by an earlier `kind: strings` doc",
                            doc.locale
                        ),
                        format!("strings/{path}/strings/{key}"),
                    ));
                } else {
                    table.insert(key, (path, text));
                }
            }
        }
    }
    // A translation whose placeholders differ from the default locale's drops or invents a value.
    if let Some(base_table) = tables.get(default.as_str()) {
        for (locale, table) in &tables {
            if *locale == default {
                continue;
            }
            for (key, (path, text)) in table {
                let Some((_, base)) = base_table.get(key) else {
                    continue;
                };
                if placeholders(text) != placeholders(base) {
                    out.push(diagnostic(
                        code::STRINGS_PLACEHOLDER_MISMATCH,
                        format!(
                            "string `{key}` in locale `{locale}` uses different `{{placeholders}}` than `{default}`"
                        ),
                        format!("strings/{path}/strings/{key}"),
                    ));
                }
            }
        }
    }
    out
}
