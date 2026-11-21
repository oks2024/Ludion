//! The two data-doc kinds localization owns — `kind: strings` and `kind: locales` — and the
//! validation pass over them (`L1711`, `L1716`, `L1718`, `L1722`, `L1723`).

use std::collections::{BTreeMap, BTreeSet};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::World;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::default_locale;
use super::strings::placeholders;
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
