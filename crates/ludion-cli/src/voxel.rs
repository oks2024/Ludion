//! Implementation of `ludion voxel <new|apply|upscale|diff|from-json>` — the write side of voxel
//! authoring.
//!
//! The read side (`check-scene --slice/--column/--mirror-x`, `render --model`) tells an author what
//! a model *is*; these verbs change it. All of them are build-time file tools, no engine process:
//! they read text, run the pure `ludion_scene` ops engine, and write text.
//!
//! Every writing verb is **validate-then-swap**: the whole new document is built in memory, parsed
//! and validated there, and only then written through `ludion_core::atomic`.
//! A model on disk therefore never holds a half-applied edit — which matters beyond tidiness,
//! because the editor's file watcher reloads on save and a torn write would blank the model in a
//! live viewport.
//!
//! `apply` refuses a model carrying `generated_from:` (`L1132`): its script is the source of truth,
//! and a hand edit would be erased by the next regeneration. `--dry-run` still previews.
//!
//! Overwrite policy splits the verbs in two: `new` and `upscale` refuse to replace an existing file
//! (a birth that clobbers a sculpted model is a lost afternoon), while `from-json` replaces it every
//! time — a generated model's script is its source of truth, so regenerating over the file IS the
//! workflow, and the `voxel-drift` gate exists to prove the two agree.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ludion_core::atomic::{create_new_atomic, write_atomic};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{PaletteEntry, VoxelComments, VoxelDocument};

use crate::diagnostic;

/// Runs `ludion voxel new <out> --size W,H,D [--name] [--scale] [--palette-from]`.
pub fn run_new(
    out: &Path,
    size: &str,
    name: Option<&str>,
    scale: Option<f64>,
    palette_from: Option<&Path>,
) -> ExitCode {
    let size = match parse_size(size) {
        Ok(size) => size,
        Err(message) => return diagnostic::usage_error(message),
    };
    if let Some(value) = scale
        && !(value.is_finite() && value > 0.0)
    {
        return diagnostic::usage_error(format_args!(
            "--scale must be a positive number, got {value}"
        ));
    }

    let palette = match palette_from {
        Some(path) => match read_palette_from(path) {
            Ok(palette) => palette,
            Err(exit) => return exit,
        },
        None => vec![default_palette_entry()],
    };

    let name = name
        .map(str::to_string)
        .or_else(|| model_stem(out).map(|stem| stem.to_string()));
    let [sx, sy, sz] = size;
    let doc = VoxelDocument {
        name: name.clone(),
        size,
        scale,
        generated_from: None,
        budget_exempt: None,
        palette,
        slices: vec![vec![".".repeat(sx as usize); sz as usize]; sy as usize],
    };
    let comments = scaffold_comments(name.as_deref(), size, scale);
    let text = ludion_scene::emit_voxel_with_comments(&doc, &comments);
    if let Err(exit) = create_validated(
        out,
        &text,
        "voxel new",
        "edit it with `voxel apply`, or choose another path",
    ) {
        return exit;
    }
    println!(
        "{}: created ({sx}x{sy}x{sz}, {} palette entr{})",
        out.display(),
        doc.palette.len(),
        if doc.palette.len() == 1 { "y" } else { "ies" }
    );
    ExitCode::SUCCESS
}

/// The scaffold's comment block: the three things every good model records, plus one anchor comment
/// per layer so an author never has to count layers to find where they are.
fn scaffold_comments(name: Option<&str>, size: [u32; 3], scale: Option<f64>) -> VoxelComments {
    let mut comments = VoxelComments::default();
    let title = name.unwrap_or("model");
    comments.header.push(format!(
        "{title} — one line on the silhouette: what this reads as at a glance."
    ));
    comments.header.push(
        "Front = row z=0 (meshes at world -Z, so -Z is the model's forward). Bottom-anchored at \
         y=0."
            .to_string(),
    );
    match scale {
        Some(scale) => comments.header.push(format!(
            "Intended in-game scale: {scale} (grid height {} x {scale} = {} world units).",
            size[1],
            f64::from(size[1]) * scale
        )),
        None => comments.header.push(
            "No intended scale declared yet — add `scale:` once the model's world height is known."
                .to_string(),
        ),
    }
    comments.header.push(
        "Palette glyphs are single printable-ASCII characters; `.` is always empty.".to_string(),
    );
    for y in 0..size[1] {
        comments.set_layer(y, format!("y{y}"));
    }
    comments
}

