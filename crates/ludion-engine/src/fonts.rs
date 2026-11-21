//! The font library: the content root's `fonts.yaml` (`kind: fonts`) resolved into loaded faces
//! (`Plan/font-assets.md` decisions 5, 8 and 10).
//!
//! [`FontLibrary::face`] maps a component's optional `font` name and the active locale to a
//! [`FontFace`]: `None` is the table's reserved `default` entry, an unknown or failed name falls
//! back to that entry too, and a `locales:` override replaces a name for one locale.
//! Without a loadable `default` the answer is the embedded 8×8 face. The whole table loads when the library is built — extract holds only `&World`,
//! and a table is a handful of small documents — so lookups never touch the disk. Like the other
//! libraries it is never hashed or snapshotted; faults are kept for `report_font_faults`, which
//! raises each once per session.

use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_font::FontFace;
use ludion_scene::FontsDocument;

use crate::components::{Text, UiList, UiRadial, UiText};

/// The table's file name at the content root.
pub(crate) const FONTS_FILE: &str = "fonts.yaml";
/// The reserved table name for text that names no face.
const DEFAULT_NAME: &str = "default";

/// Whether a content root has a font table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TableState {
    Absent,
    Faulted,
    Loaded,
}

/// A face with the root-relative paths of its document and atlas image.
#[derive(Debug)]
struct Loaded {
    face: FontFace,
    doc_path: String,
    image_path: String,
}

#[derive(Debug, Default)]
struct Inner {
    /// `None` when the root has no `fonts.yaml` — not a fault.
    table: Option<FontsDocument>,
    /// One entry per distinct path, sorted by path.
    faces: Vec<(String, Result<Loaded, Diagnostic>)>,
    /// A fault in the table itself.
    table_fault: Option<Diagnostic>,
}

/// The loaded font table of a content root. Cloning is cheap, so a game system can clone it out of
/// the world before a query.
#[derive(Clone, Debug, Default)]
pub struct FontLibrary {
    inner: Arc<Inner>,
}

impl FontLibrary {
    /// Loads `fonts.yaml` and every face it lists under `root`. Never fails: a missing table is the
    /// empty library, and a bad table or face is kept as a fault with the embedded fallback.
    pub fn load(root: ludion_assets::AssetRoot<'_>) -> FontLibrary {
        let mut inner = Inner::default();
        let table_path = root.dir().join(FONTS_FILE);
        if !table_path.is_file() {
            return FontLibrary {
                inner: Arc::new(inner),
            };
        }
        let source = match std::fs::read_to_string(&table_path) {
            Ok(source) => source,
            Err(error) => {
                inner.table_fault = Some(fault(
                    FONTS_FILE,
                    &format!("cannot read: {error}"),
                    Vec::new(),
                ));
                return FontLibrary {
                    inner: Arc::new(inner),
                };
            }
        };
        match ludion_scene::check_fonts(&source) {
            Ok(table) => {
                let mut paths: Vec<&str> = table
                    .fonts
                    .iter()
                    .chain(table.locales.iter().flat_map(|(_, faces)| faces))
                    .map(|(_, p)| p.as_str())
                    .collect();
                paths.sort_unstable();
                paths.dedup();
                inner.faces = paths
                    .into_iter()
                    .map(|path| (path.to_string(), load_face(root, path)))
                    .collect();
                inner.table = Some(table);
            }
            Err(diagnostics) => {
                let mut first = diagnostics
                    .into_iter()
                    .next()
                    .expect("a failed check reports");
                first.message = format!("{FONTS_FILE}: {}", first.message);
                inner.table_fault = Some(first);
            }
        }
        FontLibrary {
            inner: Arc::new(inner),
        }
    }

