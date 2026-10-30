//! Implementation of `ludion export --target ios|ios-sim`.
//!
//! Emits a self-contained Xcode project folder for a Ludion game: the embedded template
//! (`templates/ios/`) with its placeholders substituted, the game built as a static library, and the
//! default scene's directory and every `[[import]]` tree (at its project-relative path, synced
//! first) as bundle content. Plain Rust shelling out to `cargo` and `xcrun` — platform knowledge
//! lives in the template's Objective-C, plist and pbxproj, never in the engine.
//!
//! The cheap refusals (flag combinations, manifest, output folder, toolchain) all run before the
//! build. `--run` then builds the project with `xcodebuild` and installs and launches it in the
//! simulator with `xcrun simctl`, every step's output streaming through.
//!
//! Reflection probes are baked against the written bundle, not the project: only `cooked/ios` is in
//! the bundle, so the cooked maps' identities (which fold the cook target) key the probes as the
//! device computes them. The bake assumes a GPU with ASTC, as every Apple-silicon Mac has.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use clap::ValueEnum;
use ludion_core::codes::code::{
    EXPORT_FAILED, EXPORT_FLAGS_INVALID, EXPORT_TOOLCHAIN_UNAVAILABLE, OUTPUT_IO,
    REFLECTION_PROBE_NOT_BUNDLED, SCENE_IO,
};
use ludion_core::diagnostics::Diagnostic;

use ludion_engine::manifest::{self, Manifest};
use ludion_engine::{ProbeBake, ProbeBakeReport, ProbeOutcome};

/// The file a finished export leaves in its folder, so a re-export may overwrite it.
const MARKER: &str = ".ludion-export";

/// A platform `ludion export` can target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum IosTarget {
    /// An iPhone or iPad.
    Ios,
    /// The iOS simulator on an Apple-silicon Mac.
    IosSim,
}

impl IosTarget {
    /// The Rust target triple.
    fn triple(self) -> &'static str {
        match self {
            Self::Ios => "aarch64-apple-ios",
            Self::IosSim => "aarch64-apple-ios-sim",
        }
    }

    /// The Xcode SDK the target builds against.
    fn sdk(self) -> &'static str {
        match self {
            Self::Ios => "iphoneos",
            Self::IosSim => "iphonesimulator",
        }
    }
}

/// One generated file: its path relative to the export folder and its template body.
struct TemplateFile {
    rel_path: &'static str,
    body: &'static str,
}

/// The embedded template, with its [`Substitutions`] made on write. The pbxproj is
/// stored with a `.tpl` suffix for the same reason `new`'s `Cargo.toml.tpl` is.
const TEMPLATES: &[TemplateFile] = &[
    TemplateFile {
        rel_path: "{{name}}.xcodeproj/project.pbxproj",
        body: include_str!("../templates/ios/project.pbxproj.tpl"),
    },
    TemplateFile {
        rel_path: "Host/Info.plist",
        body: include_str!("../templates/ios/Info.plist"),
    },
    TemplateFile {
        rel_path: "Host/ludion.h",
        body: include_str!("../templates/ios/ludion.h"),
    },
    TemplateFile {
        rel_path: "Host/main.m",
        body: include_str!("../templates/ios/main.m"),
    },
    TemplateFile {
        rel_path: "Host/LudionViewController.h",
        body: include_str!("../templates/ios/LudionViewController.h"),
    },
    TemplateFile {
        rel_path: "Host/LudionViewController.m",
        body: include_str!("../templates/ios/LudionViewController.m"),
    },
];

/// The flags of `ludion export` past the target and the two folders.
pub struct Options<'a> {
    /// Build the game with its `wire` feature.
    pub debug: bool,
    /// Files copied into the bundle's content root, by file name.
    pub replays: &'a [PathBuf],
    /// Build, install and launch in the simulator after exporting.
    pub run: bool,
    /// Arguments the launched app receives.
    pub app_args: &'a [String],
}

/// Runs `ludion export`.
///
/// Returns a non-zero exit for a flag combination, manifest or output folder it refuses, a missing
/// toolchain (`L0022`), a failed build or simulator step, or a filesystem error.
pub fn run(target: IosTarget, out: &Path, project: &Path, options: &Options) -> ExitCode {
    match export(target, out, project, options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            failure.report();
            ExitCode::FAILURE
        }
    }
}

/// Why an export stopped.
enum Failure {
    Diagnostic(Diagnostic),
    /// Several diagnostics from one step (the derived-import sync), each printed on its own.
    Diagnostics(Vec<Diagnostic>),
}

impl Failure {
    fn report(&self) {
        match self {
            Self::Diagnostic(diagnostic) => crate::diagnostic::print(diagnostic),
            Self::Diagnostics(diagnostics) => {
                for diagnostic in diagnostics {
                    crate::diagnostic::print(diagnostic);
                }
            }
        }
    }
}

/// A failed write, create or copy into the export folder.
fn output_io(message: String) -> Diagnostic {
    Diagnostic::new(OUTPUT_IO, message)
        .with_help("check that the export folder is writable and the disk has space")
}

/// A step of the export that is not about flags, the manifest or the toolchain.
fn export_failed(message: impl Into<String>, help: &str) -> Diagnostic {
    Diagnostic::new(EXPORT_FAILED, message).with_help(help.to_string())
}

impl From<Diagnostic> for Failure {
    fn from(diagnostic: Diagnostic) -> Self {
        Self::Diagnostic(diagnostic)
    }
}

/// The flag combinations that make no sense, before anything is read or built.
fn check_flags(target: IosTarget, options: &Options) -> Result<(), Diagnostic> {
    let refuse = |message: String, help: &str| {
        Err(Diagnostic::new(EXPORT_FLAGS_INVALID, message).with_help(help.to_string()))
    };
    if options.run && target == IosTarget::Ios {
        return refuse(
            "`--run` supports `ios-sim` only".into(),
            "use `--target ios-sim --run`; for a device, open the exported project in Xcode",
        );
    }
    if !options.replays.is_empty() && !options.debug {
        return refuse(
            "`--replay` needs `--debug`: only a build with `wire` reads app arguments".into(),
            "add `--debug`",
        );
    }
    let runnable = options.run && options.debug;
    if !options.app_args.is_empty() && !runnable {
        return refuse(
            "arguments after `--` need `--run` and `--debug`: only a debug build reads them".into(),
            "add `--run --debug`, or drop the arguments",
        );
    }
    for replay in options.replays {
        if !replay.is_file() {
            return refuse(
                format!("`--replay {}` is not a file", replay.display()),
                "name a recorded `.lrp` file",
            );
        }
    }
    Ok(())
}

