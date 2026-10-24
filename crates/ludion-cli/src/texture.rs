//! Implementation of `ludion texture build | check | vendor`.
//!
//! `build` evaluates a `kind: texture` recipe (`ludion-texgen`) into its committed, write-only map
//! set plus a `generated.json` provenance sidecar; `check` rebuilds at the recorded size and
//! byte-compares (the drift gate `mesh apply --check` established); `vendor` copies an embedded
//! `std:` recipe next to the project so it can be edited. Both `build` and `check` run the
//! engine's PBR plausibility validators, with bounds a rules TOML may override.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use ludion_core::atomic::write_atomic;
use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, Severity};
use ludion_texgen::validate::{self, Level, Rule};
use ludion_texgen::{FsResolver, MapSet, Maps};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::diagnostic;

/// The default build size.
pub const DEFAULT_SIZE: u32 = 1024;

const SIDECAR: &str = "generated.json";
const RECIPE_SUFFIX: &str = ".texture.yaml";

// --- rules --------------------------------------------------------------------------------------

/// One validator's overrides; each field left out keeps the value below it.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleSpec {
    min: Option<f32>,
    max: Option<f32>,
    level: Option<LevelSpec>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LevelSpec {
    Warn,
    Error,
}

/// A rules file: project-wide bounds by validator name, plus per-recipe overrides keyed by the
/// recipe's file name.
#[derive(Debug, Default, Deserialize)]
struct Rules {
    #[serde(default)]
    textures: BTreeMap<String, BTreeMap<String, RuleSpec>>,
    #[serde(flatten)]
    base: BTreeMap<String, RuleSpec>,
}

impl Rules {
    fn load(path: &Path) -> Result<Rules, Vec<Diagnostic>> {
        let invalid = |message: String| {
            vec![
                Diagnostic::new(code::TEXTURE_RULES_INVALID, message)
                    .with_help("the shape is `<validator> = { min, max, level }` plus `[textures.\"<recipe file>\"]` tables; `ludion explain L1289`"),
            ]
        };
        let text = std::fs::read_to_string(path).map_err(|error| {
            invalid(format!(
                "could not read rules `{}`: {error}",
                path.display()
            ))
        })?;
        let rules: Rules = toml::from_str(&text)
            .map_err(|error| invalid(format!("rules `{}`: {error}", path.display())))?;
        let unknown: Vec<Diagnostic> = rules
            .base
            .keys()
            .map(|name| (name, name.clone()))
            .chain(rules.textures.iter().flat_map(|(file, set)| {
                set.keys()
                    .map(move |name| (name, format!("textures.\"{file}\".{name}")))
            }))
            .filter(|(name, _)| validate::default_rule(name).is_none())
            .map(|(name, at)| {
                let help = match ludion_core::suggest::nearest(name, validate::NAMES) {
                    Some(near) => format!("did you mean `{near}`?"),
                    None => format!("the validators are: {}", validate::NAMES.join(", ")),
                };
                Diagnostic::new(
                    code::TEXTURE_RULES_INVALID,
                    format!("rules `{}`: `{at}` is not a validator", path.display()),
                )
                .with_help(help)
            })
            .collect();
        if unknown.is_empty() {
            Ok(rules)
        } else {
            Err(unknown)
        }
    }

    /// The bounds for `name` on the recipe `file`: the engine default, overlaid by the project
    /// base, overlaid by the recipe's own override, field by field.
    fn rule(&self, file: &str, name: &str) -> Rule {
        let mut rule = validate::default_rule(name).expect("validator names are checked on load");
        let specs = [
            self.base.get(name),
            self.textures.get(file).and_then(|set| set.get(name)),
        ];
        for spec in specs.into_iter().flatten() {
            rule.min = spec.min.or(rule.min);
            rule.max = spec.max.or(rule.max);
            rule.level = match spec.level {
                Some(LevelSpec::Warn) => Level::Warn,
                Some(LevelSpec::Error) => Level::Error,
                None => rule.level,
            };
        }
        rule
    }
}

// --- reports ------------------------------------------------------------------------------------

#[derive(Serialize)]
struct StepTime {
    name: String,
    ms: f64,
}

#[derive(Serialize)]
struct Verdict {
    name: &'static str,
    value: f32,
    min: Option<f32>,
    max: Option<f32>,
    level: &'static str,
    pass: bool,
}

/// What `generated.json` records: enough for `check` to rebuild without being told the size.
#[derive(Serialize, Deserialize)]
struct Sidecar {
    generated_by: String,
    recipe: String,
    size: u32,
    maps: Vec<String>,
}