    /// The face for `name` under `locale`; `None` is the table's `default` entry. Resolution order:
    /// the locale's override of `name`, the base `name`, the locale's override of `default`, the
    /// base `default`, then the embedded face. A face that failed to load counts as absent at each
    /// stage.
    ///
    /// ```
    /// use ludion_engine::FontLibrary;
    ///
    /// let library = FontLibrary::default();
    /// // With no table every name and locale resolves to the embedded face.
    /// let face = library.face(Some("body"), Some("fr"));
    /// assert!(std::ptr::eq(face, library.face(None, None)));
    /// ```
    pub fn face(&self, name: Option<&str>, locale: Option<&str>) -> &FontFace {
        self.table_face(name, locale)
            .unwrap_or_else(|| FontFace::embedded())
    }

    /// The loaded table face `name` resolves to under `locale`, `None` where
    /// [`face`](FontLibrary::face) would fall back to the embedded one.
    pub(crate) fn table_face(&self, name: Option<&str>, locale: Option<&str>) -> Option<&FontFace> {
        self.resolved(name, locale).map(|loaded| &loaded.face)
    }

    /// The root-relative paths of the document and atlas image `name` resolves to under `locale`.
    pub(crate) fn paths(&self, name: Option<&str>, locale: Option<&str>) -> Option<(&str, &str)> {
        self.resolved(name, locale)
            .map(|loaded| (loaded.doc_path.as_str(), loaded.image_path.as_str()))
    }

    /// Whether any locale overrides a face, so a lookup needs the active locale at all.
    pub fn has_locale_overrides(&self) -> bool {
        self.inner
            .table
            .as_ref()
            .is_some_and(|table| table.locales.iter().any(|(_, faces)| !faces.is_empty()))
    }

    /// Whether the content root has no table, a malformed one, or a usable one.
    pub(crate) fn table_state(&self) -> TableState {
        if self.inner.table_fault.is_some() {
            TableState::Faulted
        } else if self.inner.table.is_some() {
            TableState::Loaded
        } else {
            TableState::Absent
        }
    }

    /// What `name` resolves to under `locale`: the locale's override, the base entry, then the same
    /// two for `default`, else `None` (the embedded face).
    fn resolved(&self, name: Option<&str>, locale: Option<&str>) -> Option<&Loaded> {
        let over = |n: Option<&str>| locale.and_then(|l| self.overridden(l, n));
        over(name)
            .or_else(|| self.loaded(name))
            .or_else(|| over(None))
            .or_else(|| self.loaded(None))
    }

    /// Whether `key` (a normalized content-root-relative path) is the table or one of its faces,
    /// loaded or not.
    pub(crate) fn holds(&self, key: &str) -> bool {
        key == FONTS_FILE
            || self
                .inner
                .faces
                .binary_search_by(|(p, _)| p.as_str().cmp(key))
                .is_ok()
    }

    fn loaded(&self, name: Option<&str>) -> Option<&Loaded> {
        let table = self.inner.table.as_ref()?;
        let name = name.unwrap_or(DEFAULT_NAME);
        let i = table
            .fonts
            .binary_search_by(|(n, _)| n.as_str().cmp(name))
            .ok()?;
        self.at_path(table.fonts[i].1.as_str())
    }

    fn overridden(&self, locale: &str, name: Option<&str>) -> Option<&Loaded> {
        let table = self.inner.table.as_ref()?;
        let name = name.unwrap_or(DEFAULT_NAME);
        let (_, faces) = table.locales.iter().find(|(tag, _)| tag == locale)?;
        let i = faces.binary_search_by(|(n, _)| n.as_str().cmp(name)).ok()?;
        self.at_path(faces[i].1.as_str())
    }

    fn at_path(&self, path: &str) -> Option<&Loaded> {
        let j = self
            .inner
            .faces
            .binary_search_by(|(p, _)| p.as_str().cmp(path))
            .ok()?;
        self.inner.faces[j].1.as_ref().ok()
    }

    /// Whether the table defines `name`, whether or not its face loaded.
    pub fn contains(&self, name: &str) -> bool {
        self.inner.table.as_ref().is_some_and(|table| {
            table
                .fonts
                .binary_search_by(|(n, _)| n.as_str().cmp(name))
                .is_ok()
        })
    }

