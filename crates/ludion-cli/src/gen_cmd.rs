//! Implementation of `ludion gen <nodes|check|run|status|fetch|sheet|accept>` — a thin CLI over
//! `ludion-gen` (`Plan/comfy-generation.md`).
//!
//! Every verb that contacts a server takes `--url` (resolved through [`ludion_gen::resolve_url`], so
//! `--url` beats `LUDION_COMFY_URL` beats ComfyUI's own loopback default); `sheet` and `accept`
//! never contact one and take none. Every verb takes `--json`, which prints exactly one compact
//! JSON object on one stdout line, on success and on failure: `{ok, …the verb's fields,
//! diagnostics}` with `ok` true exactly when the exit code is 0. Diagnostics always render to
//! stderr through [`crate::diagnostic`], the same renderer `check-scene` and `import` use, and
//! `diagnostics` holds what was rendered.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, Severity};
use ludion_gen::Accepted;
use ludion_gen::run::display_path;
use ludion_scene::{GenerationDocument, InputValue, Origin, ScalarValue};
use serde_json::{Map, Value, json};

use crate::diagnostic;

/// What one verb has said so far: the diagnostics it rendered to stderr, in order, which
/// [`Reporter::finish`] puts in the `--json` object.
struct Reporter {
    json: bool,
    diagnostics: Vec<Diagnostic>,
}

impl Reporter {
    fn new(json: bool) -> Self {
        Reporter {
            json,
            diagnostics: Vec::new(),
        }
    }

    fn print(&mut self, diagnostic: &Diagnostic) {
        diagnostic::print(diagnostic);
        self.diagnostics.push(diagnostic.clone());
    }

    fn print_all(&mut self, diagnostics: &[Diagnostic]) {
        for diagnostic in diagnostics {
            self.print(diagnostic);
        }
    }