fn export(target: IosTarget, out: &Path, project: &Path, options: &Options) -> Result<(), Failure> {
    check_flags(target, options)?;
    let debug = options.debug;
    let project = project.canonicalize().map_err(|err| {
        Diagnostic::new(
            SCENE_IO,
            format!("could not read `{}`: {err}", project.display()),
        )
        .with_help("check the project path (relative to the current directory) and permissions")
    })?;
    let manifest = manifest::load(&project)?;
    let substitutions = Substitutions {
        name: &manifest.name,
        bundle_id: manifest.ios_bundle_id()?,
        quality: manifest.ios_quality()?.unwrap_or_default(),
    };
    let out = resolve_out(out, &project)?;
    toolchain(target)?;
    let cook = crate::cook::CookRequest {
        enabled: true,
        extra_targets: vec![ludion_assets::cooked::CookTarget::Ios],
    };
    crate::sync::ensure_fresh(&project, &cook).map_err(Failure::Diagnostics)?;

    let archive = build_staticlib(&project, target, debug)?;
    let shipped = write_export(
        &out,
        &project,
        &manifest,
        &substitutions,
        &archive,
        options.replays,
    )?;
    let probes = bake_bundle_probes(&project, &manifest, &out.join("content"))?;

    let mut line = format!(
        "shipped {} cooked ios maps ({:.1} MB)",
        shipped.maps,
        shipped.bytes as f64 / 1e6
    );
    if probes.cubes > 0 {
        line.push_str(&format!(
            "; shipped {} probe cubes ({:.1} MB)",
            probes.cubes,
            probes.bytes as f64 / 1e6
        ));
    }
    if shipped.uncooked > 0 {
        line.push_str(&format!(
            "; {} referenced maps not cooked load their PNG",
            shipped.uncooked
        ));
    }
    if shipped.unscanned > 0 {
        line.push_str(&format!(
            "; {} meshes could not be scanned, so their maps may be missing from the store",
            shipped.unscanned
        ));
    }
    println!("{line}");
    println!(
        "exported `{}` for {} to {}",
        manifest.name,
        target.triple(),
        out.display()
    );
    if options.run {
        return simulator_lane(
            &out,
            &manifest.name,
            substitutions.bundle_id,
            options.app_args,
        )
        .map_err(|message| {
            export_failed(
                message,
                "fix the step above (its own output is printed first)",
            )
            .into()
        });
    }
    println!();
    println!("next: build it with Xcode's command line —");
    println!(
        "  xcodebuild {}",
        xcodebuild_args(&out, &manifest.name, target).join(" ")
    );
    Ok(())
}

/// Makes `out` absolute and checks it: apart from the project (the checkout and `target/` stay out
/// of the bundle), and empty or a previous export.
fn resolve_out(out: &Path, project: &Path) -> Result<PathBuf, Diagnostic> {
    const HELP: &str = "choose an empty or new folder outside the project";
    let out = absolute_through_symlinks(out).map_err(|message| export_failed(message, HELP))?;
    if out.starts_with(project) || project.starts_with(&out) {
        return Err(export_failed(
            format!(
                "`{}` overlaps the project `{}` — export to a folder outside it",
                out.display(),
                project.display()
            ),
            HELP,
        ));
    }
    if out.exists() {
        if !out.is_dir() {
            return Err(export_failed(
                format!("`{}` is not a folder", out.display()),
                HELP,
            ));
        }
        let mut entries = std::fs::read_dir(&out).map_err(|err| {
            Diagnostic::new(
                SCENE_IO,
                format!("could not read `{}`: {err}", out.display()),
            )
            .with_help("check the folder's permissions")
        })?;
        if entries.next().is_some() && !out.join(MARKER).exists() {
            return Err(export_failed(
                format!(
                    "`{}` is not empty and is not a previous export (no `{MARKER}`)",
                    out.display()
                ),
                HELP,
            ));
        }
    }
    Ok(out)
}

/// Absolute and lexically clean, with the deepest existing ancestor resolved through symlinks (the
/// folder itself need not exist yet).
fn absolute_through_symlinks(path: &Path) -> Result<PathBuf, String> {
    let absolute =
        std::path::absolute(path).map_err(|err| format!("`{}`: {err}", path.display()))?;
    let mut clean = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    let mut tail = Vec::new();
    let mut head = clean.as_path();
    while !head.exists() {
        let (Some(parent), Some(name)) = (head.parent(), head.file_name()) else {
            break;
        };
        tail.push(name.to_os_string());
        head = parent;
    }
    let mut resolved = head
        .canonicalize()
        .map_err(|err| format!("`{}`: {err}", head.display()))?;
    resolved.extend(tail.iter().rev());
    Ok(resolved)
}

/// Checks Xcode's SDK for the target and the rustup target exist.
fn toolchain(target: IosTarget) -> Result<(), Diagnostic> {
    toolchain_with(target, "xcrun", "rustup")
}

