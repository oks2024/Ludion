//! Implementation of `ludion docs [TOPIC]` and `ludion devdocs [TOPIC]`.
//!
//! Two verbs, two documentation roots, one implementation. `docs/` is written for someone building
//! a game *with* Ludion and ships inside the binary; `dev-docs/` is written for someone editing
//! Ludion's own source and exists only in an engine checkout. Everything below the [`DocSet`]
//! parameter is shared: discovery, listing, lookup, `--section`, frontmatter.

use ludion_core::codes::code::{
    DOCS_DIR_NOT_FOUND, DOCS_MALFORMED, DOCS_UNKNOWN_SECTION, DOCS_UNKNOWN_TOPIC,
};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The repo-root `docs/*.md` topics baked into the binary at build time (`build.rs`), so `ludion
/// docs` works when the CLI is installed outside the engine checkout. Directory-driven: a doc added
/// to `docs/` joins this table on the next rebuild. `dev-docs/` is deliberately absent — see
/// [`DEV_DOCS`].
mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_docs.rs"));
}

/// One documentation root as a verb serves it. The single axis the two verbs differ on, so every
/// function below takes it instead of existing twice.
struct DocSet {
    /// The verb that serves this root, as it appears in help text (`docs`, `devdocs`).
    verb: &'static str,
    /// The root's directory name under the checkout root (`docs`, `dev-docs`).
    dir: &'static str,
    /// The baked-in copies served outside a checkout. Empty means checkout-only: outside a
    /// checkout the verb has nothing to serve and raises [`L0008`](DOCS_DIR_NOT_FOUND).
    embedded: &'static [(&'static str, &'static str)],
}

/// `ludion docs` — the user-facing topics, embedded in the binary so an installed CLI can serve
/// them anywhere.
const USER_DOCS: DocSet = DocSet {
    verb: "docs",
    dir: "docs",
    embedded: embedded::EMBEDDED_DOCS,
};

/// `ludion devdocs` — the engine-internals topics. Checkout-only *by design*, not by omission:
/// nothing in them can be acted on without the engine's source in front of you, so baking them
/// into every installed binary (and every published tarball) would ship weight no reader outside
/// the checkout can use.
const DEV_DOCS: DocSet = DocSet {
    verb: "devdocs",
    dir: "dev-docs",
    embedded: &[],
};

/// Runs `ludion docs [topic] [--path] [--section]`. See [`run_set`] for the shared behavior.
pub fn run(topic: Option<&str>, path: bool, section: Option<&str>) -> ExitCode {
    if let Ok(cwd) = std::env::current_dir()
        && let Some(warning) = crate::new::stale_guide_warning(&cwd)
    {
        eprintln!("{warning}");
    }
    run_set(&USER_DOCS, topic, path, section)
}

/// Runs `ludion devdocs [topic] [--path] [--section]`. See [`run_set`] for the shared behavior;
/// the one difference is [`DEV_DOCS`]'s empty embedded table, which makes every form of the verb an
/// [`L0008`](DOCS_DIR_NOT_FOUND) outside an engine checkout.
pub fn run_dev(topic: Option<&str>, path: bool, section: Option<&str>) -> ExitCode {
    run_set(&DEV_DOCS, topic, path, section)
}

