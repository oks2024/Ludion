//! Integration tests for `ludion docs` and `ludion devdocs`, driven against the built `ludion`
//! binary.
//!
//! Mirrors `explain.rs`'s house style: run the real binary as a subprocess and assert on its exit
//! code and output. Discovery walks up from the working directory, so every test pins `cwd`
//! explicitly — either the real repo root (which makes the listing tests double as the CI
//! frontmatter gate for the real `docs/` and `dev-docs/`) or a hand-rolled temp fixture.
//!
//! The two verbs serve two roots and differ on exactly one thing: `docs/` is embedded in the
//! binary, `dev-docs/` is checkout-only. Tests naming a topic therefore pick from the right root —
//! `scene-format` for `docs`, `determinism` for `devdocs`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The real repository root: the `ludion-cli` manifest dir is `<root>/crates/ludion-cli`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// A fresh fixture-root path in the OS temp dir, unique per test process.
fn fixture_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "ludion-cli-docs-test-{name}-{}",
        std::process::id()
    ))
}

/// Lays down a minimal directory that discovery accepts as an engine checkout: the two doc roots
/// the verbs serve, plus the `crates/ludion-core/Cargo.toml` marker that says *engine*, not "some
/// project with a docs folder".
fn engine_checkout_fixture(root: &Path) {
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::create_dir_all(root.join("dev-docs")).unwrap();
    std::fs::create_dir_all(root.join("crates").join("ludion-core")).unwrap();
    std::fs::write(
        root.join("crates").join("ludion-core").join("Cargo.toml"),
        "[package]\nname = \"ludion-core\"\n",
    )
    .unwrap();
}

/// Runs `ludion <verb> <args...>` with the given working directory.
fn run_verb_in(verb: &str, dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg(verb)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run the ludion binary")
}

/// Runs `ludion docs <args...>` with the given working directory.
fn run_docs_in(dir: &Path, args: &[&str]) -> Output {
    run_verb_in("docs", dir, args)
}

/// Runs `ludion devdocs <args...>` with the given working directory.
fn run_devdocs_in(dir: &Path, args: &[&str]) -> Output {
    run_verb_in("devdocs", dir, args)
}