/// [`toolchain`] with the two programs named, so a test can point at ones that do not exist. A
/// missing `rustup` skips its half (a toolchain not managed by it) and lets cargo fail instead.
fn toolchain_with(target: IosTarget, xcrun: &str, rustup: &str) -> Result<(), Diagnostic> {
    let sdk = target.sdk();
    let sdk_path = Command::new(xcrun)
        .args(["--sdk", sdk, "--show-sdk-path"])
        .stderr(Stdio::null())
        .output();
    let sdk_found =
        matches!(&sdk_path, Ok(o) if o.status.success() && !o.stdout.trim_ascii().is_empty());
    if !sdk_found {
        return Err(Diagnostic::new(
            EXPORT_TOOLCHAIN_UNAVAILABLE,
            format!("`{xcrun} --sdk {sdk} --show-sdk-path` found no SDK — an iOS export needs Xcode"),
        )
        .with_help(
            "install Xcode, open it once, and select it with `xcode-select -s /Applications/Xcode.app`",
        ));
    }
    if let Ok(installed) = Command::new(rustup)
        .args(["target", "list", "--installed"])
        .stderr(Stdio::null())
        .output()
    {
        let installed = String::from_utf8_lossy(&installed.stdout);
        let triple = target.triple();
        if !installed.lines().any(|line| line.trim() == triple) {
            return Err(Diagnostic::new(
                EXPORT_TOOLCHAIN_UNAVAILABLE,
                format!("the rustup target `{triple}` is not installed"),
            )
            .with_help(format!("run `rustup target add {triple}`")));
        }
    }
    Ok(())
}

/// Builds the game's library as a static archive for `target` and returns its path.
fn build_staticlib(project: &Path, target: IosTarget, debug: bool) -> Result<PathBuf, Diagnostic> {
    const HELP: &str = "cargo printed its own errors above; fix the game's build and rerun";
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .args(["rustc", "--manifest-path"])
        .arg(project.join("Cargo.toml"))
        .args([
            "--lib",
            "--crate-type",
            "staticlib",
            "--target",
            target.triple(),
        ])
        .args(["--release", "--no-default-features"])
        .env("IPHONEOS_DEPLOYMENT_TARGET", "16.0");
    if debug {
        command.args(["--features", "wire"]);
    }
    command
        .arg("--message-format=json-render-diagnostics")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let output = command.output().map_err(|err| {
        export_failed(
            format!("could not run cargo: {err}"),
            "check that `cargo` is on PATH (`cargo --version`)",
        )
    })?;
    if !output.status.success() {
        return Err(export_failed(
            format!("the staticlib build failed ({})", output.status),
            HELP,
        ));
    }
    archive_from_messages(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
        export_failed(
            "cargo reported no static archive for the game's library",
            "the game's library must build as a `staticlib`",
        )
    })
}

/// The last `compiler-artifact` message's `.a` file: the final crate built, the game's.
fn archive_from_messages(stdout: &str) -> Option<PathBuf> {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact")
        .filter_map(|message| {
            message["filenames"]
                .as_array()?
                .iter()
                .filter_map(|name| name.as_str())
                .find(|name| name.ends_with(".a"))
                .map(PathBuf::from)
        })
        .next_back()
}

/// The probe entries the bundle's store holds after the bake.
#[derive(Debug, Default, PartialEq, Eq)]
struct ProbesShipped {
    cubes: usize,
    bytes: u64,
}

/// A probe as the user knows it: its `Name`, else its entity.
fn probe_label(probe: &ProbeBake) -> String {
    probe
        .name
        .clone()
        .unwrap_or_else(|| format!("entity {}", probe.entity.index()))
}

/// Checks each baked probe has its entry in the bundle's store. Reports the first missing one, the
/// rest as `related`: the bake drops a failed write silently, so this is the only catch.
fn check_probe_entries(
    content_root: &Path,
    baked: &[(PathBuf, ProbeBakeReport)],
) -> Result<ProbesShipped, Diagnostic> {
    let store = content_root.join(".ludion/cooked/probes");
    let mut shipped = ProbesShipped::default();
    let mut missing = Vec::new();
    for (scene, report) in baked {
        let scene = scene
            .strip_prefix(content_root)
            .unwrap_or(scene)
            .to_string_lossy()
            .replace('\\', "/");
        for probe in &report.probes {
            let entry = store.join(ludion_assets::cooked::cooked_file_name(probe.key));
            match std::fs::metadata(&entry) {
                Ok(meta) => {
                    shipped.cubes += 1;
                    shipped.bytes += meta.len();
                }
                Err(_) => missing.push(format!(
                    "reflection probe `{}` of `{scene}` has no entry in the bundle's probe store",
                    probe_label(probe)
                )),
            }
        }
    }
    let mut missing = missing.into_iter();
    let Some(first) = missing.next() else {
        return Ok(shipped);
    };
    let mut diagnostic = Diagnostic::new(REFLECTION_PROBE_NOT_BUNDLED, first).with_help(
        "rerun `ludion export`; the bake could not write the entry (disk full or permissions?)",
    );
    for message in missing {
        diagnostic =
            diagnostic.with_related(Diagnostic::new(REFLECTION_PROBE_NOT_BUNDLED, message));
    }
    Err(diagnostic)
}

/// Bakes every probe of the bundle's scenes into the bundle's store through the game binary
/// (`--bake-probes`, at the quality the device resolves) and checks the entries landed. A project
/// without probes builds nothing.
fn bake_bundle_probes(
    project: &Path,
    manifest: &Manifest,
    content_root: &Path,
) -> Result<ProbesShipped, Failure> {
    let scene_dir = manifest
        .default_scene
        .parent()
        .expect("manifest::load guarantees a parent directory");
    let scenes = crate::bake_probes::probe_scenes(&content_root.join(scene_dir));
    if scenes.is_empty() {
        return Ok(ProbesShipped::default());
    }
    // Explicit, so a developer's `LUDION_QUALITY` cannot leak in: the flag beats the environment.
    let quality = manifest.ios_quality()?.unwrap_or("full");
    let bin = build_host_bin(project)?;
    let mut baked = Vec::new();
    for scene in scenes {
        let rel = scene.strip_prefix(content_root).unwrap_or(&scene);
        let failed = |detail: String| {
            export_failed(
                format!("baking the probes of `{}` failed: {detail}", rel.display()),
                "the game's own output is above; fix it and rerun",
            )
        };
        let output = Command::new(&bin)
            .arg("--scene")
            .arg(&scene)
            .args(["--bake-probes", "--quality", quality])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|err| failed(format!("could not run `{}`: {err}", bin.display())))?;
        let report = parse_bake_report(&String::from_utf8_lossy(&output.stdout));
        let ok = report.as_ref().is_some_and(|(ok, _, _)| *ok);
        if !output.status.success() || !ok {
            let detail = report
                .map(|(_, _, diagnostics)| diagnostics)
                .filter(|diagnostics| !diagnostics.is_empty())
                .unwrap_or_else(|| format!("{}", output.status));
            return Err(failed(detail).into());
        }
        let (_, report, _) = report.expect("ok implies a parsed report");
        baked.push((scene, report));
    }
    Ok(check_probe_entries(content_root, &baked)?)
}