/// Runs `ludion voxel apply <model> --ops <ops.yaml> [--dry-run]`.
pub fn run_apply(model_path: &Path, ops_path: &Path, dry_run: bool) -> ExitCode {
    let source = match read_file(model_path) {
        Ok(source) => source,
        Err(exit) => return exit,
    };
    if !dry_run
        && let Ok(doc) = ludion_scene::check_voxel(&source)
        && let Some(script) = doc.generated_from
    {
        let model = model_path.display();
        let script_path = ludion_core::paths::slash(&model_path.with_file_name(&script));
        let model_arg = ludion_core::paths::slash(model_path);
        return diagnostic::report(
            model_path,
            &[Diagnostic::new(
                code::VOXEL_OP_INAPPLICABLE,
                format!(
                    "`{model}` is generated by `{script}` (`generated_from:`) and is write-only"
                ),
            )
            .with_path("/generated_from")
            .with_help(format!(
                "edit `{script}` and regenerate (`node {script_path} <model>.json`, then `ludion \
                 voxel from-json --in <model>.json --out {model_arg}`); `--dry-run` still previews \
                 ops"
            ))],
        );
    }
    let ops_source = match read_file(ops_path) {
        Ok(source) => source,
        Err(exit) => return exit,
    };
    let ops = match ludion_scene::parse_voxel_ops(&ops_source) {
        Ok(ops) => ops,
        Err(diagnostics) => return diagnostic::report(ops_path, &diagnostics),
    };
    let edit = match ludion_scene::apply_voxel_ops(&source, &ops) {
        Ok(edit) => edit,
        Err(diagnostics) => return diagnostic::report(model_path, &diagnostics),
    };

    for stray in &edit.dropped_comments {
        let place = if stray.line > 0 {
            format!("line {} of `{}` — ", stray.line, model_path.display())
        } else {
            format!("`{}` — ", model_path.display())
        };
        diagnostic::print(
            &Diagnostic::new(
                code::VOXEL_COMMENT_DROPPED,
                format!(
                    "{place}the comment `{}` has no place in the re-emitted model and was dropped",
                    stray.text
                ),
            )
            .with_help(
                "a reshape or y-shift moved its layer off the grid, or it sat where the writer \
                 cannot put it back; re-add it by hand in the written model",
            ),
        );
    }
    for warning in &edit.warnings {
        diagnostic::print(warning);
    }
    print_voxel_warnings(&edit.doc);

    if !dry_run && let Err(exit) = write_validated(model_path, &edit.text) {
        return exit;
    }

    let total: u64 = edit.changed.iter().sum();
    let [sx, sy, sz] = edit.doc.size;
    println!(
        "{}: {} {} op{}, {total} cell{} changed ({})",
        model_path.display(),
        if dry_run { "would apply" } else { "applied" },
        ops.len(),
        if ops.len() == 1 { "" } else { "s" },
        if total == 1 { "" } else { "s" },
        if edit.surgical {
            "surgical — every other byte unchanged"
        } else {
            "re-emitted canonically — the grid changed shape or its layers moved"
        }
    );
    for (index, (op, changed)) in ops.iter().zip(&edit.changed).enumerate() {
        println!(
            "  {index:>3}  {:<10} {changed} cell{}",
            op.name(),
            if *changed == 1 { "" } else { "s" }
        );
    }
    // One count, used twice: `component_count` is a whole-grid flood fill, so asking it again for
    // the plural would walk the model a second time.
    let components = edit.doc.component_count();
    println!(
        "  now {sx}x{sy}x{sz}, {} filled, {components} component{}{}",
        edit.doc.filled_count(),
        if components == 1 { "" } else { "s" },
        if dry_run {
            " (dry run — nothing written)"
        } else {
            ""
        }
    );
    print_exposure_note(&edit.doc);
    ExitCode::SUCCESS
}

