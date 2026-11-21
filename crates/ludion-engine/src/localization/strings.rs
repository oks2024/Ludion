//! String lookup: the derived [`StringIndex`], the locale fallback, and `{name}` placeholders.

use std::collections::{BTreeMap, BTreeSet};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::World;

use super::docs::StringsDoc;
use super::{default_locale, locale};
use crate::diagnostics::report_full;
use crate::docs::DocStore;

/// locale → key → text over every loaded `kind: strings` doc, rebuilt when the store's generation
/// moves. A resource: derived, never hashed or snapshotted.
#[derive(Debug, Default)]
pub(crate) struct StringIndex {
    /// `None` until built; then the store generation it was built from (`None` when no store).
    generation: Option<Option<u64>>,
    by_locale: BTreeMap<String, BTreeMap<String, String>>,
}

fn refresh(world: &mut World) {
    let generation = world
        .resource::<DocStore<StringsDoc>>()
        .map(DocStore::generation);
    let current = world
        .resource::<StringIndex>()
        .and_then(|index| index.generation);
    if current == Some(generation) {
        return;
    }
    let mut by_locale: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    if let Some(store) = world.resource::<DocStore<StringsDoc>>() {
        for (_, doc) in store.iter() {
            let table = by_locale.entry(doc.locale.clone()).or_default();
            for (key, text) in &doc.strings {
                // The first writer wins; a duplicate is `L1711` at validation.
                table.entry(key.clone()).or_insert_with(|| text.clone());
            }
        }
    }
    world.insert_resource(StringIndex {
        generation: Some(generation),
        by_locale,
    });
}

/// `key` in `locale`: that locale's text, else the default locale's (`L5132`, once per
/// locale/key), else the key itself (`L5133`, once per key).
pub(crate) fn resolve(world: &mut World, locale: &str, key: &str) -> String {
    refresh(world);
    let default = default_locale(world);
    let Some(index) = world.resource::<StringIndex>() else {
        return key.to_owned();
    };
    let lookup = |wanted: &str| {
        index
            .by_locale
            .get(wanted)
            .and_then(|table| table.get(key))
            .cloned()
    };
    if let Some(text) = lookup(locale) {
        return text;
    }
    if locale != default
        && let Some(text) = lookup(&default)
    {
        report_full(
            world,
            format!("string_fell_back:{locale}/{key}"),
            Diagnostic::new(
                code::STRING_FELL_BACK,
                format!("string `{key}` has no `{locale}` text; showing the `{default}` one"),
            )
            .with_help("add the key to a `kind: strings` doc for that locale"),
        );
        return text;
    }
    report_full(
        world,
        format!("string_missing:{key}"),
        Diagnostic::new(
            code::STRING_MISSING,
            format!("string `{key}` is in no locale; showing the key itself"),
        )
        .with_help("add the key to a `kind: strings` doc; see `ludion docs localization`"),
    );
    key.to_owned()
}

/// One piece of a parsed template.
enum Piece<'a> {
    Text(String),
    Placeholder(&'a str),
}

/// Splits `text` into literal runs and `{name}` placeholders. `{{` and `}}` are literal braces; a
/// `{` with no closing `}` before the next `{`, and an empty `{}`, stay literal.
fn parse(text: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut literal = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(['{', '}']) {
        literal.push_str(&rest[..at]);
        let tail = &rest[at..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            literal.push_str(&tail[..1]);
            rest = &tail[2..];
        } else if let Some(inner) = tail.strip_prefix('{') {
            match inner.find(['{', '}']) {
                Some(end) if inner.as_bytes()[end] == b'}' && end > 0 => {
                    if !literal.is_empty() {
                        pieces.push(Piece::Text(std::mem::take(&mut literal)));
                    }
                    pieces.push(Piece::Placeholder(&inner[..end]));
                    rest = &inner[end + 1..];
                }
                _ => {
                    literal.push('{');
                    rest = inner;
                }
            }
        } else {
            literal.push('}');
            rest = &tail[1..];
        }
    }
    literal.push_str(rest);
    if !literal.is_empty() {
        pieces.push(Piece::Text(literal));
    }
    pieces
}

/// The distinct `{name}` placeholders of `text`.
pub(crate) fn placeholders(text: &str) -> BTreeSet<&str> {
    parse(text)
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Placeholder(name) => Some(name),
            Piece::Text(_) => None,
        })
        .collect()
}

/// The template with every `{name}` removed and `{{`/`}}` as one brace: the characters a face must
/// carry whatever a placeholder fills in.
pub(crate) fn literals(text: &str) -> String {
    parse(text)
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Text(literal) => Some(literal),
            Piece::Placeholder(_) => None,
        })
        .collect()
}

/// `text` with each `{name}` replaced from `args`, and the names no arg supplied (left as
/// `{name}` in the output).
pub(crate) fn format(text: &str, args: &[(&str, &str)]) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut missing = Vec::new();
    for piece in parse(text) {
        match piece {
            Piece::Text(literal) => out.push_str(&literal),
            Piece::Placeholder(name) => match args.iter().find(|(arg, _)| *arg == name) {
                Some((_, value)) => out.push_str(value),
                None => {
                    out.push('{');
                    out.push_str(name);
                    out.push('}');
                    if !missing.iter().any(|seen| seen == name) {
                        missing.push(name.to_owned());
                    }
                }
            },
        }
    }
    (out, missing)
}

/// Resolves `key` in the active locale and fills `args`, warning `L5134` once per missing name.
pub(crate) fn render(world: &mut World, key: &str, args: &[(&str, &str)]) -> String {
    let active = locale(world);
    let text = resolve(world, &active, key);
    let (out, missing) = format(&text, args);
    for name in missing {
        report_full(
            world,
            format!("string_arg:{key}/{name}"),
            Diagnostic::new(
                code::STRING_ARG_MISSING,
                format!("string `{key}` has the placeholder `{{{name}}}`, but no argument `{name}` was passed"),
            )
            .with_help("pass it in `say_with`'s args or `format_text`'s"),
        );
    }
    out
}

/// The text for `key` in the active locale, with the locale fallback and its warnings.
///
/// # Example
/// ```
/// use ludion_engine::localization;
/// use ludion_ecs::World;
///
/// // Without a table the key itself comes back (and `L5133` is reported when diagnostics exist).
/// let mut world = World::new();
/// assert_eq!(localization::text(&mut world, "ui.start"), "ui.start");
/// ```
pub fn text(world: &mut World, key: &str) -> String {
    render(world, key, &[])
}

/// [`text`] with `{name}` placeholders filled from `args`; a missing one stays `{name}` and reports
/// `L5134` once.
///
/// # Example
/// ```
/// use ludion_engine::localization;
/// use ludion_ecs::World;
///
/// let mut world = World::new();
/// assert_eq!(localization::format_text(&mut world, "Hello {{{name}}}", &[("name", "Neon")]), "Hello {Neon}");
/// ```
pub fn format_text(world: &mut World, key: &str, args: &[(&str, &str)]) -> String {
    render(world, key, args)
}