/// The implementation both verbs share, over `set`'s root.
///
/// A topic hit prints the doc *text* — identical stdout whether the doc comes off disk or out of
/// the binary, because callers (the editor personas among them) ask for documentation, not a
/// filename. `--path` asks for the file instead, and needs the on-disk directory.
///
/// **Checkout mode** — when an ancestor of the working directory is an *engine* checkout (see
/// [`find_docs_root`]): with no topic, lists every topic doc in the root — one `<topic> —
/// <description>` line per well-formed doc on stdout — and reports each doc with malformed
/// frontmatter as an [`L0010`](DOCS_MALFORMED) diagnostic on stderr (a non-zero exit when any doc is
/// malformed is deliberate: running the listing in CI is the frontmatter gate). With a topic, prints
/// `<dir>/<topic>.md`'s text, or its path under `--path`.
///
/// **Embedded mode** — when no ancestor is an engine checkout (an installed binary): the same
/// listing and lookup UX backed by the baked-in docs, except `--path` (no file exists to name),
/// which is [`L0008`](DOCS_DIR_NOT_FOUND). A set with no embedded table has no such mode at all —
/// every form is an `L0008`. Either mode raises [`L0009`](DOCS_UNKNOWN_TOPIC) with a nearest-match
/// suggestion for an unknown topic.
///
/// `section` (`--section <heading>`) narrows a topic hit to one heading's text — see
/// [`extract_section`] — via [`print_body`], applied identically whether the body came off disk or
/// out of the embedded table. An unmatched heading is [`L0015`](DOCS_UNKNOWN_SECTION).
fn run_set(set: &DocSet, topic: Option<&str>, path: bool, section: Option<&str>) -> ExitCode {
    if let Some(root) = find_docs_root() {
        return match topic {
            Some(topic) => lookup(set, &root, topic, path, section),
            None => list(set, &root),
        };
    }
    if set.embedded.is_empty() {
        return report_checkout_only(set);
    }
    match topic {
        Some(topic) => lookup_embedded(set, topic, path, section),
        None => list_embedded(set),
    }
}

/// The whole of a checkout-only verb run outside an engine checkout: one
/// [`L0008`](DOCS_DIR_NOT_FOUND) for listing, lookup and `--path` alike. Stated as a rule rather
/// than an accident, because the natural reading of a missing doc is "this binary was built wrong".
fn report_checkout_only(set: &DocSet) -> ExitCode {
    let diagnostic = Diagnostic::new(
        DOCS_DIR_NOT_FOUND,
        format!(
            "`ludion {}` needs the engine checkout's `{}/` directory, and this binary is running \
             outside an engine checkout",
            set.verb, set.dir
        ),
    )
    .with_help(format!(
        "engine-internals docs ship only in an engine checkout — they are not baked into the \
         binary, because nothing in them can be acted on without the engine's source. Run `ludion \
         {}` from inside an engine checkout (any subdirectory works — discovery walks up), or run \
         `ludion docs` for the topics written for building a game with Ludion",
        set.verb
    ));
    eprintln!("{diagnostic}");
    ExitCode::FAILURE
}

/// The marker that identifies an *engine* checkout, as opposed to any project that happens to have
/// the same two generic names in it.
///
/// `CLAUDE.md` + `docs/` used to be the test, and it misfired: `ludion new` writes a `CLAUDE.md` into
/// every scaffolded project, so a user who added an ordinary `docs/` directory silently flipped into
/// checkout mode — `ludion docs` listed *their* files and `ludion docs determinism` became an
/// `L0009` with a suggestion drawn from their filenames, while the real docs sat unread in the
/// binary. A crate path in the engine's own workspace cannot be produced by accident.
const ENGINE_MARKER: &str = "crates/ludion-core/Cargo.toml";

/// Walks up from the current working directory looking for the engine checkout root: the first
/// ancestor carrying both [`ENGINE_MARKER`] and a `docs/` directory. Both verbs share this test —
/// `dev-docs/` is not part of it, so a checkout missing it reports a missing topic rather than
/// silently reading a stranger's directory.
///
/// Returns `None` when no ancestor qualifies (or the current directory is unreadable) — the caller
/// serves the embedded docs instead, or refuses when its set has none.
fn find_docs_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let marker = ENGINE_MARKER
            .split('/')
            .fold(dir.clone(), |path, segment| path.join(segment));
        if marker.is_file() && dir.join("docs").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Runs the verb with no topic: lists every topic in `set`'s root with its one-line description.