/// `(ok, report, diagnostic messages)` from the JSON a game's `--bake-probes` prints
/// ([`ProbeBakeReport::to_json`]'s shape).
fn parse_bake_report(stdout: &str) -> Option<(bool, ProbeBakeReport, String)> {
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let probes = json["probes"]
        .as_array()?
        .iter()
        .map(|probe| {
            Some(ProbeBake {
                entity: serde_json::from_value(probe["entity"].clone()).ok()?,
                name: probe["name"].as_str().map(str::to_string),
                key: u64::from_str_radix(probe["key"].as_str()?, 16).ok()?,
                outcome: serde_json::from_value::<ProbeOutcome>(probe["outcome"].clone()).ok()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let diagnostics = json["diagnostics"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|d| d["message"].as_str())
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    Some((
        json["ok"].as_bool()?,
        ProbeBakeReport {
            store: json["store"].as_str().unwrap_or_default().into(),
            probes,
        },
        diagnostics,
    ))
}

/// Builds the project's host binary (default features: `--bake-probes` lives behind `wire`) and
/// returns its path. Debug is enough: the bake is GPU work.
fn build_host_bin(project: &Path) -> Result<PathBuf, Diagnostic> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["build", "--manifest-path"])
        .arg(project.join("Cargo.toml"))
        .args(["--bins", "--message-format=json-render-diagnostics"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|err| {
            export_failed(
                format!("could not run cargo: {err}"),
                "check that `cargo` is on PATH (`cargo --version`)",
            )
        })?;
    if !output.status.success() {
        return Err(export_failed(
            format!(
                "the host binary build, needed to bake reflection probes, failed ({})",
                output.status
            ),
            "cargo printed its own errors above; fix the game's build and rerun",
        ));
    }
    executable_from_messages(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
        export_failed(
            "cargo reported no executable for the game",
            "the game needs a `[[bin]]` target to bake its probes",
        )
    })
}

/// The last `compiler-artifact` message's `executable`: the final crate built, the game's.
fn executable_from_messages(stdout: &str) -> Option<PathBuf> {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact")
        .filter_map(|message| message["executable"].as_str().map(PathBuf::from))
        .next_back()
}

/// Lays the export folder down: the substituted template, the archive, the content, the marker.
fn write_export(
    out: &Path,
    project: &Path,
    manifest: &Manifest,
    substitutions: &Substitutions,
    archive: &Path,
    replays: &[PathBuf],
) -> Result<Shipped, Diagnostic> {
    let scene_dir = manifest
        .default_scene
        .parent()
        .expect("manifest::load guarantees a parent directory");
    let content_src = project.join(scene_dir);
    if !content_src.is_dir() {
        return Err(export_failed(
            format!(
                "the default scene's directory `{}` does not exist",
                content_src.display()
            ),
            "check `default_scene` in `ludion.toml`",
        ));
    }

    for template in TEMPLATES {
        write_file(
            &out.join(substitutions.apply(template.rel_path)),
            &substitutions.apply(template.body),
        )?;
    }

    let archive_dst = out.join("Ludion/libgame.a");
    create_parent(&archive_dst)?;
    ludion_core::atomic::copy_atomic(archive, &archive_dst)
        .map_err(|err| output_io(format!("could not copy `{}`: {err}", archive.display())))?;

    let content_root = out.join("content");
    match std::fs::remove_dir_all(&content_root) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(output_io(format!(
                "could not clear `{}`: {err}",
                content_root.display()
            )));
        }
    }
    copy_tree(&content_src, &content_root.join(scene_dir))?;
    // The app's host finds it by walking up from the scene directory, so its `[audio]` caps apply.
    let manifest_dst = content_root.join("ludion.toml");
    create_parent(&manifest_dst)?;
    ludion_core::atomic::copy_atomic(&project.join("ludion.toml"), &manifest_dst)
        .map_err(|err| output_io(format!("could not copy `ludion.toml`: {err}")))?;
    for import in &manifest.imports {
        let derived = project.join(&import.to);
        if !derived.is_dir() {
            return Err(export_failed(
                format!(
                    "the `[[import]]` tree `{}` does not exist",
                    derived.display()
                ),
                "run `ludion sync`",
            ));
        }
    }
    // A sync may be mid-swap on these trees (`ludion dev`, the editor's game slot): wait for it.
    let locks = crate::sync::lock_imports(project, &manifest.imports)?;
    for import in &manifest.imports {
        // The sync record (`sync::RECORD_FILE`) is bookkeeping, not content: `copy_tree` skips it
        // with every other dot-file.
        copy_tree(&project.join(&import.to), &content_root.join(&import.to))?;
    }
    let shipped = ship_cooked(project, manifest, &content_root)?;
    drop(locks);
    for replay in replays {
        let name = replay.file_name().expect("check_flags made it a file");
        let dst = content_root.join(name);
        if dst.exists() {
            return Err(export_failed(
                format!(
                    "two `--replay` files are named `{}`",
                    name.to_string_lossy()
                ),
                "rename one: replays are copied into the bundle by file name",
            ));
        }
        ludion_core::atomic::copy_atomic(replay, &dst)
            .map_err(|err| output_io(format!("could not copy `{}`: {err}", replay.display())))?;
    }

    write_file(&out.join(MARKER), "")?;
    Ok(shipped)
}

/// What the bundle took from the `ios` cooked store.
#[derive(Debug, Default)]
struct Shipped {
    maps: usize,
    bytes: u64,
    /// Referenced maps with no store entry: they load their PNG on the device.
    uncooked: usize,
    /// Meshes or maps the scan could not read: the referenced set may be incomplete.
    unscanned: usize,
}