/// Runs `ludion voxel upscale <model> --factor N [--out <path>]`.
///
/// The default output path sits beside the source: `<stem>-x<factor>.voxel.yaml`. Like `voxel new`,
/// this never overwrites an existing file — delete it first if that is really what's wanted.
pub fn run_upscale(model_path: &Path, factor: u32, out: Option<&Path>) -> ExitCode {
    let source = match read_file(model_path) {
        Ok(source) => source,
        Err(exit) => return exit,
    };
    let origin_label = model_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let upscale = match ludion_scene::upscale_voxel(&source, factor, origin_label.as_deref()) {
        Ok(upscale) => upscale,
        Err(diagnostics) => return diagnostic::report(model_path, &diagnostics),
    };

    let out_path = match out {
        Some(out) => out.to_path_buf(),
        None => match default_upscale_path(model_path, factor) {
            Some(path) => path,
            None => {
                return diagnostic::usage_error(format_args!(
                    "could not derive a default output name from `{}`; pass `--out`",
                    model_path.display()
                ));
            }
        },
    };
    for stray in &upscale.dropped_comments {
        diagnostic::print(
            &Diagnostic::new(
                code::VOXEL_COMMENT_DROPPED,
                format!(
                    "line {} of `{}` — the comment `{}` was dropped",
                    stray.line,
                    model_path.display(),
                    stray.text
                ),
            )
            .with_help("a comment cannot be re-anchored after upscaling; re-add it by hand"),
        );
    }
    print_voxel_warnings(&upscale.doc);

    if let Err(exit) = create_validated(
        &out_path,
        &upscale.text,
        "voxel upscale",
        "delete it first, or pass a different `--out`",
    ) {
        return exit;
    }

    let [ox, oy, oz] = upscale.source_size;
    let [nx, ny, nz] = upscale.doc.size;
    let components = upscale.doc.component_count();
    println!(
        "{} -> {}: {ox}x{oy}x{oz} -> {nx}x{ny}x{nz} (x{factor}), {} filled, {components} \
         component{}",
        model_path.display(),
        out_path.display(),
        upscale.doc.filled_count(),
        if components == 1 { "" } else { "s" },
    );
    if let Some(dropped) = upscale.dropped_generated_from {
        println!(
            "  generated_from: dropped (was `{dropped}`) — the upscale is not that script's output"
        );
    }
    if let Some(new_scale) = upscale.doc.scale {
        let old_scale = upscale
            .source_scale
            .unwrap_or(new_scale * f64::from(factor));
        println!("  scale: {old_scale} -> {new_scale} (grid height x scale unchanged)");
    }
    println!(
        "  comments: header carried plus 1 line noting the upscale; {} stray comment{} dropped",
        upscale.dropped_comments.len(),
        if upscale.dropped_comments.len() == 1 {
            ""
        } else {
            "s"
        }
    );
    ExitCode::SUCCESS
}

/// The default `voxel upscale` output path: `<dir>/<stem>-x<factor>.voxel.yaml`, or `None` when the
/// model's file name does not carry a stem `voxel upscale` can extend (no extension at all).
fn default_upscale_path(model_path: &Path, factor: u32) -> Option<PathBuf> {
    let stem = model_stem(model_path)?;
    let dir = model_path.parent().unwrap_or_else(|| Path::new("."));
    Some(dir.join(format!("{stem}-x{factor}.voxel.yaml")))
}