fn level_text(level: Level) -> &'static str {
    match level {
        Level::Warn => "warn",
        Level::Error => "error",
    }
}

/// The map a validator is about, as the `/outputs/<map>` pointer names it.
fn output_of(validator: &str) -> &'static str {
    match validator {
        "albedo_out_of_range" | "seam_base_color" => "base_color",
        "metallic_ambiguous" => "metallic",
        "roughness_mirror" => "roughness",
        "normal_invalid" => "normal",
        _ => "height",
    }
}

fn bounds_text(rule: &Rule) -> String {
    match (rule.min, rule.max) {
        (Some(min), Some(max)) => format!("{min}..{max}"),
        (Some(min), None) => format!("at least {min}"),
        (None, Some(max)) => format!("at most {max}"),
        (None, None) => "unbounded".to_string(),
    }
}

fn validate_maps(maps: &Maps, rules: &Rules, file: &str) -> (Vec<Verdict>, Vec<Diagnostic>) {
    let mut verdicts = Vec::new();
    let mut diagnostics = Vec::new();
    for measure in validate::measure(maps) {
        let rule = rules.rule(file, measure.name);
        let pass = rule.passes(measure.value);
        if !pass {
            let code = match rule.level {
                Level::Warn => code::TEXTURE_CHECK_WARNING,
                Level::Error => code::TEXTURE_CHECK_FAILED,
            };
            diagnostics.push(
                Diagnostic::new(
                    code,
                    format!(
                        "validator `{}` measured {:.4}, outside {}",
                        measure.name,
                        measure.value,
                        bounds_text(&rule)
                    ),
                )
                .with_path(format!("/outputs/{}", output_of(measure.name)))
                .with_help(
                    "adjust the recipe, or relax the bound with `--rules` when the departure is intended",
                ),
            );
        }
        verdicts.push(Verdict {
            name: measure.name,
            value: measure.value,
            min: rule.min,
            max: rule.max,
            level: level_text(rule.level),
            pass,
        });
    }
    (verdicts, diagnostics)
}

/// Wall-clock timing for the build report. The CLI is not sim-side, so this carries the sim-side
/// denylist's one `allow`, as `check_scene`'s `timed` does.
#[derive(Clone, Copy)]
struct Stopwatch(std::time::Instant);

impl Stopwatch {
    #[allow(clippy::disallowed_methods)]
    fn start() -> Self {
        Stopwatch(std::time::Instant::now())
    }

    #[allow(clippy::disallowed_methods)]
    fn elapsed(self) -> Duration {
        self.0.elapsed()
    }
}

// --- shared -------------------------------------------------------------------------------------

struct Built {
    maps: Maps,
    set: MapSet,
    steps: Vec<StepTime>,
    total_ms: f64,
}

fn io_error(message: String) -> Diagnostic {
    Diagnostic::new(code::SCENE_IO, message)
        .with_help("check the path (relative to the current directory) and permissions")
}

fn recipe_file(recipe: &Path) -> String {
    recipe
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn default_out(recipe: &Path) -> PathBuf {
    let file = recipe_file(recipe);
    let stem = file.strip_suffix(RECIPE_SUFFIX).unwrap_or_else(|| {
        recipe
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("texture")
    });
    recipe.with_file_name(stem)
}

fn evaluate_recipe(recipe: &Path, size: u32) -> Result<Built, Vec<Diagnostic>> {
    ludion_texgen::check_size(size)?;
    let text = std::fs::read_to_string(recipe).map_err(|e| {
        vec![io_error(format!(
            "could not read `{}`: {e}",
            recipe.display()
        ))]
    })?;
    let base = recipe
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let loaded = ludion_texgen::load(&text, &FsResolver::new(base))?;
    let start = Stopwatch::start();
    let mut last = Duration::ZERO;
    let mut steps = Vec::new();
    let maps = ludion_texgen::evaluate(&loaded, size, &mut |name, _| {
        let now = start.elapsed();
        steps.push(StepTime {
            name: name.to_string(),
            ms: (now - last).as_secs_f64() * 1000.0,
        });
        last = now;
    });
    let set = ludion_texgen::encode(&maps)
        .map_err(|reason| vec![Diagnostic::new(code::TEXTURE_DATA, reason).with_path("/size")])?;
    Ok(Built {
        maps,
        set,
        steps,
        total_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

/// Every file a set holds, in write order.
fn set_files(set: &MapSet) -> Vec<(&'static str, &[u8])> {
    let mut files = vec![
        ("base_color.png", set.base_color.as_slice()),
        ("metallic_roughness.png", set.metallic_roughness.as_slice()),
        ("normal.png", set.normal.as_slice()),
        ("occlusion.png", set.occlusion.as_slice()),
    ];
    if let Some(emissive) = &set.emissive {
        files.push(("emissive.png", emissive));
    }
    files.push(("height.png", set.height.as_slice()));
    files
}

/// `to` spelled relative to the directory `from`, with forward slashes.
fn relative(from: &Path, to: &Path) -> String {
    fn parts(path: &Path) -> Vec<String> {
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let mut out: Vec<String> = Vec::new();
        for component in absolute.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other.as_os_str().to_string_lossy().into_owned()),
            }
        }
        out
    }
    let (from, to) = (parts(from), parts(to));
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return to.join("/");
    }
    let mut out = vec![".."; from.len() - common];
    out.extend(to[common..].iter().map(String::as_str));
    out.join("/")
}