fn list(set: &DocSet, root: &Path) -> ExitCode {
    let docs_dir = root.join(set.dir);
    let topics = match topics(&docs_dir) {
        Ok(topics) => topics,
        Err(err) => {
            let diagnostic = Diagnostic::new(
                DOCS_DIR_NOT_FOUND,
                format!(
                    "could not read the agent-docs directory `{}`: {err}",
                    docs_dir.display()
                ),
            );
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    let mut any_malformed = false;
    for (stem, path) in &topics {
        match std::fs::read_to_string(path) {
            Ok(text) => match parse_frontmatter(&text) {
                Ok(description) => println!("{stem} — {description}"),
                Err(what) => {
                    report_malformed(path, &format!("malformed frontmatter: {what}"));
                    any_malformed = true;
                }
            },
            Err(err) => {
                report_malformed(path, &format!("could not be read: {err}"));
                any_malformed = true;
            }
        }
    }
    // One bad doc never hides the good ones (all listed above), but it does fail the run —
    // `ludion docs` in CI is what enforces the frontmatter grammar.
    if any_malformed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Runs the verb with a topic in checkout mode: prints `<dir>/<topic>.md`'s text — or its path
/// under `--path` — falling back to `set`'s embedded copy when no such file exists, and only then
/// raising [`L0009`](DOCS_UNKNOWN_TOPIC).
///
/// The fallback is the second half of the fix for mis-detected checkouts: a tighter
/// [`ENGINE_MARKER`] stops the misfire, and this makes any *residual* misfire recoverable instead of
/// fatal — the docs are in the binary either way, so refusing to read them because a directory scan
/// disagreed is a self-inflicted failure. `--path` cannot fall back: an embedded doc has no path.
/// A set with an empty embedded table simply has nothing to fall back to.
///
/// Frontmatter validity is deliberately irrelevant throughout: a malformed doc must still resolve.
/// No branch parses it.
fn lookup(
    set: &DocSet,
    root: &Path,
    topic: &str,
    want_path: bool,
    section: Option<&str>,
) -> ExitCode {
    let docs_dir = root.join(set.dir);
    let path = docs_dir.join(format!("{topic}.md"));
    if path.is_file() {
        if want_path {
            // Joined from the discovered root, never canonicalized — `fs::canonicalize` on Windows
            // prepends the `\\?\` verbatim prefix, which is noise for every consumer.
            println!("{}", path.display());
            return ExitCode::SUCCESS;
        }
        return match std::fs::read_to_string(&path) {
            // Byte-for-byte the emit `lookup_embedded` performs (via the shared `print_body`), so a
            // doc — whole or one `--section` — reads the same in either mode.
            Ok(text) => print_body(&text, topic, section),
            Err(err) => {
                // L0010 follows `list`'s precedent for an unreadable doc, but the help is written
                // for the IO failure — the frontmatter grammar is not the fix here.
                let diagnostic = Diagnostic::new(
                    DOCS_MALFORMED,
                    format!("`{}`: could not be read: {err}", path.display()),
                )
                .with_help(
                    "the file exists but could not be opened — check its permissions, or that it \
                     is a regular readable file",
                );
                eprintln!("{diagnostic}");
                ExitCode::FAILURE
            }
        };
    }
    if !want_path && let Some(body) = embedded_body(set, topic) {
        return print_body(body, topic, section);
    }
    // Suggest the nearest existing topic when the name is plausibly a typo. `topics` is sorted and
    // the embedded table is built sorted, so the suggestion is deterministic; a listing failure just
    // narrows the candidates to the embedded set.
    let mut stems: Vec<String> = topics(&docs_dir)
        .map(|topics| topics.into_iter().map(|(stem, _)| stem).collect())
        .unwrap_or_default();
    for (stem, _) in set.embedded {
        if !stems.iter().any(|s| s == stem) {
            stems.push((*stem).to_string());
        }
    }
    stems.sort();
    let mut help = String::new();
    if let Some(nearest) = suggest::nearest(topic, stems.iter().map(String::as_str)) {
        help.push_str(&format!("did you mean `{nearest}`? "));
    }
    help.push_str(&format!(
        "run `ludion {}` with no argument to list every topic; if the topic genuinely does \
         not exist yet, create `{}/{topic}.md` (the `ludion-docs` skill has the template)",
        set.verb, set.dir
    ));
    let diagnostic = Diagnostic::new(
        DOCS_UNKNOWN_TOPIC,
        format!(
            "no doc file for topic `{topic}` (`{}` does not exist)",
            path.display()
        ),
    )
    .with_help(help);
    eprintln!("{diagnostic}");
    ExitCode::FAILURE
}

/// The baked-in body of `topic` in `set`, or `None` if the binary carries no such doc (always so
/// for a checkout-only set).
fn embedded_body(set: &DocSet, topic: &str) -> Option<&'static str> {
    set.embedded
        .iter()
        .find(|(stem, _)| *stem == topic)
        .map(|(_, body)| *body)
}

/// Embedded-mode listing: the same `<topic> — <description>` UX as [`list`], read from the docs
/// baked into the binary. The embedded set is generated from the committed `docs/` tree, which the
/// repo-mode listing already gates for well-formed frontmatter, so a malformed entry here is a build
/// against a broken tree — still reported as [`L0010`](DOCS_MALFORMED) for consistency.
fn list_embedded(set: &DocSet) -> ExitCode {
    let mut any_malformed = false;
    for (stem, body) in set.embedded {
        match parse_frontmatter(body) {
            Ok(description) => println!("{stem} — {description}"),
            Err(what) => {
                report_malformed(
                    Path::new(&format!("{}/{stem}.md", set.dir)),
                    &format!("malformed frontmatter: {what}"),
                );
                any_malformed = true;
            }
        }
    }
    if any_malformed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Embedded-mode lookup: prints the doc text from the baked-in copy, or an
/// [`L0009`](DOCS_UNKNOWN_TOPIC) diagnostic with a nearest-match suggestion when no embedded topic
/// has that name. `--path` cannot be served here at all — nothing is on disk to name — so it is an
/// [`L0008`](DOCS_DIR_NOT_FOUND) regardless of whether the topic exists.
fn lookup_embedded(set: &DocSet, topic: &str, want_path: bool, section: Option<&str>) -> ExitCode {
    if want_path {
        let diagnostic = Diagnostic::new(
            DOCS_DIR_NOT_FOUND,
            format!(
                "`--path` needs the checkout's `{}/` directory, and this binary is running \
                 outside an engine checkout",
                set.dir
            ),
        )
        .with_help(
            "run the command from inside an engine checkout (any subdirectory works — discovery \
             walks up). Without `--path`, the doc text is served from the copies baked into the \
             binary at build time",
        );
        eprintln!("{diagnostic}");
        return ExitCode::FAILURE;
    }
    if let Some(body) = embedded_body(set, topic) {
        return print_body(body, topic, section);
    }
    let stems = set.embedded.iter().map(|(stem, _)| *stem);
    let mut help = String::new();
    if let Some(nearest) = suggest::nearest(topic, stems) {
        help.push_str(&format!("did you mean `{nearest}`? "));
    }
    help.push_str(&format!(
        "run `ludion {}` with no argument to list every topic; this binary is running outside \
         an engine checkout, so it serves the docs baked in at build time",
        set.verb
    ));
    let diagnostic = Diagnostic::new(
        DOCS_UNKNOWN_TOPIC,
        format!("no doc for topic `{topic}` in the embedded agent-docs"),
    )
    .with_help(help);
    eprintln!("{diagnostic}");
    ExitCode::FAILURE
}

/// Enumerates the topic docs — the `.md` files directly inside `docs_dir` — as `(stem, path)`
/// pairs, sorted by filename. The explicit sort matters: `read_dir` order is OS-dependent, and
/// deterministic output is a project rule.
fn topics(docs_dir: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut topics = Vec::new();
    for entry in std::fs::read_dir(docs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() || path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        topics.push((stem.to_string(), path));
    }
    topics.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(topics)
}

/// Prints `text` (a topic's full body) or, when `section` is given, just the section
/// [`extract_section`] finds under that heading. The single call site both `lookup` and
/// `lookup_embedded` route their body through, so `--section` behaves identically regardless of
/// which mode served the doc.
fn print_body(text: &str, topic: &str, section: Option<&str>) -> ExitCode {
    let Some(heading) = section else {
        print!("{text}");
        return ExitCode::SUCCESS;
    };
    match extract_section(text, heading) {
        Ok(body) => {
            print!("{body}");
            ExitCode::SUCCESS
        }
        Err(headings) => {
            report_unknown_section(topic, heading, &headings);
            ExitCode::FAILURE
        }
    }
}

/// One ATX heading found while scanning a doc for `--section`, carried back on a miss so the
/// caller can build a nearest-match suggestion and a top-level listing.
struct DocHeading {
    level: u8,
    text: String,
}

/// Extracts the text under the first ATX heading (`#{1,6} `) whose trimmed text equals `heading`,
/// case-insensitively — from that heading through everything up to, but excluding, the next heading
/// of equal or higher level (or the end of the text). First match wins if a heading's text repeats.
///
/// Fenced code blocks (opened/closed by a line whose trimmed text starts with ` ``` ` or `~~~`) are
/// skipped while scanning for headings: the engine guide's YAML examples contain `#`-prefixed
/// comment lines (e.g. `# ...palette entry A...`) that would otherwise misread as headings.
///
/// On a miss, returns every heading actually found, in document order, so the caller can suggest
/// the nearest one and list the doc's top-level headings — never an empty diagnostic naming nothing
/// to try instead.
fn extract_section(text: &str, heading: &str) -> Result<String, Vec<DocHeading>> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut levels: Vec<u8> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut line_starts: Vec<usize> = Vec::new();
    let mut fence: Option<&'static str> = None;
    for (i, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim_end_matches(['\n', '\r']).trim_start();
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            fence = Some("```");
            continue;
        }
        if trimmed.starts_with("~~~") {
            fence = Some("~~~");
            continue;
        }
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes)
            && let Some(rest) = trimmed[hashes..].strip_prefix(' ')
        {
            levels.push(hashes as u8);
            texts.push(rest.trim().to_string());
            line_starts.push(i);
        }
    }

    let target = heading.trim();
    let Some(hit) = texts.iter().position(|t| t.eq_ignore_ascii_case(target)) else {
        return Err(levels
            .into_iter()
            .zip(texts)
            .map(|(level, text)| DocHeading { level, text })
            .collect());
    };
    let hit_level = levels[hit];
    let start = line_starts[hit];
    let end = (hit + 1..levels.len())
        .find(|&j| levels[j] <= hit_level)
        .map(|j| line_starts[j])
        .unwrap_or(lines.len());
    Ok(lines[start..end].concat())
}

