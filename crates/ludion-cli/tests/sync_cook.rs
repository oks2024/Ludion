//! Integration tests for the cook stage of `ludion sync`, driven against the built binary on scratch
//! projects whose every texture map is unique, so each cooked map has its own store key. A first
//! sync cooks each map and a second reuses all of them; an edited source re-cooks alone and the
//! sweep drops its old entry; colour maps cook as sRGB and data maps as linear; the store is
//! byte-identical at 1, 2 and 8 threads; `--no-cook` leaves the store alone; `--target ios` adds
//! the ASTC store; `max_texture_size` caps level 0 and moves the key; a bad `[cook]` key is refused
//! as `L0023`; planted debris is swept; a sync killed mid-cook leaves only whole files and the next
//! sync completes the store; and a map that cannot be cooked fails the run without undoing the
//! import. A `[cook] roots` project (no `[[import]]` table) cooks every map the root's meshes
//! reference, reuses them, sweeps what no mesh references, refuses an absent root or a link under it
//! (`L1324`) without sweeping, and warns once (`L1325`) about a map that leaves its root; an
//! import-only project keeps its keys, store and record.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Output};
use std::time::Duration;

use ludion_assets::content_hash;
use ludion_assets::cooked::{ColorSpace, CookSettings, CookTarget, cook_key, read_ktx2};

fn textured() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-textured/scene.gltf")
}

/// A per-process scratch project root, removed when the test passes (kept on a panic for
/// debugging); the pid keeps concurrent test runs of this file off each other's trees.
struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sync-cook-{name}-{}", std::process::id()));
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        assert!(
            e.kind() == std::io::ErrorKind::NotFound,
            "stale scratch dir {dir:?} could not be removed: {e}"
        );
    }
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

fn run_ludion(args: &[&str]) -> Output {
    ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

const MANIFEST: &str = "name = \"probe\"\ndefault_scene = \"scenes/main.yaml\"\n";

/// Writes `ludion.toml` with one `[[import]]` table (plus `import_extra` keys) and then `tail`.
fn write_manifest(dir: &Path, import_extra: &str, tail: &str) {
    std::fs::write(
        dir.join("ludion.toml"),
        format!(
            "{MANIFEST}\n[[import]]\nfrom = \"kits/*/export/*.gltf\"\nto = \"assets/derived\"\n{import_extra}\n{tail}"
        ),
    )
    .unwrap();
}

/// A square RGBA PNG whose pixels depend on `seed`, so no two seeds share a file hash.
fn png_bytes(size: u32, seed: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let wave = x.wrapping_mul(3).wrapping_add(y.wrapping_mul(5)) ^ seed.wrapping_mul(37);
            pixels.extend_from_slice(&[
                wave as u8,
                (x ^ seed) as u8,
                (y.wrapping_add(seed)) as u8,
                255,
            ]);
        }
    }
    writer.write_image_data(&pixels).unwrap();
    drop(writer);
    out
}

fn kit_seed(kit: &str, image: usize) -> u32 {
    kit.bytes()
        .fold(7u32, |acc, byte| {
            acc.wrapping_mul(31).wrapping_add(u32::from(byte))
        })
        .wrapping_mul(16)
        .wrapping_add(image as u32)
}

/// Writes kit `kit`'s glTF (the textured fixture with every image pointed at its own PNG file).
fn write_kit(dir: &Path, kit: &str, size: u32) {
    let export = dir.join("kits").join(kit).join("export");
    std::fs::create_dir_all(&export).unwrap();
    let mut gltf: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(textured()).unwrap()).unwrap();
    let images = gltf["images"].as_array_mut().unwrap();
    for (index, image) in images.iter_mut().enumerate() {
        let name = format!("img{index}.png");
        image["uri"] = serde_json::Value::String(name.clone());
        std::fs::write(export.join(name), png_bytes(size, kit_seed(kit, index))).unwrap();
    }
    std::fs::write(
        export.join(format!("{kit}.gltf")),
        serde_json::to_string_pretty(&gltf).unwrap(),
    )
    .unwrap();
}