/// Runs `ludion voxel from-json [--in <json>] <--out <model> | --check <model> | --validate>`.
///
/// The one verb here that **overwrites** `--out`: regenerating a model whose generator script is
/// its source of truth is the whole point, so refusing (as `new` and `upscale` do) would defeat it.
/// `--check` is the write-free half the `voxel-drift` gate runs, byte-comparing against a committed
/// file — the same shape as `generate-map --check`. `--validate` is write-free too, but standalone:
/// it runs `--out`'s validation (the `L1115`/`L1129` warnings, an exposure note) with no committed
/// file to compare against and nothing written, for eyeballing a script's output before it earns a
/// path on disk.
pub fn run_from_json(
    input: Option<&Path>,
    out: Option<&Path>,
    check: Option<&Path>,
    validate: bool,
) -> ExitCode {
    let source = match input {
        Some(path) => match read_file(path) {
            Ok(source) => source,
            Err(exit) => return exit,
        },
        None => match std::io::read_to_string(std::io::stdin()) {
            Ok(source) => source,
            Err(error) => {
                diagnostic::print(
                    &Diagnostic::new(
                        code::SCENE_IO,
                        format!("could not read the generator intermediate from stdin: {error}"),
                    )
                    .with_help(
                        "have the generator write the JSON (`emit({ ..., out: process.argv[2] })`) \
                         and pass `--in model.voxel.json` — a PowerShell pipe re-encodes the bytes \
                         on the way through (`ludion explain L1135`)",
                    ),
                );
                return ExitCode::FAILURE;
            }
        },
    };
    if let Some(warning) = double_encoding_warning(&source) {
        diagnostic::print(&warning);
    }
    // The intermediate's own diagnostics point into the JSON, so they are reported against the
    // input, not the model being written.
    let origin = input.unwrap_or_else(|| Path::new("<stdin>"));
    let (doc, comments) = match ludion_scene::voxel_from_json(&source) {
        Ok(pair) => pair,
        Err(diagnostics) => return diagnostic::report(origin, &diagnostics),
    };
    let text = ludion_scene::emit_voxel_with_comments(&doc, &comments);

    if let Some(committed) = check {
        // The drift gate wants to see a model it is about to reject as unconnected or over budget,
        // not just a byte diff — these never fail the exit code, which stays byte-comparison-only.
        print_voxel_warnings(&doc);
        return check_against(committed, &text);
    }
    if validate {
        print_voxel_warnings(&doc);
        print_exposure_note(&doc);
        let [sx, sy, sz] = doc.size;
        let components = doc.component_count();
        println!(
            "{sx}x{sy}x{sz}, {} filled, {components} component{}, {} palette entr{} — validated, \
             nothing written",
            doc.filled_count(),
            if components == 1 { "" } else { "s" },
            doc.palette.len(),
            if doc.palette.len() == 1 { "y" } else { "ies" }
        );
        return ExitCode::SUCCESS;
    }
    let Some(out) = out else {
        return diagnostic::usage_error(
            "`voxel from-json` needs one of `--out <model>`, `--check <model>`, or `--validate`",
        );
    };

    print_voxel_warnings(&doc);
    if let Err(exit) = write_validated(out, &text) {
        return exit;
    }

    let [sx, sy, sz] = doc.size;
    let components = doc.component_count();
    println!(
        "{}: generated {sx}x{sy}x{sz}, {} filled, {components} component{}, {} palette entr{}",
        out.display(),
        doc.filled_count(),
        if components == 1 { "" } else { "s" },
        doc.palette.len(),
        if doc.palette.len() == 1 { "y" } else { "ies" }
    );
    match &doc.generated_from {
        Some(script) => {
            println!("  generated_from: {script} (this file is write-only — edit the script)")
        }
        None => println!(
            "  no `generated_from` declared — add one so the voxel-drift gate keeps the file \
             honest, or hand-sculpt from here"
        ),
    }
    ExitCode::SUCCESS
}

