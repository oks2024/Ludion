//! Implementation of `ludion new <name>`.
//!
//! Scaffolds a runnable game project at `./<name>`, modeled on `games/hello-paddle`: a starter
//! component + system, a scene mixing built-in visuals with it, the sim-side determinism `clippy.toml`,
//! sim-assertion / WorldHash-determinism / golden / drift-safety tests that pass out of the box, the
//! project manifest, a short `CLAUDE.md`, and `LUDION-GUIDE.md` (the shipped engine guide).
//!
//! Templates are embedded from `crates/ludion-cli/templates/` via `include_str!`, with a dumb
//! `{{name}}` / `{{name_ident}}` / `{{…_dep}}` substitution (no template engine). The guide has a
//! **single source of truth** — `docs/engine-guide.md` — embedded directly here (frontmatter
//! stripped) so `ludion docs` and every generated project stay in sync with zero duplication.
//!
//! The generated `Cargo.toml`'s engine dependencies come in two flavors, chosen at runtime:
//! - **Checkout flow:** the running CLI's baked engine root (derived at build time from
//!   `CARGO_MANIFEST_DIR`) exists and contains `crates/ludion`, so the deps point at that checkout by
//!   absolute path — move the project anywhere and it still builds against the engine here. Because
//!   the repo workspace's `members = ["crates/*", "games/*"]` would claim any project scaffolded under
//!   those globs — making it a member of two workspaces — `new` refuses to scaffold there (`L0011`).
//! - **Installed flow:** the binary was `cargo install`ed, so the baked engine root is absent; the
//!   deps pin the published version matching this binary (`CARGO_PKG_VERSION`) instead. Set
//!   `LUDION_NEW_INSTALLED=1` to force this flow (used by the installed-mode scaffolding test).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ludion_core::codes::code::{self, NEW_TARGET_IN_WORKSPACE};
use ludion_core::diagnostics::Diagnostic;

use ludion_engine::manifest::is_plain_name_char;

/// One generated file: its path relative to the project root and its template body.
struct TemplateFile {
    /// Path relative to the project root (forward slashes; split into components when writing).
    rel_path: &'static str,
    /// The template body, with `{{…}}` placeholders substituted before writing.
    body: &'static str,
}

/// The `Cargo.toml` template. Stored with a `.tpl` suffix: a literal `templates/Cargo.toml` would
/// make cargo treat `templates/` as a nested package root and drop the whole tree from the
/// published tarball.
const CARGO_TEMPLATE: &str = include_str!("../templates/Cargo.toml.tpl");

/// The embedded templates, one per generated file (the guide is handled separately).
const TEMPLATES: &[TemplateFile] = &[
    TemplateFile {
        rel_path: "Cargo.toml",
        body: CARGO_TEMPLATE,
    },
    TemplateFile {
        rel_path: "clippy.toml",
        body: include_str!("../templates/clippy.toml"),
    },
    TemplateFile {
        rel_path: "ludion.toml",
        body: include_str!("../templates/ludion.toml"),
    },
    TemplateFile {
        rel_path: "CLAUDE.md",
        body: include_str!("../templates/CLAUDE.md"),
    },
    TemplateFile {
        rel_path: "src/main.rs",
        body: include_str!("../templates/src/main.rs"),
    },
    TemplateFile {
        rel_path: "src/lib.rs",
        body: include_str!("../templates/src/lib.rs"),
    },
    TemplateFile {
        rel_path: "src/components.rs",
        body: include_str!("../templates/src/components.rs"),
    },
    TemplateFile {
        rel_path: "src/systems.rs",
        body: include_str!("../templates/src/systems.rs"),
    },
    TemplateFile {
        rel_path: "scenes/main.yaml",
        body: include_str!("../templates/scenes/main.yaml"),
    },
    TemplateFile {
        rel_path: "scenes/model-stage.yaml",
        body: include_str!("../templates/scenes/model-stage.yaml"),
    },
    TemplateFile {
        rel_path: "scenes/model-stage-night.yaml",
        body: include_str!("../templates/scenes/model-stage-night.yaml"),
    },
    TemplateFile {
        rel_path: "scenes/effects-stage.yaml",
        body: include_str!("../templates/scenes/effects-stage.yaml"),
    },
    TemplateFile {
        rel_path: "tests/gameplay.rs",
        body: include_str!("../templates/tests/gameplay.rs"),
    },
    TemplateFile {
        rel_path: "tests/golden.rs",
        body: include_str!("../templates/tests/golden.rs"),
    },
    TemplateFile {
        rel_path: "tests/drift.rs",
        body: include_str!("../templates/tests/drift.rs"),
    },
];