/// A project of the named kits, each with unique maps of `size`², and one `[[import]]`.
fn unique_maps_project(name: &str, kits: &[&str], size: u32) -> Scratch {
    let dir = scratch(name);
    write_manifest(&dir, "", "");
    for kit in kits {
        write_kit(&dir, kit, size);
    }
    dir
}

/// A project whose manifest has `[cook] roots = ["scenes/assets"]` and no `[[import]]` table, with
/// each part imported by `ludion import` into `scenes/assets/group/<part>` and a stray YAML beside.
fn roots_project(name: &str, parts: &[&str]) -> Scratch {
    let dir = scratch(name);
    std::fs::write(
        dir.join("ludion.toml"),
        format!("{MANIFEST}\n[cook]\nroots = [\"scenes/assets\"]\n"),
    )
    .unwrap();
    for part in parts {
        write_kit(&dir, part, 64);
        let gltf = dir.join(format!("kits/{part}/export/{part}.gltf"));
        let out = dir.join(format!("scenes/assets/group/{part}"));
        let imported = run_ludion(&[
            "import",
            gltf.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ]);
        assert!(
            imported.status.success(),
            "{}",
            String::from_utf8_lossy(&imported.stderr)
        );
    }
    std::fs::write(dir.join("scenes/assets/group/review.yaml"), "format: 1\n").unwrap();
    dir
}

/// A directory link `link` -> `target`; a junction on Windows, which needs no privilege.
fn link_dir(link: &Path, target: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(target, link).is_err() {
        let status = ProcCommand::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap()
            .status;
        assert!(status.success(), "could not create a junction");
    }
}

fn diagnostic_codes(json: &serde_json::Value) -> Vec<String> {
    json["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap().to_string())
        .collect()
}

type Run = (bool, serde_json::Value, String);

fn sync_with(project: &Path, extra: &[&str]) -> Run {
    let mut args = vec!["sync", "--project", project.to_str().unwrap(), "--json"];
    args.extend_from_slice(extra);
    let output = run_ludion(&args);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON object ({e}): {stdout}"));
    (
        output.status.success(),
        json,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn sync(project: &Path) -> Run {
    sync_with(project, &[])
}

fn store(project: &Path, target: &str) -> PathBuf {
    project.join(".ludion/cooked").join(target)
}

/// The `(name, bytes)` of every file in a store directory, sorted by name.
fn store_files(project: &Path, target: &str) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = store(project, target)
        .read_dir()
        .map(|read| {
            read.map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    std::fs::read(path).unwrap(),
                )
            })
            .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn names(files: &[(String, Vec<u8>)]) -> BTreeSet<String> {
    files.iter().map(|(name, _)| name.clone()).collect()
}

fn png_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "png") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// One map a derived mesh references: its PNG, whether it cooks as sRGB, and the `f32` bits of the
/// mask cutoff when it is a mask mesh's base-colour map.
type Unit = (PathBuf, bool, Option<u32>);

/// The units the derived meshes reference, read straight off the mesh documents (a colour slot is
/// sRGB, the rest linear; only a mask mesh's base colour carries its cutoff), independently of the
/// cook stage.
fn expected_units(project: &Path) -> BTreeSet<Unit> {
    expected_units_under(project, "assets/derived")
}

/// [`expected_units`] over the meshes below the project-relative directory `tree`.
fn expected_units_under(project: &Path, tree: &str) -> BTreeSet<Unit> {
    let mut units = BTreeSet::new();
    let mut stack = vec![project.join(tree)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path.to_string_lossy().ends_with(".mesh.yaml") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let mask = text.lines().any(|line| line == "alpha_mode: mask");
            let cutoff = text
                .lines()
                .find_map(|line| line.strip_prefix("alpha_cutoff: "))
                .map_or(0.5, |value| value.parse::<f32>().unwrap());
            let mut slot: Option<(bool, Option<u32>)> = None;
            for line in text.lines() {
                if let Some(name) = line.strip_suffix("_texture:") {
                    let cutoff = (mask && name == "base_color").then(|| cutoff.to_bits());
                    slot = Some((matches!(name, "base_color" | "emissive"), cutoff));
                } else if let (Some((srgb, cutoff)), Some(reference)) =
                    (slot, line.trim().strip_prefix("path: "))
                {
                    let reference = reference.trim_matches('"');
                    let png = path.parent().unwrap().join(reference);
                    units.insert((std::fs::canonicalize(png).unwrap(), srgb, cutoff));
                    slot = None;
                }
            }
        }
    }
    units
}