/// Copies the `ios` store entries the import trees reference into the bundle, beside the PNGs
/// (the app's host finds the store from the bundled manifest). A missing entry is counted, not an
/// error: that map falls back to its PNG.
fn ship_cooked(
    project: &Path,
    manifest: &Manifest,
    content_root: &Path,
) -> Result<Shipped, Diagnostic> {
    use ludion_assets::cooked::CookTarget;

    let mut shipped = Shipped::default();
    if manifest
        .cook
        .as_ref()
        .is_some_and(|section| !section.textures)
    {
        return Ok(shipped);
    }
    let mut trees = Vec::new();
    for import in &manifest.imports {
        let tree = crate::cook::CookTree::whole(project.join(&import.to), import.max_texture_size)
            .map_err(|err| output_io(format!("could not read `{}`: {err}", import.to.display())))?;
        trees.push(tree);
    }
    let referenced = crate::cook::referenced_files(project, &trees, CookTarget::Ios);
    shipped.unscanned = referenced.failed.len();
    let store = crate::cook::store_dir(project, CookTarget::Ios);
    let dst_dir = content_root
        .join(".ludion/cooked")
        .join(CookTarget::Ios.name());
    for name in &referenced.files {
        let src = store.join(name);
        let Ok(meta) = std::fs::metadata(&src) else {
            shipped.uncooked += 1;
            continue;
        };
        let dst = dst_dir.join(name);
        create_parent(&dst)?;
        ludion_core::atomic::copy_atomic(&src, &dst)
            .map_err(|err| output_io(format!("could not copy `{}`: {err}", src.display())))?;
        shipped.maps += 1;
        shipped.bytes += meta.len();
    }
    Ok(shipped)
}

/// What the template's `{{…}}` placeholders become.
struct Substitutions<'a> {
    /// `{{name}}`: the manifest's `name`.
    name: &'a str,
    /// `{{bundle_id}}`: the `[ios]` bundle identifier.
    bundle_id: &'a str,
    /// `{{quality}}`: the `[ios]` quality preset, empty when the manifest names none.
    quality: &'a str,
}

impl Substitutions<'_> {
    /// The dumb `{{…}}` substitution.
    fn apply(&self, body: &str) -> String {
        body.replace("{{name}}", self.name)
            .replace("{{bundle_id}}", self.bundle_id)
            .replace("{{quality}}", self.quality)
    }
}

fn create_parent(path: &Path) -> Result<(), Diagnostic> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| output_io(format!("could not create `{}`: {err}", parent.display())))?;
    }
    Ok(())
}

fn write_file(path: &Path, body: &str) -> Result<(), Diagnostic> {
    ludion_core::atomic::write_atomic(path, body)
        .map_err(|err| output_io(format!("could not write `{}`: {err}", path.display())))
}

/// Copies a directory tree, skipping dot-files.
fn copy_tree(src: &Path, dst: &Path) -> Result<(), Diagnostic> {
    std::fs::create_dir_all(dst)
        .map_err(|err| output_io(format!("could not create `{}`: {err}", dst.display())))?;
    let read_failed = |err: std::io::Error| {
        Diagnostic::new(
            SCENE_IO,
            format!("could not read `{}`: {err}", src.display()),
        )
        .with_help("check the folder's permissions")
    };
    let entries = std::fs::read_dir(src).map_err(read_failed)?;
    for entry in entries {
        let entry = entry.map_err(read_failed)?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            ludion_core::atomic::copy_atomic(&from, &to)
                .map_err(|err| output_io(format!("could not copy `{}`: {err}", from.display())))?;
        }
    }
    Ok(())
}

/// The `xcodebuild` arguments that build the exported project, in the one (Release) configuration.
fn xcodebuild_args(out: &Path, name: &str, target: IosTarget) -> Vec<String> {
    let mut args = vec![
        "-project".to_string(),
        format!("{}/{name}.xcodeproj", out.display()),
        "-target".into(),
        name.into(),
        "-sdk".into(),
        target.sdk().into(),
        "-configuration".into(),
        "Release".into(),
    ];
    match target {
        IosTarget::IosSim => args.push("CODE_SIGNING_ALLOWED=NO".into()),
        IosTarget::Ios => {
            args.push("DEVELOPMENT_TEAM=<your team id>".into());
            args.push("-allowProvisioningUpdates".into());
        }
    }
    args.push(format!("SYMROOT={}/build", out.display()));
    args.push("build".into());
    args
}

/// A simulator device `simctl` can install into.
#[derive(Debug, PartialEq, Eq)]
struct Simulator {
    udid: String,
    name: String,
    booted: bool,
}

/// The first booted simulator, else the first available iPhone of the newest iOS runtime, from
/// `xcrun simctl list devices available -j`.
fn pick_simulator(list_json: &str) -> Option<Simulator> {
    let list: serde_json::Value = serde_json::from_str(list_json).ok()?;
    let mut runtimes: Vec<(Vec<u32>, &Vec<serde_json::Value>)> = list["devices"]
        .as_object()?
        .iter()
        .filter_map(|(key, devices)| {
            let version = key.rsplit_once("SimRuntime.iOS-")?.1;
            let version: Option<Vec<u32>> =
                version.split('-').map(|part| part.parse().ok()).collect();
            Some((version?, devices.as_array()?))
        })
        .collect();
    runtimes.sort_by(|a, b| b.0.cmp(&a.0));
    let devices = || {
        runtimes
            .iter()
            .flat_map(|(_, devices)| devices.iter())
            .filter(|device| device["isAvailable"].as_bool().unwrap_or(true))
            .filter_map(|device| {
                Some(Simulator {
                    udid: device["udid"].as_str()?.to_string(),
                    name: device["name"].as_str()?.to_string(),
                    booted: device["state"] == "Booted",
                })
            })
    };
    devices()
        .find(|device| device.booted)
        .or_else(|| devices().find(|device| device.name.starts_with("iPhone")))
}

/// Runs `program` with inherited stdio; an error if it cannot start or exits non-zero.
fn stream(program: &str, args: &[String]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|err| format!("could not run `{program}`: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{program} {}` failed ({status})", args.join(" ")))
    }
}