/// The canonical engine guide — the same doc `ludion docs engine-guide` resolves — embedded so every
/// generated project carries an identical `LUDION-GUIDE.md`. Its docs frontmatter is stripped on
/// write. Included from the vendored copy — committed, mirrored from `docs/engine-guide.md` by
/// `node scripts/check.mjs --fix` and verified by `build.rs` — so the crates.io package, which has
/// no repo-root `docs/`, is self-contained.
const ENGINE_GUIDE: &str = include_str!("../embedded-docs/engine-guide.md");

/// Runs `ludion new <name>`: scaffolds a standalone game project at `./<name>`.
///
/// Prints the created file tree on stdout and a next-steps block. Returns a non-zero exit on a bad
/// name, a target inside the engine workspace's member globs (`L0011`), an existing target, or a
/// filesystem error.
pub fn run(name: &str) -> ExitCode {
    if let Err(diagnostic) = validate_name(name) {
        eprintln!("{diagnostic}");
        return ExitCode::FAILURE;
    }

    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            eprintln!(
                "{}",
                Diagnostic::new(
                    code::SCENE_IO,
                    format!("could not read the current directory: {err}")
                )
                .with_help("run `ludion new` from a directory that still exists and is readable")
            );
            return ExitCode::FAILURE;
        }
    };
    let project_dir = cwd.join(name);

    // Resolve to an absolute path (lexically — the target need not exist yet) for the workspace guard.
    let target_abs = std::path::absolute(&project_dir).unwrap_or_else(|_| project_dir.clone());
    let engine_root = engine_workspace_root();
    if let Some(member_glob) = member_glob_containing(&target_abs, &engine_root) {
        let diagnostic = Diagnostic::new(
            NEW_TARGET_IN_WORKSPACE,
            format!(
                "refusing to scaffold `{}` inside the engine workspace's `{member_glob}/*` member \
                 glob — the project would belong to two Cargo workspaces at once",
                target_abs.display()
            ),
        )
        .with_help(
            "create the project outside the engine checkout (a sibling directory, or anywhere not \
             under the engine's `crates/` or `games/`); its Cargo.toml points at this engine by \
             absolute path, so it still builds from anywhere. See `ludion explain L0011`",
        );
        eprintln!("{diagnostic}");
        return ExitCode::FAILURE;
    }

    if project_dir.exists() {
        let diagnostic = Diagnostic::new(
            code::CREATE_TARGET_EXISTS,
            format!("`{}` already exists", project_dir.display()),
        )
        .with_help(
            "choose a name whose directory does not exist yet; `ludion new` never writes into an \
             existing directory",
        );
        eprintln!("{diagnostic}");
        return ExitCode::FAILURE;
    }

    let name_ident = name.replace('-', "_");
    // A bundle identifier allows no `_`.
    let name_bundle = name.replace('_', "-");
    let deps = engine_dependencies(&engine_root);

    let substitute = |body: &str| -> String {
        body.replace("{{name_ident}}", &name_ident)
            .replace("{{name_bundle}}", &name_bundle)
            .replace("{{ludion_dep}}", &deps.ludion)
            .replace("{{ludion_testkit_dep}}", &deps.ludion_testkit)
            .replace("{{ludion_engine_dep}}", &deps.ludion_engine)
            .replace("{{ludion_core_dep}}", &deps.ludion_core)
            .replace("{{name}}", name)
    };

    // Write every templated file, then the guide.
    let mut written: Vec<&str> = Vec::new();
    for template in TEMPLATES {
        if let Err(err) = write_file(&project_dir, template.rel_path, &substitute(template.body)) {
            eprintln!("{}", output_failed(err));
            return ExitCode::FAILURE;
        }
        written.push(template.rel_path);
    }
    // An empty `goldens/` so the first bless (`LUDION_BLESS=1 LUDION_BLESS_ANY_ADAPTER=1 cargo
    // test`, through `tests/golden.rs`) has a home to write into.
    if let Err(err) = std::fs::create_dir_all(project_dir.join("goldens")) {
        eprintln!(
            "{}",
            output_failed(format!("could not create `goldens/`: {err}"))
        );
        return ExitCode::FAILURE;
    }
    if let Err(err) = write_file(
        &project_dir,
        "LUDION-GUIDE.md",
        &strip_frontmatter(ENGINE_GUIDE),
    ) {
        eprintln!("{}", output_failed(err));
        return ExitCode::FAILURE;
    }
    written.push("LUDION-GUIDE.md");

    print_summary(name, &project_dir, &written);
    ExitCode::SUCCESS
}