/// **The CI frontmatter gate for the real `docs/`.** Listing from the repo root succeeds — every
/// committed topic doc has well-formed frontmatter — and known topics appear in the output.
#[test]
fn listing_in_repo_succeeds_and_names_known_topics() {
    let output = run_docs_in(&repo_root(), &[]);
    assert!(
        output.status.success(),
        "expected `ludion docs` to succeed (a committed doc has malformed frontmatter?), \
         stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Topics that live in `docs/`; the engine-internals topics belong to `devdocs` now.
    assert!(stdout.contains("engine-guide"), "stdout was: {stdout}");
    assert!(stdout.contains("scene-format"), "stdout was: {stdout}");
    assert!(!stdout.contains("crate-graph"), "stdout was: {stdout}");
    // Each line is `<topic> — <description>`.
    assert!(stdout.contains(" — "), "stdout was: {stdout}");
}

/// **The CI frontmatter gate for the real `dev-docs/`.** Same contract as the `docs` listing, over
/// the engine-internals root.
#[test]
fn devdocs_listing_in_repo_succeeds_and_names_known_topics() {
    let output = run_devdocs_in(&repo_root(), &[]);
    assert!(
        output.status.success(),
        "expected `ludion devdocs` to succeed (a committed doc has malformed frontmatter?), \
         stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("determinism"), "stdout was: {stdout}");
    assert!(stdout.contains("crate-graph"), "stdout was: {stdout}");
    assert!(!stdout.contains("engine-guide"), "stdout was: {stdout}");
    assert!(stdout.contains(" — "), "stdout was: {stdout}");
}

/// `ludion docs scene-format` inside a checkout prints the doc *text*, not a path — the regression
/// that motivated `--path`: persona agents ask for documentation and used to get a filename.
#[test]
fn lookup_prints_the_doc_body() {
    let output = run_docs_in(&repo_root(), &["scene-format"]);
    assert!(
        output.status.success(),
        "expected `ludion docs scene-format` to succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("doc body is UTF-8");
    let on_disk = std::fs::read_to_string(repo_root().join("docs").join("scene-format.md"))
        .expect("read docs/scene-format.md");
    assert_eq!(
        stdout, on_disk,
        "checkout lookup must emit the file verbatim, frontmatter included"
    );
    assert!(
        !stdout.trim().ends_with("scene-format.md"),
        "stdout must be the body, not a path; stdout was: {stdout}"
    );
}

/// `ludion devdocs determinism` prints `dev-docs/determinism.md` verbatim — the same body-not-path
/// contract, over the checkout-only root.
#[test]
fn devdocs_lookup_prints_the_doc_body() {
    let output = run_devdocs_in(&repo_root(), &["determinism"]);
    assert!(
        output.status.success(),
        "expected `ludion devdocs determinism` to succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("doc body is UTF-8");
    let on_disk = std::fs::read_to_string(repo_root().join("dev-docs").join("determinism.md"))
        .expect("read dev-docs/determinism.md");
    assert_eq!(
        stdout, on_disk,
        "checkout lookup must emit the file verbatim, frontmatter included"
    );
}

/// `ludion docs scene-format --path` prints the path to `docs/scene-format.md`, joined from the
/// discovered root (no `\\?\` canonicalization noise on Windows).
#[test]
fn path_flag_prints_the_doc_path() {
    let output = run_docs_in(&repo_root(), &["scene-format", "--path"]);
    assert!(
        output.status.success(),
        "expected `ludion docs scene-format --path` to succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_path_tail(&output, "docs", "scene-format.md");
}

/// `ludion devdocs determinism --path` prints the path under `dev-docs/`, not `docs/`.
#[test]
fn devdocs_path_flag_prints_the_doc_path() {
    let output = run_devdocs_in(&repo_root(), &["determinism", "--path"]);
    assert!(
        output.status.success(),
        "expected `ludion devdocs determinism --path` to succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_path_tail(&output, "dev-docs", "determinism.md");
}

/// Asserts a `--path` run printed `<...>/<dir>/<file>` with no verbatim-prefix noise. Compares
/// path *components* so the assertion holds under both `/` and `\` separators.
fn assert_path_tail(output: &Output, dir: &str, file: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let printed = stdout.trim();
    assert!(
        !printed.contains("\\\\?\\"),
        "path was canonicalized with the verbatim prefix: {printed}"
    );
    let mut components = Path::new(printed).components().rev();
    assert_eq!(
        components.next().map(|c| c.as_os_str().to_os_string()),
        Some(file.into()),
        "printed path was: {printed}"
    );
    assert_eq!(
        components.next().map(|c| c.as_os_str().to_os_string()),
        Some(dir.into()),
        "printed path was: {printed}"
    );
}

/// A typo'd topic fails with an `L0009` diagnostic that suggests the nearest existing topic.
#[test]
fn unknown_topic_is_l0009_with_a_suggestion() {
    let output = run_docs_in(&repo_root(), &["scene-formt"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L0009]"), "stderr was: {stderr}");
    assert!(stderr.contains("scene-format"), "stderr was: {stderr}");
}

/// The same for `devdocs`, whose candidate set is `dev-docs/` — and, with no embedded table, only
/// `dev-docs/`.
#[test]
fn devdocs_unknown_topic_is_l0009_with_a_suggestion() {
    let output = run_devdocs_in(&repo_root(), &["determinsm"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L0009]"), "stderr was: {stderr}");
    assert!(stderr.contains("determinism"), "stderr was: {stderr}");
    assert!(
        stderr.contains("dev-docs/"),
        "the help must name the devdocs root, stderr was: {stderr}"
    );
}

/// A doc with malformed frontmatter fails the listing with `L0010` — while the well-formed docs
/// still list on stdout, and path lookup of the malformed topic still succeeds.
#[test]
fn malformed_frontmatter_fails_listing_but_not_lookup() {
    let root = fixture_root("malformed");
    let _cleanup = Cleanup(root.clone());
    engine_checkout_fixture(&root);
    write_frontmatter_fixture(&root.join("docs"));
    assert_listing_gate(run_docs_in(&root, &[]));

    // Frontmatter validity is irrelevant for lookup — the malformed doc must still resolve.
    let lookup = run_docs_in(&root, &["broken"]);
    assert!(
        lookup.status.success(),
        "expected `ludion docs broken` to succeed despite the malformed frontmatter, stderr: {}",
        String::from_utf8_lossy(&lookup.stderr)
    );
    let lookup_stdout = String::from_utf8_lossy(&lookup.stdout);
    assert_eq!(
        lookup_stdout, "---\ntitle: not the allowed key\n---\n\n# Broken\n",
        "the unparsed body is emitted verbatim"
    );
}

/// `devdocs` carries the same frontmatter gate over its own root: the malformed doc fails the
/// listing with a non-zero exit, the well-formed one still lists, and lookup is unaffected.
#[test]
fn devdocs_malformed_frontmatter_fails_listing_but_not_lookup() {
    let root = fixture_root("devdocs-malformed");
    let _cleanup = Cleanup(root.clone());
    engine_checkout_fixture(&root);
    write_frontmatter_fixture(&root.join("dev-docs"));
    assert_listing_gate(run_devdocs_in(&root, &[]));

    let lookup = run_devdocs_in(&root, &["broken"]);
    assert!(
        lookup.status.success(),
        "expected `ludion devdocs broken` to succeed despite the malformed frontmatter, stderr: {}",
        String::from_utf8_lossy(&lookup.stderr)
    );
}

/// Writes the two-doc frontmatter fixture into `dir`: one malformed doc and one well-formed doc.
/// The good one uses CRLF line endings on purpose — a Windows checkout must parse the same as an
/// LF one.
fn write_frontmatter_fixture(dir: &Path) {
    std::fs::write(
        dir.join("broken.md"),
        "---\ntitle: not the allowed key\n---\n\n# Broken\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("good.md"),
        "---\r\ndescription: A well-formed fixture topic\r\n---\r\n\r\n# Good\r\n",
    )
    .unwrap();
}

/// Asserts a listing run over [`write_frontmatter_fixture`]'s directory: non-zero exit, an `L0010`
/// naming the bad doc, and the good doc still listed on stdout.
fn assert_listing_gate(listing: Output) {
    assert!(
        !listing.status.success(),
        "expected the listing to fail on the malformed doc"
    );
    let stderr = String::from_utf8_lossy(&listing.stderr);
    assert!(stderr.contains("error[L0010]"), "stderr was: {stderr}");
    assert!(stderr.contains("broken"), "stderr was: {stderr}");
    let stdout = String::from_utf8_lossy(&listing.stdout);
    assert!(
        stdout.contains("good — A well-formed fixture topic"),
        "the well-formed topic should still list, stdout was: {stdout}"
    );
}

/// A fresh, empty directory outside any repository — no ancestor is an engine checkout — so the
/// binary falls back to its embedded docs (and `devdocs` has nothing to fall back to).
fn outside_any_repo(name: &str) -> (PathBuf, Cleanup) {
    let root = fixture_root(name);
    let cleanup = Cleanup(root.clone());
    // A nested empty dir, so even the fixture root itself carries no repo markers.
    let nested = root.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    (nested, cleanup)
}

/// Outside any repository, listing falls back to the docs embedded in the binary at build time —
/// it succeeds and names known topics instead of failing.
#[test]
fn embedded_listing_outside_repo_names_known_topics() {
    let (dir, _cleanup) = outside_any_repo("embedded-list");
    let output = run_docs_in(&dir, &[]);
    assert!(
        output.status.success(),
        "expected embedded listing to succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("engine-guide"), "stdout was: {stdout}");
    assert!(stdout.contains("scene-format"), "stdout was: {stdout}");
    assert!(stdout.contains(" — "), "stdout was: {stdout}");
}

/// Outside any repository, `ludion docs <topic>` prints the doc *body* from the embedded copy
/// (there is no on-disk file to point at).
#[test]
fn embedded_lookup_outside_repo_prints_the_body() {
    let (dir, _cleanup) = outside_any_repo("embedded-lookup");
    let output = run_docs_in(&dir, &["scene-format"]);
    assert!(
        output.status.success(),
        "expected `ludion docs scene-format` to succeed outside a repo, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The body, not a path: the frontmatter description and Markdown content are present, and no
    // `.md` path is printed.
    assert!(
        !stdout.trim().ends_with("scene-format.md"),
        "embedded lookup must print the body, not a path; stdout was: {stdout}"
    );
    assert!(
        stdout.contains("---") && stdout.contains('#'),
        "expected Markdown doc body, stdout was: {stdout}"
    );
}

/// `--path` outside any repository is an `L0008`: there is no `docs/` directory, so no file path
/// exists to print — the one thing embedded mode cannot serve.
#[test]
fn path_flag_outside_repo_is_l0008() {
    let (dir, _cleanup) = outside_any_repo("embedded-path");
    let output = run_docs_in(&dir, &["scene-format", "--path"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L0008]"), "stderr was: {stderr}");
    assert!(
        String::from_utf8_lossy(&output.stdout).is_empty(),
        "nothing should reach stdout"
    );
}

/// **The pin for the audience split.** `devdocs` is checkout-only *by design*: outside an engine
/// checkout every form — listing, lookup, `--path`, `--section` — is an `L0008`, and there is no
/// embedded fallback quietly answering instead. A topic that genuinely exists in `dev-docs/` is
/// used deliberately: the refusal must be about the missing checkout, never about the topic.
#[test]
fn devdocs_outside_a_checkout_is_l0008_for_every_form() {
    let (dir, _cleanup) = outside_any_repo("devdocs-no-checkout");
    let forms: [&[&str]; 4] = [
        &[],
        &["determinism"],
        &["determinism", "--path"],
        &["determinism", "--section", "Gotchas"],
    ];
    for args in forms {
        let output = run_devdocs_in(&dir, args);
        assert!(
            !output.status.success(),
            "expected a non-zero exit for `devdocs {}`",
            args.join(" ")
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("error[L0008]"),
            "`devdocs {}` stderr was: {stderr}",
            args.join(" ")
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).is_empty(),
            "no doc text may reach stdout — there is no embedded fallback for engine-internals \
             docs; `devdocs {}` printed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            !stderr.contains("error[L0009]"),
            "the refusal must be the missing checkout, not an unknown topic; stderr was: {stderr}"
        );
    }
}

/// **The pin for the bug `--path` was added to fix.** The two modes must be one command: for the
/// same topic, checkout-mode stdout and embedded-mode stdout are the same bytes. A caller must
/// never have to know which mode it got.
#[test]
fn checkout_and_embedded_lookup_agree_byte_for_byte() {
    let in_checkout = run_docs_in(&repo_root(), &["scene-format"]);
    assert!(in_checkout.status.success(), "checkout lookup failed");
    let (dir, _cleanup) = outside_any_repo("cross-mode");
    let embedded = run_docs_in(&dir, &["scene-format"]);
    assert!(embedded.status.success(), "embedded lookup failed");
    assert_eq!(
        in_checkout.stdout, embedded.stdout,
        "the same topic must read identically on disk and embedded — `build.rs` copies \
         `docs/*.md` into `embedded-docs/` as raw bytes, so any difference here is a real drift, \
         not a line-ending artifact"
    );
}

/// An unknown topic outside any repo still fails with an `L0009` diagnostic and a suggestion,
/// matching checkout-mode UX.
#[test]
fn embedded_unknown_topic_is_l0009_with_a_suggestion() {
    let (dir, _cleanup) = outside_any_repo("embedded-unknown");
    let output = run_docs_in(&dir, &["scene-formt"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L0009]"), "stderr was: {stderr}");
    assert!(stderr.contains("scene-format"), "stderr was: {stderr}");
}

/// **The pin for the downstream-project misfire.** `ludion new` writes a `CLAUDE.md` into every
/// scaffolded project, so `CLAUDE.md` + `docs/` used to be enough to flip a *user's* project into
/// checkout mode: their files got listed and `ludion docs scene-format` became an `L0009` naming one
/// of their filenames. Detection now needs an engine-only marker, so this fixture — a project
/// carrying both generic names and its own unrelated doc — must serve the engine's embedded docs.
#[test]
fn a_project_with_its_own_claude_md_and_docs_is_not_a_checkout() {
    let root = fixture_root("downstream-project");
    let _cleanup = Cleanup(root.clone());
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("CLAUDE.md"), "# My Game\n").unwrap();
    std::fs::write(
        root.join("docs").join("level-design.md"),
        "# How our levels work\n",
    )
    .unwrap();

    let lookup = run_docs_in(&root, &["scene-format"]);
    assert!(
        lookup.status.success(),
        "the engine's own topic must still resolve inside a downstream project, stderr: {}",
        String::from_utf8_lossy(&lookup.stderr)
    );
    let engine_doc = std::fs::read_to_string(repo_root().join("docs").join("scene-format.md"))
        .expect("read docs/scene-format.md");
    assert_eq!(
        String::from_utf8_lossy(&lookup.stdout),
        engine_doc,
        "the embedded engine doc must be served, not the project's own docs/"
    );

    let listing = run_docs_in(&root, &[]);
    let stdout = String::from_utf8_lossy(&listing.stdout);
    assert!(
        stdout.contains("engine-guide") && !stdout.contains("level-design"),
        "the listing must be the engine's topics, not the project's; stdout was: {stdout}"
    );
}

/// Inside a genuine checkout, a topic missing from `docs/` still resolves from the embedded copy —
/// the recoverable-miss half of the same fix. Only when neither source has it is it an `L0009`.
#[test]
fn a_checkout_falls_back_to_the_embedded_copy_on_a_miss() {
    let root = fixture_root("checkout-fallback");
    let _cleanup = Cleanup(root.clone());
    engine_checkout_fixture(&root);
    std::fs::write(
        root.join("docs").join("local-only.md"),
        "---\ndescription: A fixture topic\n---\n\n# Local\n",
    )
    .unwrap();

    let hit = run_docs_in(&root, &["scene-format"]);
    assert!(
        hit.status.success(),
        "an engine topic absent from this checkout's docs/ must fall back to embedded, stderr: {}",
        String::from_utf8_lossy(&hit.stderr)
    );

    let miss = run_docs_in(&root, &["nowhere-at-all"]);
    assert!(!miss.status.success(), "expected a non-zero exit");
    assert!(
        String::from_utf8_lossy(&miss.stderr).contains("error[L0009]"),
        "stderr was: {}",
        String::from_utf8_lossy(&miss.stderr)
    );

    // `--path` cannot fall back — an embedded doc has no file to name.
    let path_miss = run_docs_in(&root, &["scene-format", "--path"]);
    assert!(
        !path_miss.status.success(),
        "`--path` must not answer from the embedded copy"
    );
}

/// Completeness gate: the embedded topic set (listed outside any repo) is exactly the `docs/*.md`
/// files on disk. A new doc that fails to embed — e.g. the build script missed it — breaks here.
/// `dev-docs/` is deliberately out of scope: it is never embedded.
#[test]
fn embedded_topics_match_the_docs_directory() {
    // Topics on disk: every `.md` stem directly in the repo `docs/`.
    let docs_dir = repo_root().join("docs");
    let mut on_disk: Vec<String> = std::fs::read_dir(&docs_dir)
        .expect("read docs/")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            } else {
                None
            }
        })
        .collect();
    on_disk.sort();

    // Topics embedded in the binary: the first token of each listing line, read outside any repo.
    let (dir, _cleanup) = outside_any_repo("embedded-complete");
    let output = run_docs_in(&dir, &[]);
    assert!(
        output.status.success(),
        "embedded listing failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("listing is UTF-8");
    let mut embedded: Vec<String> = stdout
        .lines()
        .filter_map(|line| line.split(" — ").next().map(str::trim).map(str::to_string))
        .filter(|s| !s.is_empty())
        .collect();
    embedded.sort();

    assert_eq!(
        embedded, on_disk,
        "embedded docs must match docs/*.md exactly — a doc failed to embed or an extra one leaked in"
    );
}

/// Lays down a checkout fixture with one topic doc — in `dir` (`docs` or `dev-docs`) — built for
/// `--section` tests: three level-2 sections (one, `Beta`, carrying a level-3 subsection) and a
/// fenced YAML example whose comment line (`# fake heading`) would misread as a heading — and would
/// wrongly close `Alpha`'s section early, since a level-1 heading is "equal or higher" than
/// `Alpha`'s level 2 — if fence-tracking were broken.
fn section_fixture_root(name: &str, dir: &str) -> PathBuf {
    let root = fixture_root(name);
    engine_checkout_fixture(&root);
    std::fs::write(
        root.join(dir).join("section-fixture.md"),
        "---\n\
         description: A fixture topic for --section tests\n\
         ---\n\
         \n\
         # Section Fixture\n\
         \n\
         ## Alpha\n\
         \n\
         Alpha body text.\n\
         \n\
         ```yaml\n\
         # fake heading\n\
         key: value\n\
         ```\n\
         \n\
         More alpha text after the fence.\n\
         \n\
         ## Beta\n\
         \n\
         Beta body.\n\
         \n\
         ### Beta Sub\n\
         \n\
         Sub body.\n\
         \n\
         ## Gamma\n\
         \n\
         Gamma body.\n",
    )
    .unwrap();
    root
}

/// `--section Alpha` prints just that heading's text, through everything up to (excluding) the
/// next `##` heading.
#[test]
fn section_flag_extracts_one_heading() {
    let root = section_fixture_root("section-hit", "docs");
    let _cleanup = Cleanup(root.clone());
    let output = run_docs_in(&root, &["section-fixture", "--section", "Alpha"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("section body is UTF-8");
    assert!(stdout.starts_with("## Alpha"), "stdout was: {stdout}");
    assert!(
        stdout.contains("More alpha text after the fence."),
        "stdout was: {stdout}"
    );
    assert!(!stdout.contains("## Beta"), "stdout was: {stdout}");
}

/// `--section` behaves identically on `devdocs` — the extraction is one shared implementation, and
/// this is the pin that keeps it one.
#[test]
fn devdocs_section_flag_extracts_one_heading() {
    let root = section_fixture_root("devdocs-section-hit", "dev-docs");
    let _cleanup = Cleanup(root.clone());
    let output = run_devdocs_in(&root, &["section-fixture", "--section", "Alpha"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("section body is UTF-8");
    assert!(stdout.starts_with("## Alpha"), "stdout was: {stdout}");
    assert!(!stdout.contains("## Beta"), "stdout was: {stdout}");
}

/// The heading match is case-insensitive.
#[test]
fn section_flag_matches_case_insensitively() {
    let root = section_fixture_root("section-case", "docs");
    let _cleanup = Cleanup(root.clone());
    let output = run_docs_in(&root, &["section-fixture", "--section", "aLpHa"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("section body is UTF-8");
    assert!(stdout.starts_with("## Alpha"), "stdout was: {stdout}");
}

/// **Fence immunity.** A `#`-prefixed line inside a fenced code block is not a heading: it neither
/// terminates `Alpha`'s section early nor is itself an addressable section.
#[test]
fn section_flag_ignores_headings_inside_fenced_code() {
    let root = section_fixture_root("section-fence", "docs");
    let _cleanup = Cleanup(root.clone());

    let alpha = run_docs_in(&root, &["section-fixture", "--section", "Alpha"]);
    assert!(
        alpha.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&alpha.stderr)
    );
    let alpha_stdout = String::from_utf8(alpha.stdout).expect("section body is UTF-8");
    assert!(
        alpha_stdout.contains("More alpha text after the fence."),
        "the fenced `# fake heading` line wrongly terminated Alpha's section early; stdout was: \
         {alpha_stdout}"
    );

    let fake = run_docs_in(&root, &["section-fixture", "--section", "fake heading"]);
    assert!(!fake.status.success(), "expected a non-zero exit");
    assert!(
        String::from_utf8_lossy(&fake.stderr).contains("error[L0015]"),
        "stderr was: {}",
        String::from_utf8_lossy(&fake.stderr)
    );
}

/// An unmatched `--section` fails with `L0015`, suggesting the nearest real heading and listing
/// the topic's top-level headings.
#[test]
fn section_flag_miss_is_l0015_with_a_suggestion() {
    let root = section_fixture_root("section-miss", "docs");
    let _cleanup = Cleanup(root.clone());
    assert_section_miss(run_docs_in(
        &root,
        &["section-fixture", "--section", "Alph"],
    ));
}

/// The same on `devdocs`.
#[test]
fn devdocs_section_flag_miss_is_l0015_with_a_suggestion() {
    let root = section_fixture_root("devdocs-section-miss", "dev-docs");
    let _cleanup = Cleanup(root.clone());
    assert_section_miss(run_devdocs_in(
        &root,
        &["section-fixture", "--section", "Alph"],
    ));
}

/// Asserts an unmatched `--section` run: non-zero exit, `L0015`, a nearest-match suggestion, and
/// the topic's top-level headings.
fn assert_section_miss(output: Output) {
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[L0015]"), "stderr was: {stderr}");
    assert!(stderr.contains("Alpha"), "stderr was: {stderr}");
    assert!(
        stderr.contains("Beta") && stderr.contains("Gamma"),
        "expected the topic's top-level headings listed, stderr was: {stderr}"
    );
}

/// `--section` without a topic is rejected by clap before `docs::run` ever sees it.
#[test]
fn section_flag_without_topic_is_rejected_by_clap() {
    let root = section_fixture_root("section-no-topic", "docs");
    let _cleanup = Cleanup(root.clone());
    let output = run_docs_in(&root, &["--section", "Alpha"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required"),
        "expected clap's missing-topic error, stderr was: {stderr}"
    );
}

/// `devdocs` carries the same clap wiring: `--section` requires a topic.
#[test]
fn devdocs_section_flag_without_topic_is_rejected_by_clap() {
    let root = section_fixture_root("devdocs-section-no-topic", "dev-docs");
    let _cleanup = Cleanup(root.clone());
    let output = run_devdocs_in(&root, &["--section", "Alpha"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("required"),
        "expected clap's missing-topic error, stderr was: {stderr}"
    );
}

/// `--section` and `--path` are mutually exclusive: a section is text, not a file to point at.
#[test]
fn section_flag_with_path_is_rejected_by_clap() {
    let root = section_fixture_root("section-with-path", "docs");
    let _cleanup = Cleanup(root.clone());
    let output = run_docs_in(&root, &["section-fixture", "--section", "Alpha", "--path"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "expected clap's conflicts_with error, stderr was: {stderr}"
    );
}

/// And the same conflict on `devdocs`.
#[test]
fn devdocs_section_flag_with_path_is_rejected_by_clap() {
    let root = section_fixture_root("devdocs-section-with-path", "dev-docs");
    let _cleanup = Cleanup(root.clone());
    let output = run_devdocs_in(&root, &["section-fixture", "--section", "Alpha", "--path"]);
    assert!(!output.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "expected clap's conflicts_with error, stderr was: {stderr}"
    );
}

/// Removes the test's temp fixture dir on drop, success or failure.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
