//! The player's persisted input rebinds: the lenient `save/input-bindings.yaml` and its canonical
//! text (`Plan/input-actions.md`, `ludion docs input`).
//!
//! The file is the shared per-machine save shape: a `format: 1` / `kind: input-bindings` envelope
//! over an [`InputBindingsDoc`]. A file that cannot be honored (bad envelope, bad YAML, a value of the
//! wrong shape, a newer `format`) is backed up and replaced by the default bindings with one `L4277`;
//! a single bad action never resets the file — [`Actions::apply_overrides`] drops it with `L4273` /
//! `L4274` and applies the rest. The same text rides a recording's header (`save_docs`), which is
//! what a replay installs instead of reading the machine's file.

use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::World;
use ludion_sim::actions::{Actions, InputBindingsDoc};
use serde_json::Value;

/// The document `format:` this build reads and writes; a newer file is backed up and defaulted, so
/// an older build never silently downgrades it.
const INPUT_BINDINGS_FORMAT: u64 = 1;

/// The `kind:` of the file, and its key in a replay header's `save_docs`.
pub(crate) const INPUT_BINDINGS_KIND: &str = "input-bindings";

/// Where the file lives, relative to the save root.
pub(crate) const INPUT_BINDINGS_PATH: &str = "save/input-bindings.yaml";

/// Reads `path` leniently. A missing file is no overrides and no warning; a file that cannot be
/// honored is renamed aside (`input-bindings.yaml.bak.<hash>`) and the defaults start with one
/// `L4277`.
pub(crate) fn load(path: &Path) -> (InputBindingsDoc, Vec<Diagnostic>) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (InputBindingsDoc::default(), Vec::new());
        }
        Err(error) => {
            let reason = format!("it could not be read: {error}");
            return (
                InputBindingsDoc::default(),
                vec![reset(path, &reason, false)],
            );
        }
    };
    match parse(&text) {
        Ok(doc) => (doc, Vec::new()),
        Err(reason) => (
            InputBindingsDoc::default(),
            vec![reset(path, &reason, true)],
        ),
    }
}

/// Parses embedded text (a replay header's `save_docs` entry) the way [`load`] parses the file, but
/// never touches disk: unparseable text is the defaults with one `L4277`.
pub(crate) fn parse_text(text: &str) -> (InputBindingsDoc, Vec<Diagnostic>) {
    match parse(text) {
        Ok(doc) => (doc, Vec::new()),
        Err(reason) => (
            InputBindingsDoc::default(),
            vec![reset_diagnostic(
                &reason,
                "the recorded bindings were not installed",
            )],
        ),
    }
}

/// The canonical `kind: input-bindings` text.
pub(crate) fn emit(doc: &InputBindingsDoc) -> String {
    let schema = serde_json::to_value(schemars::schema_for!(InputBindingsDoc))
        .expect("the input bindings schema is JSON");
    let value = serde_json::to_value(doc).expect("input bindings serialize");
    ludion_scene::emit_doc(INPUT_BINDINGS_KIND, &value, &schema)
}

/// The text to write when a system asked [`Actions::save`] since the last drain, clearing the
/// request; `None` when there is no request or no [`Actions`].
pub(crate) fn drain_save(world: &mut World) -> Option<String> {
    let actions = world.resource_mut::<Actions>()?;
    actions
        .take_save_request()
        .then(|| emit(actions.overrides()))
}

fn parse(text: &str) -> Result<InputBindingsDoc, String> {
    let envelope = ludion_scene::open_input_bindings(text).map_err(|errors| {
        errors
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    if envelope.format > INPUT_BINDINGS_FORMAT {
        return Err(format!(
            "its `format` is {}, newer than the {INPUT_BINDINGS_FORMAT} this engine reads",
            envelope.format
        ));
    }
    let Value::Object(fields) = envelope.fields else {
        return Err("it is not a mapping".to_string());
    };
    serde_json::from_value(Value::Object(fields))
        .map_err(|error| format!("a value has the wrong shape: {error}"))
}

/// Backs the file up (when asked) and builds the one `L4277`.
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
        code::INPUT_BINDINGS_RESET,
        format!(
            "`input-bindings.yaml` was not loaded: {reason}; the default bindings apply and {kept}"
        ),
    )
    .with_path("input-bindings.yaml")
    .with_help(
        "nothing is required: the next `Actions::save()` writes a fresh file; to recover old \
         bindings, reconcile the backup by hand",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludion-input-bindings-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("input-bindings.yaml")
    }

    fn doc(json: serde_json::Value) -> InputBindingsDoc {
        serde_json::from_value(json).unwrap()
    }

    fn sample() -> InputBindingsDoc {
        doc(serde_json::json!({ "actions": {
            "scan": [{ "key": "F" }, { "pad": "right_shoulder" }],
        } }))
    }

    #[test]
    fn a_missing_file_is_no_overrides_without_a_warning() {
        let (loaded, diagnostics) = load(&scratch("missing"));
        assert_eq!(loaded, InputBindingsDoc::default());
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn emit_and_load_round_trip() {
        let text = emit(&sample());
        assert!(
            text.starts_with("format: 1\nkind: input-bindings\n"),
            "{text}"
        );
        let path = scratch("round-trip");
        std::fs::write(&path, &text).unwrap();
        let (back, diagnostics) = load(&path);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(back, sample());
        assert_eq!(emit(&back), text, "emit is idempotent");
        assert_eq!(parse_text(&text).0, sample());
    }

    #[test]
    fn an_unloadable_file_is_backed_up_and_defaulted_once() {
        for (name, text) in [
            ("garbage", "format: 1\nkind: input-bindings\n  actions: [\n"),
            ("no-kind", "format: 1\nactions: {}\n"),
            ("wrong-kind", "format: 1\nkind: settings\n"),
            ("newer", "format: 9\nkind: input-bindings\n"),
            (
                "shape",
                "format: 1\nkind: input-bindings\nactions:\n  scan: 3\n",
            ),
            (
                "unknown-key",
                "format: 1\nkind: input-bindings\nextra: 1\nactions: {}\n",
            ),
        ] {
            let path = scratch(name);
            std::fs::write(&path, text).unwrap();
            let (loaded, diagnostics) = load(&path);
            assert_eq!(loaded, InputBindingsDoc::default(), "{name}");
            let codes: Vec<_> = diagnostics.iter().map(|d| d.code.as_str()).collect();
            assert_eq!(codes, ["L4277"], "{name}");
            assert!(!path.exists(), "{name}: the bad file moved aside");
            let backups = std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("input-bindings.yaml.bak.")
                })
                .count();
            assert_eq!(backups, 1, "{name}");
        }
    }

    #[test]
    fn unparseable_embedded_text_defaults_without_touching_disk() {
        let (loaded, diagnostics) = parse_text("not a document");
        assert_eq!(loaded, InputBindingsDoc::default());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code.as_str(), "L4277");
    }
}