fn settings(
    target: CookTarget,
    srgb: bool,
    cap: Option<u32>,
    alpha_cutoff: Option<u32>,
) -> CookSettings {
    CookSettings {
        target,
        color_space: if srgb {
            ColorSpace::Srgb
        } else {
            ColorSpace::Linear
        },
        max_texture_size: cap,
        alpha_cutoff: alpha_cutoff.map(f32::from_bits),
    }
}

/// The store file names the units should have for `target` and `cap`.
fn expected_names(
    units: &BTreeSet<Unit>,
    target: CookTarget,
    cap: Option<u32>,
) -> BTreeSet<String> {
    units
        .iter()
        .map(|(png, srgb, cutoff)| {
            let hash = content_hash(&std::fs::read(png).unwrap());
            ludion_assets::cooked::cooked_file_name(cook_key(
                hash,
                &settings(target, *srgb, cap, *cutoff),
            ))
        })
        .collect()
}

fn field(json: &serde_json::Value, key: &str) -> u64 {
    json[key]
        .as_u64()
        .unwrap_or_else(|| panic!("`{key}` is not a number in {json}"))
}

fn assert_clean(run: &Run) {
    assert!(run.0, "{}\n{}", run.1, run.2);
    assert_eq!(run.1["cook_failed"], serde_json::json!([]), "{}", run.1);
}

#[test]
fn a_first_sync_cooks_every_map_and_a_second_reuses_them() {
    let dir = unique_maps_project("cook-first", &["a", "b"], 64);
    let first = sync(&dir);
    assert_clean(&first);
    let units = expected_units(&dir);
    assert!(units.len() >= 10, "{} units", units.len());
    assert_eq!(field(&first.1, "cooked") as usize, units.len());
    assert_eq!(field(&first.1, "cook_reused"), 0);

    let files = store_files(&dir, "desktop");
    assert_eq!(
        names(&files),
        expected_names(&units, CookTarget::Desktop, None)
    );
    for (name, bytes) in &files {
        let parses = [ColorSpace::Srgb, ColorSpace::Linear]
            .into_iter()
            .any(|space| read_ktx2(bytes, CookTarget::Desktop, space).is_ok());
        assert!(parses, "{name} is not a cooked BC7 file");
    }
    let total: u64 = files.iter().map(|(_, bytes)| bytes.len() as u64).sum();
    assert_eq!(field(&first.1, "store_bytes"), total);

    let second = sync(&dir);
    assert_clean(&second);
    assert_eq!(field(&second.1, "cooked"), 0);
    assert_eq!(field(&second.1, "cook_reused") as usize, units.len());
    assert_eq!(store_files(&dir, "desktop"), files);
}

#[test]
fn an_edited_source_recooks_one_map_and_the_sweep_drops_the_old_entry() {
    let dir = unique_maps_project("cook-edit", &["a", "b"], 64);
    assert_clean(&sync(&dir));
    let before = names(&store_files(&dir, "desktop"));

    // Image 2 is the normal map: one role, one key.
    let source = dir.join("kits/a/export/img2.png");
    std::fs::write(&source, png_bytes(64, 9_999)).unwrap();
    let again = sync(&dir);
    assert_clean(&again);
    assert_eq!(field(&again.1, "cooked"), 1, "{}", again.1);
    assert_eq!(field(&again.1, "cook_reused") as usize, before.len() - 1);

    let after = names(&store_files(&dir, "desktop"));
    assert_eq!(after.len(), before.len());
    assert_eq!(before.difference(&after).count(), 1);
    assert_eq!(after.difference(&before).count(), 1);
    assert_eq!(
        after,
        expected_names(&expected_units(&dir), CookTarget::Desktop, None)
    );
}