fn sidecar_bytes(recipe: &Path, out_dir: &Path, size: u32, set: &MapSet) -> Vec<u8> {
    let sidecar = Sidecar {
        generated_by: "ludion texture build".to_string(),
        recipe: relative(out_dir, recipe),
        size,
        maps: set_files(set)
            .iter()
            .map(|(n, _)| (*n).to_string())
            .collect(),
    };
    let mut text = serde_json::to_string_pretty(&sidecar).expect("sidecar serializes");
    text.push('\n');
    text.into_bytes()
}

fn map_report(maps: &Maps, out_dir: &Path) -> Value {
    let mut object = serde_json::Map::new();
    for (name, stat) in validate::map_stats(maps) {
        let file = if name == "metallic" || name == "roughness" {
            "metallic_roughness.png".to_string()
        } else {
            format!("{name}.png")
        };
        object.insert(
            name.to_string(),
            json!({
                "path": path_text(&out_dir.join(file)),
                "min": stat.min,
                "mean": stat.mean,
                "max": stat.max,
            }),
        );
    }
    Value::Object(object)
}

fn verdict_line(v: &Verdict) -> String {
    let mut bounds = Vec::new();
    if let Some(min) = v.min {
        bounds.push(format!("min {min}"));
    }
    if let Some(max) = v.max {
        bounds.push(format!("max {max}"));
    }
    format!(
        "  validator {} {:.4} ({}) {}",
        v.name,
        v.value,
        bounds.join(", "),
        if v.pass { "ok" } else { v.level }
    )
}

fn has_error(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|d| d.severity == Severity::Error)
}

/// A runtime failure: diagnostics to stderr and, under `--json`, one `{ok: false, …}` object.
fn fail(path: &Path, diagnostics: &[Diagnostic], json: bool) -> ExitCode {
    diagnostic::report(path, diagnostics);
    if json {
        let fields = json!({ "recipe": path_text(path) });
        print_json(&Value::Object(diagnostic::json_envelope(
            false,
            fields,
            diagnostics,
        )));
    }
    ExitCode::FAILURE
}