    /// Ends the verb: under `--json`, prints the one object (`fields` between `ok` and
    /// `diagnostics`); the exit code is success exactly when `ok`.
    fn finish(self, ok: bool, fields: Value) -> ExitCode {
        if self.json {
            print_json(&Value::Object(diagnostic::json_envelope(
                ok,
                fields,
                &self.diagnostics,
            )));
        }
        if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
}

// --- gen nodes --------------------------------------------------------------------------------

/// Runs `ludion gen nodes [filter] [--class C] [--url] [--json]`.
pub fn run_nodes(
    filter: Option<&str>,
    class: Option<&str>,
    url: Option<&str>,
    json: bool,
) -> ExitCode {
    let mut out = Reporter::new(json);
    let none = || json!({ "nodes": Value::Null });
    let server_url = match ludion_gen::resolve_url(url) {
        Ok(url) => url,
        Err(diag) => {
            out.print(&diag);
            return out.finish(false, none());
        }
    };
    let object_info = match ludion_gen::Client::new(&server_url).object_info() {
        Ok(value) => value,
        Err(diag) => {
            out.print(&diag);
            return out.finish(false, none());
        }
    };
    let Some(classes) = object_info.as_object() else {
        out.print(
            &Diagnostic::new(
                code::COMFY_REPLY_MALFORMED,
                "the server's `/object_info` reply is not an object of node classes",
            )
            .with_help(format!(
                "check that `{server_url}` is a ComfyUI server (`LUDION_COMFY_URL` or `--url`)"
            )),
        );
        return out.finish(false, none());
    };

    let mut matched: Vec<(&String, Value)> = classes
        .iter()
        .filter(|(name, spec)| matches_selection(name, spec, filter, class))
        .map(|(name, spec)| (name, describe_node(spec)))
        .collect();
    matched.sort_by(|a, b| a.0.cmp(b.0));

    if let Some(class) = class
        && matched.is_empty()
    {
        let installed: Vec<&str> = classes.keys().map(String::as_str).collect();
        let mut diag = Diagnostic::new(
            code::WORKFLOW_UNKNOWN_NODE,
            format!("no node class named `{class}` on this server"),
        );
        if let Some(near) = ludion_core::suggest::nearest(class, installed) {
            diag = diag.with_help(format!("nearest installed class is `{near}`"));
        }
        out.print(&diag);
        return out.finish(false, none());
    }

    let mut nodes = Value::Null;
    if json {
        nodes = matched
            .iter()
            .map(|(name, desc)| node_json(name, desc))
            .collect();
    } else if class.is_some() {
        for (name, desc) in &matched {
            print!("{}", class_table(name, desc));
        }
    } else {
        for (name, desc) in &matched {
            println!("{}", compact_line(name, desc));
        }
    }
    out.finish(true, json!({ "nodes": nodes }))
}

/// Whether class `name` (with its `/object_info` `spec`) is selected by `--class` (exact match) or
/// `filter` (a case-insensitive substring over the class name or its category).
fn matches_selection(name: &str, spec: &Value, filter: Option<&str>, class: Option<&str>) -> bool {
    if let Some(class) = class {
        return name == class;
    }
    let Some(filter) = filter else {
        return true;
    };
    let filter = filter.to_ascii_lowercase();
    if name.to_ascii_lowercase().contains(&filter) {
        return true;
    }
    let category = spec.get("category").and_then(Value::as_str).unwrap_or("");
    category.to_ascii_lowercase().contains(&filter)
}

/// One `/object_info` class turned into `{category, required, optional, outputs}` — the shape both
/// the compact listing, the full table and `--json` read from, so all three describe one class the
/// same way.
fn describe_node(spec: &Value) -> Value {
    json!({
        "category": spec.get("category").and_then(Value::as_str).unwrap_or(""),
        "required": describe_inputs(spec, "required"),
        "optional": describe_inputs(spec, "optional"),
        "outputs": spec.get("output").and_then(Value::as_array).cloned().unwrap_or_default(),
    })
}

/// The `required` or `optional` half of a class's `input` object, each entry turned into
/// `{type, default?, options?}`.
fn describe_inputs(spec: &Value, section: &str) -> Value {
    let mut out = Map::new();
    if let Some(inputs) = spec
        .get("input")
        .and_then(|input| input.get(section))
        .and_then(Value::as_object)
    {
        for (name, entry) in inputs {
            out.insert(name.clone(), describe_input(entry));
        }
    }
    Value::Object(out)
}

/// One input entry: ComfyUI spells it `[type_or_options, meta]`, where the first element is either
/// a type name (`"INT"`, `"MODEL"`, …) or, for an inline combo, an array of option values. A
/// `meta.default` and `meta.options` (the named-combo spelling) are carried across when present.
fn describe_input(entry: &Value) -> Value {
    let Some(pair) = entry.as_array() else {
        return json!({ "type": entry.clone() });
    };
    let type_spec = pair.first().cloned().unwrap_or(Value::Null);
    let meta = pair.get(1).cloned().unwrap_or(Value::Null);

    let (type_name, options) = match &type_spec {
        Value::Array(options) => ("COMBO".to_string(), Some(options.clone())),
        Value::String(name) => (
            name.clone(),
            meta.get("options").and_then(Value::as_array).cloned(),
        ),
        other => (other.to_string(), None),
    };

    let mut out = Map::new();
    out.insert("type".to_string(), Value::from(type_name));
    if let Some(default) = meta.get("default") {
        out.insert("default".to_string(), default.clone());
    }
    if let Some(options) = options {
        out.insert("options".to_string(), Value::Array(options));
    }
    Value::Object(out)
}

/// One `--json` node object: `desc` plus its class name.
fn node_json(class: &str, desc: &Value) -> Value {
    json!({
        "class": class,
        "category": desc["category"],
        "required": desc["required"],
        "optional": desc["optional"],
        "outputs": desc["outputs"],
    })
}

/// The compact one-line-per-class listing: `Class  [category]  required: a: INT, b: STRING…
/// optional: …  -> OUTPUT, OUTPUT`.
fn compact_line(class: &str, desc: &Value) -> String {
    let category = desc["category"].as_str().unwrap_or("");
    let mut line = format!("{class}  [{category}]");
    let required = input_names(&desc["required"]);
    if !required.is_empty() {
        line.push_str(&format!("  required: {required}"));
    }
    let optional = input_names(&desc["optional"]);
    if !optional.is_empty() {
        line.push_str(&format!("  optional: {optional}"));
    }
    let outputs = output_names(&desc["outputs"]);
    if !outputs.is_empty() {
        line.push_str(&format!("  -> {outputs}"));
    }
    line
}

/// `name: TYPE, name: TYPE, …`, sorted by name — one section of a [`compact_line`].
fn input_names(section: &Value) -> String {
    let Some(inputs) = section.as_object() else {
        return String::new();
    };
    let mut names: Vec<&String> = inputs.keys().collect();
    names.sort();
    names
        .into_iter()
        .map(|name| format!("{name}: {}", inputs[name]["type"].as_str().unwrap_or("?")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `OUTPUT, OUTPUT, …` from a class's `output` array.
fn output_names(outputs: &Value) -> String {
    let Some(outputs) = outputs.as_array() else {
        return String::new();
    };
    outputs
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `--class C`'s full input table: one row per input with its type, default and enum options.
fn class_table(class: &str, desc: &Value) -> String {
    let category = desc["category"].as_str().unwrap_or("");
    let mut out = format!("{class}  [{category}]\nrequired:\n");
    out.push_str(&input_rows(&desc["required"]));
    out.push_str("optional:\n");
    out.push_str(&input_rows(&desc["optional"]));
    let outputs = output_names(&desc["outputs"]);
    out.push_str(&format!(
        "outputs: {}\n",
        if outputs.is_empty() {
            "(none)"
        } else {
            &outputs
        }
    ));
    out
}

fn input_rows(section: &Value) -> String {
    let Some(inputs) = section.as_object() else {
        return "  (none)\n".to_string();
    };
    if inputs.is_empty() {
        return "  (none)\n".to_string();
    }
    let mut names: Vec<&String> = inputs.keys().collect();
    names.sort();
    let mut out = String::new();
    for name in names {
        let entry = &inputs[name];
        let ty = entry["type"].as_str().unwrap_or("?");
        let mut row = format!("  {name}: {ty}");
        if let Some(default) = entry.get("default") {
            row.push_str(&format!(" (default {default})"));
        }
        if let Some(options) = entry.get("options").and_then(Value::as_array) {
            let options: Vec<String> = options.iter().map(value_display).collect();
            row.push_str(&format!(" [{}]", options.join(", ")));
        }
        out.push_str(&row);
        out.push('\n');
    }
    out
}

/// A JSON scalar as a plain reading — a string without its quotes, a number or bool as-is.
fn value_display(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// --- gen check --------------------------------------------------------------------------------

/// Runs `ludion gen check <recipe> [--url] [--json]`.
///
/// The offline pass (document, paths, workflow format already checked by
/// [`ludion_gen::recipe::resolve`], then titles via [`ludion_gen::workflow::apply`] with a
/// placeholder upload map, then [`ludion_gen::workflow::OutputKind`] via `graph.output_kind()`)
/// decides the exit code; the live pass against `/object_info` is reported but never changes it, so
/// CI can run the offline half with no server at all. An unreachable server degrades its `L1167` to
/// a warning, printed after the offline result.
pub fn run_check(recipe: &Path, url: Option<&str>, json: bool) -> ExitCode {
    let mut out = Reporter::new(json);
    let resolved = match ludion_gen::resolve(recipe) {
        Ok(resolved) => resolved,
        Err(diagnostics) => {
            out.print_all(&diagnostics);
            return out.finish(
                false,
                json!({
                    "recipe": display_path(recipe),
                    "workflow": Value::Null,
                    "kind": Value::Null,
                    "overrides": Value::Null,
                    "offline": "error",
                    "live": "skipped",
                }),
            );
        }
    };

    let mut offline_diagnostics = Vec::new();
    let placeholder_uploads = placeholder_uploads(&resolved.doc);
    if let Err(diags) =
        ludion_gen::workflow::apply(&resolved.graph, &resolved.doc, &placeholder_uploads)
    {
        offline_diagnostics.extend(diags);
    }
    let kind = match resolved.graph.output_kind() {
        Ok(kind) => Some(kind),
        Err(diag) => {
            offline_diagnostics.push(diag);
            None
        }
    };
    offline_diagnostics.extend(ludion_gen::run::identical_candidates(
        &resolved,
        resolved.doc.count,
    ));
    out.print_all(&offline_diagnostics);
    let offline_ok = !offline_diagnostics
        .iter()
        .any(|diag| diag.severity.is_error());

    let overrides = override_table(&resolved.doc);
    if !json {
        println!(
            "kind: {}",
            kind.map(ludion_gen::OutputKind::as_str).unwrap_or("?")
        );
        print!("{}", render_overrides(&resolved.doc));
    }

    let live = match ludion_gen::resolve_url(url)
        .and_then(|server| ludion_gen::Client::new(&server).object_info())
    {
        Ok(object_info) => {
            let diags =
                ludion_gen::workflow::validate(&resolved.graph, &resolved.doc, &object_info);
            out.print_all(&diags);
            if diags.iter().any(|diag| diag.severity.is_error()) {
                "error"
            } else {
                "ok"
            }
        }
        Err(diag) if diag.code == code::COMFY_UNREACHABLE => {
            out.print(&diag.with_severity(Severity::Warning));
            "skipped"
        }
        Err(diag) => {
            out.print(&diag);
            "error"
        }
    };

    if !json {
        println!("offline: {}", if offline_ok { "ok" } else { "failed" });
        println!("live: {}", live_text(live));
    }

    out.finish(
        offline_ok,
        json!({
            "recipe": display_path(recipe),
            "workflow": display_path(&resolved.workflow_path),
            "kind": kind.map(ludion_gen::OutputKind::as_str),
            "overrides": overrides,
            "offline": if offline_ok { "ok" } else { "error" },
            "live": live,
        }),
    )
}

/// The live pass's `"ok"`/`"error"`/`"skipped"` verdict as the text form spells it: `error` reads
/// `failed`, and a skipped pass names the code it degraded from, so a reader is never left guessing
/// whether the server was even asked.
fn live_text(live: &str) -> String {
    match live {
        "ok" => "ok".to_string(),
        "skipped" => format!("skipped ({})", code::COMFY_UNREACHABLE),
        _ => "failed".to_string(),
    }
}

/// A placeholder `images:` upload map: every declared image name mapped to its own file name (or,
/// failing that, the name itself), so [`ludion_gen::workflow::apply`] can resolve an `ImageRef`
/// override offline — an `images:` entry has already been checked to exist on disk by
/// [`ludion_gen::recipe::resolve`], so this never claims a server has anything, only that the name
/// resolves to *some* string, which is all title/type checking needs.
fn placeholder_uploads(doc: &GenerationDocument) -> BTreeMap<String, String> {
    doc.images
        .iter()
        .map(|(name, path)| {
            let placeholder = Path::new(path)
                .file_name()
                .and_then(|file_name| file_name.to_str())
                .map(str::to_string)
                .unwrap_or_else(|| name.clone());
            (name.clone(), placeholder)
        })
        .collect()
}

/// The recipe's `inputs:` as `{title: {input: value}}`, the JSON half of the override table.
fn override_table(doc: &GenerationDocument) -> Value {
    ludion_gen::inputs_to_json(&doc.inputs)
}

/// The plain-text override table: one indented `input: value` line per title.
fn render_overrides(doc: &GenerationDocument) -> String {
    if doc.inputs.is_empty() {
        return "overrides: (none)\n".to_string();
    }
    let mut out = String::from("overrides:\n");
    for (title, entries) in &doc.inputs {
        out.push_str(&format!("  {title}:\n"));
        for (input, value) in entries {
            out.push_str(&format!(
                "    {input}: {}\n",
                input_value_display(value, &doc.images)
            ));
        }
    }
    out
}

/// One override value on one line. An image reference is the only structured value a recipe can
/// spell, and its own key is `image`, so the document's nested spelling prints as `image: image:
/// subject`; here it reads as one value — the reference, and the file it resolves to.
fn input_value_display(value: &InputValue, images: &[(String, String)]) -> String {
    match value {
        InputValue::Scalar(ScalarValue::Bool(b)) => b.to_string(),
        InputValue::Scalar(ScalarValue::Integer(i)) => i.to_string(),
        InputValue::Scalar(ScalarValue::Number(n)) => n.to_string(),
        InputValue::Scalar(ScalarValue::Text(text)) => text.clone(),
        InputValue::ImageRef(name) => match images
            .iter()
            .find(|(declared, _)| declared == name)
            .map(|(_, path)| path)
        {
            Some(path) => format!("images.{name} -> {path}"),
            None => format!("images.{name}"),
        },
    }
}

// --- gen run ----------------------------------------------------------------------------------

/// Runs `ludion gen run <recipe> [--count] [--seed] [--image] [--timeout] [--no-wait] [--url]
/// [--json]`.
#[allow(clippy::too_many_arguments)]
pub fn run_run(
    recipe: &Path,
    count: Option<u32>,
    seed: Option<u64>,
    image: Option<&Path>,
    timeout: Option<u64>,
    no_wait: bool,
    url: Option<&str>,
    json: bool,
) -> ExitCode {
    let mut out = Reporter::new(json);
    let server_url = match ludion_gen::resolve_url(url) {
        Ok(url) => url,
        Err(diag) => {
            out.print(&diag);
            return out.finish(false, run_fields(None));
        }
    };
    let resolved = match ludion_gen::resolve(recipe) {
        Ok(resolved) => resolved,
        Err(diagnostics) => {
            out.print_all(&diagnostics);
            return out.finish(false, run_fields(None));
        }
    };

    if let Some(diag) =
        ludion_gen::run::identical_candidates(&resolved, count.unwrap_or(resolved.doc.count))
    {
        out.print(&diag);
    }

    let opts = ludion_gen::RunOptions {
        count,
        seed,
        image: image.map(Path::to_path_buf),
        timeout: timeout
            .map(Duration::from_secs)
            .unwrap_or(ludion_gen::run::DEFAULT_TIMEOUT),
        wait: !no_wait,
        url: server_url,
    };

    match ludion_gen::run(&resolved, &opts) {
        Ok(report) => {
            print_failures(&mut out, &report);
            if !json {
                if opts.wait {
                    print_run_report(&report);
                } else {
                    print_submitted(&report);
                }
                println!("{}", next_hint(&report, no_wait));
            }
            out.finish(run_ok(&report), run_report_json(&report))
        }
        Err(error) => {
            out.print_all(&error.diagnostics);
            out.finish(false, run_fields(error.dir.as_deref()))
        }
    }
}

/// A [`ludion_gen::RunReport`] as `--json` prints it: the report's own fields plus `dir`, which the
/// struct itself never serializes (its own folder is not a fact the file inside it should record).
fn run_report_json(report: &ludion_gen::RunReport) -> Value {
    let mut value = serde_json::to_value(report).unwrap_or(Value::Null);
    if let Value::Object(object) = &mut value {
        object.insert("dir".to_string(), Value::from(display_path(&report.dir)));
    }
    value
}

/// What `run` and `fetch` put in the `--json` object when they have no report to print: the
/// folder's `run.json` keys plus `dir` when it holds a readable one, else just `dir`, `null` when
/// the run was refused before its folder existed.
fn run_fields(dir: Option<&Path>) -> Value {
    let Some(dir) = dir else {
        return json!({ "dir": Value::Null });
    };
    match ludion_gen::run::read_report(dir) {
        Ok(report) => run_report_json(&report),
        Err(_) => json!({ "dir": display_path(dir) }),
    }
}

/// One `L1169` per failed or lost job on stderr, before the report on stdout.
fn print_failures(out: &mut Reporter, report: &ludion_gen::RunReport) {
    out.print_all(&report.failure_diagnostics());
}

/// Success is a run whose jobs all produced output, or one just submitted; `partial` and `failed`
/// exit 1 with the report already printed.
fn run_ok(report: &ludion_gen::RunReport) -> bool {
    use ludion_gen::run::{STATUS_OK, STATUS_SUBMITTED};
    report.status == STATUS_OK || report.status == STATUS_SUBMITTED
}

/// The literal follow-up command a non-`--json` `gen run` ends its text output on: `gen fetch` while
/// the jobs are still on the server, `gen run <recipe>` for a run with no candidate, `gen accept …
/// --out <dir>` for a mesh run (`gen sheet` refuses one with `L1171`), or `gen sheet` otherwise.
fn next_hint(report: &ludion_gen::RunReport, no_wait: bool) -> String {
    let dir = display_path(&report.dir);
    if no_wait {
        format!("next: ludion gen fetch {dir}")
    } else if report.status == ludion_gen::run::STATUS_FAILED {
        format!("next: ludion gen run {}", report.recipe)
    } else if report.kind == ludion_gen::OutputKind::Mesh.as_str() {
        format!("next: ludion gen accept {dir} 0 --out <dir>")
    } else {
        format!("next: ludion gen sheet {dir}")
    }
}

/// A finished (`gen run`, waited) or completed (`gen fetch`) run: the folder, what it produced, and
/// how long it took.
fn print_run_report(report: &ludion_gen::RunReport) {
    println!("{}", display_path(&report.dir));
    println!("kind: {}", report.kind);
    println!("count: {}", report.count);
    println!("status: {}", report.status);
    for output in &report.outputs {
        println!(
            "#{} seed {} {}",
            output.index,
            ludion_gen::run::seed_text(output.seed),
            output.file
        );
    }
    for failure in &report.failed {
        println!(
            "job {} seed {} {}",
            failure.job,
            ludion_gen::run::seed_text(report.job_seed(failure.job as usize)),
            failure.reason
        );
    }
    println!("duration: {} ms", report.duration_ms);
}

/// A `--no-wait` run: the folder and the job ids `gen status`/`gen fetch` will need.
fn print_submitted(report: &ludion_gen::RunReport) {
    println!("{}", display_path(&report.dir));
    for id in &report.prompt_ids {
        println!("job {id}");
    }
}

// --- gen status ---------------------------------------------------------------------------------

/// Runs `ludion gen status [<run-dir>] [--url] [--json]`.
pub fn run_status(run_dir: Option<&Path>, url: Option<&str>, json: bool) -> ExitCode {
    let mut out = Reporter::new(json);
    let none = || match run_dir {
        Some(dir) => json!({
            "dir": display_path(dir),
            "report": ludion_gen::run::read_report(dir)
                .map_or(Value::Null, |report| run_report_json(&report)),
            "ready": Value::Null,
            "pending": Value::Null,
            "failed": Value::Null,
            "queue": Value::Null,
        }),
        None => json!({ "queue": Value::Null, "runs": Value::Null }),
    };
    let server_url = match ludion_gen::resolve_url(url) {
        Ok(url) => url,
        Err(diag) => {
            out.print(&diag);
            return out.finish(false, none());
        }
    };

    match run_dir {
        Some(run_dir) => match ludion_gen::status(run_dir, &server_url) {
            Ok(status) => {
                if !json {
                    if status.ready {
                        println!("{}: ready (nothing pending)", status.report.run_id);
                    } else {
                        println!(
                            "{}: not ready, pending: {}",
                            status.report.run_id,
                            status.pending.join(", ")
                        );
                    }
                    for failure in &status.failed {
                        println!(
                            "job {} {}: {}",
                            failure.job,
                            failure.reason,
                            failure.summary()
                        );
                    }
                }
                out.finish(
                    true,
                    json!({
                        "dir": display_path(run_dir),
                        "report": run_report_json(&status.report),
                        "ready": status.ready,
                        "pending": status.pending,
                        "failed": status.failed,
                        "queue": status.queue,
                    }),
                )
            }
            Err(diagnostics) => {
                out.print_all(&diagnostics);
                out.finish(false, none())
            }
        },
        None => {
            let queue = match ludion_gen::Client::new(&server_url).queue() {
                Ok(queue) => queue,
                Err(diag) => {
                    out.print(&diag);
                    return out.finish(false, none());
                }
            };
            let runs = latest_runs(&find_project_root());
            if !json {
                let (running, pending) = queue_counts(&queue);
                println!("queue: {running} running, {pending} pending");
                for run in &runs {
                    println!("{}: {} {}", run.recipe, run.run_id, run.status);
                }
            }
            let runs_json: Vec<Value> = runs
                .iter()
                .map(|run| {
                    json!({
                        "recipe": run.recipe,
                        "run_id": run.run_id,
                        "status": run.status,
                    })
                })
                .collect();
            out.finish(true, json!({ "queue": queue, "runs": runs_json }))
        }
    }
}

/// `queue_running`/`queue_pending` array lengths from a raw `/queue` reply.
fn queue_counts(queue: &Value) -> (usize, usize) {
    let count = |key: &str| queue.get(key).and_then(Value::as_array).map_or(0, Vec::len);
    (count("queue_running"), count("queue_pending"))
}

/// One recipe's most recent run, for the bare `gen status`.
struct LatestRun {
    recipe: String,
    run_id: String,
    status: String,
}

/// The most recent run folder under every `generation/runs/<recipe>/` beneath `project_root`,
/// sorted by recipe name. The newest run is the greatest id by `ludion_gen::run::run_order`.
fn latest_runs(project_root: &Path) -> Vec<LatestRun> {
    let runs_dir = project_root.join(ludion_gen::run::RUNS_DIR);
    let Ok(recipes) = std::fs::read_dir(&runs_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut recipe_dirs: Vec<PathBuf> = recipes
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    recipe_dirs.sort();
    for recipe_dir in recipe_dirs {
        let Some(recipe) = recipe_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&recipe_dir) else {
            continue;
        };
        let latest = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .max_by(|a, b| {
                let name = |path: &PathBuf| {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                };
                ludion_gen::run::run_order(&name(a), &name(b))
            });
        let Some(latest) = latest else { continue };
        if let Ok(report) = ludion_gen::run::read_report(&latest) {
            out.push(LatestRun {
                recipe,
                run_id: report.run_id,
                status: report.status,
            });
        }
    }
    out
}

/// The nearest ancestor of the current directory holding a `generation/` folder, or the current
/// directory itself when none does — the same root a recipe's own `Resolved::project_root` walks
/// up to, but starting from `cwd` rather than a recipe path.
fn find_project_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut dir = cwd.as_path();
    loop {
        if dir.join(ludion_gen::recipe::GENERATION_DIR).is_dir() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return cwd,
        }
    }
}

// --- gen fetch ----------------------------------------------------------------------------------

/// Runs `ludion gen fetch <run-dir> [--url] [--json]`.
pub fn run_fetch(run_dir: &Path, url: Option<&str>, json: bool) -> ExitCode {
    let mut out = Reporter::new(json);
    let server_url = match ludion_gen::resolve_url(url) {
        Ok(url) => url,
        Err(diag) => {
            out.print(&diag);
            return out.finish(false, run_fields(Some(run_dir)));
        }
    };
    match ludion_gen::fetch(run_dir, &server_url) {
        Ok(report) => {
            print_failures(&mut out, &report);
            if !json {
                print_run_report(&report);
            }
            out.finish(run_ok(&report), run_report_json(&report))
        }
        Err(diagnostics) => {
            out.print_all(&diagnostics);
            out.finish(false, run_fields(Some(run_dir)))
        }
    }
}

// --- gen sheet ----------------------------------------------------------------------------------

/// Runs `ludion gen sheet <run-dir> [--cols] [--out] [--json]`.
pub fn run_sheet(run_dir: &Path, cols: Option<u32>, out: Option<&Path>, json: bool) -> ExitCode {
    let mut reporter = Reporter::new(json);
    let report = ludion_gen::run::read_report(run_dir);
    let run_id = report.as_ref().ok().map(|report| report.run_id.clone());
    match ludion_gen::sheet::compose(run_dir, cols, out) {
        Ok(path) => {
            let cells = report.map(|report| report.outputs.len()).unwrap_or(0);
            if !json {
                println!(
                    "{} ({cells} cell{})",
                    display_path(&path),
                    if cells == 1 { "" } else { "s" }
                );
            }
            reporter.finish(
                true,
                json!({
                    "run_id": run_id.unwrap_or_default(),
                    "sheet": display_path(&path),
                    "cells": cells,
                }),
            )
        }
        Err(diagnostics) => {
            reporter.print_all(&diagnostics);
            reporter.finish(
                false,
                json!({
                    "run_id": run_id,
                    "sheet": Value::Null,
                    "cells": Value::Null,
                }),
            )
        }
    }
}

// --- gen accept ---------------------------------------------------------------------------------

/// Runs `ludion gen accept <run-dir> <index> --out <path> [--allow-lossy] [--json]`.
///
/// A mesh candidate goes through the import crate in-process, so `--out` is the import directory
/// and the registry the imported scene is emitted through is the built-in one every other CLI
/// command validates against. `allow_lossy` is that import's mode, spelled as the `import` verb
/// spells it, because an `L1202` on a generated `.glb` has no re-export to send anyone back to.
pub fn run_accept(
    run_dir: &Path,
    index: u32,
    out: &Path,
    allow_lossy: bool,
    json: bool,
) -> ExitCode {
    let mut reporter = Reporter::new(json);
    let registry = crate::check_scene::builtin_registry();
    match ludion_gen::accept::accept(run_dir, index, out, &registry, allow_lossy) {
        Ok(accepted) => {
            // The importer's own diagnostics, `L1112` among them: a warning about density, never a
            // reason the accept did not happen.
            for warning in accepted.mesh.iter().flat_map(|mesh| &mesh.warnings) {
                reporter.print(warning);
            }
            if !json {
                print_accepted(&accepted);
            }
            reporter.finish(true, accept_json(&accepted))
        }
        Err(diagnostics) => {
            reporter.print_all(&diagnostics);
            reporter.finish(
                false,
                json!({
                    "source": Value::Null,
                    "destination": Value::Null,
                    "sidecar": Value::Null,
                    "origin": Value::Null,
                }),
            )
        }
    }
}

/// An [`Accepted`] as `--json` prints it: one object per accept, carrying `source`, `destination`,
/// `sidecar` and `origin` whatever the candidate was, plus `meshes` and `textures` when a mesh
/// import wrote a tree instead of one file.
fn accept_json(accepted: &Accepted) -> Value {
    let mut object = json!({
        "source": display_path(&accepted.source),
        "destination": display_path(&accepted.destination),
        "sidecar": accepted.sidecar.as_deref().map(display_path),
        "origin": origin_json(&accepted.origin),
    });
    if let (Some(mesh), Value::Object(fields)) = (&accepted.mesh, &mut object) {
        let documents: Vec<Value> = mesh
            .meshes
            .iter()
            .map(|doc| {
                json!({
                    "path": display_path(&doc.path),
                    "vertices": doc.vertices,
                    "triangles": doc.triangles,
                })
            })
            .collect();
        let textures: Vec<String> = mesh
            .textures
            .iter()
            .map(|path| display_path(path))
            .collect();
        fields.insert("meshes".to_string(), Value::Array(documents));
        fields.insert("textures".to_string(), Value::from(textures));
    }
    object
}

/// The text form: the destination first, then whatever else the accept wrote — a sidecar for an
/// image, or the mesh documents with their counts and the textures beside them for a mesh.
fn print_accepted(accepted: &Accepted) {
    println!("{}", display_path(&accepted.destination));
    if let Some(sidecar) = &accepted.sidecar {
        println!("{}", display_path(sidecar));
    }
    let Some(mesh) = &accepted.mesh else {
        return;
    };
    for doc in &mesh.meshes {
        println!("{}", display_path(&doc.path));
        println!("  vertices: {}", doc.vertices);
        println!("  triangles: {}", doc.triangles);
    }
    for texture in &mesh.textures {
        println!("{}", display_path(texture));
    }
}

/// An [`Origin`] as `--json` prints it, fields in [`ludion_scene`]'s own emit order (`tool`,
/// `comfy_version`, `recipe`, `recipe_hash`, `run_id`, `date`, `seed`, `workflow`, `workflow_hash`,
/// `checkpoints`, `images`, `prompt`, `inputs`) — the struct's own fields, not the `kind: origin`
/// sidecar document's envelope (`Origin` carries no `format`/`kind` of its own). `images` takes the
/// shape `run.json`'s own `images` has, so the same fact reads the same way in both.
fn origin_json(origin: &Origin) -> Value {
    let images: Vec<Value> = origin
        .images
        .iter()
        .map(|image| {
            json!({
                "name": image.name,
                "path": image.path,
                "hash": image.hash,
            })
        })
        .collect();
    json!({
        "tool": origin.tool,
        "comfy_version": origin.comfy_version,
        "recipe": origin.recipe,
        "recipe_hash": origin.recipe_hash,
        "run_id": origin.run_id,
        "date": origin.date,
        "seed": origin.seed,
        "workflow": origin.workflow,
        "workflow_hash": origin.workflow_hash,
        "checkpoints": origin.checkpoints,
        "images": images,
        "prompt": origin.prompt,
        "inputs": ludion_gen::inputs_to_json(&origin.inputs),
    })
}

// --- shared -----------------------------------------------------------------------------------

/// Prints one JSON object on stdout, compact on one line with a trailing newline — the one `--json`
/// rendering every `gen` verb shares.
fn print_json(value: &Value) {
    let mut text = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    text.push('\n');
    print!("{text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(json: Value) -> Value {
        json
    }

    #[test]
    fn describe_input_reads_a_typed_input_with_a_default() {
        let entry = json!(["INT", { "default": 512, "min": 16, "max": 16384 }]);
        let desc = describe_input(&entry);
        assert_eq!(desc["type"], "INT");
        assert_eq!(desc["default"], 512);
        assert!(desc.get("options").is_none());
    }

    #[test]
    fn describe_input_reads_an_inline_combo_as_options() {
        let entry = json!([["euler", "dpmpp_2m"], {}]);
        let desc = describe_input(&entry);
        assert_eq!(desc["type"], "COMBO");
        assert_eq!(desc["options"], json!(["euler", "dpmpp_2m"]));
    }

    #[test]
    fn describe_input_reads_a_named_combos_options_from_meta() {
        let entry = json!(["COMBO", { "options": ["normal", "karras"] }]);
        let desc = describe_input(&entry);
        assert_eq!(desc["type"], "COMBO");
        assert_eq!(desc["options"], json!(["normal", "karras"]));
    }

    #[test]
    fn describe_node_separates_required_and_optional_and_reads_outputs() {
        let node = spec(json!({
            "input": {
                "required": { "seed": ["INT", { "default": 0 }] },
                "optional": { "denoise": ["FLOAT", { "default": 1.0 }] }
            },
            "output": ["LATENT"],
            "category": "sampling"
        }));
        let desc = describe_node(&node);
        assert_eq!(desc["category"], "sampling");
        assert_eq!(desc["required"]["seed"]["type"], "INT");
        assert_eq!(desc["optional"]["denoise"]["type"], "FLOAT");
        assert_eq!(desc["outputs"], json!(["LATENT"]));
    }

    #[test]
    fn matches_selection_is_case_insensitive_over_class_and_category() {
        let node = json!({ "category": "loaders" });
        assert!(matches_selection(
            "CheckpointLoaderSimple",
            &node,
            Some("checkpoint"),
            None
        ));
        assert!(matches_selection(
            "CheckpointLoaderSimple",
            &node,
            Some("LOAD"),
            None
        ));
        assert!(!matches_selection(
            "CheckpointLoaderSimple",
            &node,
            Some("sampler"),
            None
        ));
        assert!(matches_selection(
            "CheckpointLoaderSimple",
            &node,
            Some("anything"),
            Some("CheckpointLoaderSimple")
        ));
    }

    #[test]
    fn compact_line_lists_required_optional_and_outputs() {
        let desc = json!({
            "category": "sampling",
            "required": { "seed": { "type": "INT" } },
            "optional": { "denoise": { "type": "FLOAT" } },
            "outputs": ["LATENT"],
        });
        let line = compact_line("KSampler", &desc);
        assert_eq!(
            line,
            "KSampler  [sampling]  required: seed: INT  optional: denoise: FLOAT  -> LATENT"
        );
    }

    #[test]
    fn class_table_prints_defaults_and_options() {
        let desc = json!({
            "category": "sampling",
            "required": {
                "sampler_name": { "type": "COMBO", "options": ["euler", "dpmpp_2m"] },
                "steps": { "type": "INT", "default": 20 },
            },
            "optional": {},
            "outputs": ["LATENT"],
        });
        let table = class_table("KSampler", &desc);
        assert!(table.contains("steps: INT (default 20)"), "{table}");
        assert!(
            table.contains("sampler_name: COMBO [euler, dpmpp_2m]"),
            "{table}"
        );
        assert!(table.contains("outputs: LATENT"), "{table}");
    }

    #[test]
    fn render_overrides_lists_every_title_and_input() {
        let doc = GenerationDocument {
            inputs: vec![(
                "Positive Prompt".to_string(),
                vec![(
                    "text".to_string(),
                    InputValue::Scalar(ScalarValue::Text("hi".to_string())),
                )],
            )],
            ..GenerationDocument::default()
        };
        let text = render_overrides(&doc);
        assert_eq!(text, "overrides:\n  Positive Prompt:\n    text: hi\n");
    }

    /// The text table prints an image reference as one value. The document's own spelling nests
    /// `image:` under an input named `image`, which used to reach the terminal as
    /// `image: image: subject`.
    #[test]
    fn render_overrides_prints_an_image_reference_as_one_value_with_its_file() {
        let doc = GenerationDocument {
            images: vec![(
                "subject".to_string(),
                "../references/idol-reference.png".to_string(),
            )],
            inputs: vec![(
                "Load Subject".to_string(),
                vec![(
                    "image".to_string(),
                    InputValue::ImageRef("subject".to_string()),
                )],
            )],
            ..GenerationDocument::default()
        };
        assert_eq!(
            render_overrides(&doc),
            "overrides:\n  Load Subject:\n    image: images.subject -> \
             ../references/idol-reference.png\n"
        );
    }

    #[test]
    fn an_undeclared_image_reference_still_prints_as_one_value() {
        assert_eq!(
            input_value_display(&InputValue::ImageRef("subject".to_string()), &[]),
            "images.subject"
        );
    }

    #[test]
    fn override_table_renders_an_image_ref_as_an_image_object() {
        let doc = GenerationDocument {
            inputs: vec![(
                "Load Subject".to_string(),
                vec![(
                    "image".to_string(),
                    InputValue::ImageRef("subject".to_string()),
                )],
            )],
            ..GenerationDocument::default()
        };
        let table = override_table(&doc);
        assert_eq!(
            table["Load Subject"]["image"],
            json!({ "image": "subject" })
        );
    }

    #[test]
    fn queue_counts_reads_both_arrays() {
        let queue = json!({ "queue_running": [1], "queue_pending": [1, 2] });
        assert_eq!(queue_counts(&queue), (1, 2));
    }

    #[test]
    fn queue_counts_defaults_to_zero_when_absent() {
        assert_eq!(queue_counts(&json!({})), (0, 0));
    }

    #[test]
    fn placeholder_uploads_maps_each_image_name_to_its_file_name() {
        let doc = GenerationDocument {
            images: vec![
                (
                    "subject".to_string(),
                    "../references/idol-reference.png".to_string(),
                ),
                ("mask".to_string(), "mask.png".to_string()),
            ],
            ..GenerationDocument::default()
        };
        let uploads = placeholder_uploads(&doc);
        assert_eq!(
            uploads.get("subject").map(String::as_str),
            Some("idol-reference.png")
        );
        assert_eq!(uploads.get("mask").map(String::as_str), Some("mask.png"));
    }

    #[test]
    fn origin_json_renders_the_structs_own_fields_in_emit_order() {
        let origin = Origin {
            tool: "comfyui".to_string(),
            comfy_version: "0.3.99".to_string(),
            recipe: "generation/recipes/idol.gen.yaml".to_string(),
            recipe_hash: Some("0000000012345678".to_string()),
            run_id: "20260918T101112Z-1a2b3c4d".to_string(),
            date: "2026-09-18".to_string(),
            seed: Some(9),
            workflow: "generation/workflows/txt2img.json".to_string(),
            workflow_hash: "00000000deadbeef".to_string(),
            checkpoints: vec!["flux2-klein-4b.safetensors".to_string()],
            images: vec![ludion_scene::OriginImage {
                name: "subject".to_string(),
                path: "generation/references/idol-reference.png".to_string(),
                hash: "0123456789abcdef".to_string(),
            }],
            prompt: "a weathered stone idol".to_string(),
            inputs: vec![(
                "Positive Prompt".to_string(),
                vec![(
                    "text".to_string(),
                    InputValue::Scalar(ScalarValue::Text("a weathered stone idol".to_string())),
                )],
            )],
        };
        let value = origin_json(&origin);
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            vec![
                "tool",
                "comfy_version",
                "recipe",
                "recipe_hash",
                "run_id",
                "date",
                "seed",
                "workflow",
                "workflow_hash",
                "checkpoints",
                "images",
                "prompt",
                "inputs",
            ]
        );
        assert_eq!(value["seed"], 9);
        assert_eq!(
            value["inputs"]["Positive Prompt"]["text"],
            "a weathered stone idol"
        );
        assert_eq!(
            value["images"][0],
            json!({
                "name": "subject",
                "path": "generation/references/idol-reference.png",
                "hash": "0123456789abcdef",
            }),
            "an image entry reads the same here as it does in `run.json`"
        );
    }

    /// A minimal [`Origin`] for an [`Accepted`] under test — [`accept_json`] only reprints it.
    fn sample_origin() -> Origin {
        Origin {
            tool: "comfyui".to_string(),
            comfy_version: "0.3.99".to_string(),
            recipe: "generation/recipes/idol-mesh.gen.yaml".to_string(),
            recipe_hash: None,
            run_id: "20260918T101112Z-1a2b3c4d".to_string(),
            date: "2026-09-18".to_string(),
            seed: Some(7),
            workflow: "generation/workflows/trellis2-img2mesh.json".to_string(),
            workflow_hash: "00000000deadbeef".to_string(),
            checkpoints: Vec::new(),
            images: Vec::new(),
            prompt: String::new(),
            inputs: Vec::new(),
        }
    }

    #[test]
    fn accept_json_keeps_the_image_shape_to_four_keys() {
        let accepted = Accepted {
            source: PathBuf::from("generation/runs/idol/run-1/0000.png"),
            destination: PathBuf::from("assets/idol.png"),
            sidecar: Some(PathBuf::from("assets/idol.png.origin.yaml")),
            origin: sample_origin(),
            mesh: None,
        };
        let value = accept_json(&accepted);
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["source", "destination", "sidecar", "origin"]);
    }

    #[test]
    fn accept_json_adds_the_documents_and_textures_a_mesh_import_wrote() {
        let accepted = Accepted {
            source: PathBuf::from("generation/runs/idol/run-1/0000.glb"),
            destination: PathBuf::from("assets/idol"),
            sidecar: None,
            origin: sample_origin(),
            mesh: Some(ludion_gen::AcceptedMesh {
                meshes: vec![ludion_gen::AcceptedMeshDocument {
                    path: PathBuf::from("assets/idol").join("meshes/idol.mesh.yaml"),
                    vertices: 2481,
                    triangles: 4962,
                }],
                textures: vec![PathBuf::from("assets/idol").join("textures/abc.png")],
                warnings: Vec::new(),
            }),
        };
        let value = accept_json(&accepted);
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            vec![
                "source",
                "destination",
                "sidecar",
                "origin",
                "meshes",
                "textures",
            ]
        );
        assert_eq!(value["sidecar"], Value::Null);
        assert_eq!(
            value["meshes"][0]["path"],
            "assets/idol/meshes/idol.mesh.yaml"
        );
        assert_eq!(value["meshes"][0]["vertices"], 2481);
        assert_eq!(value["meshes"][0]["triangles"], 4962);
        assert_eq!(value["textures"][0], "assets/idol/textures/abc.png");
    }

    #[test]
    fn display_path_replaces_every_backslash_a_platform_join_leaves_behind() {
        // `PathBuf::join` inserts the platform separator between components regardless of what
        // separator the joined-in text already used, so a run folder built from a literal
        // `"generation/runs"` and platform-joined path segments mixes `/` and `\` on Windows.
        let mixed = PathBuf::from("generation/runs")
            .join("idol")
            .join("20260918T101112Z-1a2b3c4d");
        let displayed = display_path(&mixed);
        assert!(!displayed.contains('\\'), "{displayed}");
        assert_eq!(displayed, "generation/runs/idol/20260918T101112Z-1a2b3c4d");
    }

    #[test]
    fn live_text_spells_ok_failed_and_a_skipped_pass_names_its_code() {
        assert_eq!(live_text("ok"), "ok");
        assert_eq!(live_text("error"), "failed");
        assert_eq!(live_text("skipped"), "skipped (L1167)");
    }

    /// A minimal [`ludion_gen::RunReport`] for [`next_hint`] — only `kind` and `dir` matter to it.
    fn sample_report(kind: &str, dir: &str) -> ludion_gen::RunReport {
        ludion_gen::RunReport {
            run_id: "20260918T101112Z-1a2b3c4d".to_string(),
            recipe: "generation/recipes/idol.gen.yaml".to_string(),
            recipe_hash: "0000000012345678".to_string(),
            workflow: "generation/workflows/txt2img.json".to_string(),
            workflow_hash: "00000000deadbeef".to_string(),
            graph_hash: "1a2b3c4d5e6f7080".to_string(),
            seed_base: 7,
            count: 1,
            kind: kind.to_string(),
            comfy_version: "0.3.99".to_string(),
            checkpoints: Vec::new(),
            images: Vec::new(),
            prompt: None,
            inputs: None,
            prompt_ids: Vec::new(),
            seeds: Vec::new(),
            outputs: Vec::new(),
            failed: Vec::new(),
            started: "2026-09-18T10:11:12Z".to_string(),
            finished: String::new(),
            duration_ms: 0,
            status: "submitted".to_string(),
            dir: PathBuf::from(dir),
        }
    }

    #[test]
    fn only_ok_and_submitted_runs_exit_zero() {
        let mut report = sample_report("image", "generation/runs/idol/run-1");
        for (status, success) in [
            ("ok", true),
            ("submitted", true),
            ("partial", false),
            ("failed", false),
        ] {
            report.status = status.to_string();
            assert_eq!(run_ok(&report), success, "{status}");
        }
    }

    #[test]
    fn next_hint_for_a_failed_run_submits_the_recipe_again() {
        let mut report = sample_report("image", "generation/runs/idol/run-1");
        report.status = "failed".to_string();
        assert_eq!(
            next_hint(&report, false),
            "next: ludion gen run generation/recipes/idol.gen.yaml"
        );
    }

    #[test]
    fn next_hint_names_fetch_for_a_no_wait_run_regardless_of_kind() {
        let report = sample_report("mesh", "generation/runs/idol/run-1");
        assert_eq!(
            next_hint(&report, true),
            "next: ludion gen fetch generation/runs/idol/run-1"
        );
    }

    #[test]
    fn next_hint_names_accept_with_out_for_a_finished_mesh_run() {
        let report = sample_report("mesh", "generation/runs/idol/run-1");
        assert_eq!(
            next_hint(&report, false),
            "next: ludion gen accept generation/runs/idol/run-1 0 --out <dir>"
        );
    }

    #[test]
    fn next_hint_names_sheet_for_a_finished_image_run() {
        let report = sample_report("image", "generation/runs/idol/run-1");
        assert_eq!(
            next_hint(&report, false),
            "next: ludion gen sheet generation/runs/idol/run-1"
        );
    }
}