/// The doc's headings for `--section`'s "here's what exists" help — the level actually used for
/// navigation, not the singular `# Title` line every topic doc opens with. Takes the minimum
/// heading level that has more than one heading at it; falls back to the true minimum when a doc
/// genuinely has only one heading overall (nothing to distinguish).
fn top_level_headings(headings: &[DocHeading]) -> Vec<&str> {
    let Some(min_level) = headings.iter().map(|h| h.level).min() else {
        return Vec::new();
    };
    let at_min: Vec<&str> = headings
        .iter()
        .filter(|h| h.level == min_level)
        .map(|h| h.text.as_str())
        .collect();
    if at_min.len() > 1 {
        return at_min;
    }
    match headings
        .iter()
        .map(|h| h.level)
        .filter(|&l| l > min_level)
        .min()
    {
        Some(next_level) => headings
            .iter()
            .filter(|h| h.level == next_level)
            .map(|h| h.text.as_str())
            .collect(),
        None => at_min,
    }
}

/// Prints one [`L0015`](DOCS_UNKNOWN_SECTION) diagnostic: no heading in `topic` matched `heading`.
/// The help carries a nearest-match suggestion over every heading found, plus the topic's
/// top-level headings so an agent can pick a real one without re-fetching the whole doc.
fn report_unknown_section(topic: &str, heading: &str, headings: &[DocHeading]) {
    let mut help = String::new();
    if let Some(nearest) = suggest::nearest(heading, headings.iter().map(|h| h.text.as_str())) {
        help.push_str(&format!("did you mean `{nearest}`? "));
    }
    let top = top_level_headings(headings);
    if top.is_empty() {
        help.push_str(&format!(
            "topic `{topic}` has no ATX headings to select a section from"
        ));
    } else {
        help.push_str(&format!(
            "`{topic}`'s top-level headings: {}",
            top.join(", ")
        ));
    }
    let diagnostic = Diagnostic::new(
        DOCS_UNKNOWN_SECTION,
        format!("no `{heading}` heading in topic `{topic}`"),
    )
    .with_help(help);
    eprintln!("{diagnostic}");
}