/// Byte-compares the regenerated text against the committed model, writing nothing: exit 0 when
/// identical, non-zero with a first-divergence report otherwise.
fn check_against(committed_path: &Path, text: &str) -> ExitCode {
    let committed = match std::fs::read_to_string(committed_path) {
        Ok(committed) => committed,
        Err(error) => {
            eprintln!(
                "{}",
                Diagnostic::new(
                    code::SCENE_IO,
                    format!(
                        "DRIFT {}: could not read the committed model to compare: {error}",
                        committed_path.display()
                    ),
                )
                .with_help("generate it first with `ludion voxel from-json --out <model>`")
            );
            return ExitCode::FAILURE;
        }
    };
    if committed == text {
        println!(
            "{}: identical to its generator's output",
            committed_path.display()
        );
        return ExitCode::SUCCESS;
    }
    let mut message = format!(
        "DRIFT {}: the committed model differs from what its generator now produces",
        committed_path.display()
    );
    if let Some((line, was, now)) = first_divergence(&committed, text) {
        message.push_str(&format!(
            "\n  line {line} committed: {was}\n  line {line} generated: {now}"
        ));
    }
    eprintln!(
        "{}",
        Diagnostic::new(code::VOXEL_GEN_INTERMEDIATE, message).with_help(
            "regenerate it (`node <script> | ludion voxel from-json --out <model>`) and commit the \
             result — the model file is write-only, the script is the source of truth"
        )
    );
    ExitCode::FAILURE
}

/// The first line at which two texts part company: its 1-based number and both sides, with a line
/// that ran off the end of its text shown as `(end of file)`.
fn first_divergence(committed: &str, generated: &str) -> Option<(usize, String, String)> {
    let mut was = committed.lines();
    let mut now = generated.lines();
    let mut line = 0usize;
    loop {
        line += 1;
        match (was.next(), now.next()) {
            (None, None) => return None,
            (a, b) if a == b => continue,
            (a, b) => {
                let show = |value: Option<&str>| {
                    value
                        .map(|text| format!("{text:?}"))
                        .unwrap_or_else(|| "(end of file)".to_string())
                };
                return Some((line, show(a), show(b)));
            }
        }
    }
}