fn path_text(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

fn print_json(value: &Value) {
    let mut text = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    text.push('\n');
    print!("{text}");
}

fn load_rules(rules: Option<&Path>) -> Result<Rules, Vec<Diagnostic>> {
    rules.map_or_else(|| Ok(Rules::default()), Rules::load)
}

// --- build --------------------------------------------------------------------------------------

/// Runs `ludion texture build <recipe> [--size N] [--out DIR] [--rules TOML] [--json]`.
pub fn run_build(
    recipe: &Path,
    size: u32,
    out: Option<&Path>,
    rules: Option<&Path>,
    json: bool,
) -> ExitCode {
    let rules = match load_rules(rules) {
        Ok(rules) => rules,
        Err(diagnostics) => return fail(recipe, &diagnostics, json),
    };
    let built = match evaluate_recipe(recipe, size) {
        Ok(built) => built,
        Err(diagnostics) => return fail(recipe, &diagnostics, json),
    };
    let out_dir = out.map_or_else(|| default_out(recipe), Path::to_path_buf);
    let sidecar = sidecar_bytes(recipe, &out_dir, size, &built.set);
    let mut written = Vec::new();
    if let Err(error) = write_set(&out_dir, &built.set, &sidecar, &mut written) {
        return fail(recipe, &[io_error(error)], json);
    }
    let (verdicts, diagnostics) = validate_maps(&built.maps, &rules, &recipe_file(recipe));
    let ok = !has_error(&diagnostics);

    if json {
        let fields = json!({
            "recipe": path_text(recipe),
            "out_dir": path_text(&out_dir),
            "size": size,
            "total_ms": built.total_ms,
            "steps": built.steps,
            "maps": map_report(&built.maps, &out_dir),
            "validators": verdicts,
            "written": written.iter().map(|p| path_text(p)).collect::<Vec<_>>(),
        });
        print_json(&Value::Object(diagnostic::json_envelope(
            ok,
            fields,
            &diagnostics,
        )));
        diagnostic::report(recipe, &diagnostics);
        return if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    println!(
        "{}: built {size}x{size} -> {} ({:.0} ms)",
        path_text(recipe),
        path_text(&out_dir),
        built.total_ms
    );
    for path in &written {
        let bytes = std::fs::metadata(path).map_or(0, |m| m.len());
        println!("  {} ({bytes} bytes)", path_text(path));
    }
    for verdict in &verdicts {
        println!("{}", verdict_line(verdict));
    }
    diagnostic::report(recipe, &diagnostics)
}

fn write_set(
    out_dir: &Path,
    set: &MapSet,
    sidecar: &[u8],
    written: &mut Vec<PathBuf>,
) -> Result<(), String> {
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("could not create `{}`: {e}", out_dir.display()))?;
    let mut files = set_files(set);
    files.push((SIDECAR, sidecar));
    for (name, bytes) in files {
        let path = out_dir.join(name);
        write_atomic(&path, bytes)
            .map_err(|e| format!("could not write `{}`: {e}", path.display()))?;
        written.push(path);
    }
    if set.emissive.is_none() {
        let stale = out_dir.join("emissive.png");
        if stale.exists() {
            std::fs::remove_file(&stale)
                .map_err(|e| format!("could not remove `{}`: {e}", stale.display()))?;
        }
    }
    Ok(())
}

// --- check --------------------------------------------------------------------------------------

fn drift(path: &Path, why: &str) -> Diagnostic {
    Diagnostic::new(code::TEXTURE_DRIFT, format!("DRIFT {}: {why}", path_text(path)))
        .with_help("run `ludion texture build <recipe>` (same --size) and commit the result; never edit the maps by hand")
}

/// Runs `ludion texture check <recipe> [--out DIR] [--rules TOML] [--json]`.
pub fn run_check(recipe: &Path, out: Option<&Path>, rules: Option<&Path>, json: bool) -> ExitCode {
    let rules = match load_rules(rules) {
        Ok(rules) => rules,
        Err(diagnostics) => return fail(recipe, &diagnostics, json),
    };
    let out_dir = out.map_or_else(|| default_out(recipe), Path::to_path_buf);
    let sidecar_path = out_dir.join(SIDECAR);
    let recorded = std::fs::read_to_string(&sidecar_path)
        .map_err(|e| e.to_string())
        .and_then(|text| serde_json::from_str::<Sidecar>(&text).map_err(|e| e.to_string()));
    let recorded = match recorded {
        Ok(recorded) => recorded,
        Err(why) => {
            let never = drift(
                &sidecar_path,
                &format!("never built: no readable provenance sidecar ({why})"),
            )
            .with_help(format!("run `ludion texture build {}`", path_text(recipe)));
            return fail(recipe, &[never], json);
        }
    };
    let size = recorded.size;
    let built = match evaluate_recipe(recipe, size) {
        Ok(built) => built,
        Err(diagnostics) => return fail(recipe, &diagnostics, json),
    };
    let sidecar = sidecar_bytes(recipe, &out_dir, size, &built.set);
    let mut expected = set_files(&built.set);
    expected.push((SIDECAR, sidecar.as_slice()));

    let mut drifted = Vec::new();
    for (name, bytes) in &expected {
        let path = out_dir.join(name);
        match std::fs::read(&path) {
            Ok(committed) if committed == *bytes => {}
            Ok(_) => drifted.push(drift(
                &path,
                "the committed file differs from what the recipe now builds",
            )),
            Err(error) => drifted.push(drift(
                &path,
                &format!("could not read the committed file to compare: {error}"),
            )),
        }
    }
    if let Ok(entries) = std::fs::read_dir(&out_dir) {
        let mut stray: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "png"))
            .filter(|p| {
                !expected
                    .iter()
                    .any(|(n, _)| p.file_name().is_some_and(|f| f == *n))
            })
            .collect();
        stray.sort();
        for path in stray {
            drifted.push(drift(&path, "the recipe does not build that file"));
        }
    }
    let (verdicts, mut diagnostics) = validate_maps(&built.maps, &rules, &recipe_file(recipe));
    let drift_lines: Vec<String> = drifted.iter().map(|d| d.message.clone()).collect();
    diagnostics.splice(0..0, drifted);
    let ok = !has_error(&diagnostics);

    if json {
        let fields = json!({
            "recipe": path_text(recipe),
            "out_dir": path_text(&out_dir),
            "size": size,
            "drift": drift_lines,
            "validators": verdicts,
        });
        print_json(&Value::Object(diagnostic::json_envelope(
            ok,
            fields,
            &diagnostics,
        )));
        diagnostic::report(recipe, &diagnostics);
        return if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    if drift_lines.is_empty() {
        println!(
            "{}: up to date -> {} ({} files, {size}x{size})",
            path_text(recipe),
            path_text(&out_dir),
            expected.len()
        );
    }
    for verdict in &verdicts {
        println!("{}", verdict_line(verdict));
    }
    diagnostic::report(recipe, &diagnostics)
}