/// Builds the exported project for the simulator, installs it on a simulator and launches it,
/// blocking until the app exits.
fn simulator_lane(
    out: &Path,
    name: &str,
    bundle_id: &str,
    app_args: &[String],
) -> Result<(), String> {
    let mut build = xcodebuild_args(out, name, IosTarget::IosSim);
    build.insert(0, "-quiet".into());
    stream("xcodebuild", &build)?;
    let app = out
        .join("build/Release-iphonesimulator")
        .join(format!("{name}.app"));

    let listing = Command::new("xcrun")
        .args(["simctl", "list", "devices", "available", "-j"])
        .stderr(Stdio::inherit())
        .output()
        .map_err(|err| format!("could not run `xcrun simctl`: {err}"))?;
    let simulator = pick_simulator(&String::from_utf8_lossy(&listing.stdout)).ok_or(
        "no simulator to run on: `xcrun simctl list devices available` shows no booted device or \
         iPhone — create one in Xcode (Window > Devices and Simulators)",
    )?;
    println!("simulator: {} ({})", simulator.udid, simulator.name);
    let udid = simulator.udid;
    if !simulator.booted {
        stream("xcrun", &["simctl".into(), "boot".into(), udid.clone()])?;
        stream(
            "xcrun",
            &["simctl".into(), "bootstatus".into(), udid.clone()],
        )?;
    }
    stream(
        "xcrun",
        &[
            "simctl".into(),
            "install".into(),
            udid.clone(),
            app.display().to_string(),
        ],
    )?;
    let mut launch: Vec<String> = [
        "simctl",
        "launch",
        "--console-pty",
        "--terminate-running-process",
    ]
    .map(String::from)
    .into();
    launch.push(udid);
    launch.push(bundle_id.into());
    launch.extend(app_args.iter().cloned());
    stream("xcrun", &launch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baked(key: u64, name: Option<&str>) -> ProbeBake {
        ProbeBake {
            entity: ludion_core::ids::EntityId::new(7, 0),
            name: name.map(str::to_string),
            key,
            outcome: ProbeOutcome::Captured,
        }
    }

    fn bundle_with_entries(tag: &str, keys: &[u64]) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("ludion-export-probes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = root.join(".ludion/cooked/probes");
        std::fs::create_dir_all(&store).unwrap();
        for key in keys {
            std::fs::write(
                store.join(ludion_assets::cooked::cooked_file_name(*key)),
                [0u8; 10],
            )
            .unwrap();
        }
        root
    }

    #[test]
    fn present_probe_entries_pass_and_are_counted() {
        let root = bundle_with_entries("ok", &[1, 2]);
        let report = ProbeBakeReport {
            store: PathBuf::new(),
            probes: vec![baked(1, Some("a")), baked(2, None)],
        };
        let shipped = check_probe_entries(&root, &[(root.join("scenes/m.yaml"), report)]).unwrap();
        assert_eq!(
            shipped,
            ProbesShipped {
                cubes: 2,
                bytes: 20
            }
        );
    }

    #[test]
    fn a_missing_probe_entry_names_the_scene_and_the_probe() {
        let root = bundle_with_entries("miss", &[1]);
        let report = ProbeBakeReport {
            store: PathBuf::new(),
            probes: vec![baked(1, Some("a")), baked(2, Some("lamp"))],
        };
        let err = check_probe_entries(&root, &[(root.join("scenes/m.yaml"), report)]).unwrap_err();
        assert_eq!(err.code, REFLECTION_PROBE_NOT_BUNDLED);
        assert!(err.message.contains("`lamp`") && err.message.contains("scenes/m.yaml"));
        assert!(err.related.is_empty());
    }

    #[test]
    fn several_missing_entries_are_one_diagnostic_with_related() {
        let root = bundle_with_entries("two", &[]);
        let report = ProbeBakeReport {
            store: PathBuf::new(),
            probes: vec![baked(1, None), baked(2, None)],
        };
        let err = check_probe_entries(&root, &[(root.join("m.yaml"), report)]).unwrap_err();
        assert!(err.message.contains("entity 7"));
        assert_eq!(err.related.len(), 1);
    }

    #[test]
    fn the_last_artifacts_executable_is_the_host_binary() {
        let stdout = concat!(
            "{\"reason\":\"compiler-artifact\",\"executable\":\"/t/dep-bin\"}\n",
            "{\"reason\":\"compiler-artifact\",\"executable\":\"/t/game\"}\n",
            "{\"reason\":\"compiler-artifact\",\"executable\":null}\n",
            "{\"reason\":\"build-finished\",\"success\":true}\n",
        );
        assert_eq!(
            executable_from_messages(stdout),
            Some(PathBuf::from("/t/game"))
        );
        assert_eq!(executable_from_messages("not json\n"), None);
    }

    #[test]
    fn a_probe_less_bundle_builds_no_host_binary() {
        let root =
            std::env::temp_dir().join(format!("ludion-export-noprobe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let content = root.join("content");
        std::fs::create_dir_all(content.join("scenes")).unwrap();
        std::fs::write(
            content.join("scenes/main.yaml"),
            "format: 1\nentities: []\n",
        )
        .unwrap();
        std::fs::write(
            root.join("ludion.toml"),
            "name = \"probe\"\ndefault_scene = \"scenes/main.yaml\"\n",
        )
        .unwrap();
        let manifest = manifest::load(&root).unwrap();
        // The project has no Cargo.toml: any build attempt would fail.
        let shipped = bake_bundle_probes(&root, &manifest, &content).ok().unwrap();
        assert_eq!(shipped, ProbesShipped::default());
    }

    #[test]
    fn a_missing_xcrun_is_the_toolchain_diagnostic() {
        let err =
            toolchain_with(IosTarget::IosSim, "ludion-no-such-program", "rustup").unwrap_err();
        assert_eq!(err.code, EXPORT_TOOLCHAIN_UNAVAILABLE);
    }

    #[test]
    fn the_import_trees_ship_without_the_sync_record() {
        let root = std::env::temp_dir().join(format!("ludion-export-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        let derived = project.join("assets/derived/kit");
        std::fs::create_dir_all(&derived).unwrap();
        std::fs::create_dir_all(project.join("scenes")).unwrap();
        std::fs::write(project.join("scenes/main.yaml"), "entities: []\n").unwrap();
        std::fs::write(derived.join("model.prefab.yaml"), "kind: prefab\n").unwrap();
        std::fs::write(project.join("assets/derived/.ludion-sync.json"), "{}\n").unwrap();
        std::fs::write(
            project.join("ludion.toml"),
            "name = \"probe\"\ndefault_scene = \"scenes/main.yaml\"\n\n[[import]]\nfrom = \"art/*/export/*.gltf\"\nto = \"assets/derived\"\n",
        )
        .unwrap();
        let manifest = manifest::load(&project).unwrap();
        let archive = root.join("libgame.a");
        std::fs::write(&archive, "archive").unwrap();
        let out = root.join("out");
        let substitutions = Substitutions {
            name: "probe",
            bundle_id: "dev.ludion.probe",
            quality: "",
        };

        write_export(&out, &project, &manifest, &substitutions, &archive, &[]).unwrap();

        assert!(out.join("content/scenes/main.yaml").is_file());
        assert!(out.join("content/ludion.toml").is_file());
        assert!(
            out.join("content/assets/derived/kit/model.prefab.yaml")
                .is_file()
        );
        assert!(
            !out.join("content/assets/derived/.ludion-sync.json")
                .exists()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project whose `kit` mesh names two maps, with an `ios` store holding both (and a stale
    /// entry) and a `desktop` store entry. Returns the root, the project and the store file names.
    fn cooked_project(tag: &str, cook_table: &str) -> (PathBuf, PathBuf, [String; 2]) {
        use ludion_assets::cooked::{ColorSpace, CookSettings, CookTarget, cook_key};

        let root = std::env::temp_dir().join(format!("ludion-export-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        let kit = project.join("assets/derived/kit");
        std::fs::create_dir_all(kit.join("textures")).unwrap();
        std::fs::create_dir_all(project.join("scenes")).unwrap();
        std::fs::write(project.join("scenes/main.yaml"), "entities: []\n").unwrap();
        std::fs::write(
            kit.join("model.mesh.yaml"),
            "format: 1\nkind: mesh\nname: model\n\
             positions:\n  - 0\n  - 0\n  - 0\n  - 1\n  - 0\n  - 0\n  - 0\n  - 1\n  - 0\n\
             uvs:\n  - 0\n  - 0\n  - 1\n  - 0\n  - 0\n  - 1\n\
             indices:\n  - 0\n  - 1\n  - 2\n\
             base_color_texture:\n  path: \"textures/b.png\"\n\
             normal_texture:\n  path: \"textures/n.png\"\n",
        )
        .unwrap();
        let maps = [("b", ColorSpace::Srgb), ("n", ColorSpace::Linear)];
        let mut names = Vec::new();
        for (name, color_space) in maps {
            let bytes = format!("png {name}").into_bytes();
            std::fs::write(kit.join(format!("textures/{name}.png")), &bytes).unwrap();
            let settings = CookSettings {
                target: CookTarget::Ios,
                color_space,
                max_texture_size: None,
                alpha_cutoff: None,
            };
            let file = ludion_assets::cooked::cooked_file_name(cook_key(
                ludion_assets::content_hash(&bytes),
                &settings,
            ));
            let ios = project.join(".ludion/cooked/ios");
            std::fs::create_dir_all(&ios).unwrap();
            std::fs::write(ios.join(&file), format!("ktx2 {name}")).unwrap();
            names.push(file);
        }
        std::fs::write(project.join(".ludion/cooked/ios/stale.ktx2"), "stale").unwrap();
        let desktop = project.join(".ludion/cooked/desktop");
        std::fs::create_dir_all(&desktop).unwrap();
        std::fs::write(desktop.join("0000000000000001.ktx2"), "bc7").unwrap();
        std::fs::write(
            project.join("ludion.toml"),
            format!(
                "name = \"probe\"\ndefault_scene = \"scenes/main.yaml\"\n\n[[import]]\nfrom = \"art/*/export/*.gltf\"\nto = \"assets/derived\"\n{cook_table}"
            ),
        )
        .unwrap();
        let [b, n] = <[String; 2]>::try_from(names).unwrap();
        (root, project, [b, n])
    }

    fn export_cooked(root: &Path, project: &Path) -> Shipped {
        let manifest = manifest::load(project).unwrap();
        let archive = root.join("libgame.a");
        std::fs::write(&archive, "archive").unwrap();
        let substitutions = Substitutions {
            name: "probe",
            bundle_id: "dev.ludion.probe",
            quality: "",
        };
        write_export(
            &root.join("out"),
            project,
            &manifest,
            &substitutions,
            &archive,
            &[],
        )
        .unwrap()
    }

    #[test]
    fn the_referenced_ios_maps_ship_beside_the_pngs() {
        let (root, project, [b, n]) = cooked_project("cooked", "");
        let shipped = export_cooked(&root, &project);
        let store = root.join("out/content/.ludion/cooked/ios");
        assert_eq!(std::fs::read(store.join(&b)).unwrap(), b"ktx2 b");
        assert_eq!(std::fs::read(store.join(&n)).unwrap(), b"ktx2 n");
        assert!(!store.join("stale.ktx2").exists());
        assert!(!root.join("out/content/.ludion/cooked/desktop").exists());
        assert_eq!(std::fs::read_dir(&store).unwrap().count(), 2);
        let textures = root.join("out/content/assets/derived/kit/textures");
        assert!(textures.join("b.png").is_file() && textures.join("n.png").is_file());
        assert_eq!(
            (shipped.maps, shipped.uncooked, shipped.unscanned),
            (2, 0, 0)
        );
        assert_eq!(shipped.bytes, 12);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_map_without_a_store_entry_is_counted_and_the_export_goes_on() {
        let (root, project, [b, n]) = cooked_project("uncooked", "");
        std::fs::remove_file(project.join(".ludion/cooked/ios").join(&b)).unwrap();
        let shipped = export_cooked(&root, &project);
        let store = root.join("out/content/.ludion/cooked/ios");
        assert!(!store.join(&b).exists());
        assert!(store.join(&n).is_file());
        assert_eq!(
            (shipped.maps, shipped.uncooked, shipped.unscanned),
            (1, 1, 0)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cook_textures_false_ships_no_store() {
        let (root, project, _) = cooked_project("nocook", "\n[cook]\ntextures = false\n");
        let shipped = export_cooked(&root, &project);
        assert!(!root.join("out/content/.ludion").exists());
        assert_eq!(shipped.maps, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_import_trees_are_copied_under_the_sync_lock() {
        let root = std::env::temp_dir().join(format!("ludion-export-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        let derived = project.join("assets/derived/kit");
        std::fs::create_dir_all(&derived).unwrap();
        std::fs::create_dir_all(project.join("scenes")).unwrap();
        std::fs::write(project.join("scenes/main.yaml"), "entities: []\n").unwrap();
        std::fs::write(derived.join("model.prefab.yaml"), "kind: prefab\n").unwrap();
        std::fs::write(
            project.join("ludion.toml"),
            "name = \"probe\"\ndefault_scene = \"scenes/main.yaml\"\n\n[[import]]\nfrom = \"art/*/export/*.gltf\"\nto = \"assets/derived\"\n",
        )
        .unwrap();
        let manifest = manifest::load(&project).unwrap();
        let archive = root.join("libgame.a");
        std::fs::write(&archive, "archive").unwrap();
        let out = root.join("out");
        let shipped = out.join("content/assets/derived/kit/model.prefab.yaml");
        let substitutions = Substitutions {
            name: "probe",
            bundle_id: "dev.ludion.probe",
            quality: "",
        };

        let held = crate::sync::lock_imports(&project, &manifest.imports).unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let result = write_export(&out, &project, &manifest, &substitutions, &archive, &[]);
                done.send(result.is_ok()).unwrap();
            });
            // The scene directory is copied just before the lock is taken, so once it exists the
            // export has provably reached `lock_imports` and the held lock is what stops it.
            let scene = out.join("content/scenes/main.yaml");
            let mut polls = 600;
            while !scene.exists() {
                assert!(
                    matches!(
                        finished.recv_timeout(std::time::Duration::from_millis(100)),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                    ),
                    "the export finished before it reached the lock"
                );
                polls -= 1;
                assert!(polls > 0, "the export never reached the lock");
            }
            assert!(
                finished
                    .recv_timeout(std::time::Duration::from_millis(500))
                    .is_err(),
                "the export finished while a sync held the lock"
            );
            assert!(!shipped.exists(), "the tree was copied under a held lock");
            drop(held);
            assert!(
                finished
                    .recv_timeout(std::time::Duration::from_secs(60))
                    .expect("the export finishes once the lock is released")
            );
        });
        assert!(shipped.is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_placeholder_survives_substitution() {
        for template in TEMPLATES {
            for text in [template.rel_path, template.body] {
                let out = Substitutions {
                    name: "vigil",
                    bundle_id: "dev.ludion.vigil",
                    quality: "medium",
                }
                .apply(text);
                assert!(!out.contains("{{"), "{}: {out}", template.rel_path);
            }
        }
    }

    const LIST: &str = r#"{"devices": {
        "com.apple.CoreSimulator.SimRuntime.iOS-17-5": [
            {"udid": "A", "name": "iPad Pro", "state": "Shutdown", "isAvailable": true},
            {"udid": "B", "name": "iPhone 15", "state": "Shutdown", "isAvailable": true}],
        "com.apple.CoreSimulator.SimRuntime.iOS-18-0": [
            {"udid": "C", "name": "iPad Air", "state": "Shutdown", "isAvailable": true},
            {"udid": "D", "name": "iPhone 16", "state": "Shutdown", "isAvailable": true}],
        "com.apple.CoreSimulator.SimRuntime.iOS-9-3": [
            {"udid": "E", "name": "iPhone 6", "state": "Shutdown", "isAvailable": true}],
        "com.apple.CoreSimulator.SimRuntime.watchOS-11-0": [
            {"udid": "W", "name": "iPhone-paired Watch", "state": "Shutdown", "isAvailable": true}]
    }}"#;

    #[test]
    fn the_newest_ios_runtimes_first_iphone_is_picked() {
        let picked = pick_simulator(LIST).expect("a simulator");
        assert_eq!(
            (picked.udid.as_str(), picked.booted),
            ("D", false),
            "iOS 18 beats 17.5 and 9.3 by version, an iPhone beats an iPad, watchOS is ignored"
        );
    }

    #[test]
    fn a_booted_simulator_wins_over_a_newer_iphone() {
        let list = LIST.replace(
            r#""udid": "A", "name": "iPad Pro", "state": "Shutdown""#,
            r#""udid": "A", "name": "iPad Pro", "state": "Booted""#,
        );
        let picked = pick_simulator(&list).expect("a simulator");
        assert_eq!((picked.udid.as_str(), picked.booted), ("A", true));
    }

    #[test]
    fn no_simulator_is_none() {
        assert_eq!(pick_simulator(r#"{"devices": {}}"#), None);
        assert_eq!(pick_simulator("not json"), None);
        let ipads = r#"{"devices": {"com.apple.CoreSimulator.SimRuntime.iOS-18-0": [
            {"udid": "C", "name": "iPad Air", "state": "Shutdown"}]}}"#;
        assert_eq!(pick_simulator(ipads), None);
        let unavailable = r#"{"devices": {"com.apple.CoreSimulator.SimRuntime.iOS-18-0": [
            {"udid": "D", "name": "iPhone 16", "state": "Shutdown", "isAvailable": false}]}}"#;
        assert_eq!(pick_simulator(unavailable), None);
    }

    #[test]
    fn the_archive_is_the_last_artifact_with_one() {
        let stdout = concat!(
            "{\"reason\":\"compiler-artifact\",\"filenames\":[\"/t/libdep.rlib\"]}\n",
            "{\"reason\":\"compiler-artifact\",\"filenames\":[\"/t/libdep.a\"]}\n",
            "{\"reason\":\"compiler-artifact\",\"filenames\":[\"/t/libgame.a\"]}\n",
            "{\"reason\":\"build-finished\",\"success\":true}\n",
        );
        assert_eq!(
            archive_from_messages(stdout),
            Some(PathBuf::from("/t/libgame.a"))
        );
    }
}