/// Runs `ludion voxel diff <a> <b>`: exit 0 when the two models are identical, 1 when they differ,
/// 2 when a model cannot be read or validated — as `diff(1)`, so a script can tell a difference
/// from a failure.
pub fn run_diff(a_path: &Path, b_path: &Path) -> ExitCode {
    let (Ok(a), Ok(b)) = (read_model(a_path), read_model(b_path)) else {
        return ExitCode::from(2);
    };

    let mut differs = false;
    for line in palette_diff(&a, &b) {
        differs = true;
        println!("{line}");
    }
    if a.palette == b.palette {
        println!("palette: identical ({} entries)", a.palette.len());
    }

    let overlap = [
        a.size[0].min(b.size[0]),
        a.size[1].min(b.size[1]),
        a.size[2].min(b.size[2]),
    ];
    if a.size != b.size {
        differs = true;
        println!(
            "SIZE DIFFERS: {}x{}x{} vs {}x{}x{} — diffing the overlapping {}x{}x{}",
            a.size[0],
            a.size[1],
            a.size[2],
            b.size[0],
            b.size[1],
            b.size[2],
            overlap[0],
            overlap[1],
            overlap[2],
        );
    } else {
        println!("size: {}x{}x{}", a.size[0], a.size[1], a.size[2]);
    }

    const MAX_SAMPLES: usize = 20;
    let mut total = 0u64;
    let mut layers = 0u32;
    for y in 0..overlap[1] {
        let mut samples = Vec::new();
        let mut count = 0u64;
        for z in 0..overlap[2] {
            for x in 0..overlap[0] {
                let (left, right) = (glyph_at(&a, x, y, z), glyph_at(&b, x, y, z));
                if left == right {
                    continue;
                }
                count += 1;
                if samples.len() < MAX_SAMPLES {
                    samples.push(format!("({x},{z}) '{left}'->'{right}'"));
                }
            }
        }
        if count == 0 {
            continue;
        }
        layers += 1;
        total += count;
        println!(
            "y{y}: {count} cell{} changed",
            if count == 1 { "" } else { "s" }
        );
        for sample in &samples {
            println!("  {sample}");
        }
        if count as usize > samples.len() {
            println!("  ... and {} more", count as usize - samples.len());
        }
    }

    if total > 0 {
        differs = true;
        println!(
            "total: {total} cell{} changed across {layers} layer{}",
            if total == 1 { "" } else { "s" },
            if layers == 1 { "" } else { "s" }
        );
    } else {
        println!("cells: identical");
    }

    if differs {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// The palette lines of a diff: entries added, removed, or re-coloured, glyph order.
fn palette_diff(a: &VoxelDocument, b: &VoxelDocument) -> Vec<String> {
    let mut lines = Vec::new();
    for entry in &b.palette {
        if !a.palette.iter().any(|other| other.glyph == entry.glyph) {
            lines.push(format!("palette +{}: added", entry.glyph));
        }
    }
    for entry in &a.palette {
        match b.palette.iter().find(|other| other.glyph == entry.glyph) {
            None => lines.push(format!("palette -{}: removed", entry.glyph)),
            Some(other) if other != entry => {
                lines.push(format!("palette ~{}: changed", entry.glyph))
            }
            Some(_) => {}
        }
    }
    lines
}

/// The raw glyph at a cell (`.` when empty or out of range) — a text read, so a diff reports a
/// recolour as well as a silhouette change.
fn glyph_at(doc: &VoxelDocument, x: u32, y: u32, z: u32) -> char {
    doc.slices
        .get(y as usize)
        .and_then(|layer| layer.get(z as usize))
        .and_then(|row| row.chars().nth(x as usize))
        .unwrap_or('.')
}

// --- Shared plumbing ----------------------------------------------------------------------------

/// What UTF-8's multi-byte lead bytes turn into when a shell decodes them as Latin-1/CP1252 and
/// re-encodes the result: `Â`/`Ã` lead U+0080..U+07FF, `â` leads the general-punctuation block where
/// the em-dash lives.
const DOUBLE_ENCODED_LEADS: [char; 3] = ['Â', 'Ã', 'â'];

/// What UTF-8's continuation bytes (0x80..=0xBF) turn into under the same round trip. Latin-1 maps
/// them to U+0080..U+00BF directly; CP1252 substitutes this punctuation block for 0x80..=0x9F, so
/// both renderings count.
const CP1252_PUNCTUATION: [char; 27] = [
    '€', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', 'Ž', '\u{2018}', '\u{2019}',
    '\u{201c}', '\u{201d}', '•', '–', '—', '˜', '™', 'š', '›', 'œ', 'ž', 'Ÿ',
];

/// The `L1135` warning when `source` carries doubly-encoded UTF-8, or `None`.
///
/// Narrow on purpose: only a [lead][DOUBLE_ENCODED_LEADS] immediately followed by a re-encoded
/// continuation byte counts. Ordinary prose puts an ASCII letter after `â`/`Ã` (`château`, `São`),
/// so this stays silent on plain accented text and on pure ASCII — the cost of a false positive
/// here is an author distrusting a real warning later.
fn double_encoding_warning(source: &str) -> Option<Diagnostic> {
    let mut hits = 0usize;
    let mut first: Option<(usize, String)> = None;
    let mut line = 1usize;
    let mut previous: Option<(char, usize)> = None;
    for c in source.chars() {
        if let Some((lead, lead_line)) = previous
            && DOUBLE_ENCODED_LEADS.contains(&lead)
            && (matches!(c, '\u{80}'..='\u{bf}') || CP1252_PUNCTUATION.contains(&c))
        {
            hits += 1;
            first.get_or_insert_with(|| (lead_line, format!("{lead}{c}")));
        }
        previous = Some((c, line));
        if c == '\n' {
            line += 1;
        }
    }
    let (first_line, sample) = first?;
    Some(
        Diagnostic::new(
            code::VOXEL_GEN_DOUBLE_ENCODED,
            format!(
                "the generator intermediate carries {hits} double-encoded character sequence{} \
                 (first `{sample}` on line {first_line})",
                if hits == 1 { "" } else { "s" }
            ),
        )
        .with_help(
            "a PowerShell pipe — and `>` redirection with it — re-encodes a process's stdout, so \
             `node model.voxel.gen.mjs | ludion voxel from-json` can corrupt every non-ASCII \
             character. Have the generator write the file (`emit({ ..., out: process.argv[2] })`) \
             and read it back with `--in model.voxel.json`; `ludion explain L1135` has the detail",
        ),
    )
}

/// Prints the `L1115` cell-budget and `L1129` connectivity warnings a document may carry — the
/// checks every writing verb (`apply`, `upscale`, `from-json`'s `--out`/`--check`/`--validate`)
/// runs on its result, whether or not that result is about to be written to disk.
fn print_voxel_warnings(doc: &VoxelDocument) {
    if let Some(warning) = ludion_scene::voxel_budget_warning(doc) {
        diagnostic::print(&warning);
    }
    if let Some(warning) = ludion_scene::voxel_connectivity_warning(doc) {
        diagnostic::print(&warning);
    }
}

/// Prints the one-line exposure footer when a document has fully enclosed cells — the ops-side half
/// of `check-scene --exposure`: an edit or a generated model that buried detail inside itself says
/// so here, without a render. Silent when nothing is buried; no per-op attribution, because a cell
/// is enclosed by the whole result, not by whatever wrote it last.
fn print_exposure_note(doc: &VoxelDocument) {
    let exposure = doc.exposure(ludion_scene::ExposureView::Any);
    if exposure.enclosed > 0 {
        let by_glyph: Vec<String> = exposure
            .glyphs
            .iter()
            .filter(|glyph| glyph.enclosed > 0)
            .map(|glyph| format!("{} {}", glyph.glyph, glyph.enclosed))
            .collect();
        println!(
            "  exposure: {} fully enclosed cell{} ({})",
            exposure.enclosed,
            if exposure.enclosed == 1 { "" } else { "s" },
            by_glyph.join(", ")
        );
    }
}

/// Validates `text` and, only if it checks clean, writes it to `path` atomically
/// — so the file on disk is never observed half-written or invalid.
fn write_validated(path: &Path, text: &str) -> Result<(), ExitCode> {
    validate_model(path, text)?;
    if let Err(error) = write_atomic(path, text) {
        return Err(report_write_failure(path, &error));
    }
    Ok(())
}

/// [`write_validated`] for a verb that never overwrites: the write itself is the existence check,
/// so a file that appears between a pre-check and the write cannot be replaced.
fn create_validated(path: &Path, text: &str, verb: &str, help: &str) -> Result<(), ExitCode> {
    validate_model(path, text)?;
    create_text(path, text, verb, help)
}

/// Creates `path` with `text`, refusing with `L0030` when it exists. `verb` and `help` word the
/// refusal.
pub(crate) fn create_text(path: &Path, text: &str, verb: &str, help: &str) -> Result<(), ExitCode> {
    match create_new_atomic(path, text) {
        Ok(()) => Ok(()),
        // A regular file standing where a parent folder belongs reports the same kind, from the
        // folder create; the target itself is then absent.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.exists() => {
            diagnostic::print(
                &Diagnostic::new(
                    code::CREATE_TARGET_EXISTS,
                    format!(
                        "`{}` already exists; `{verb}` never overwrites",
                        path.display()
                    ),
                )
                .with_help(help.to_string()),
            );
            Err(ExitCode::FAILURE)
        }
        Err(error) => Err(report_write_failure(path, &error)),
    }
}

fn validate_model(path: &Path, text: &str) -> Result<(), ExitCode> {
    if let Err(diagnostics) = ludion_scene::check_voxel(text) {
        eprintln!("the edited model would not validate; nothing was written:");
        return Err(diagnostic::report(path, &diagnostics));
    }
    Ok(())
}

fn report_write_failure(path: &Path, error: &std::io::Error) -> ExitCode {
    eprintln!(
        "{}",
        Diagnostic::new(
            code::OUTPUT_IO,
            format!("could not write `{}`: {error}", path.display()),
        )
        .with_help(
            "check that the destination directory is writable and the file is not open in \
             another process"
        )
    );
    ExitCode::FAILURE
}

fn read_file(path: &Path) -> Result<String, ExitCode> {
    std::fs::read_to_string(path).map_err(|error| {
        eprintln!(
            "{}",
            Diagnostic::new(
                code::SCENE_IO,
                format!("could not read `{}`: {error}", path.display()),
            )
            .with_help("check the path (relative to the current directory) and permissions")
        );
        ExitCode::FAILURE
    })
}

fn read_model(path: &Path) -> Result<VoxelDocument, ExitCode> {
    let source = read_file(path)?;
    ludion_scene::check_voxel(&source).map_err(|diagnostics| diagnostic::report(path, &diagnostics))
}

/// The one-entry neutral-gray palette stub `voxel new` and `sculpt new` scaffold when
/// `--palette-from` names nothing.
pub(crate) fn default_palette_entry() -> PaletteEntry {
    PaletteEntry {
        glyph: "A".to_string(),
        color: [0.5, 0.5, 0.5, 1.0],
        emissive: None,
        metallic: None,
        roughness: None,
    }
}

/// Reads `--palette-from`'s palette, from either a voxel or a sculpt document — dispatched on
/// [`ludion_scene::peek_kind`] the way `check-scene` dispatches a document to its checker, so
/// `voxel new --palette-from` and `sculpt new --palette-from` share one colour-language source.
pub(crate) fn read_palette_from(path: &Path) -> Result<Vec<PaletteEntry>, ExitCode> {
    let source = read_file(path)?;
    match ludion_scene::peek_kind(&source) {
        Some(kind) if kind == "sculpt" => ludion_scene::check_sculpt(&source)
            .map(|doc| doc.palette)
            .map_err(|diagnostics| diagnostic::report(path, &diagnostics)),
        _ => ludion_scene::check_voxel(&source)
            .map(|doc| doc.palette)
            .map_err(|diagnostics| diagnostic::report(path, &diagnostics)),
    }
}

/// The model's name for the scaffold: the file stem with its `.voxel` suffix dropped.
fn model_stem(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".voxel.yaml").or_else(|| {
        name.strip_suffix(".yaml")
            .and_then(|stem| stem.strip_suffix(".voxel").or(Some(stem)))
    })?;
    (!stem.is_empty()).then_some(stem)
}