// --- vendor -------------------------------------------------------------------------------------

/// Runs `ludion texture vendor <name> [dest] [--json]`.
pub fn run_vendor(name: &str, dest: Option<&Path>, json: bool) -> ExitCode {
    let bare = name.strip_prefix("std:").unwrap_or(name);
    let source = Path::new(name);
    let Some(text) = ludion_texgen::stdlib::get(bare) else {
        let help = match ludion_core::suggest::nearest(bare, ludion_texgen::stdlib::names()) {
            Some(near) => format!("did you mean `std:{near}`?"),
            None => format!(
                "the embedded recipes are: {}",
                ludion_texgen::stdlib::names()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        let unknown = Diagnostic::new(
            code::TEXTURE_GRAPH,
            format!("there is no embedded recipe `std:{bare}`"),
        )
        .with_help(help);
        return fail(source, &[unknown], json);
    };
    let dest = dest.map_or_else(
        || PathBuf::from(format!("{bare}{RECIPE_SUFFIX}")),
        Path::to_path_buf,
    );
    if dest.exists() {
        let exists = io_error(format!(
            "`{}` already exists; vendor never overwrites",
            dest.display()
        ))
        .with_help("pick another destination or remove the file first");
        return fail(source, &[exists], json);
    }
    if let Err(error) = write_atomic(&dest, text) {
        return fail(
            source,
            &[io_error(format!(
                "could not write `{}`: {error}",
                dest.display()
            ))],
            json,
        );
    }
    if json {
        let fields = json!({ "name": format!("std:{bare}"), "dest": path_text(&dest) });
        print_json(&Value::Object(diagnostic::json_envelope(true, fields, &[])));
    } else {
        println!(
            "wrote {}; reference it as path: {} instead of std:{bare}",
            path_text(&dest),
            path_text(&dest)
        );
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(text: &str) -> Rules {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn rule_resolution_is_default_then_base_then_recipe_override() {
        let r = rules(
            "seam_height = { max = 3.0 }\n\
             height_range = { min = 0.2, level = \"error\" }\n\
             [textures.\"wall.texture.yaml\"]\n\
             seam_height = { max = 9.0, level = \"error\" }\n",
        );
        let default = validate::default_rule("seam_height").unwrap();
        assert_eq!(default.max, Some(1.5));
        assert_eq!(r.rule("other.texture.yaml", "seam_height").max, Some(3.0));
        let wall = r.rule("wall.texture.yaml", "seam_height");
        assert_eq!((wall.max, wall.level), (Some(9.0), Level::Error));
        let height = r.rule("other.texture.yaml", "height_range");
        assert_eq!((height.min, height.level), (Some(0.2), Level::Error));
        assert_eq!(
            r.rule("x", "albedo_out_of_range"),
            validate::default_rule("albedo_out_of_range").unwrap()
        );
    }

    #[test]
    fn a_misspelled_rule_field_or_level_does_not_parse() {
        assert!(toml::from_str::<Rules>("seam_height = { mx = 1.0 }").is_err());
        assert!(toml::from_str::<Rules>("seam_height = { level = \"fatal\" }").is_err());
    }

    #[test]
    fn relative_paths_climb_out_of_the_out_dir() {
        let rel = relative(Path::new("/a/b/stem"), Path::new("/a/b/stem.texture.yaml"));
        assert_eq!(rel, "../stem.texture.yaml");
        assert_eq!(
            relative(Path::new("/a/out"), Path::new("/a/b/x.texture.yaml")),
            "../b/x.texture.yaml"
        );
    }

    #[test]
    fn the_default_out_dir_is_the_recipe_stem() {
        assert_eq!(
            default_out(Path::new("tex/tiles.texture.yaml")),
            PathBuf::from("tex/tiles")
        );
    }
}