#[test]
fn colour_maps_cook_as_srgb_and_data_maps_as_linear() {
    let dir = unique_maps_project("cook-roles", &["a"], 64);
    assert_clean(&sync(&dir));
    let units = expected_units(&dir);
    assert!(units.iter().any(|(_, srgb, _)| *srgb) && units.iter().any(|(_, srgb, _)| !*srgb));
    for (png, srgb, cutoff) in &units {
        let hash = content_hash(&std::fs::read(png).unwrap());
        let key = cook_key(hash, &settings(CookTarget::Desktop, *srgb, None, *cutoff));
        let bytes = std::fs::read(
            store(&dir, "desktop").join(ludion_assets::cooked::cooked_file_name(key)),
        )
        .unwrap_or_else(|e| panic!("{}: {e}", png.display()));
        let (right, wrong) = if *srgb {
            (ColorSpace::Srgb, ColorSpace::Linear)
        } else {
            (ColorSpace::Linear, ColorSpace::Srgb)
        };
        assert!(
            read_ktx2(&bytes, CookTarget::Desktop, right).is_ok(),
            "{}",
            png.display()
        );
        assert!(
            read_ktx2(&bytes, CookTarget::Desktop, wrong).is_err(),
            "{}",
            png.display()
        );
    }
}

#[test]
fn the_store_is_byte_identical_at_every_thread_count() {
    let mut stores = Vec::new();
    for threads in ["1", "2", "8"] {
        let dir = unique_maps_project(&format!("cook-threads-{threads}"), &["a", "b", "c"], 64);
        let output = run_ludion(&[
            "--threads",
            threads,
            "sync",
            "--project",
            dir.to_str().unwrap(),
            "--json",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        stores.push(store_files(&dir, "desktop"));
    }
    assert!(!stores[0].is_empty());
    assert_eq!(stores[0], stores[1], "1 vs 2 threads");
    assert_eq!(stores[0], stores[2], "1 vs 8 threads");
}

#[test]
fn no_cook_writes_no_store_and_leaves_an_existing_one_untouched() {
    let dir = unique_maps_project("cook-skip", &["a"], 64);
    let skipped = sync_with(&dir, &["--no-cook"]);
    assert!(skipped.0, "{}", skipped.2);
    assert!(!dir.join(".ludion").exists());
    assert_eq!(skipped.1["cooked"], 0);
    assert_eq!(skipped.1["cook_reused"], 0);
    assert_eq!(skipped.1["cook_failed"], serde_json::json!([]));
    assert_eq!(skipped.1["store_bytes"], 0);

    assert_clean(&sync(&dir));
    let files = store_files(&dir, "desktop");
    std::fs::write(store(&dir, "desktop").join("keep.txt"), "x").unwrap();
    let untouched = sync_with(&dir, &["--no-cook"]);
    assert!(untouched.0, "{}", untouched.2);
    assert!(store(&dir, "desktop").join("keep.txt").is_file());
    assert_eq!(
        store_files(&dir, "desktop").len(),
        files.len() + 1,
        "--no-cook must not sweep"
    );
}

#[test]
fn the_ios_target_fills_its_own_astc_store() {
    let dir = unique_maps_project("cook-ios", &["a"], 64);
    let run = sync_with(&dir, &["--target", "ios"]);
    assert_clean(&run);
    let units = expected_units(&dir);
    assert_eq!(field(&run.1, "cooked") as usize, units.len() * 2);
    let ios = store_files(&dir, "ios");
    assert_eq!(names(&ios), expected_names(&units, CookTarget::Ios, None));
    for (name, bytes) in &ios {
        let parses = [ColorSpace::Srgb, ColorSpace::Linear]
            .into_iter()
            .any(|space| read_ktx2(bytes, CookTarget::Ios, space).is_ok());
        assert!(parses, "{name} is not a cooked ASTC file");
    }
    assert_eq!(
        names(&store_files(&dir, "desktop")),
        expected_names(&units, CookTarget::Desktop, None)
    );
}

#[test]
fn max_texture_size_caps_level_zero_and_moves_the_key() {
    let dir = unique_maps_project("cook-cap", &["a"], 64);
    assert_clean(&sync(&dir));
    let uncapped = names(&store_files(&dir, "desktop"));

    write_manifest(&dir, "max_texture_size = 32\n", "");
    let capped = sync(&dir);
    assert_clean(&capped);
    let units = expected_units(&dir);
    assert_eq!(field(&capped.1, "cooked") as usize, units.len());
    let files = store_files(&dir, "desktop");
    let now = names(&files);
    assert_eq!(now, expected_names(&units, CookTarget::Desktop, Some(32)));
    assert!(now.is_disjoint(&uncapped));
    for (_, bytes) in &files {
        let image = [ColorSpace::Srgb, ColorSpace::Linear]
            .into_iter()
            .find_map(|space| read_ktx2(bytes, CookTarget::Desktop, space).ok())
            .unwrap();
        assert_eq!((image.width, image.height), (32, 32));
    }
}

#[test]
fn a_bad_cook_key_is_refused_as_l0023() {
    let dir = unique_maps_project("cook-bad-key", &["a"], 64);
    write_manifest(&dir, "", "[cook]\nbogus = 1\n");
    let (ok, json, _) = sync(&dir);
    assert!(!ok);
    let codes: Vec<&str> = json["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"L0023"), "{json}");
    assert!(!dir.join(".ludion").exists());
}

#[test]
fn planted_debris_in_the_store_is_swept() {
    let dir = unique_maps_project("cook-debris", &["a"], 64);
    assert_clean(&sync(&dir));
    let files = store_files(&dir, "desktop");
    let root = store(&dir, "desktop");
    std::fs::write(root.join("junk.tmp"), "x").unwrap();
    std::fs::write(root.join("notes.txt"), "x").unwrap();
    std::fs::write(root.join("0000000000000000.ktx2"), "x").unwrap();
    std::fs::create_dir(root.join("subdir")).unwrap();
    let again = sync(&dir);
    assert_clean(&again);
    assert_eq!(field(&again.1, "cooked"), 0);
    assert_eq!(store_files(&dir, "desktop"), files);
}

/// True when a sync of `dir` was killed while still running with some, but not all, of its maps
/// stored.
fn kill_mid_cook(dir: &Path, total: usize) -> bool {
    let mut child = ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["sync", "--project", dir.to_str().unwrap(), "--json"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to run the ludion binary");
    let ktx2 = || {
        store(dir, "desktop").read_dir().map_or(0, |read| {
            read.filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "ktx2"))
                .count()
        })
    };
    let mut polls = 0;
    let running = loop {
        let running = child.try_wait().unwrap().is_none();
        if ktx2() >= 1 || polls > 120_000 || !running {
            break running;
        }
        polls += 1;
        std::thread::sleep(Duration::from_millis(1));
    };
    let _ = child.kill();
    let _ = child.wait();
    let stored = ktx2();
    running && stored >= 1 && stored < total
}

#[test]
fn a_killed_sync_leaves_only_whole_files_and_the_next_sync_completes_the_store() {
    let kits: Vec<String> = (0..8).map(|index| format!("k{index}")).collect();
    let kit_refs: Vec<&str> = kits.iter().map(String::as_str).collect();
    let dir = (0..5)
        .find_map(|attempt| {
            let dir = unique_maps_project(&format!("cook-killed-{attempt}"), &kit_refs, 512);
            // The import tree is needed to know the total: import first, without cooking.
            let imported = sync_with(&dir, &["--no-cook"]);
            assert!(imported.0, "{}", imported.2);
            let total = expected_units(&dir).len();
            kill_mid_cook(&dir, total).then_some((dir, total))
        })
        .expect("the kill never landed mid-cook");
    let (dir, total) = dir;

    for (name, bytes) in store_files(&dir, "desktop") {
        let is_key = name.len() == 21
            && name.ends_with(".ktx2")
            && name[..16].bytes().all(|b| b.is_ascii_hexdigit());
        if is_key {
            let parses = [ColorSpace::Srgb, ColorSpace::Linear]
                .into_iter()
                .any(|space| read_ktx2(&bytes, CookTarget::Desktop, space).is_ok());
            assert!(parses, "{name} is a torn cooked file");
        }
    }

    let next = sync(&dir);
    assert_clean(&next);
    assert_eq!(
        (field(&next.1, "cooked") + field(&next.1, "cook_reused")) as usize,
        total
    );
    let files = store_files(&dir, "desktop");
    assert_eq!(files.len(), total);
    assert!(
        files
            .iter()
            .all(|(name, _)| name.ends_with(".ktx2") && name.len() == 21)
    );
}

#[test]
fn a_map_that_cannot_be_cooked_fails_the_run_without_undoing_the_import() {
    let dir = unique_maps_project("cook-fail", &["a"], 64);
    assert_clean(&sync(&dir));
    let stem = dir.join("assets/derived/a");
    let victim = png_files(&stem.join("textures")).remove(0);
    std::fs::write(&victim, b"this is not a png").unwrap();

    let (ok, json, stderr) = sync(&dir);
    assert!(!ok, "{json}");
    assert_eq!(
        json["failed"],
        serde_json::json!([]),
        "the import must not fail: {json}"
    );
    let failed = json["cook_failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1, "{json}");
    assert_eq!(failed[0]["target"], "desktop");
    assert!(
        failed[0]["map"]
            .as_str()
            .unwrap()
            .ends_with(victim.file_name().unwrap().to_str().unwrap())
    );
    assert!(!failed[0]["diagnostics"].as_array().unwrap().is_empty());
    assert!(stderr.contains("could not cook"), "{stderr}");
    assert!(
        stem.join("meshes").is_dir(),
        "the import tree stays swapped in"
    );
}

/// What `ludion sync` writes is what the engine reads: after a sync, an engine on the cooked axis
/// loads the synced meshes with every map from the store (no PNG decode, so no `L1271`).
#[test]
fn needs_gpu_a_synced_project_loads_cooked_with_no_fallback() {
    use ludion_engine::{Engine, TextureSource, profile_engine};

    match ludion_renderer::Renderer::new() {
        Ok(renderer) if renderer.cook_target().is_some() => {}
        Ok(_) => return eprintln!("SKIPPED: no BC7 or ASTC support"),
        Err(diagnostic)
            if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diagnostic)) =>
        {
            return eprintln!("SKIPPED: no GPU adapter");
        }
        Err(diagnostic) => panic!("renderer init failed: {diagnostic:?}"),
    }
    let dir = unique_maps_project("sync-then-load", &["a"], 16);
    assert_clean(&sync(&dir));

    let derived = dir.join("assets/derived");
    let mut meshes = Vec::new();
    let mut stack = vec![derived.clone()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.to_string_lossy().ends_with(".mesh.yaml") {
                let rel = path.strip_prefix(&*dir).unwrap();
                meshes.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    meshes.sort();
    assert!(!meshes.is_empty(), "the sync derived no mesh");
    let mut scene = String::from("format: 1\nentities:\n");
    for mesh in &meshes {
        scene +=
            &format!("  - components:\n      Mesh:\n        shape:\n          asset: {mesh}\n");
    }

    let mut engine = Engine::builder()
        .project_root(&*dir)
        .build()
        .expect("builds");
    engine.set_textures(TextureSource::Cooked);
    engine.observe_load();
    engine.load_inline_scene(&scene).expect("the scene loads");
    let report = profile_engine(&mut engine, "synced".into(), 1, 0).expect("profiles");
    let texture = report
        .load
        .expect("observed")
        .assets
        .into_iter()
        .find(|counters| counters.kind == "texture")
        .expect("a texture row");
    assert!(
        texture.cooked > 0,
        "no map was read from the store: {texture:?}"
    );
    assert_eq!(
        texture.decoded, 0,
        "a map fell back to its PNG: {texture:?}"
    );
}

#[test]
fn a_mask_meshs_base_colour_cooks_under_the_cutoff_key_and_the_rest_do_not() {
    let dir = scratch("cook-mask");
    write_manifest(&dir, "", "");
    write_kit(&dir, "a", 64);
    let gltf_path = dir.join("kits/a/export/a.gltf");
    let mut gltf: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&gltf_path).unwrap()).unwrap();
    for material in gltf["materials"].as_array_mut().unwrap() {
        material["alphaMode"] = "MASK".into();
        material["alphaCutoff"] = 0.5.into();
    }
    std::fs::write(&gltf_path, serde_json::to_string_pretty(&gltf).unwrap()).unwrap();

    let run = sync(&dir);
    assert_clean(&run);
    let units = expected_units(&dir);
    assert!(
        units.iter().any(|(_, _, cutoff)| cutoff.is_some()),
        "no mask base colour found: {units:?}"
    );
    assert!(
        units
            .iter()
            .any(|(_, srgb, cutoff)| !*srgb && cutoff.is_none()),
        "data maps must stay plain"
    );
    assert_eq!(field(&run.1, "cooked") as usize, units.len());
    assert_eq!(
        names(&store_files(&dir, "desktop")),
        expected_names(&units, CookTarget::Desktop, None)
    );
}

#[test]
fn a_root_without_import_cooks_every_map_then_reuses_them() {
    let dir = roots_project("roots-first", &["a", "b"]);
    let first = sync(&dir);
    assert_clean(&first);
    let units = expected_units_under(&dir, "scenes/assets");
    assert!(units.len() >= 10, "{} units", units.len());
    assert_eq!(field(&first.1, "cooked") as usize, units.len());
    let files = store_files(&dir, "desktop");
    assert_eq!(
        names(&files),
        expected_names(&units, CookTarget::Desktop, None)
    );

    let second = sync(&dir);
    assert_clean(&second);
    assert_eq!(field(&second.1, "cooked"), 0);
    assert_eq!(field(&second.1, "cook_reused") as usize, units.len());
    assert_eq!(store_files(&dir, "desktop"), files);
}

#[test]
fn a_root_map_is_swept_when_no_mesh_references_it() {
    let dir = roots_project("roots-sweep", &["a", "b"]);
    assert_clean(&sync(&dir));
    let both = expected_units_under(&dir, "scenes/assets");
    std::fs::remove_dir_all(dir.join("scenes/assets/group/b")).unwrap();
    let only_a = expected_units_under(&dir, "scenes/assets");
    assert!(only_a.len() < both.len());
    assert_clean(&sync(&dir));
    assert_eq!(
        names(&store_files(&dir, "desktop")),
        expected_names(&only_a, CookTarget::Desktop, None)
    );
}

#[test]
fn an_absent_root_is_l1324_and_sweeps_nothing() {
    let dir = roots_project("roots-absent", &["a"]);
    assert_clean(&sync(&dir));
    let before = store_files(&dir, "desktop");
    std::fs::rename(dir.join("scenes/assets"), dir.join("scenes/moved")).unwrap();
    let (ok, json, stderr) = sync(&dir);
    assert!(!ok, "{json}");
    assert!(stderr.contains("L1324"), "{stderr}");
    assert_eq!(diagnostic_codes(&json), ["L1324"]);
    assert_eq!(store_files(&dir, "desktop"), before);
}

#[test]
fn a_link_under_a_root_is_l1324_and_sweeps_nothing() {
    let dir = roots_project("roots-link", &["a"]);
    assert_clean(&sync(&dir));
    let before = store_files(&dir, "desktop");
    link_dir(&dir.join("scenes/assets/group/loop"), &dir.join("kits"));
    let (ok, json, stderr) = sync(&dir);
    assert!(!ok, "{json}");
    assert!(stderr.contains("L1324"), "{stderr}");
    assert!(diagnostic_codes(&json).contains(&"L1324".to_string()));
    assert_eq!(store_files(&dir, "desktop"), before);
}

#[test]
fn a_map_leaving_its_root_warns_l1325_and_stays_uncooked() {
    let dir = roots_project("roots-leave", &["a"]);
    let mesh = textured_mesh(&dir.join("scenes/assets/group/a"));
    let text = std::fs::read_to_string(&mesh).unwrap();
    let line = text
        .lines()
        .find(|line| line.trim().starts_with("path: "))
        .expect("a texture path")
        .to_string();
    let indent = &line[..line.len() - line.trim_start().len()];
    std::fs::write(
        &mesh,
        text.replacen(&line, &format!("{indent}path: ../../../../outside.png"), 1),
    )
    .unwrap();

    let (ok, json, stderr) = sync(&dir);
    assert!(ok, "{json}\n{stderr}");
    assert_eq!(json["cook_failed"], serde_json::json!([]), "{json}");
    assert_eq!(diagnostic_codes(&json), ["L1325"], "{json}");
    assert!(stderr.contains("L1325"), "{stderr}");
    assert!(stderr.contains("outside.png"), "{stderr}");
}

/// The first `*.mesh.yaml` below `dir`.
fn textured_mesh(dir: &Path) -> PathBuf {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.to_string_lossy().ends_with(".mesh.yaml")
                && std::fs::read_to_string(&path).unwrap().contains("path: ")
            {
                return path;
            }
        }
    }
    panic!("no textured mesh under {dir:?}");
}