/// The `L1038` diagnostic for a scaffold file or directory that could not be written.
fn output_failed(message: String) -> Diagnostic {
    Diagnostic::new(code::OUTPUT_IO, message)
        .with_help("check that the current directory is writable and has free space")
}

/// Rust keywords (strict, reserved and 2024-edition `gen`) and path words: a name equal to one,
/// once `-` becomes `_`, cannot be the library identifier `src/main.rs` calls.
const RESERVED_IDENTS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "gen",
];

/// Names Cargo refuses for a package or that collide with a `target/` directory or a sysroot crate.
const RESERVED_PACKAGES: &[&str] = &[
    "test",
    "std",
    "core",
    "alloc",
    "proc_macro",
    "deps",
    "examples",
    "build",
    "incremental",
];

/// The package names the template's `[dependencies]` and `[dev-dependencies]` declare. The template
/// holds `{{...}}` placeholders, so it is read by line, not parsed as TOML.
fn template_dependencies() -> impl Iterator<Item = &'static str> {
    let mut in_dependencies = false;
    CARGO_TEMPLATE.lines().filter_map(move |line| {
        let line = line.trim();
        if line.starts_with('[') {
            in_dependencies = matches!(line, "[dependencies]" | "[dev-dependencies]");
            return None;
        }
        if !in_dependencies || line.is_empty() || line.starts_with('#') {
            return None;
        }
        line.split_once('=').map(|(key, _)| key.trim())
    })
}

/// Whether `name`, as a library identifier, is a keyword, a name Cargo refuses or a dependency the
/// template itself uses (the package would depend on itself); a Windows device name is also
/// reserved (the directory could not be created).
fn is_reserved(name: &str) -> bool {
    let ident = name.replace('-', "_");
    RESERVED_IDENTS.contains(&ident.as_str())
        || RESERVED_PACKAGES.contains(&ident.as_str())
        || template_dependencies().any(|dependency| dependency.replace('-', "_") == ident)
        || ludion_core::paths::is_reserved_device(name)
}