/// Parses `--size W,H,D`: one comma-joined token of three positive integers (the `--column x,z`
/// convention).
fn parse_size(value: &str) -> Result<[u32; 3], String> {
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != 3 {
        return Err(format!(
            "--size expects `W,H,D` (one comma-joined token of three positive integers), got \
             `{value}`"
        ));
    }
    let mut size = [0u32; 3];
    for (slot, part) in size.iter_mut().zip(&parts) {
        *slot = part.trim().parse::<u32>().map_err(|_| {
            format!(
                "--size component `{}` is not a positive integer",
                part.trim()
            )
        })?;
        if *slot == 0 {
            return Err("--size components must be at least 1".to_string());
        }
    }
    let cells: u128 = size.iter().map(|&d| u128::from(d)).product();
    if cells > u128::from(ludion_scene::VOXEL_MAX_CELLS) {
        return Err(format!(
            "--size {value} holds {cells} cells, past the {}-cell ceiling (256x256x256)",
            ludion_scene::VOXEL_MAX_CELLS
        ));
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_parses_one_comma_joined_token() {
        assert_eq!(parse_size("4,6,8").unwrap(), [4, 6, 8]);
        assert!(parse_size("4,6").is_err());
        assert!(parse_size("4,0,8").is_err());
        assert!(parse_size("4,x,8").is_err());
        assert_eq!(parse_size("256,256,256").unwrap(), [256, 256, 256]);
        assert!(parse_size("256,256,257").is_err());
        assert!(parse_size("4294967295,4294967295,4294967295").is_err());
    }

    #[test]
    fn the_scaffold_name_drops_the_voxel_suffix() {
        assert_eq!(
            model_stem(Path::new("scenes/voxels/statue.voxel.yaml")),
            Some("statue")
        );
        assert_eq!(model_stem(Path::new("statue.yaml")), Some("statue"));
    }
}