/// Prints one [`L0010`](DOCS_MALFORMED) diagnostic naming the offending doc file and precisely
/// what is wrong with it.
fn report_malformed(path: &Path, what: &str) {
    let diagnostic = Diagnostic::new(DOCS_MALFORMED, format!("`{}`: {what}", path.display()))
        .with_help(
            "frontmatter is exactly three lines — `---`, `description: <one line, <=100 chars>`, \
             `---` — and `description` is the only allowed key; see `ludion explain L0010`",
        );
    eprintln!("{diagnostic}");
}

/// Parses the strict three-line frontmatter at the top of a topic doc, returning its
/// `description` value.
///
/// The grammar is deliberately rigid (see L0010): line 1 is exactly `---`, line 2 is
/// `description: <value>` where the value is non-empty, single-line, and at most 100 characters
/// (`description` is the only allowed key), line 3 is exactly `---`. Anything else is an `Err`
/// naming what is wrong. Hand-parsed on purpose — three fixed lines do not justify a YAML
/// dependency.
fn parse_frontmatter(text: &str) -> Result<String, String> {
    // `str::lines` splits on `\n` and strips a trailing `\r`, so CRLF checkouts parse the same.
    let mut lines = text.lines();

    match lines.next() {
        Some("---") => {}
        Some(other) => {
            return Err(format!(
                "line 1 must be exactly the opening `---` fence, found `{other}`"
            ));
        }
        None => return Err("the file is empty — expected `---` frontmatter on line 1".to_string()),
    }

    let description = match lines.next() {
        Some(line) => match line.strip_prefix("description:") {
            Some(value) => {
                let value = value.trim();
                if value.is_empty() {
                    return Err("the `description:` value is empty".to_string());
                }
                let len = value.chars().count();
                if len > 100 {
                    return Err(format!(
                        "the description is {len} characters long — the limit is 100"
                    ));
                }
                value.to_string()
            }
            None => {
                // Name the offending key when line 2 at least looks like `key: value`.
                return Err(match line.split_once(':') {
                    Some((key, _)) => format!(
                        "line 2 uses the key `{}` — `description` is the only allowed key",
                        key.trim()
                    ),
                    None => format!("line 2 must be `description: <value>`, found `{line}`"),
                });
            }
        },
        None => {
            return Err(
                "the frontmatter ends after the opening `---` — expected a `description:` line"
                    .to_string(),
            );
        }
    };

    match lines.next() {
        Some("---") => Ok(description),
        Some(other) => Err(format!(
            "line 3 must be exactly the closing `---` fence, found `{other}` — the description \
             is a single line and `description` is the only allowed key"
        )),
        None => Err("the frontmatter is missing its closing `---` fence on line 3".to_string()),
    }
}