/// Validates the project name, which becomes the directory, the Cargo package and binary name, the
/// library identifier `src/main.rs` calls and the manifest `name`: non-empty, no path separators, no
/// leading dot, only the manifest's charset, a leading ASCII letter, and not reserved.
fn validate_name(name: &str) -> Result<(), Diagnostic> {
    let refuse = |message: String| {
        Err(Diagnostic::new(code::NEW_NAME_INVALID, message)
            .with_help(format!("try `ludion new {}`", suggest_name(name))))
    };
    if name.is_empty() {
        return refuse("the project name is empty".to_string());
    }
    if name.contains('/') || name.contains('\\') {
        return refuse(format!(
            "`{name}` contains a path separator — `new` takes a bare project name and creates \
             `./<name>`"
        ));
    }
    if name.starts_with('.') {
        return refuse(format!(
            "`{name}` starts with `.` — choose a plain project name"
        ));
    }
    if let Some(bad) = name.chars().find(|c| !is_plain_name_char(*c)) {
        return refuse(format!(
            "`{name}` contains `{bad}` — a project name is ASCII letters, digits, `-` and `_`"
        ));
    }
    if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return refuse(format!(
            "`{name}` does not start with a letter — a Rust crate name cannot start with a digit, \
             `-` or `_`"
        ));
    }
    if is_reserved(name) {
        return refuse(format!(
            "`{name}` is reserved — a Rust keyword, a name Cargo refuses or that collides with \
             `target/`, a dependency the template itself uses, or a Windows device name"
        ));
    }
    Ok(())
}

/// A name close to `name` that passes [`validate_name`].
fn suggest_name(name: &str) -> String {
    let mut suggestion = String::new();
    for c in name.to_ascii_lowercase().chars() {
        let c = if is_plain_name_char(c) { c } else { '-' };
        if c != '-' || !suggestion.ends_with('-') {
            suggestion.push(c);
        }
    }
    let mut suggestion = suggestion.trim_matches(['-', '_']).to_owned();
    if suggestion.is_empty() {
        return "my-game".to_owned();
    }
    if !suggestion.starts_with(|c: char| c.is_ascii_alphabetic()) {
        suggestion.insert_str(0, "game-");
    }
    if is_reserved(&suggestion) {
        suggestion.push_str("-game");
    }
    debug_assert!(validate_name(&suggestion).is_ok());
    suggestion
}

/// The engine checkout's workspace root, derived from this crate's compile-time location
/// (`<root>/crates/ludion-cli`).
fn engine_workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent() // <root>/crates
        .and_then(Path::parent) // <root>
        .map(Path::to_path_buf)
        .expect("ludion-cli lives at <workspace-root>/crates/ludion-cli")
}

/// The four engine dependency specs the `Cargo.toml` template needs, each already formatted as a
/// TOML value (`{ path = … }` or `{ version = … }`).
struct EngineDeps {
    ludion: String,
    ludion_testkit: String,
    ludion_engine: String,
    ludion_core: String,
}

/// Chooses path deps (checkout flow) vs version deps (installed flow) for the generated project.
///
/// Installed flow is used when the baked engine root is missing `crates/ludion` — i.e. the binary
/// was `cargo install`ed and there is no checkout to point at — or when forced via
/// `LUDION_NEW_INSTALLED` (the test override). All engine crates share one workspace version, so the
/// pinned version is this binary's own `CARGO_PKG_VERSION`.
fn engine_dependencies(engine_root: &Path) -> EngineDeps {
    // The facade's features (`wire`, `window`) are the game's own, so its dependency turns the
    // defaults off; the other crates have none to turn off.
    let crate_spec = |crate_name: &str, tail: &str| -> String {
        if installed_mode(engine_root) {
            format!("{{ version = \"{}\"{tail} }}", env!("CARGO_PKG_VERSION"))
        } else {
            let root = engine_root.to_string_lossy().replace('\\', "/");
            format!("{{ path = \"{root}/crates/{crate_name}\"{tail} }}")
        }
    };
    EngineDeps {
        ludion: crate_spec("ludion", ", default-features = false"),
        ludion_testkit: crate_spec("ludion-testkit", ""),
        ludion_engine: crate_spec("ludion-engine", ""),
        ludion_core: crate_spec("ludion-core", ""),
    }
}

/// Whether to scaffold in installed flow (version deps). True when forced by `LUDION_NEW_INSTALLED`,
/// or when the baked engine root does not contain `crates/ludion` (no checkout to depend on).
fn installed_mode(engine_root: &Path) -> bool {
    std::env::var_os("LUDION_NEW_INSTALLED").is_some()
        || !engine_root.join("crates/ludion").is_dir()
}

