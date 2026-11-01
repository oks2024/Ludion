//! Anti-drift guard: `ludion import --help`, the `L1202` explain text and the `ludion-import` crate
//! rustdoc all name the importer's one `UNSUPPORTED_FEATURES` list.

use std::process::Command;

/// Strips markdown/rustdoc decoration and collapses whitespace so wrapped lines compare equal.
fn normalize(text: &str) -> String {
    let cleaned: String = text
        .lines()
        .map(|l| {
            l.trim_start()
                .trim_start_matches("//!")
                .trim_start_matches("///")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned: String = cleaned
        .chars()
        .filter(|c| !matches!(c, '`' | '*'))
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ludion(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary");
    assert!(output.status.success(), "ludion {args:?} failed");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn help_explain_and_rustdoc_name_the_same_unsupported_set() {
    let rustdoc = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ludion-import/src/lib.rs"
    ))
    .expect("read ludion-import/src/lib.rs");
    let texts = [
        (
            "`ludion import --help`",
            normalize(&ludion(&["import", "--help"])),
        ),
        (
            "`ludion explain L1202`",
            normalize(&ludion(&["explain", "L1202"])),
        ),
        ("the ludion-import crate rustdoc", normalize(&rustdoc)),
    ];
    for phrase in ludion_import::UNSUPPORTED_FEATURES {
        for (name, text) in &texts {
            assert!(
                text.contains(phrase),
                "{name} does not name the unsupported glTF feature \"{phrase}\"; \
                 update it to match `ludion_import::UNSUPPORTED_FEATURES`"
            );
        }
    }
    // Old phrasings of the set: a text that still says one has gone stale.
    for stale in [
        "a non-point light",
        "a texture,",
        "non-OPAQUE",
        "a doubleSided",
    ] {
        for (name, text) in &texts {
            assert!(
                !text.contains(stale),
                "{name} still carries the stale unsupported-feature phrasing \"{stale}\"; \
                 update it to match `ludion_import::UNSUPPORTED_FEATURES`"
            );
        }
    }
}