    /// The table's names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.inner
            .table
            .iter()
            .flat_map(|table| table.fonts.iter().map(|(name, _)| name.as_str()))
    }

    /// Every load fault, keyed by the path that failed.
    pub(crate) fn faults(&self) -> impl Iterator<Item = (&str, &Diagnostic)> {
        let table = self.inner.table_fault.iter().map(|d| (FONTS_FILE, d));
        let faces = self
            .inner
            .faces
            .iter()
            .filter_map(|(path, loaded)| loaded.as_ref().err().map(|d| (path.as_str(), d)));
        table.chain(faces)
    }
}

/// An `L1255` for `file`. A document that fails its checks keeps the first diagnostic's message
/// and JSON Pointer, and the rest as `related`; an unreadable file has no pointer.
fn fault(file: &str, why: &str, checked: Vec<Diagnostic>) -> Diagnostic {
    let mut checked = checked.into_iter();
    let first = checked.next();
    let why = first.as_ref().map_or(why, |d| d.message.as_str());
    let mut diagnostic = Diagnostic::new(
        code::FONT_LOAD,
        format!(
            "the font `{file}` cannot load: {why}; text using it is drawn with the default face"
        ),
    )
    .with_help(
        "check the path in `fonts.yaml`, or regenerate the face with `ludion generate-atlas`",
    );
    if let Some(pointer) = first.and_then(|d| d.path) {
        diagnostic = diagnostic.with_path(pointer);
    }
    for rest in checked {
        diagnostic = diagnostic.with_related(rest);
    }
    diagnostic
}

fn load_face(root: ludion_assets::AssetRoot<'_>, path: &str) -> Result<Loaded, Diagnostic> {
    let source = ludion_assets::load_bytes(root, path)
        .map_err(|d| fault(path, &d.message, Vec::new()))
        .and_then(|asset| {
            String::from_utf8(asset.bytes)
                .map_err(|error| fault(path, &format!("cannot read: {error}"), Vec::new()))
        })?;
    let doc = ludion_scene::check_font(&source).map_err(|d| fault(path, "", d))?;
    let image_path = ludion_core::paths::sibling(path, &doc.image);
    let face = FontFace::new(doc).map_err(|d| fault(path, "", d))?;
    Ok(Loaded {
        face,
        doc_path: path.to_string(),
        image_path,
    })
}

/// Reports each library fault (`L1255`, or `L1253` for a malformed table) and each `font` name a component uses that the table lacks
/// (`L1254`), once per path or name through the diagnostics latch.
pub(crate) fn report_font_faults(world: &mut ludion_ecs::World) {
    let Some(library) = world.resource::<FontLibrary>().cloned() else {
        return;
    };
    for (path, diagnostic) in library.faults() {
        if crate::diagnostics::is_latched(world, diagnostic.code, path) {
            continue;
        }
        crate::diagnostics::report_full(world, path, diagnostic.clone());
    }
    let unknown = |name: &&String| !library.contains(name);
    let mut names: Vec<String> = Vec::new();
    names.extend(
        world
            .query_ids::<(&Text,)>()
            .filter_map(|(_, (c,))| c.font.as_ref().filter(unknown).cloned()),
    );
    names.extend(
        world
            .query_ids::<(&UiText,)>()
            .filter_map(|(_, (c,))| c.font.as_ref().filter(unknown).cloned()),
    );
    names.extend(
        world
            .query_ids::<(&UiList,)>()
            .filter_map(|(_, (c,))| c.font.as_ref().filter(unknown).cloned()),
    );
    names.extend(
        world
            .query_ids::<(&UiRadial,)>()
            .filter_map(|(_, (c,))| c.font.as_ref().filter(unknown).cloned()),
    );
    for name in names {
        if crate::diagnostics::is_latched(world, code::FONT_NAME_UNKNOWN, &name) {
            continue;
        }
        crate::diagnostics::report(
            world,
            code::FONT_NAME_UNKNOWN,
            name.clone(),
            format!(
                "a component names the font `{name}`, which `fonts.yaml` does not define; its \
                 text is drawn and measured with the default face"
            ),
        );
    }
}

#[cfg(test)]
#[path = "fonts_tests.rs"]
mod tests;