/// If `target` lies inside one of the engine workspace's member globs, returns that glob's directory
/// name (`"crates"` or `"games"`); otherwise `None`.
fn member_glob_containing(target: &Path, engine_root: &Path) -> Option<&'static str> {
    ["crates", "games"]
        .into_iter()
        .find(|glob| target.starts_with(engine_root.join(glob)))
}

/// Writes `body` to `<project_dir>/<rel_path>`, creating parent directories as needed.
fn write_file(project_dir: &Path, rel_path: &str, body: &str) -> Result<(), String> {
    let path = project_dir.join(rel_path);
    ludion_core::atomic::write_atomic(&path, body)
        .map_err(|e| format!("could not write `{}`: {e}", path.display()))
}

/// Strips the three-line docs frontmatter (`---` / `description: …` / `---`) from the guide, leaving
/// just the Markdown body for the generated `LUDION-GUIDE.md`.
fn strip_frontmatter(md: &str) -> String {
    let lines: Vec<&str> = md.lines().collect();
    let body_start = if lines.first() == Some(&"---") {
        // Index just past the closing fence (position is relative to `skip(1)`, so +2 overall).
        lines
            .iter()
            .skip(1)
            .position(|line| *line == "---")
            .map_or(0, |p| p + 2)
    } else {
        0
    };
    let mut body = &lines[body_start..];
    while body.first() == Some(&"") {
        body = &body[1..];
    }
    let mut out = body.join("\n");
    out.push('\n');
    out
}

/// Prints the created file tree and next steps.
fn print_summary(name: &str, project_dir: &Path, written: &[&str]) {
    println!("created `{}` — a Ludion game", project_dir.display());
    println!();
    println!("{name}/");
    let mut paths: Vec<&str> = written.to_vec();
    paths.push("goldens/");
    paths.sort_unstable();
    for path in paths {
        println!("  {path}");
    }
    println!();
    println!("next:");
    println!("  cd {name}");
    println!(
        "  cargo test                              # sim assertions + WorldHash determinism + golden"
    );
    println!(
        "  cargo run -- --scene scenes/main.yaml   # host it live (prints `listening on ws://…`)"
    );
    println!();
    println!("read LUDION-GUIDE.md before writing code.");
}

#[cfg(test)]
mod tests {
    use super::{suggest_name, template_dependencies, validate_name};

    #[test]
    fn suggestions_are_valid_names() {
        for (given, want) in [
            ("my game", "my-game"),
            ("2048", "game-2048"),
            ("a.b", "a-b"),
            ("a\"b", "a-b"),
            ("test", "test-game"),
            ("", "my-game"),
            ("-x", "x"),
            ("con", "con-game"),
        ] {
            let got = suggest_name(given);
            assert_eq!(got, want, "suggestion for {given:?}");
            assert!(validate_name(&got).is_ok(), "{got:?} must be accepted");
        }
    }

    #[test]
    fn the_template_dependencies_are_the_ones_its_manifest_declares() {
        let found: Vec<&str> = template_dependencies().collect();
        assert_eq!(
            found,
            [
                "ludion",
                "serde",
                "schemars",
                "ludion-testkit",
                "ludion-engine",
                "ludion-core",
                "serde_json"
            ]
        );
        assert_eq!(suggest_name("ludion"), "ludion-game");
    }

    #[test]
    fn reserved_and_malformed_names_are_refused() {
        for bad in [
            "",
            "a/b",
            ".x",
            "my game",
            "a.b",
            "2048",
            "_x",
            "-x",
            "fn",
            "gen",
            "Self",
            "test",
            "proc-macro",
            "ludion",
            "ludion-testkit",
            "serde",
            "serde_json",
            "serde-json",
            "schemars",
            "ludion-engine",
            "ludion_core",
            "con",
            "NUL",
            "com1",
            "ünï",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?} must be refused");
        }
        for good in ["breakout", "my-game_2", "Game", "a1"] {
            assert!(validate_name(good).is_ok(), "{good:?} must be accepted");
        }
    }
}