#[test]
fn an_import_only_project_keeps_its_keys_store_and_record() {
    for (name, tail) in [
        ("import-only", ""),
        ("import-only-cook", "[cook]\ntargets = [\"desktop\"]\n"),
    ] {
        let dir = scratch(name);
        write_manifest(&dir, "", tail);
        write_kit(&dir, "a", 64);
        assert_clean(&sync(&dir));
        let units = expected_units(&dir);
        assert_eq!(
            names(&store_files(&dir, "desktop")),
            expected_names(&units, CookTarget::Desktop, None)
        );
        assert!(!dir.join(".ludion/.ludion-sync.lock").exists());
        let record = std::fs::read(dir.join("assets/derived/.ludion-sync.json")).unwrap();

        let second = sync(&dir);
        assert_clean(&second);
        assert_eq!(
            second.1["skipped"],
            serde_json::json!(["a"]),
            "{}",
            second.1
        );
        assert_eq!(field(&second.1, "cooked"), 0);
        assert_eq!(
            std::fs::read(dir.join("assets/derived/.ludion-sync.json")).unwrap(),
            record
        );
        assert!(!dir.join(".ludion/.ludion-sync.lock").exists());
    }
}

/// A `[cook] roots` project under a `scenes/` content root: the engine's lookup folds the scene's
/// `assets/...` spelling against that depth to find the root's maps.
#[test]
fn needs_gpu_a_root_project_loads_cooked_with_no_fallback() {
    use ludion_engine::{Engine, TextureSource, profile_engine};

    match ludion_renderer::Renderer::new() {
        Ok(renderer) if renderer.cook_target().is_some() => {}
        Ok(_) => return eprintln!("SKIPPED: no BC7 or ASTC support"),
        Err(diagnostic)
            if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diagnostic)) =>
        {
            return eprintln!("SKIPPED: no GPU adapter");
        }
        Err(diagnostic) => panic!("renderer init failed: {diagnostic:?}"),
    }
    let dir = roots_project("roots-then-load", &["a"]);
    assert_clean(&sync(&dir));

    let content = dir.join("scenes");
    let mut meshes = Vec::new();
    let mut stack = vec![content.join("assets")];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.to_string_lossy().ends_with(".mesh.yaml") {
                let rel = path.strip_prefix(&content).unwrap();
                meshes.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    meshes.sort();
    assert!(!meshes.is_empty(), "the import wrote no mesh");
    let mut scene = String::from("format: 1\nentities:\n");
    for mesh in &meshes {
        scene +=
            &format!("  - components:\n      Mesh:\n        shape:\n          asset: {mesh}\n");
    }

    let mut engine = Engine::builder()
        .project_root(&content)
        .build()
        .expect("builds");
    engine.set_textures(TextureSource::Cooked);
    engine.observe_load();
    engine.load_inline_scene(&scene).expect("the scene loads");
    let report = profile_engine(&mut engine, "roots".into(), 1, 0).expect("profiles");
    let texture = report
        .load
        .expect("observed")
        .assets
        .into_iter()
        .find(|counters| counters.kind == "texture")
        .expect("a texture row");
    assert!(
        texture.cooked > 0,
        "no map was read from the store: {texture:?}"
    );
    assert_eq!(
        texture.decoded, 0,
        "a map fell back to its PNG: {texture:?}"
    );
}
