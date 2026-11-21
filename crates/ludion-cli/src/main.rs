#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion` — the command-line entry point for the Ludion engine.
//!
//! Ships `ludion explain <CODE>` (look up an `L####` diagnostic code), `ludion docs [TOPIC]`
//! (list the repo's agent-docs topics, or print the path to one topic's doc file),
//! `ludion check-scene <FILE>` (parse + validate a strict-YAML scene file), `ludion sim-hash`
//! (instantiate a scene, run N deterministic ticks, print the final world hash — the
//! cross-process determinism check —
//! and optionally `--record` the session), `ludion replay` (re-drive a recorded session and print
//! the `WorldHash` sequence it reproduces), `ludion render` (validate a scene and render its content
//! through the render pipeline to a deterministic PNG — or, with `--sprite`/`--atlas`, compose a
//! sprite document's contact sheet or a packed atlas's overview on the CPU, no adapter involved),
//! `ludion dump-graph` (print the scheduled
//! frame graph as JSON, no GPU required), `ludion test` (run — or bless — a directory's
//! golden-image test suite), `ludion serve` (host a scene live over the wire protocol so a
//! viewer or agent can attach), `ludion new` (scaffold a runnable game project, guide included), and
//! `ludion dev` (the designer's hot-reload loop: watch the game's `src/`, rebuild on change, and
//! restart the running game with its state snapshotted across the restart), and `ludion profile`
//! (drive a scene for N frames — or `--attach` a running host and capture its next N live frames —
//! and report sim per-system, GPU per-pass, and frame-total median timings as JSON, optionally
//! checked against a TOML budget file or exported as a Perfetto/Chrome trace with `--perfetto`),
//! `ludion bake-probes <SCENE>` (capture a scene's reflection probes into the probe store, or
//! report that they are already there), `ludion generate-map`
//! (compile a `kind: map` source into a canonical scene of real entities — the map-authoring
//! pipeline's deterministic compile verb), `ludion map-check` (validate a map source and print
//! a compact ASCII occupancy/reachability/gate-link report from its generated entities),
//! `ludion generate-atlas` (bake a `kind: atlas-source` document into its committed `<name>.png` +
//! `<name>.atlas.yaml` pair, or `--check` that the committed pair still matches), and
//! `ludion image-stats` (run the four `ImageStats` defect metrics over one or more PNGs, batched,
//! with `--json` and a budget-style `--check <TOML>`), and
//! `ludion voxel <new|apply|upscale|diff|from-json>` (the write side of voxel authoring: scaffold a
//! model, edit one by voxel coordinates through a typed ops file, nearest-neighbor upscale one by an
//! integer factor, diff two, or write one from a generator script's JSON intermediate),
//! `ludion sculpt new` (scaffold an implicit-surface sculpt model),
//! `ludion mesh <pick|apply>` (edit a mesh's texture maps: name the surface point under a pixel of a
//! model sheet, and derive — or drift-check — the tree a `kind: mesh-edit` document owns), and
//! `ludion texture <build|check|seeds|vendor>` (build a `kind: texture` recipe into its committed PBR
//! map set, drift-check that set, report which root seeds pass the validators, or copy an embedded
//! `std:` recipe out to edit), and
//! `ludion gen <nodes|check|run|status|fetch>` (drive a ComfyUI server: list its installed node
//! classes, validate a `kind: generation` recipe offline and against `/object_info`, submit a run
//! and follow it to a folder of candidates, and check on or complete a `--no-wait` one).

mod bake_probes;
mod check_scene;
mod clip_report;
mod cook;
mod dev;
mod diagnostic;
mod docs;
mod dump_graph;
mod explain;
mod export;
mod framing;
mod gen_cmd;
mod generate_atlas;
mod generate_map;
mod image_stats;
mod import;
mod lut;
mod map_check;
mod map_drift;
mod mesh;
mod new;
mod paired_report;
mod perfetto;
mod placement;
mod profile;
mod render;
mod replay;
mod sculpt;
mod serve;
mod sim_hash;
mod sync;
mod test;
mod texture;
mod texture_feedback;
mod texture_preview;
mod trace;
mod voxel;

use std::path::PathBuf;

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};

/// The `--aa` value parser: the names of [`ludion_engine::AntiAliasing::ALL`], clap refusing the rest.
/// Parses `--lod <screen>[:<ratio>]`.
fn parse_lod_spec(text: &str) -> Result<ludion_import::LodSpec, String> {
    let number = |part: &str, what: &str| {
        part.parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| format!("`{part}` is not a number: {what}"))
    };
    let (screen, ratio) = match text.split_once(':') {
        Some((screen, ratio)) => (screen, Some(ratio)),
        None => (text, None),
    };
    Ok(ludion_import::LodSpec {
        screen_size: number(screen, "the screen size, e.g. `0.25`")?,
        ratio: ratio
            .map(|ratio| number(ratio, "the triangle ratio after `:`, e.g. `0.25:0.5`"))
            .transpose()?,
    })
}

fn aa_value_parser() -> impl clap::builder::TypedValueParser<Value = ludion_engine::AntiAliasing> {
    use clap::builder::TypedValueParser;
    clap::builder::PossibleValuesParser::new(
        ludion_engine::AntiAliasing::ALL.map(ludion_engine::AntiAliasing::name),
    )
    .map(|name| {
        ludion_engine::AntiAliasing::from_name(&name).expect("clap admits only a listed name")
    })
}

/// The `ludion` command-line tool.
#[derive(Debug, Parser)]
#[command(name = "ludion", version, about = "The Ludion engine CLI", long_about = None)]
struct Cli {
    /// Sim worker threads for deterministic parallelism (results are thread-count invariant, so
    /// this is a throughput knob and never changes a hash, a golden, or a replay). Overrides
    /// `LUDION_THREADS`, which itself defaults to this machine's capped available parallelism.
    /// Applies to every subcommand that reads it, `serve` included — the host an editor or viewer
    /// attaches to.
    #[arg(
        long,
        global = true,
        value_name = "N",
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..)
    )]
    threads: Option<usize>,

    /// Render quality for this machine: a preset name (`low`, `medium`, `high`, `full`) or the path
    /// to a TOML override file. Overrides `LUDION_QUALITY`; without either, every verb renders
    /// everything the scene authors. Render-only — no `WorldHash`, recording, or replay moves with
    /// it — so this is a pixels-and-frame-cost knob, meaningful to `render`, `serve`, `profile` and
    /// `dump-graph`. A value that is neither a preset nor a usable file is `L0016` and refuses to
    /// start. Authoring instruments ignore it: `render --model`/`--sweep` sheets exist to be compared
    /// across machines, so they always render at full.
    #[arg(long, global = true, value_name = "PRESET|FILE")]
    quality: Option<String>,

    /// Render extent for this machine, as `WIDTHxHEIGHT` in whole pixels (`960x540`). Overrides
    /// `LUDION_RENDER_SIZE`; without either, the engine's own default extent stands. Every stage of
    /// the frame path costs in proportion to the pixel count, so this is the one knob that shrinks
    /// readback, encode, socket write, decode and blit at once — `--quality` lowers GPU work only.
    /// **Applied by `serve` alone.** It is resolved before every verb that reads a knob (all but
    /// `explain`, `docs`, `devdocs`, `image-stats`, `new`, `gen`, `mesh`, `lut`) so a malformed value
    /// refuses to start there (`L0017`, the `--threads`/`--quality` posture), but `render` and
    /// `dump-graph` take their extent from their own `--width`/`--height`: a per-machine variable
    /// that silently moved those would move goldens and committed sheets with it. Render-only — no
    /// `WorldHash`, recording, or replay moves with it.
    #[arg(long, global = true, value_name = "WxH")]
    render_size: Option<String>,

    /// The subcommand to run.
    #[command(subcommand)]
    command: Command,
}

/// Top-level `ludion` subcommands.
// Variants are clap flag structs, not hot-path payloads — `Render` alone carries a dozen
// `Option<PathBuf>`/`Option<String>` fields for its many flags, so the variant-size spread grows
// with every flag regardless of how it's boxed. Boxing individual clap fields is the wrong trade
// (it fights the derive, which special-cases bare `Option<T>`/`Vec<T>`), so this lint is off here
// rather than on the fields.
#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Print the title and long-form explanation for a diagnostic code (e.g. `L0001`),
    /// or list every registered code when no code is given.
    Explain {
        /// The diagnostic code to explain, e.g. `L0001`. Omit to list all registered codes.
        code: Option<String>,
    },
    /// List every agent-docs topic (`docs/*.md` at the repo root) with its one-line description,
    /// or print one topic's doc text when a topic is given. A doc with malformed frontmatter fails
    /// the listing — running this in CI is the frontmatter gate.
    Docs {
        /// The topic to look up (the doc's filename stem, e.g. `determinism`). Omit to list
        /// every topic.
        topic: Option<String>,
        /// Print the path to the topic's doc file instead of its text. Requires an engine
        /// checkout — an installed binary serves the doc text from baked-in copies with no file
        /// to point at.
        #[arg(long, requires = "topic")]
        path: bool,
        /// Print one heading's section instead of the whole doc: the text from the first ATX
        /// heading whose trimmed text matches (case-insensitively) `--section`'s value, through
        /// everything up to the next heading of equal or higher level. A long topic doc (the
        /// engine guide runs to thousands of lines) can be read one section at a time instead of
        /// whole. Requires `topic`; an unmatched heading is `L0015`, with a suggestion and the
        /// topic's top-level headings. Mutually exclusive with `--path` — a section is text, not
        /// a file to point at.
        #[arg(long, requires = "topic", conflicts_with = "path")]
        section: Option<String>,
    },
    /// List every engine-internals topic (`dev-docs/*.md` at the repo root) with its one-line
    /// description, or print one topic's doc text when a topic is given. These are the docs for
    /// working on Ludion itself — crate docs, the crate graph, determinism, the protocol; `ludion
    /// docs` is the separate set for building a game *with* Ludion.
    ///
    /// Engine-internals docs ship only in an engine checkout: unlike `ludion docs`, they are not
    /// baked into the binary, because nothing in them can be acted on without the engine's source
    /// in front of you. Every form of this verb outside a checkout is an `L0008` — there is no
    /// embedded fallback. Inside one it behaves exactly like `ludion docs`, malformed-frontmatter
    /// listing gate included.
    Devdocs {
        /// The topic to look up (the doc's filename stem, e.g. `determinism`). Omit to list
        /// every topic.
        topic: Option<String>,
        /// Print the path to the topic's doc file instead of its text. Like every other form of
        /// this verb, it needs an engine checkout.
        #[arg(long, requires = "topic")]
        path: bool,
        /// Print one heading's section instead of the whole doc: the text from the first ATX
        /// heading whose trimmed text matches (case-insensitively) `--section`'s value, through
        /// everything up to the next heading of equal or higher level. Requires `topic`; an
        /// unmatched heading is `L0015`, with a suggestion and the topic's top-level headings.
        /// Mutually exclusive with `--path` — a section is text, not a file to point at.
        #[arg(long, requires = "topic", conflicts_with = "path")]
        section: Option<String>,
    },
    /// Parse and validate a scene file; print L-coded diagnostics with JSON Pointers on failure.
    ///
    /// For a `kind: voxel` model or a `kind: assembly` (over its assembled grid), four authoring
    /// views can be appended (they print before the closing `OK (…)` summary and never change the
    /// validation outcome): `--mirror-x` reports left/right symmetry, `--slice` prints an annotated
    /// layer, `--column` a vertical profile, `--exposure` what of the model is actually visible
    /// (`--from game` judges that from the gameplay camera). The flags are voxel/assembly-only —
    /// passing any against a scene, mesh, or part is a usage error. For a `kind: sculpt` model,
    /// `--bounds` is the equivalent authoring view (sculpt-only, same usage-error rule); it also
    /// reads a `kind: mesh` document (a skinned one lists its joints' rest heads), and a scene,
    /// where it prints the resolved symmetric collision-layer matrix (`--json` for one JSON object
    /// on any document; a `kind: animation` clip's report also re-imports its `generated_from`
    /// glTF). On a scene, `--placement` lints the placed props: floating, sunk, or overlapping
    /// colliders (warnings only). For a `kind: sprite`, `--rows` prints every frame's glyph grid with
    /// a column ruler and row numbers.
    CheckScene {
        /// Path to the scene file (strict-subset YAML).
        file: PathBuf,
        /// (voxel/assembly only) Report X-mirror symmetry: for each cell, whether its glyph matches
        /// the cell mirrored across the model's width. Informational — an asymmetric model is legal.
        #[arg(long)]
        mirror_x: bool,
        /// (voxel/assembly only) Print layer `y` as annotated rows (a column ruler, z indices,
        /// front/back markers), or `all` for every layer bottom-up.
        #[arg(long, value_name = "y|all")]
        slice: Option<String>,
        /// (voxel/assembly only) Print the vertical profile at column `x,z` (one comma-joined
        /// token), plus a run-length summary of the filled y levels.
        #[arg(long, value_name = "x,z")]
        column: Option<String>,
        /// (voxel/assembly only) Report what of the model is visible: filled/surface/fully-enclosed
        /// totals, then one line per glyph in use (cells, enclosed, up to five enclosed samples) and
        /// each emissive glyph's exposed faces by direction (+X -X +Y -Y +Z -Z, always that order).
        /// Interiors are conventionally solid, so a big enclosed count on the structural glyph is
        /// normal — the signal is a detail or emissive glyph with enclosed cells, work that was
        /// authored and can never be seen.
        #[arg(long)]
        exposure: bool,
        /// (voxel/assembly only) Judge `--exposure`'s emissive faces from a viewpoint as well as by
        /// direction: `game` ray-tests every camera-facing face against the model's own geometry at
        /// the gameplay camera's 58° pitch, so a recess too deep to read is text rather than a
        /// render. Exposed is not the same as visible.
        //
        // `requires = "exposure"` resolves here where it silently no-ops on `render --views`: that
        // trap needs the *target* arg to carry a `conflicts_with`, and `exposure` carries none.
        // Pinned by `from_game_requires_exposure` in `tests/check_scene_voxel.rs`.
        #[arg(long, value_enum, value_name = "VIEW", requires = "exposure")]
        from: Option<CliExposureView>,
        /// (sculpt/mesh/scene) Sculpt: print per-part and per-bone world-unit bounds, each part's
        /// `min..max` and surface vertex count (or "no surface"), then each bone's bounds over its
        /// parts. Mesh: the model-space box, plus a skinned mesh's joint tree with each joint's rest
        /// head. Scene: the resolved collision-layer matrix.
        #[arg(long)]
        bounds: bool,
        /// Print one JSON object instead of text, no `OK` line: `{"path","kind","ok",…the kind's
        /// fields,"diagnostics"}` for every document kind (a scene is `"kind":"scene"`), on a coded
        /// failure too (`"ok":false`, no kind fields). With `--bounds` on a scene or a mesh its
        /// fields join that object: a scene's `"layers":[{"name","ignores"}]`, a mesh's
        /// `"model":{"min","max"}`, `"vertices"`, `"triangles"`, `"joints":[{"name","parent","head"}]`.
        /// Not combinable with the voxel view flags.
        #[arg(long)]
        json: bool,
        /// (scene only) Warn about placed props that float above the ground and touch nothing
        /// (`L1240`), sink below it (`L1241`), or whose static colliders overlap another prop's
        /// (`L1242`). Heuristic warnings; the exit code never changes.
        #[arg(long)]
        placement: bool,
        /// (`--placement`) The ground's height; default 0.
        #[arg(long, value_name = "Y", requires = "placement")]
        ground_y: Option<f64>,
        /// (`--placement`) Metres of slack for floating, contact and overlap; default 0.05.
        #[arg(long, value_name = "M", requires = "placement")]
        tolerance: Option<f64>,
        /// (`--placement`) Metres a prop may sit below the ground before `L1241`; default 0.1.
        #[arg(long, value_name = "M", requires = "placement")]
        sink: Option<f64>,
        /// (sprite only) Print each frame's glyph grid under a `-- <frame> --` header, with a column
        /// ruler above and row numbers down the left, before the `OK` line.
        #[arg(long)]
        rows: bool,
    },
    /// Instantiate a scene, run it for a fixed number of ticks, and print the final world hash
    /// (16 hex digits). Two runs with the same inputs always print the same hash.
    SimHash {
        /// Path to the scene file (strict-subset YAML); its `seed` seeds the run (0 if absent).
        #[arg(long)]
        scene: PathBuf,
        /// Optional JSON input script: an array of frame-addressed key events.
        #[arg(long)]
        script: Option<PathBuf>,
        /// How many fixed ticks to run before hashing.
        #[arg(long, default_value_t = 600)]
        frames: u64,
        /// Record the session to this replay file (`ludion replay` re-drives it).
        #[arg(long)]
        record: Option<PathBuf>,
    },
    /// Drive a scene with an input script and print a per-tick JSON Lines trace on stdout.
    ///
    /// Channel `motor` writes one line per `CharacterMotor` entity per tick: the tick, the entity,
    /// its feet and the tick's `MoveReport` (`grounded`, `landed`, `ground`, `walls`, …). Channel
    /// `sensors` writes one line per sensor enter/exit of the tick. `tick` is the frame the tick ran
    /// as. This command runs no game systems, so a motor reports only when a game moves it: use the
    /// game binary's `--trace` for that. Two runs of the same scene and script print identical bytes
    /// within one build; compare two runs, never a committed trace.
    Trace {
        /// Path to the scene file (strict-subset YAML).
        #[arg(long)]
        scene: PathBuf,
        /// Optional JSON input script, the format `sim-hash --script` reads.
        #[arg(long)]
        script: Option<PathBuf>,
        /// How many fixed ticks to trace.
        #[arg(long)]
        frames: u64,
        /// Comma-separated channels to print: `motor`, `sensors`, `ai`, `ai_states`, `perception` (the ai
        /// and perception channels need a game's systems: a game's `--trace`). Default: `motor,sensors`.
        #[arg(long, value_delimiter = ',', default_values = ["motor", "sensors"])]
        channels: Vec<ludion_engine::TraceChannel>,
    },
    /// Re-drive a fresh engine through a recorded session and print the `WorldHash` it reproduces.
    Replay {
        /// Path to the recorded replay session (as written by `sim-hash --record`, or any other
        /// `--record`-armed run).
        file: PathBuf,
        /// Print every recorded frame's `WorldHash`, one `<frame> <hash>` line per tick, instead
        /// of just the final one.
        #[arg(long)]
        hashes: bool,
    },
    /// Validate a scene and render its content through the render pipeline to a PNG on a headless
    /// GPU (`--scene`), or render a `.voxel.yaml`/`.mesh.yaml`/`.assembly.yaml`/`.sculpt.yaml`/`.character.yaml`/`.prefab.yaml` model
    /// document as a turntable contact sheet (`--model`, an assembly at its rest pose) — exactly one
    /// target is required. Two runs with the same inputs produce byte-identical PNGs. `--set`/`--sweep`
    /// tune values for the render without touching the file: a scene's components under `--scene`, a
    /// model's palette under `--model` (voxel and assembly documents, any numeric field of a sculpt;
    /// a mesh has none). The two
    /// sprite targets need no GPU at all: `--sprite` composes a `kind: sprite` document's contact
    /// sheet (every frame labelled, a texel ruler, the applied `effects:` echoed) and `--atlas` the
    /// overview of a packed `kind: atlas` document (every region outlined and named); `--font` a
    /// `kind: font` document's specimen sheet (atlas, metric band, kerning, charset, pangram).
    #[command(group(
        ArgGroup::new("render_target")
            .args(["scene", "model", "sprite", "atlas", "font"])
            .required(true)
    ))]
    Render {
        /// Path to the scene file (strict-subset YAML); validated before rendering.
        #[arg(long)]
        scene: Option<PathBuf>,
        /// Path to a model document (`.voxel.yaml`, `.mesh.yaml`, `.assembly.yaml`, `.sculpt.yaml`, `.character.yaml`, or `.prefab.yaml`)
        /// to render as a turntable contact sheet instead of a scene (an assembly renders at its rest
        /// pose).
        /// Conflicts with `--scene` and `--dump-timings`.
        ///
        /// Comma-separated, **two or more paths render a lineup**: every model in one cell, side by
        /// side on one ground plane under one rig, sized against each other by their declared
        /// `scale:` — so "is my facade lighter than its neighbour's" is one look instead of two
        /// sheets compared from memory. A lineup takes one view (`game` when `--views` is omitted),
        /// caps at 8 models, requires `--out`, and conflicts with `--set`/`--sweep`.
        #[arg(long, value_delimiter = ',', conflicts_with = "dump_timings")]
        model: Vec<PathBuf>,
        /// Which turntable views to include in the contact sheet, comma-separated (canonical sheet
        /// order is `front,back,left,right,top,iso`, plus the opt-in `game` steep top-down;
        /// duplicates are ignored). Only valid with `--model`. Omit for the default six (no `game`),
        /// or for `game` alone in a lineup, which renders exactly one view.
        //
        // `conflicts_with = "scene"` rather than `requires = "model"`: clap 4.6's `requires`
        // resolution silently no-ops when its target arg (`model`) itself carries a
        // `conflicts_with` — verified in isolation before landing this. `conflicts_with = "scene"`
        // gets the same effect through the `scene`/`model` `ArgGroup`: `--views` can only coexist
        // with `--model`.
        #[arg(long, value_enum, value_delimiter = ',', conflicts_with = "scene")]
        views: Vec<CliModelView>,
        /// Light the contact sheet with an existing scene's rig instead of the synthesized bright
        /// neutral key + fill: every entity of `<SCENE>` carrying an `Environment`, `PostProcess`,
        /// `DirectionalLight`, `PointLight`, or `SpotLight` is lifted whole into the sheet (a `ground` entity's
        /// `Mesh` color is borrowed too), so a palette can be judged under the rig the game will
        /// actually show it in — e.g. `--rig examples/gallery/night-street.yaml`. A rig carrying any
        /// light replaces the key + fill outright. Only valid with `--model`. The sheet subject is
        /// normalized to a unit sphere, so a game rig's absolute point-light ranges do not
        /// reproduce their in-game falloff (sky and directional light do).
        //
        // `conflicts_with = "scene"` rather than `requires = "model"`, matching `--views` — see the
        // clap 4.6 note there.
        #[arg(long, value_name = "SCENE", conflicts_with = "scene")]
        rig: Option<PathBuf>,
        /// Contact sheet of a `kind: sprite` document: every frame labelled, upscaled with a texel
        /// ruler and again at the authored game scale (`--scale N`, whole texels per pixel, default
        /// 1), the applied `effects:` echoed in the footer. `--set POINTER=VALUE` edits the palette
        /// (`/palette/<glyph>/color/<i>`, `/palette/<glyph>/emissive`), `/seed`, a frame pivot, or an
        /// effect parameter (`/effects/<i>/<param>[/<j>]`) before baking; `--sweep POINTER=V,V,…`
        /// renders one cell per value; `--steps` draws the paint and the image after each effect
        /// instead. Needs no GPU adapter.
        //
        // Every flag this mode cannot honour is listed here rather than declared `requires` on the
        // other side — see the clap 4.6 note on `--views`.
        #[arg(long, value_name = "DOC", conflicts_with_all = [
            "scene", "model", "atlas", "rig", "views", "dump_timings", "dump_voxels", "dump_gi",
            "dump_probes", "frames", "clip", "time", "strip", "hold", "onion", "true_size", "part",
            "isolate", "channel",
        ])]
        sprite: Option<PathBuf>,
        /// Overview sheet of a `kind: atlas` document: the packed image upscaled, every region
        /// outlined and named, pivots marked. With `--clip <flipbook>` it draws that clip instead —
        /// the entry showing at `--time <seconds>`, or `--strip N` evenly spaced phases in a row, or
        /// `--onion` every entry overlaid and pivot-aligned with the ghosts behind the one showing.
        /// `--scale N` is whole sheet pixels per texel there, exactly as under `--sprite`. Needs no
        /// GPU adapter.
        //
        // `--set`/`--sweep` stay refused: a packed atlas is already baked, so there is nothing for
        // them to act on. The clip flags are refused rather than ignored when `--clip` is absent —
        // `ludion_engine::check_atlas_clip_flags`, which is `L4008` with the spelling that works.
        #[arg(long, value_name = "DOC", conflicts_with_all = [
            "scene", "model", "sprite", "rig", "views", "dump_timings", "dump_voxels", "dump_gi",
            "dump_probes", "frames", "true_size", "set", "sweep", "part", "isolate", "channel",
        ])]
        atlas: Option<PathBuf>,
        /// Specimen sheet of a `kind: font` document: its atlas image, a metric band over
        /// ascender/baseline/descender rules, a kerned vs unkerned sample, the charset and a pangram
        /// at three sizes, drawn through the same layout the game uses. Needs no GPU adapter.
        #[arg(long, value_name = "DOC", conflicts_with_all = [
            "scene", "model", "sprite", "atlas", "rig", "views", "dump_timings", "dump_voxels",
            "dump_gi", "dump_probes", "frames", "true_size", "set", "sweep", "part", "isolate",
            "channel",
        ])]
        font: Option<PathBuf>,
        /// Where to write the rendered PNG. Required with `--scene`, unless `--dump-voxels`/
        /// `--dump-gi`/`--dump-probes` is the only thing being asked for; with one `--model` or `--sprite` it
        /// defaults to `<name>.sheet.png` in the current directory (the document's file stem,
        /// `.voxel`/`.mesh`/`.assembly`/`.sprite` suffix stripped) and with `--atlas` to
        /// `<name>.atlas-sheet.png` (`.atlas` stripped), with `--font` to `<name>.font-sheet.png`
        /// (`.font` stripped), or `<name>.atlas-frame.png` /
        /// `.atlas-strip.png` / `.atlas-onion.png` for the three `--clip` instruments — a sprite
        /// document and the atlas packed from it share a stem, so their sheets must not share a
        /// name. A lineup names no single model, so it requires this.
        #[arg(long, required_unless_present_any = [
            "model", "dump_voxels", "dump_gi", "dump_probes", "sprite", "atlas", "font",
        ])]
        out: Option<PathBuf>,
        /// Output width in pixels (the sheet width in `--model` mode; integer cell division may
        /// shrink the final sheet slightly).
        #[arg(long, default_value_t = 1920, value_parser = clap::value_parser!(u32).range(1..))]
        width: u32,
        /// Output height in pixels (the sheet height in `--model` mode; integer cell division may
        /// shrink the final sheet slightly).
        #[arg(long, default_value_t = 1080, value_parser = clap::value_parser!(u32).range(1..))]
        height: u32,
        /// Optional path to also write per-pass GPU timings as JSON (one entry per scheduled pass).
        /// Only valid with `--scene` — a model sheet is many renders, not one.
        #[arg(long, conflicts_with = "model")]
        dump_timings: Option<PathBuf>,
        /// Render the scene's `GiVolume` occupancy/albedo/emissive grid (if extraction built one) as
        /// one stitched, labeled slice-sheet PNG instead of/alongside the scene image
        /// (`Plan/gi-wave0-voxelizer.md` §11) — the debug instrument for "did the walls fill, did the
        /// torches register". Only valid with `--scene`, and not with `--sweep` (the dump reads one
        /// grid, not many renders). Needs no GPU: the scene must author a `GiVolume` for there to be
        /// anything to dump (`L2104` otherwise, `L2111` when it authors one but nothing in it
        /// voxelizes — `--set` can tune a `cell_size` an existing `GiVolume` has, but cannot create
        /// the component). When `--out` is omitted, the command writes only this dump and never
        /// requests a GPU adapter at all.
        #[arg(long, conflicts_with_all = ["model", "sweep"])]
        dump_voxels: Option<PathBuf>,
        /// Render the scene's baked GI probe volume (if extraction built one) as one stitched,
        /// labeled slice-sheet PNG instead of/alongside the scene image
        /// (`Plan/gi-wave1-sky-visibility.md` §10) — VISIBILITY and BENT NORMAL blocks, plus a
        /// BOUNCE block (`Plan/gi-wave2-emissive-bounce.md` §8) showing the baked one-bounce
        /// radiance, tone-mapped, or titled "NOT BAKED" on a scene that authored `bounce: 0.0`. The
        /// debug instrument for "are the interior probes actually darker than the doorway" and "is
        /// the bounce actually warming the room". Only valid with `--scene`, and not with `--sweep`
        /// (the dump reads one baked volume, not many renders). The scene must still author a
        /// `GiVolume` for there to be anything to bake (`L2104` otherwise, `L2111` when it authors
        /// one but nothing in it voxelizes, checked before any adapter is requested — `--set` can
        /// tune a field an existing `GiVolume` has, but cannot create the component).
        /// **Unlike `--dump-voxels`, this DOES need a GPU adapter**: the probe volume exists only as
        /// a baked GPU texture, so producing the sheet means baking it (or hitting the renderer's
        /// own cache) and reading the result back — the one honest difference from its sibling.
        #[arg(long, conflicts_with_all = ["model", "sweep"])]
        dump_gi: Option<PathBuf>,
        /// Writes each reflection probe of the `--scene` to `<DIR>`: `probe-<rank>-captured.png` (the
        /// six cube faces side by side), `probe-<rank>-prefiltered.png` (one row per mip) and
        /// `probes.json` (rank, shape, position, store key, and whether the probe store holds it).
        /// Needs a GPU adapter. Captures every probe fresh and never reads or writes the store, so
        /// the files are a function of the scene alone. Without `--out` it writes only the dump.
        #[arg(long, value_name = "DIR", conflicts_with_all = ["model", "sweep"])]
        dump_probes: Option<PathBuf>,
        /// Override one value for this render, in memory only — the file on disk is never written.
        /// Repeatable. Spelled `<json-pointer>=<value>`, e.g.
        /// `--set /entities/0/components/PostProcess/bloom_strength=0.8`. The value is read as JSON
        /// when it parses as JSON and as a plain string otherwise, so `=0.8` is a number and
        /// `=night` is the string "night". Every applied override is echoed before the summary line,
        /// a created field as `unset -> <value>`.
        ///
        /// With `--scene` the pointer must reach a component the scene already has (`L4008`
        /// otherwise), but its last segment may name a field the file left at its default — that
        /// field is created, and a misspelled one is the schema's own `L1014`.
        ///
        /// With `--model` the domain is the model's palette, which is what a value-calibration pass
        /// tunes: `/palette/<glyph>/color/<0..3>`, `/palette/<glyph>/emissive`,
        /// `/palette/<glyph>/metallic`, `/palette/<glyph>/roughness`. An `.assembly.yaml` model's one
        /// `palette:` is the same domain. A `.sculpt.yaml` model widens it to every numeric field:
        /// `/resolution`, `/triangles`, the palette, and a part's numbers (`/parts/<i>/blend`,
        /// `/parts/<i>/at/<0..2>`, `/parts/<i>/radius`, …). Anything else is `L4008`, as is a
        /// `.mesh.yaml` model (no palette). A scalar the entry left unset is created.
        ///
        /// With `--sprite` the domain is the sprite document: `/palette/<glyph>/color/<0..3>`,
        /// `/palette/<glyph>/emissive`, `/seed`, a frame pivot component, and an effect parameter
        /// `/effects/<i>/<param>[/<j>]`. Anything else is `L4008`, listing the shapes that work.
        #[arg(long = "set", value_name = "POINTER=VALUE")]
        set: Vec<String>,
        /// Render once per value and stitch the results into one labelled contact sheet, for the
        /// judgement calls a metric cannot make. Spelled `<json-pointer>=<v,v,…>`, e.g.
        /// `--sweep /entities/0/components/PostProcess/bloom_strength=0.5,1.0,2.0` or
        /// `--sweep /palette/G/emissive=1.6,2.2,2.8`. Values split on commas literally, so a swept
        /// value cannot itself contain one, and each is typed like a `--set` value. Any `--set`
        /// overrides apply to every cell. The grid is near-square and at most 16 cells; `--out`
        /// receives the sheet.
        ///
        /// The pointer takes the same domain `--set` does in each mode. A `--model` sweep renders
        /// one view per value, so it needs exactly one `--views` entry; a `--sprite` sweep bakes the
        /// document once per value and lays the candidates out as a grid. Not valid with
        /// `--dump-timings` — a sheet is many renders, not the one instrumented render that times.
        #[arg(long, value_name = "POINTER=V,V,…", conflicts_with = "dump_timings")]
        sweep: Option<String>,
        /// (`--sprite` only) A steps sheet instead of the contact sheet: one row per frame, the
        /// authored paint then the image after each effect that applies to it, cumulatively, each
        /// cell captioned with that effect's echo line. At most 16 cells; narrow it with `--frame`.
        /// `--set` applies; `--sweep` is refused. Defaults `--out` to `<name>.steps.png`.
        #[arg(long, conflicts_with_all = ["scene", "model", "atlas", "font", "sweep"])]
        steps: bool,
        /// (`--sprite --steps` only) Draw only this frame's row of the steps sheet.
        #[arg(
            long,
            value_name = "NAME",
            conflicts_with_all = ["scene", "model", "atlas", "font", "sweep"]
        )]
        frame: Option<String>,
        /// Tick the built-in schedule before extracting, so a burst particle emitter or an assembly
        /// clip is judged in motion instead of frozen at tick 0 — a burst is invisible at tick 0. A
        /// single value (`--frames 90`) ticks that many times, then renders one PNG exactly as
        /// `--out` always does; two or more, ascending (`--frames 0,30,60,90`), render the scene once
        /// per value and stitch a labelled `frame N` strip through the same contact-sheet machinery
        /// `--sweep` uses, one row wide. Only the built-in (game-system-free) schedule ever runs, so
        /// this needs no game to show motion — everything it moves is a pure function of sim time,
        /// read straight off at extraction. Absent, the render is tick 0, byte-identical to every
        /// render before this flag existed. Conflicts with `--sweep`/`--model`; two or more values
        /// requires `--out`, exactly like `--sweep`.
        #[arg(long, value_delimiter = ',', conflicts_with_all = ["model", "sweep"])]
        frames: Vec<u64>,
        /// (scene only) Draw world-space debug lines over the frame: `all` or a comma list of
        /// categories (`--debug-draw path`), 1 px wide, dimmed where geometry hides them. This host
        /// runs no game systems, so it shows the engine's own categories (`path`, a `PathFollower`'s
        /// route); a game's own are drawn by its binary's `--render-out --debug-draw`. A category that
        /// drew nothing is named on the summary line, so a typo shows. Not valid with a
        /// multi-value `--frames` strip. `ludion docs debug-draw` has the detail.
        #[arg(
            long,
            value_name = "CATEGORIES",
            conflicts_with_all = ["model", "sprite", "atlas", "sweep"]
        )]
        debug_draw: Option<String>,
        /// (scene only) Draw a full-screen debug view instead of the finished picture:
        /// `ssr_confidence` (the screen-space reflection trace's confidence as gray),
        /// `ssr_footprint` (the colour-pyramid level it read, over the level count; magenta where
        /// it found no hit) or `probe_index` (the winning reflection probe per pixel, coloured by
        /// rank; gray where no probe applies, black where there is no surface) or
        /// `local_shadow_atlas` (the local-light shadow atlas, letterboxed) or `sun_cascades` (the
        /// lit frame, each mesh fragment tinted by the sun cascade that shades it) or
        /// `exposure_meter` (the metered frame dimmed, with the meter's histogram, trimmed bins,
        /// mean and mid-grey target over its bottom quarter). A view never turns
        /// its effect on: where the scene's `PostProcess.ssr` is 0, this quality masks the trace,
        /// the scene has no probes, no local light is granted a shadow, the sun fits no cascades,
        /// or `PostProcess.exposure_mode` is `manual`, the normal frame is drawn and the summary
        /// line says so.
        #[arg(
            long,
            value_enum,
            value_name = "VIEW",
            conflicts_with_all = ["model", "sprite", "atlas", "font", "sweep"]
        )]
        debug_view: Option<CliDebugView>,
        /// (scene only) Frame the shot around a target: `--look-at x,y,z` moves the scene's one
        /// `Camera` to orbit that point and focuses the scene's `PostProcess` depth of field on it
        /// (`focus_distance` = the framing distance). Azimuth is degrees about +Y from +Z (0 puts the
        /// camera on the target's +Z side looking -Z), pitch is degrees looking down, distance is
        /// metres eye to target. Each of `--azimuth`/`--pitch`/`--distance` left out is read off the
        /// scene camera, so `--look-at` alone reproduces the game camera's view of that point. The
        /// camera and focus move through ordinary overrides, echoed like `--set`, and an explicit
        /// `--set` on the same field still wins. Negative values need no quoting.
        // The framing flags check "needs --look-at" in code: `requires` silently no-ops when its
        // target carries `conflicts_with` — see the clap 4.6 note on `--views`.
        #[arg(
            long,
            value_name = "X,Y,Z",
            allow_hyphen_values = true,
            conflicts_with_all = ["model", "sprite", "atlas"]
        )]
        look_at: Option<String>,
        /// (scene only) Framing azimuth in degrees; see `--look-at`.
        #[arg(
            long,
            value_name = "DEG",
            allow_hyphen_values = true,
            conflicts_with_all = ["model", "sprite", "atlas"]
        )]
        azimuth: Option<f32>,
        /// (scene only) Framing pitch in degrees, positive looking down, -90..90; see `--look-at`.
        #[arg(
            long,
            value_name = "DEG",
            allow_hyphen_values = true,
            conflicts_with_all = ["model", "sprite", "atlas"]
        )]
        pitch: Option<f32>,
        /// (scene only) Framing distance in metres, eye to target; see `--look-at`.
        #[arg(
            long,
            value_name = "M",
            conflicts_with_all = ["model", "sprite", "atlas"]
        )]
        distance: Option<f32>,
        /// (scene only) Vertical field of view in degrees, written onto a perspective scene camera;
        /// see `--look-at`.
        #[arg(
            long,
            value_name = "DEG",
            conflicts_with_all = ["model", "sprite", "atlas"]
        )]
        fov: Option<f32>,
        /// Frame the model at the size it really gets: the sheet cell is sized so the subject covers
        /// the pixels its declared `scale:` earns under the vertical slice's gameplay camera,
        /// instead of the unit-sphere fit that renders a 12-unit building and a 2-unit character
        /// identically. `--width`/`--height` then set the cell's aspect only, so passing either
        /// explicitly alongside this is an error. Only valid with `--model`.
        //
        // `conflicts_with = "scene"` rather than `requires = "model"`, matching `--views` — see the
        // clap 4.6 note there.
        #[arg(long, conflicts_with = "scene")]
        true_size: bool,
        /// Draw one clip instead of the default subject: with `--model`, pose the assembly,
        /// character or prefab with one of the clips it binds instead of its rest pose (`--clip wave`; a
        /// character or prefab renders alone, with no gap analysis); with `--atlas`, draw one
        /// of the flipbooks the atlas carries instead of the packing overview (`--clip ship/bank`).
        /// Either way `check-scene <document>` lists the names that resolve.
        ///
        /// Under `--model` every phase additionally prints a gap analysis: the clip time, the posed
        /// model's connected-component count, and — when it is more than one — which parts own each
        /// piece that came away. That is how "the arm detaches at t = 0.4" becomes one line of text
        /// instead of an image to squint at.
        //
        // `conflicts_with = "scene"` rather than `requires = "model"`, matching `--views` — see the
        // clap 4.6 note there.
        #[arg(long, value_name = "NAME", conflicts_with = "scene")]
        clip: Option<String>,
        /// The clip instant to draw, in seconds (default `0`). Needs `--clip` (or `--crossfade`,
        /// where it is how far the outgoing clip has played when the fade begins, or `--blend`, where
        /// it is the instant every phase plays at). Under `--model` the
        /// time is quantized to the nearest 60 Hz sim tick — a pose is only ever
        /// `(now - started) * seconds_per_tick` — and the printed report names the exact tick and
        /// seconds used; under `--atlas` it selects the flipbook entry showing at that second, and
        /// alongside `--onion` it is the entry drawn opaque.
        #[arg(long, value_name = "SECONDS", conflicts_with = "scene")]
        time: Option<f64>,
        /// Render N evenly spaced phases of the clip side by side in one image instead of one
        /// instant — the contact sheet for a whole clip. Needs `--clip`, `--crossfade` or `--blend`
        /// (where it sets the phase count, 5 by default); 2 to 8 phases under
        /// `--model`, 2 to 16 under `--atlas`. A looping clip spaces them `i * duration / N` (the
        /// last phase is not the first repeated); a one-shot spaces them `i * duration / (N - 1)`,
        /// showing both endpoints.
        #[arg(long, value_name = "N", conflicts_with = "scene")]
        strip: Option<u32>,
        /// Under `--model`, render the blend of one bound clip into another — an assembly's, a
        /// character's or a prefab's — as a strip of phases, each at its fade weight: `--crossfade run:idle:0.25`
        /// fades `run` out and `idle` in over 0.25 s. Five phases unless `--strip N` says otherwise
        /// (2 to 8), weights 0 to 1 printed in the report. `--time` is how far `run` has played
        /// when the fade begins. Conflicts with `--clip`; one model only.
        #[arg(long, value_name = "FROM:TO:SECONDS", conflicts_with_all = ["scene", "clip", "atlas", "sprite"])]
        crossfade: Option<String>,
        /// Under `--model`, sweep a blend space of an assembly or character as a strip of phases:
        /// `--blend locomotion:0..2` evaluates the 1D blend `locomotion` at parameters swept
        /// linearly from 0 to 2 across five phases (`--strip N` for 2 to 8); `--blend aim:0.5:-1..1`
        /// holds the first parameter of a 2D blend and sweeps the second; plain values give one
        /// phase. Each phase prints its parameters, its shared phase in cycles and the gap analysis,
        /// all at `--time`. `check-scene <model>.assembly.yaml` lists a model's blends and their
        /// points. Conflicts with `--clip` and `--crossfade`; one model only; a prefab has no blends.
        #[arg(long, value_name = "NAME:P0[:P1]", conflicts_with_all = ["scene", "clip", "crossfade", "atlas", "sprite"])]
        blend: Option<String>,
        /// Under `--model`, stack a layer over the pose of `--clip`, `--crossfade` or `--blend`:
        /// `--layer wave:1.0:arm_only` plays the clip (or blend) `wave` at weight 1.0 over the part
        /// mask `arm_only`; `--layer breathe:0.5::additive` is half an additive layer over every part.
        /// The fields are `CLIP:WEIGHT[:MASK[:MODE]]` with a weight in 0 to 1 and a mode of `override`
        /// (the default) or `additive`. Repeat the flag for more layers, bottom first, at most four
        /// (`L1670`). The report echoes the stack and poses the gap analysis through it;
        /// `check-scene <model>.assembly.yaml` lists the masks. A prefab has no layers.
        #[arg(long, value_name = "CLIP:WEIGHT[:MASK[:MODE]]", conflicts_with_all = ["scene", "atlas", "sprite"])]
        layer: Vec<String>,
        /// Under `--model`, aim an assembly's `ik:` look-at over the pose of `--clip`, `--crossfade`
        /// or `--blend`: `--look 40,0` turns the head 40° left (`YAW,PITCH` in degrees, +yaw left,
        /// +pitch down, as `Look`); `--look look=0..100,0` names the look-at `look` and sweeps its yaw
        /// across five phases (`--strip N` for 2 to 8), every phase at `--time`, so "the head past its
        /// 70° limit" is a strip. A bare value needs exactly one look-at. The report prints each
        /// phase's `look [yaw, pitch]` and the gap analysis through the solver the game draws with;
        /// `check-scene <model>.assembly.yaml` lists the entries. Assemblies only; not with a `--blend`
        /// range.
        #[arg(long, value_name = "[NAME=]YAW,PITCH", conflicts_with_all = ["scene", "atlas", "sprite"])]
        look: Option<String>,
        /// Under `--model`, reach an assembly's `ik:` chain over the pose of `--clip`, `--crossfade`
        /// or `--blend`: `--ik hand_r=6,12,2` drives the chain `hand_r`'s effector to the point x=6,
        /// y=12, z=2 **in the assembly's grid voxels** (the frame `place:` is written in, the same
        /// numbers `check-scene --bounds` and `--layers` print). A target out of reach leaves the limb
        /// straight, aimed at it. Repeat the flag for more chains. The report echoes the targets and
        /// the gap analysis shows whether the limb tore out of its plug; `check-scene
        /// <model>.assembly.yaml` lists the chains. Assemblies only.
        #[arg(long, value_name = "NAME=X,Y,Z", conflicts_with_all = ["scene", "atlas", "sprite"])]
        ik: Vec<String>,
        /// Under `--model`, show each `--clip`/`--crossfade` phase at the pose a game shows with
        /// `Animation.hold: N`: the phase snaps down to the last multiple of N ticks, so
        /// neighbouring phases can repeat a pose. 1 (the default) is every tick; 0 is `L1317`.
        /// `ludion docs character-animation` § Stepped playback.
        #[arg(long, value_name = "N", conflicts_with_all = ["scene", "atlas", "sprite"])]
        hold: Option<u32>,
        /// Frame the `--model` sheet on one named part of a `.sculpt.yaml` model: every view is
        /// fitted to that part's meshed bounds while the rest of the model stays in the picture, so
        /// "does the eye sit in the socket" is one image. An unknown part, or one that meshes to no
        /// surface, is `L4008`; `check-scene <model>.sculpt.yaml --bounds` lists the parts. One
        /// model only.
        //
        // `conflicts_with = "scene"` rather than `requires = "model"`, matching `--views` — see the
        // clap 4.6 note there.
        #[arg(long, value_name = "NAME", conflicts_with = "scene")]
        part: Option<String>,
        /// With `--part`, draw only that part's own triangles and hide the rest of the model (`L4008`
        /// without `--part`).
        #[arg(long, conflicts_with = "scene")]
        isolate: bool,
        /// Draw one contribution of the `--model` stage instead of the finished picture. `emissive`
        /// zeroes every light, the ambient term and the whole sky/IBL environment, drops the cell
        /// caption and flattens the post stack to a pass-through, so a non-black pixel means
        /// emission and nothing else — the categorical answer to "this glows and nothing else
        /// does". Every model sheet gives a `kind: mesh` document referencing an `emissive_texture`
        /// a unit `Mesh.emissive`, since a standalone document carries no scene to author that
        /// intensity in and the map alone emits nothing; the channel's job is the mask, so a
        /// document with no map renders black here.
        /// Writes `<name>-emissive.sheet.png` by default, never over the ordinary sheet. Only valid
        /// with `--model`, and not alongside `--rig`, whose lights are the thing being removed.
        //
        // `conflicts_with = "scene"` rather than `requires = "model"`, matching `--views` — see the
        // clap 4.6 note there.
        #[arg(long, value_enum, value_name = "CHANNEL", conflicts_with_all = ["scene", "rig"])]
        channel: Option<CliModelChannel>,
        /// Overlay every entry of an atlas flipbook in one image, aligned by the regions' own
        /// pivots: the entry showing is opaque and the ones before it are ghosted, so a motion arc
        /// reads in one picture. Needs `--clip` under `--atlas`; `--time` picks which entry is the
        /// opaque one, and the entries after it are left out.
        #[arg(long, conflicts_with_all = ["scene", "model"])]
        onion: bool,
        /// Render the whole chain at this multiple of `--width`/`--height` and resample it back
        /// before any UI overlay: above 1.0 it supersamples (anti-aliasing without a new shader),
        /// below 1.0 it draws fewer pixels and magnifies. A quality setting, never a look change.
        /// Must be in `[0.25, 4.0]`. Beats the factor `--quality` supplies; without it a plain
        /// `--scene` render draws at the quality's own. Applies to `--sweep` cells too, but not to
        /// `--model`.
        ///
        /// Under `--sprite`, and under `--atlas --clip`, it means something else entirely — the
        /// authored game scale the art draws at, in whole sheet pixels per texel (the contact
        /// sheet's second block, or the whole flipbook instrument). It must then be a whole number
        /// in `1..=16` (default 1); a fractional one is refused, because half a texel is not a pixel
        /// a pixel-art author can judge. The `--atlas` overview derives its own upscale and ignores
        /// this.
        #[arg(long, conflicts_with = "model")]
        scale: Option<f32>,
        /// Anti-aliasing mode: `off` (the default) or `smaa`, SMAA 1x after composite. Applies to a plain `--scene` render; `--sweep`, `--frames` strips and the authoring
        /// instruments (`--model`, `--sprite`, `--atlas`) ignore it, as they ignore `--quality`. A host
        /// quality setting like `--scale`: no hash, golden or replay moves. The game's own
        /// options-menu choice beats it.
        #[arg(long, value_name = "off|smaa", value_parser = aa_value_parser())]
        aa: Option<ludion_engine::AntiAliasing>,
    },
    /// Capture a scene's reflection probes into the probe store (`.ludion/cooked/probes/` beside the
    /// scene) and print one line per probe: its name, store key and whether it was `captured` now or
    /// already `loaded`. Bakes at this machine's `--quality`, the one the hosts will run, because the
    /// key hashes it. `--probe` recaptures the one probe with that `Name`. A scene without probes
    /// prints `no reflection probes`; an unknown `--probe` is `L1280`; no GPU is `L2003`. Loads with
    /// the built-in registry, so a game's scene (`L1013`) is baked by the game's `--bake-probes`.
    /// `--json` prints `{ok, scene, store, probes, diagnostics}`, the object that mode prints too.
    BakeProbes {
        /// Path to the scene file (strict-subset YAML).
        scene: PathBuf,
        /// Recapture only the probe with this `Name`, replacing its stored entry.
        #[arg(long, value_name = "NAME")]
        probe: Option<String>,
        /// Print one JSON object on stdout instead of the per-probe lines.
        #[arg(long)]
        json: bool,
    },
    /// Validate a scene, then print the scheduled frame graph as JSON (passes, virtual
    /// resources, execution order, and transient aliasing). Pure CPU — needs no GPU.
    DumpGraph {
        /// Path to the scene file (strict-subset YAML); validated before dumping.
        #[arg(long)]
        scene: PathBuf,
        /// Graph width in pixels (affects resource descriptors in the dump).
        #[arg(long, default_value_t = 1920)]
        width: u32,
        /// Graph height in pixels (affects resource descriptors in the dump).
        #[arg(long, default_value_t = 1080)]
        height: u32,
        /// Dump the graph a rescaled render would build: the scene targets follow the render extent,
        /// an `ldr` transient appears at it, and a `resolve` (above 1.0) or `upscale` (below) pass
        /// lands between composite and any overlay. This is how to verify a render scale without a
        /// GPU. Beats the factor `--quality` supplies; without it the dump is the one this machine's
        /// quality builds.
        #[arg(long)]
        scale: Option<f32>,
        /// Anti-aliasing mode: `off` (the default) or `smaa`, SMAA 1x after composite. The dump lists the three `smaa_*` passes after `composite`. A host
        /// quality setting like `--scale`: no hash, golden or replay moves. The game's own
        /// options-menu choice beats it.
        #[arg(long, value_name = "off|smaa", value_parser = aa_value_parser())]
        aa: Option<ludion_engine::AntiAliasing>,
    },
    /// Run (or bless) a directory's golden-image test suite: every `*.yaml`/`*.yml` scene directly
    /// inside `--dir`, each compared against `<dir>/goldens/<os>/<scene-stem>.png` (goldens are
    /// blessed per pinned OS: `windows` or `macos`). Never skips when no GPU adapter is available —
    /// that is a loud failure here, unlike in-repo `cargo test`. A directory with no scene file
    /// fails (`L2209`): a game project's own golden runs through `cargo test`.
    Test {
        /// The directory of scenes to test (default: the current directory).
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Render and (re)write every golden instead of comparing. Creating a missing golden
        /// counts as blessing it.
        #[arg(long)]
        bless: bool,
        /// Only run scenes whose file stem contains this substring. A filter matching no scene
        /// fails (`L2209`), as does a directory with no scene file.
        #[arg(long)]
        filter: Option<String>,
    },
    /// Load a scene and host it live over the wire protocol so a viewer or agent can attach.
    /// Blocks until Ctrl-C, then shuts the host down cleanly.
    Serve {
        /// Path to the scene file (strict-subset YAML); validated before serving.
        #[arg(long)]
        scene: PathBuf,
        /// The `host:port` to bind the WebSocket host on.
        #[arg(long, default_value = "127.0.0.1:9411")]
        addr: String,
        /// Record the session to this replay file, armed before the first client can connect
        /// (`ludion replay` re-drives it).
        #[arg(long)]
        record: Option<PathBuf>,
        /// Start the engine paused instead of running, for a deterministic attach point.
        #[arg(long)]
        paused: bool,
        /// Open a native window showing the game while the server runs, closing (or Ctrl-C)
        /// stops it the same as headless.
        #[arg(long)]
        window: bool,
        /// Enable dev-only shader hot reload: an `asset.reload` naming a renderer shader (`.wgsl`)
        /// recreates its pipeline live (keeping the last good one on a bad edit). Off by default —
        /// the shipping path compiles shaders in.
        #[arg(long)]
        dev_shaders: bool,
        /// Draw every frame this host renders — screenshots and the live stream alike — at this
        /// multiple of the render size, resampled back before the UI overlay. A host quality
        /// setting: connected clients still receive exactly the dimensions they asked for. Must be
        /// in `[0.25, 4.0]`. Beats the factor `--quality` supplies; without it the host draws at the
        /// quality's own.
        #[arg(long)]
        scale: Option<f32>,
        /// Reflection-probe source: `store` (the default) reads the probe store `ludion bake-probes`
        /// fills and captures on a miss with an L1279 warning; `rebake` captures every probe afresh
        /// and leaves the store alone.
        #[arg(long, value_name = "store|rebake", value_parser = ["store", "rebake"])]
        probes: Option<String>,
        /// Anti-aliasing mode: `off` (the default) or `smaa`, SMAA 1x after composite. Every frame this host renders, screenshots and the live stream alike. A host
        /// quality setting like `--scale`: no hash, golden or replay moves. The game's own
        /// options-menu choice beats it.
        #[arg(long, value_name = "off|smaa", value_parser = aa_value_parser())]
        aa: Option<ludion_engine::AntiAliasing>,
    },
    /// Compile a foreign format into canonical Ludion text: a glTF (`.gltf`/`.glb`) into a `kind: prefab`
    /// plus its mesh, animation-clip and texture assets, or a `.png` into an editable
    /// `kind: sprite` document. Deterministic: same input → byte-identical output. Any glTF
    /// feature the importer cannot map (a texture on a second UV set, a camera, a directional light,
    /// a BLEND alpha mode, a morph target, a JPEG image, any other required extension) is an `L1202` error naming the offender;
    /// `--allow-lossy` downgrades those to warnings and compiles the supported subset. A PNG whose colours outnumber a sprite
    /// document's glyphs is `L4014`, answered by `--quantize N`.
    Import {
        /// Path to the input: a glTF file (`.gltf` JSON or `.glb` binary), or a `.png` image.
        input: PathBuf,
        /// glTF: the output directory, receiving `model.prefab.yaml`, `meshes/*.mesh.yaml`,
        /// `clips/*.anim.yaml`, `textures/*.png`. PNG: the sprite document to write (`ship.sprite.yaml`); an existing
        /// directory receives `<stem>.sprite.yaml`.
        #[arg(long)]
        out: PathBuf,
        /// glTF only: downgrade unsupported-feature errors (`L1202`) to warnings and compile what
        /// maps.
        #[arg(long, conflicts_with_all = ["quantize", "name"])]
        allow_lossy: bool,
        /// glTF only: write a mesh LOD level, `<screen>[:<ratio>]`, repeatable up to three times,
        /// finest first (`--lod 0.25:0.5 --lod 0.1:0.25`). `<screen>` is the fraction of the viewport
        /// height the mesh covers below which the level draws; `<ratio>` simplifies each mesh to
        /// that fraction of its triangles (meshoptimizer) into `meshes/<name>.lod<n>.mesh.yaml`,
        /// listed in the base's `lods`. Without a ratio the level comes from the glTF's authored
        /// `<name>_LOD<n>` node (no `--lod` at all keeps those at `0.5^n`).
        #[arg(
            long = "lod",
            value_name = "SCREEN[:RATIO]",
            value_parser = parse_lod_spec,
            conflicts_with_all = ["quantize", "name"]
        )]
        lod: Vec<ludion_import::LodSpec>,
        /// PNG only: merge the image's colours down to at most N palette entries by deterministic
        /// median-cut, instead of refusing (`L4014`) when they outnumber the glyph budget. The loss
        /// is reported as an `L4015` warning.
        #[arg(long, value_name = "N")]
        quantize: Option<usize>,
        /// PNG only: the `name:` written into the sprite document, and the region prefix an atlas
        /// mints from it. Defaults to the image's file stem.
        #[arg(long)]
        name: Option<String>,
    },
    /// Re-import a project's derived glTF trees and cook their maps, and those of any `[cook] roots`
    /// directory (which needs no `[[import]]` table). Every `[[import]]` table in `ludion.toml` (`from`
    /// a glob of `.gltf` files, `to` the derived root) compiles each match `<dir>/<stem>.gltf` into
    /// `<to>/<stem>/` as `ludion import` would, skipping the ones whose sources and the importer
    /// are unchanged. The `to` tree is derived: ignored by version control, never hand-edited, and
    /// owned by its `.ludion-sync.json` record (a `to` holding other files is refused, `L1245`).
    /// A second sync of the same root waits for the first; a sync killed midway recovers on the next
    /// run; a symbolic link under a source directory is refused (`L1245`). `--json` prints one
    /// object on every exit: `ok`, `imported`, `skipped`, `removed`, `failed` (each with its
    /// diagnostics) and `diagnostics` (everything printed, a refusal as `L1245`); exit 1 if the sync
    /// was refused or any entry failed.
    ///
    /// After the imports a cook stage compresses every PNG map the imported meshes reference, and
    /// those of every `*.mesh.yaml` under a `[cook] roots` directory (uncapped; a map leaving its
    /// root is `L1325`, a bad root `L1324`), into KTX2 files under
    /// `<project>/.ludion/cooked/<target>/<key>.ktx2` (BC7 for `desktop`, ASTC 4x4 for `ios`; `[cook] targets` in `ludion.toml`, `--target` adds one, `[[import]]
    /// max_texture_size` caps a table's maps). The store is keyed by content: a map already there
    /// is reused, an entry no mesh references is deleted, and a source map that fails to cook is
    /// reported (`L1270`) and exits 1 without undoing the import. `--no-cook` (or `[cook] textures =
    /// false`) skips the stage and the sweep. `--json` adds `cooked`, `cook_reused`, `cook_failed`
    /// (each `{map, target, diagnostics}`) and `store_bytes`.
    ///
    /// Last, sync warms the reflection-probe store: each scene under the default scene's directory
    /// that holds a `ReflectionProbe` and loads with the built-in registry is baked as `bake-probes`
    /// does, at `--quality`, else this machine's. A project without probes touches no GPU; a lane
    /// with no GPU skips. A scene the CLI cannot load (a game's components) is named, to bake with
    /// the game's `--bake-probes`; a scene that fails to load otherwise is reported and the sync
    /// still exits 0 (`check-scene` gates it). A bake error exits 1, no adapter does not. Then the
    /// sync prunes the probe store by last-used time: an entry unused for 14 days goes, then the
    /// oldest past a 4 GiB cap, never one used within 24 hours or resolved by this warm; a dead
    /// writer's temp goes after an hour. A cap still exceeded is reported and exits 0. `--json`
    /// adds `probes_captured`, `probes_stored`, `probe_scenes_skipped`, `probes_pruned`,
    /// `probes_pruned_bytes`, `probe_store_bytes` and `probe_store_over_cap`.
    Sync {
        /// The game's project directory (the one holding `ludion.toml`).
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Print one JSON object on stdout instead of the per-entry lines.
        #[arg(long)]
        json: bool,
        /// Also cook for this target besides `[cook] targets`; repeatable.
        #[arg(long = "target", value_parser = ["desktop", "ios"], action = clap::ArgAction::Append)]
        targets: Vec<String>,
        /// Skip the cook stage and leave the cooked store untouched.
        #[arg(long)]
        no_cook: bool,
    },
    /// Scaffold a new, standalone Ludion game project at `./<name>`: a starter component, system,
    /// scene, the determinism clippy denylist, tests that pass out of the box, the engine guide, and (in
    /// an engine checkout) a Node wrapper, `scripts/ludion.mjs`, that builds and runs this engine's CLI.
    New {
        /// The project name (e.g. `breakout`): ASCII letters, digits, `-` and `_`, starting with a
        /// letter, and not a reserved word (a Rust keyword, `test`, a Windows device name). The
        /// project is created at `./<name>`; a refusal (`L0035`) suggests a valid name.
        #[arg(required_unless_present = "refresh_guide")]
        name: Option<String>,
        /// Rewrite the current project's `LUDION-GUIDE.md` with this CLI's engine guide, touching
        /// nothing else; `ludion docs` warns `L0036` when they differ.
        #[arg(long, conflicts_with = "name")]
        refresh_guide: bool,
    },
    /// Export the game as an Xcode project folder for iOS: the embedded host template, the game built
    /// as a static library, and the default scene's directory plus every `[[import]]` tree and
    /// `[cook] roots` directory (synced first, at its project-relative path) as bundle content.
    /// Reads the
    /// project's `ludion.toml` (`name`, `default_scene`, and `[ios] bundle_id`); needs Xcode and the
    /// matching rustup target (`L0022` otherwise). A scene with reflection probes is baked into the
    /// bundle through the game binary's `--bake-probes` (`L1281` if an entry is missing). Prints the
    /// `xcodebuild` command that builds it.
    ///
    /// `--run` (simulator only) builds it, installs it on the first booted simulator (else boots an
    /// iPhone) and launches it, streaming the app's console until it exits; the app exits when its
    /// engine ends. With `--debug`, arguments after `--` reach the app as its argv: `-- --replay-verify
    /// run.lrp` (with `--replay run.lrp`) prints the verdict on that console, read there. A simulator
    /// app is a Mac process, so its loopback `9411` contends with a desktop host on that port.
    Export {
        /// The platform: `ios` (a device) or `ios-sim` (the simulator on an Apple-silicon Mac).
        #[arg(long, value_enum)]
        target: export::IosTarget,
        /// The folder to write the Xcode project into. It must be empty, new, or a previous export,
        /// and outside the project.
        #[arg(long)]
        out: PathBuf,
        /// The game's project directory (the one holding `ludion.toml` and `Cargo.toml`).
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Build the game with its `wire` feature, so a Ludion client can attach to the running app
        /// (loopback `9411`). The default is the shipped shape, with no listener.
        #[arg(long)]
        debug: bool,
        /// Copy this file into the bundle's content root, by file name, so an argv mode can read it
        /// (`--replay-verify <name>`). Repeatable; needs `--debug`.
        #[arg(long = "replay", value_name = "FILE")]
        replays: Vec<PathBuf>,
        /// Build with `xcodebuild`, install on a simulator and launch (needs `--target ios-sim`).
        #[arg(long)]
        run: bool,
        /// The app's arguments, after `--`: what `ludion run` would take (`--replay-verify FILE`,
        /// ...), read from the bundle's content root. Needs `--run` and `--debug`.
        #[arg(last = true)]
        app_args: Vec<String>,
    },
    /// Drive a scene for N frames and report median timings as JSON: sim per-system, GPU per-pass,
    /// and the frame total. Optionally check the medians against a TOML budget file (`--check`),
    /// exiting non-zero with an `L4005` per over-budget line. Measure in `--release` on the pinned
    /// hardware a budget was calibrated on — a debug build's numbers are not comparable.
    ///
    /// `--attach <ws-url>` profiles a *running* host instead: it connects over the wire and captures
    /// the next `--frames` live frames via `profile.capture`, so the report carries the running
    /// game's own systems (which this offline path structurally cannot see — it links no game code).
    /// `--attach` conflicts with `--scene` and `--scale` (there is no scene file to load and no
    /// render-scale override to make — the attached host renders at whatever scale it already runs
    /// at); exactly one of `--scene`/`--attach` is required.
    Profile {
        /// Path to the scene file (strict-subset YAML); validated before profiling. Required unless
        /// `--attach` is given.
        #[arg(long, required_unless_present = "attach")]
        scene: Option<PathBuf>,
        /// Attach to a running protocol host (e.g. `ludion serve`) instead of profiling a scene file:
        /// a `ws://host:port` URL. One `profile.capture` measures the next `--frames` live frames.
        #[arg(long, conflicts_with_all = ["scene", "scale"])]
        attach: Option<String>,
        /// How many frames to measure: medians are taken over this many samples offline, or the
        /// live capture window's length with `--attach`.
        #[arg(long, default_value_t = 120)]
        frames: u64,
        /// Write the JSON report to this path instead of stdout.
        #[arg(long)]
        json: Option<PathBuf>,
        /// Check the medians against this TOML budget file; exit non-zero on any over-budget line.
        #[arg(long)]
        check: Option<PathBuf>,
        /// Measure the GPU passes at this render scale (in `[0.25, 4.0]`). Pixel count is a real
        /// cost — a budget calibrated at one extent does not transfer to another, so `--check`
        /// against a native-calibrated budget only means anything at 1.0. Beats the factor
        /// `--quality` supplies; without it the measurement is taken at the quality's own. Offline
        /// only (conflicts with `--attach`).
        #[arg(long)]
        scale: Option<f32>,
        /// Anti-aliasing mode: `off` (the default) or `smaa`, SMAA 1x after composite. Measures the GPU passes with it on. Offline only (conflicts with `--attach`). A host
        /// quality setting like `--scale`: no hash, golden or replay moves. The game's own
        /// options-menu choice beats it.
        #[arg(long, value_name = "off|smaa", value_parser = aa_value_parser(), conflicts_with = "attach")]
        aa: Option<ludion_engine::AntiAliasing>,
        /// Write a Chrome trace-event JSON (openable at ui.perfetto.dev) to this path. Works with
        /// both `--scene` and `--attach`: the offline driver fills the report's per-frame samples
        /// itself, and an `--attach` capture requests them automatically whenever `--perfetto` is
        /// set, so either source builds a trace on its own.
        #[arg(long)]
        perfetto: Option<PathBuf>,
    },
    /// The designer's hot-reload loop. Run from a game's project directory: it watches `src/`,
    /// rebuilds on every change, and restarts the running game with its state snapshotted across the
    /// restart (`engine.snapshot` → `engine.shutdown` → relaunch on the same port → `engine.restore`).
    /// A failed build never kills the running child — the last good binary keeps playing.
    Dev {
        /// Path to the scene file to host (strict-subset YAML), forwarded to the game. Optional: the
        /// game's builder may carry a default scene.
        #[arg(long)]
        scene: Option<PathBuf>,
        /// The `host:port` to bind the WebSocket host on (forwarded to the game). Reloads rebind the
        /// concrete address the first launch bound, so a viewer stays connected across restarts.
        #[arg(long, default_value = "127.0.0.1:9411")]
        addr: String,
        /// Host a persistent native window in the dev process itself, surviving every reload (the
        /// child stays headless). The window rides each reload's same-port blink via auto-reconnect.
        #[arg(long)]
        window: bool,
        /// Reflection-probe source for the game: `rebake` forwards `--probes rebake` to every
        /// launch, so a reload captures its probes afresh instead of reading the probe store.
        #[arg(long, value_name = "store|rebake", value_parser = ["store", "rebake"])]
        probes: Option<String>,
        /// Anti-aliasing mode: `off` (the default) or `smaa`, SMAA 1x after composite. Forwarded to every launch of the game. A host
        /// quality setting like `--scale`: no hash, golden or replay moves. The game's own
        /// options-menu choice beats it.
        #[arg(long, value_name = "off|smaa", value_parser = aa_value_parser())]
        aa: Option<ludion_engine::AntiAliasing>,
    },
    /// Compile a `kind: map` source into a canonical scene of concrete entities — the map-authoring
    /// pipeline's deterministic compile verb (mirrors `import`). Resolves the map's `kit:` and each
    /// `fragments:` entry relative to the map file, runs the pure generator, and writes the canonical
    /// scene through the built-in registry. Deterministic: same map input → byte-identical output.
    GenerateMap {
        /// Paths to the map sources (`kind: map`, strict-subset YAML). Pass several to compile them
        /// all in one run; the exit code is non-zero if any fails.
        #[arg(required = true, num_args = 1..)]
        maps: Vec<PathBuf>,
        /// Where to write the generated scene. Defaults to the map with its `.map` suffix dropped
        /// (`district-plaza.map.yaml` → `district-plaza.yaml`). Valid only with a single input.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Regenerate in memory and compare byte-for-byte against the committed scene at the output
        /// path instead of writing: prints `up to date` or `DRIFT <path>`, writes nothing, and exits
        /// non-zero on drift. The mid-loop form of the CI drift gate.
        #[arg(long)]
        check: bool,
        /// Compile against this kit instead of the map's `kit:` (a path from the working directory).
        /// `generated_from` records it, relative to the map. Needs `--out` and a single map.
        #[arg(long, value_name = "PATH")]
        kit: Option<PathBuf>,
        /// Compile with this seed instead of the map's `seed:`. `generated_from` records it. Needs
        /// `--out` and a single map.
        #[arg(long)]
        seed: Option<u64>,
        /// Reflect the compiled room across the grid's centre line in X or Z: transforms are
        /// mirrored, scale is kept, and yaw is negated for X or turned to 180-θ in (-180, 180] for
        /// Z, so forward reflects. Built-in world-space vectors reflect too; game components are
        /// copied unreflected, and a map with `fragments:` is refused (`L1399`). `generated_from`
        /// records it. Needs `--out` and a single map.
        #[arg(long, value_name = "x|z", value_parser = ["x", "z"])]
        mirror: Option<String>,
        /// Write a `kind: prefab` instead of a scene, for `spawn_prefab`. Needs `--out` naming a
        /// `.prefab.yaml` and a single map. Asset paths are rewritten relative to the prefab, except
        /// inside game components, which are copied as written. The map's `physics:` is refused and
        /// its `seed:` is not written. The first part is the instance root.
        #[arg(long)]
        prefab: bool,
    },
    /// Bake one or more `kind: atlas-source` or `kind: font-source` documents into their committed
    /// `<name>.png` + `<name>.atlas.yaml` / `<name>.font.yaml` pair — our text into our binary, the
    /// mirror of `import`'s foreign-format-in. An atlas source resolves each `inputs:` entry
    /// relative to the source file, bakes every sprite document and decodes every loose PNG, and
    /// packs them; a font source reads its TTF/OTF beside it and bakes an SDF or bitmap glyph
    /// atlas. Deterministic: same inputs → byte-identical pair. Never bakes at runtime; both files
    /// are committed.
    GenerateAtlas {
        /// Paths to the sources (`kind: atlas-source` or `kind: font-source`, strict-subset YAML). Pass several to
        /// bake them all in one run; the exit code is non-zero if any fails.
        #[arg(required = true, num_args = 1..)]
        sources: Vec<PathBuf>,
        /// Directory to write into. Defaults to each source's own directory.
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Regenerate in memory and byte-compare BOTH files against the committed pair instead of
        /// writing: prints `up to date` or one `DRIFT <path>` per differing file, and exits non-zero
        /// on drift. The mid-loop form of the CI `atlas-drift` gate, which is exactly this run.
        #[arg(long)]
        check: bool,
    },
    /// Validate a map source and print a compact ASCII occupancy/reachability/gate-link report
    /// computed from its generated entities (the map-authoring loop's mid-edit verifier). Exits
    /// non-zero on a load/generate error or an `L1027` dangling cross-district gate; placement lints
    /// (`L1028`/`L1029`/`L1031`) print as warnings and never change the exit code.
    MapCheck {
        /// Paths to the map sources (`kind: map`, strict-subset YAML). Pass several for one report
        /// each; the exit code is non-zero if any fails.
        #[arg(required = true, num_args = 1..)]
        maps: Vec<PathBuf>,
        /// The reachability flood-fill anchor: the name of a generated entity to start from. Omit to
        /// use the first marker, else the grid centre.
        #[arg(long)]
        anchor: Option<String>,
        /// The occupancy-probe radius (world units) tested at each cell centre against static
        /// colliders. Defaults to the kit's `probe.radius`, else `0.5`.
        #[arg(long)]
        radius: Option<f64>,
        /// Append a focused query for a world position (`x,z`, one comma-joined token): its cell,
        /// walkable/blocked verdict, and nearest marker. Negative coordinates are fine
        /// (`--at -6,-4`).
        #[arg(long, allow_hyphen_values = true)]
        at: Option<String>,
        /// Append a gate-to-gate traversal table: per pair of `Gate` markers, the shortest walkable
        /// path as cells → world units → seconds, plus the sector gabarit those seconds land in
        /// (transition ~30 s, standard 60–90 s, signature 120–180 s).
        #[arg(long)]
        traverse: bool,
        /// The walker speed (world units per second) the traversal seconds are computed at. Defaults
        /// to the kit's `probe.speed`, else `6`. Only meaningful with `--traverse`, which it
        /// requires.
        #[arg(long, requires = "traverse")]
        speed: Option<f64>,
        /// Compile against this kit instead of the map's `kit:`, like `generate-map --kit`, so a
        /// themed room is sealed against the kit it ships with.
        #[arg(long, value_name = "PATH")]
        kit: Option<PathBuf>,
        /// Compile with this seed instead of the map's `seed:`, like `generate-map --seed`.
        #[arg(long)]
        seed: Option<u64>,
        /// Reflect the compiled room in X or Z, like `generate-map --mirror`.
        #[arg(long, value_name = "x|z", value_parser = ["x", "z"])]
        mirror: Option<String>,
    },
    /// The map half of the drift gate, for any project: recompiles every `*.map.yaml` under the scan
    /// dirs and byte-compares it against its default output (a map claimed by a prefab or an
    /// override scene needs no default one), rechecks every scene or prefab whose `generated_from:`
    /// names a map (with its recorded `--kit`/`--seed`/`--mirror`), and requires every compile to
    /// seal (an `L1040` leak fails here). Writes nothing; exits non-zero on any fault, each naming
    /// its file and the `generate-map` command that fixes it. Warnings are ignored unless
    /// `--deny-warnings`.
    MapDrift {
        /// The project root the scan dirs and printed paths are relative to. Defaults to the
        /// current directory.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Directories (relative to the root) to scan; defaults to `scenes`.
        dirs: Vec<PathBuf>,
        /// Print one JSON object (`ok`, `maps`, `outputs`, `files`, `diagnostics`) instead of text.
        #[arg(long)]
        json: bool,
        /// Fail on every `map-check` warning except those `--allow`ed (a leak is always a fault).
        #[arg(long)]
        deny_warnings: bool,
        /// With `--deny-warnings`, tolerate a warning: `L1029` alone matches the code,
        /// `L1029:marker` also requires the message to start with that prefix. Repeatable.
        #[arg(long, requires = "deny_warnings", value_name = "CODE[:PREFIX]")]
        allow: Vec<String>,
    },
    /// Runs the four `ImageStats` defect metrics (`banding_score`, `edge_aliasing`, `clipped_low`,
    /// `clipped_high` — see `ludion_testkit::ImageStats` for what each measures) over one or more
    /// PNGs. No reference image is involved: unlike an SSIM golden compare, this asks "does this
    /// frame carry a defect?", never "did this frame change?". A defect *gate*, not a target — the
    /// numbers exist to catch a regression, not to be minimized for its own sake.
    ImageStats {
        /// Paths to the PNGs to measure. Pass several to batch a whole directory (e.g. a gallery of
        /// goldens) in one run; the exit code is non-zero if any image fails to decode or, with
        /// `--check`, breaches a ceiling.
        #[arg(required = true, num_args = 1..)]
        images: Vec<PathBuf>,
        /// Print the report as a JSON array (one self-describing object per image, each carrying its
        /// `path` plus the four metrics) instead of the default aligned table.
        #[arg(long)]
        json: bool,
        /// Check every measured metric against ceilings in this TOML file (a top-level ceiling per
        /// metric, plus optional `[images."<file name>"]` overrides); exit non-zero on any breach.
        /// An over-ceiling metric is `L4010`; an unreadable, invalid, or unknown-metric-naming file
        /// is `L4009`.
        #[arg(long)]
        check: Option<PathBuf>,
    },
    /// Author voxel models by voxel coordinates: scaffold one (`new`), edit one with a typed ops
    /// file (`apply`), nearest-neighbor upscale one by an integer factor (`upscale`), compare two
    /// (`diff`), or write one from a generator script's JSON (`from-json`). The read side stays
    /// `check-scene --slice/--column` and `render --model`.
    Voxel {
        /// Which voxel authoring verb to run.
        #[command(subcommand)]
        command: VoxelCommand,
    },
    /// Author sculpt models: scaffold one (`new`). The read side stays `check-scene --bounds` and
    /// `render --model`.
    Sculpt {
        /// Which sculpt authoring verb to run.
        #[command(subcommand)]
        command: SculptCommand,
    },
    /// Edit a mesh's texture maps: name the surface point under a pixel of a rendered model sheet
    /// (`pick`), and derive — or drift-check — the tree a `kind: mesh-edit` document owns
    /// (`apply`). Neither verb drives a sim or a render pass.
    Mesh {
        /// Which mesh-editing verb to run.
        #[command(subcommand)]
        command: MeshCommand,
        /// Shadows the top-level `--threads`/`--quality`/`--render-size` so clap's global
        /// propagation stops here instead of advertising a paragraph of unusable render knobs on
        /// every `mesh <verb> --help`; a value given anyway is refused (`knob_free`) — the `gen`
        /// posture, for the same reason.
        #[arg(
            long,
            hide = true,
            value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..)
        )]
        threads: Option<usize>,
        #[arg(long, hide = true)]
        quality: Option<String>,
        #[arg(long, hide = true)]
        render_size: Option<String>,
    },
    /// Author and inspect the `.cube` 3D LUT that `PostProcess.lut` applies: write an identity
    /// table, bake a scene's analytic grade into one, run one over a still, and report how it
    /// shifts six colour bands. Pure CPU, no GPU and no sim; every verb takes `--json`.
    Lut {
        /// Which LUT verb to run.
        #[command(subcommand)]
        command: LutCommand,
        /// Shadows the top-level `--threads`/`--quality`/`--render-size` so clap's global
        /// propagation stops here instead of advertising a paragraph of unusable render knobs on
        /// every `lut <verb> --help`; a value given anyway is refused (`knob_free`) — the `mesh`
        /// posture, for the same reason.
        #[arg(
            long,
            hide = true,
            value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..)
        )]
        threads: Option<usize>,
        #[arg(long, hide = true)]
        quality: Option<String>,
        #[arg(long, hide = true)]
        render_size: Option<String>,
    },
    /// Procedural textures: build a `kind: texture` recipe into its PBR map set and provenance
    /// sidecar (`build`), validate it and byte-compare the committed set (`check`), or copy an
    /// embedded `std:` recipe out to edit (`vendor`). Maps are generated, write-only output.
    Texture {
        /// Which texture verb to run.
        #[command(subcommand)]
        command: TextureCommand,
    },
    /// Drive a ComfyUI server: list its installed node classes (`nodes`), validate a
    /// `kind: generation` recipe (`check`), submit one and follow it to a folder of candidates
    /// (`run`), check on or complete a `--no-wait` run (`status`/`fetch`), compose its candidates
    /// into a contact sheet (`sheet`), or freeze one into the project (`accept`).
    Gen {
        /// Which generation verb to run.
        #[command(subcommand)]
        command: GenCommand,
        /// Shadows the top-level `--threads`/`--quality`/`--render-size` so clap's global
        /// propagation stops here instead of advertising a paragraph of unusable render knobs on
        /// every `gen <verb> --help`; a value given anyway is refused (`knob_free`) — no verb under
        /// `gen` drives a sim or a render pass. `threads` has the global's type, so propagation
        /// into this subcommand's matches cannot mismatch.
        #[arg(
            long,
            hide = true,
            value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..)
        )]
        threads: Option<usize>,
        #[arg(long, hide = true)]
        quality: Option<String>,
        #[arg(long, hide = true)]
        render_size: Option<String>,
    },
}

/// The `ludion gen` verbs. Every verb that contacts a server takes `--url` (overriding
/// `LUDION_COMFY_URL`, itself overriding ComfyUI's own loopback default); every one takes `--json`
/// (exactly one compact JSON object on one stdout line; diagnostics always render to stderr).
#[derive(Debug, Subcommand)]
enum GenCommand {
    /// List installed node classes from `/object_info`. With no `filter`, prints every class; with
    /// one, a case-insensitive substring match over the class name and its category. `--class`
    /// prints the one named class's full input table (types, defaults, enum options) instead of the
    /// compact listing.
    Nodes {
        /// A case-insensitive substring over the class name or category. Ignored with `--class`.
        filter: Option<String>,
        /// Print one class's full input table instead of the compact per-class listing.
        #[arg(long)]
        class: Option<String>,
        /// The ComfyUI server address, overriding `LUDION_COMFY_URL`.
        #[arg(long)]
        url: Option<String>,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
    /// Validate a recipe offline (document, paths, workflow format, override titles, output kind)
    /// and, if a server answers, against its `/object_info`. The offline result decides the exit
    /// code; a live failure or an unreachable server (`L1167`, downgraded to a warning here) never
    /// does, so this runs in CI with no server at all.
    Check {
        /// The recipe to validate (`kind: generation`, strict-subset YAML).
        recipe: PathBuf,
        /// The ComfyUI server address, overriding `LUDION_COMFY_URL`.
        #[arg(long)]
        url: Option<String>,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
    /// Submit a recipe: upload its `images:`, submit `--count` graphs at `--seed`, `--seed + 1`, …,
    /// follow them, download every output, and write `run.json`. `--no-wait` submits and returns,
    /// leaving `gen status`/`gen fetch` the other half.
    Run {
        /// The recipe to run (`kind: generation`, strict-subset YAML).
        recipe: PathBuf,
        /// Overrides the recipe's `count`.
        #[arg(long)]
        count: Option<u32>,
        /// Overrides the recipe's base `seed`.
        #[arg(long)]
        seed: Option<u64>,
        /// A scratch image replacing the recipe's *first* declared `images:` entry, for chaining a
        /// candidate from a previous run without accepting it.
        #[arg(long)]
        image: Option<PathBuf>,
        /// How long the follow may take, in seconds, before `L1170`. Defaults to 600.
        #[arg(long)]
        timeout: Option<u64>,
        /// Submit and return immediately, printing the run folder and job ids; `gen status`/
        /// `gen fetch` complete the run later.
        #[arg(long)]
        no_wait: bool,
        /// The ComfyUI server address, overriding `LUDION_COMFY_URL`.
        #[arg(long)]
        url: Option<String>,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
    /// A run's state: with a run folder, its own record plus the server's verdict on every job
    /// (ready/pending); bare, the server's `/queue` plus the most recent run of every recipe under
    /// the current project.
    Status {
        /// The run folder (`generation/runs/<recipe>/<run-id>/`). Omit for the bare queue+recipes
        /// summary.
        run_dir: Option<PathBuf>,
        /// The ComfyUI server address, overriding `LUDION_COMFY_URL`.
        #[arg(long)]
        url: Option<String>,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
    /// Complete a `--no-wait` run: download every output and finalize `run.json`. `L1170` if a job
    /// has not finished yet — `gen status` says where it is.
    Fetch {
        /// The run folder (`generation/runs/<recipe>/<run-id>/`).
        run_dir: PathBuf,
        /// The ComfyUI server address, overriding `LUDION_COMFY_URL`.
        #[arg(long)]
        url: Option<String>,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
    /// Compose a run's candidates into one contact sheet, each cell labelled `#<index> seed <n>` —
    /// the read step before `accept` picks a winner.
    Sheet {
        /// The run folder (`generation/runs/<recipe>/<run-id>/`).
        run_dir: PathBuf,
        /// Columns in the grid. Defaults to a capped `ceil(sqrt(n))`.
        #[arg(long)]
        cols: Option<u32>,
        /// Where to write the sheet. Defaults to `sheet.png` inside the run folder.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
    /// Freeze one candidate into the project: an image is re-encoded to the canonical normalized
    /// PNG with a `.origin.yaml` sidecar recording its provenance; a mesh goes through `ludion
    /// import` (the one glTF reader) and its provenance rides in each mesh document's `origin:`
    /// key. Never overwrites an existing destination (`L1172`).
    Accept {
        /// The run folder (`generation/runs/<recipe>/<run-id>/`).
        run_dir: PathBuf,
        /// The candidate's index in the run.
        index: u32,
        /// Where to write the accepted asset: the file itself for an image, its `.origin.yaml`
        /// sidecar beside it; an empty or absent directory for a mesh import.
        #[arg(long)]
        out: PathBuf,
        /// Mesh candidate only: downgrade the import's unsupported-feature errors (`L1202`) to
        /// warnings and compile what maps, exactly as `ludion import --allow-lossy` does. A
        /// generator writes its own `.glb`, so there is no export to go back and fix.
        #[arg(long)]
        allow_lossy: bool,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
}

/// The `ludion voxel` authoring verbs.
#[derive(Debug, Subcommand)]
enum VoxelCommand {
    /// Scaffold an empty voxel model: all-`.` layers, a palette, and the header comment template.
    ///
    /// The scaffold carries the three things every good model records — the silhouette intent, the
    /// `front = row z=0` convention, and the intended world scale — plus one `# y<N>` anchor comment
    /// per layer. It never overwrites an existing file.
    New {
        /// Where to write the model (`<name>.voxel.yaml`).
        out: PathBuf,
        /// The grid extent as one comma-joined token: width,height,depth (e.g. `--size 16,24,16`).
        #[arg(long, value_name = "W,H,D")]
        size: String,
        /// The model's `name:`. Defaults to the output file's stem.
        #[arg(long)]
        name: Option<String>,
        /// The declared intended world scale (grid height x scale = world height). Metadata — the
        /// mesher never reads it.
        #[arg(long)]
        scale: Option<f64>,
        /// Copy the palette from an existing voxel or sculpt document instead of writing a
        /// one-entry stub — how a new model joins an existing set's colour language.
        #[arg(long, value_name = "MODEL")]
        palette_from: Option<PathBuf>,
    },
    /// Apply a typed ops file to a model, in place and by voxel coordinates.
    ///
    /// Cell-level edits are surgical: only the changed cell characters are rewritten, so the header
    /// docstring, layer comments, blank lines, and float formatting survive byte-identical. A grid
    /// reshape (`grow`/`crop`) re-emits canonically and carries the comments across. Either way the
    /// result is validated in memory and written temp-then-rename, so a failed op leaves the model
    /// untouched.
    ///
    /// The ops file is strict-subset YAML. Coordinates are comma-joined tokens (the subset has no
    /// inline sequences), `mode: paint` writes only over already-filled cells, and the glyph `.`
    /// erases:
    ///
    ///   format: 1
    ///   kind: voxel-ops
    ///   ops:
    ///     - op: set          # one cell (out of range is an error)
    ///       at: "3,10,4"
    ///       glyph: H
    ///     - op: box          # inclusive corners; volume ops clip to the grid
    ///       from: "0,0,0"
    ///       to: "5,2,5"
    ///       glyph: F
    ///       mode: paint
    ///     - op: ellipsoid    # decimals allowed; a cell is in when (dx/rx)^2+(dy/ry)^2+(dz/rz)^2 <= 1
    ///       center: "8,20,8"
    ///       radii: "4,6,4"
    ///       glyph: F
    ///     - op: cylinder     # base disc + extent along axis (x|y|z)
    ///       base: "8,0,8"
    ///       axis: y
    ///       radius: 2
    ///       height: 12
    ///       glyph: D
    ///     - op: line         # 3D Bresenham, both endpoints included
    ///       from: "2,4,2"
    ///       to: "9,9,2"
    ///       glyph: D
    ///     - op: mirror       # copy one half over the other (axis x|y|z, keep low|high)
    ///       axis: x
    ///       keep: low
    ///     - op: shift        # signed; cells shifted out drop; a y shift carries layer comments
    ///       by: "0,1,0"
    ///     - op: grow         # add empty cells on each side (this reshapes the grid)
    ///       low: "2,0,2"
    ///       high: "2,0,2"
    ///     - op: crop         # keep an inclusive sub-box (this reshapes the grid)
    ///       from: "0,0,0"
    ///       to: "15,23,15"
    ///     - op: comment      # replace a layer's anatomy comment
    ///       y: 12
    ///       text: "y12 — shoulders"
    //
    // `verbatim_doc_comment`: the ops skeleton above is the format's discovery surface for an agent
    // reading `--help`, and clap's reflow would collapse it into one paragraph.
    #[command(verbatim_doc_comment)]
    Apply {
        /// The model to edit in place (`.voxel.yaml`).
        model: PathBuf,
        /// The ops document (`kind: voxel-ops`).
        #[arg(long)]
        ops: PathBuf,
        /// Print the same summary but write nothing — the mid-loop "what would this do".
        #[arg(long)]
        dry_run: bool,
    },
    /// Nearest-neighbor upscale a model by an integer factor: each source cell becomes a
    /// `factor`^3 block of identical cells. `size` scales by `factor`; a declared `scale` divides by
    /// `factor` so the model's intended world footprint is unchanged; `budget_exempt` carries over;
    /// `generated_from` is dropped (the output is not that script's product). Comments carry down —
    /// the header verbatim plus one appended `upscaled x<factor> from <source file>` line, and each
    /// source layer's anatomy comment moves onto the first of its `factor` output layers.
    ///
    /// The proven ladder for a 48-64^3 model: hand-author <= 32^3, `voxel upscale` to a correct,
    /// connected, palette-identical base, then sculpt detail with `voxel apply` ops.
    ///
    /// Never overwrites: the default output is `<stem>-x<factor>.voxel.yaml` beside the source, and
    /// an existing file at that path (or at `--out`) stops the command outright.
    Upscale {
        /// The source model to upscale (left untouched).
        model: PathBuf,
        /// The integer scale factor, `2..=8` (1 is a no-op copy; above 8 is almost certainly a
        /// mistake).
        #[arg(long)]
        factor: u32,
        /// Where to write the upscaled model. Defaults to `<stem>-x<factor>.voxel.yaml` beside the
        /// source.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Compare two voxel models: palette entries added/removed/changed, then per-layer changed-cell
    /// counts with up to 20 `(x,z) 'A'->'B'` samples each. Exits 0 when identical, 1 when they
    /// differ, 2 when a model cannot be read or does not validate (as diff(1)), so a script can gate
    /// on it. On a size mismatch it says so and diffs the overlapping region.
    Diff {
        /// The baseline model.
        a: PathBuf,
        /// The model to compare against it.
        b: PathBuf,
    },
    /// Write a model from a generator script's JSON intermediate — the sanctioned from-scratch
    /// generation path for a model too large to hand-author.
    ///
    /// A committed `<name>.voxel.gen.mjs` builds the grid over the shared `scripts/voxel/grid.mjs`
    /// library and writes one JSON object; this verb validates it and writes the canonical file.
    /// Scripts NEVER write YAML — canonical serialization lives in exactly one place, the engine.
    /// `ludion explain L1133` prints the intermediate's shape.
    ///
    /// Have the generator write the JSON itself (`emit({ ..., out: process.argv[2] })`) and read it
    /// back with `--in` — a PowerShell pipe, and `>` redirection with it, re-encodes the bytes on
    /// the way through (`ludion explain L1135`). Stdin still works where no shell moves the bytes.
    ///
    ///   node models/brazier.voxel.gen.mjs models/brazier.voxel.json
    ///   ludion voxel from-json --in models/brazier.voxel.json --out models/brazier.voxel.yaml
    ///
    /// Unlike `new` and `upscale` this OVERWRITES its `--out`: regenerating is the point. A model
    /// whose script owns it declares `generated_from:` and joins the repo's `voxel-drift` gate,
    /// which runs the `--check` form — regenerate in memory, byte-compare against the committed
    /// file, write nothing. `--validate` runs the same in-memory generation and validation
    /// standalone, with no committed file to compare against and nothing written — for eyeballing
    /// a script's output before it earns a path.
    #[command(verbatim_doc_comment)]
    #[command(group(
        ArgGroup::new("from_json_target").args(["out", "check", "validate"]).required(true)
    ))]
    FromJson {
        /// Read the intermediate from this file instead of stdin.
        #[arg(long = "in", value_name = "JSON")]
        input: Option<PathBuf>,
        /// Where to write the canonical model (`<name>.voxel.yaml`); an existing file is replaced.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Byte-compare against this committed model instead of writing: exit 0 when identical,
        /// non-zero with the first differing line. The drift gate's form.
        #[arg(long)]
        check: Option<PathBuf>,
        /// Validate the generated model in memory — the L1115/L1129 warnings and an exposure note
        /// — and write nothing. No committed file needed, unlike `--check`.
        #[arg(long)]
        validate: bool,
    },
}

/// The `ludion sculpt` authoring verbs.
#[derive(Debug, Subcommand)]
enum SculptCommand {
    /// Scaffold an empty sculpt model: one root bone, an empty `parts:`, and the header comment
    /// template. It never overwrites an existing file.
    New {
        /// Where to write the model (`<name>.sculpt.yaml`).
        out: PathBuf,
        /// The model's `name:`. Defaults to the output file's stem.
        #[arg(long)]
        name: Option<String>,
        /// Copy the palette from an existing voxel or sculpt document instead of writing a
        /// one-entry stub — how a new model joins an existing set's colour language.
        #[arg(long, value_name = "MODEL")]
        palette_from: Option<PathBuf>,
    },
}

/// The `ludion texture` verbs. Each takes `--json` (exactly one compact JSON object on one stdout
/// line; diagnostics always render to stderr).
#[derive(Debug, Subcommand)]
enum TextureCommand {
    /// Evaluate a recipe and write its map set: `base_color.png`, `metallic_roughness.png`,
    /// `normal.png`, `occlusion.png`, `emissive.png` (only when authored), a 16-bit `height.png`
    /// and a `generated.json` sidecar, then run the PBR plausibility validators. Maps are written
    /// even when a validator fails at level `error` (the exit code is non-zero) so they can be
    /// inspected.
    Build {
        /// The recipe (`<name>.texture.yaml`).
        recipe: PathBuf,
        /// Texels per side of every map (8..=8192).
        #[arg(long, default_value_t = texture::DEFAULT_SIZE)]
        size: u32,
        /// The directory to write. Defaults to `<recipe dir>/<name>/`.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// A TOML file overriding validator bounds and levels: `<validator> = { min, max, level }`,
        /// with `[textures."<recipe file>"]` tables for per-recipe overrides.
        #[arg(long, value_name = "TOML")]
        rules: Option<PathBuf>,
        /// Build with this root seed instead of the recipe's `seed:` (to preview a candidate from
        /// `texture seeds`). Needs `--out`: a set built from another seed is not the committed one.
        #[arg(long, value_name = "N", requires = "out")]
        seed: Option<u64>,
        /// Also write the feedback sheets and a rendered preview to an ignored folder.
        #[arg(long)]
        preview: bool,
        /// The feedback folder. Defaults to `<recipe dir>/.ludion/texture/<name>/`.
        #[arg(long, value_name = "DIR", requires = "preview")]
        preview_out: Option<PathBuf>,
        /// With `--preview`, skip the rendered `preview.png` (no GPU is touched).
        #[arg(long, requires = "preview")]
        no_render: bool,
        /// Print one JSON report object (timings per step, per-map statistics, validators).
        #[arg(long)]
        json: bool,
    },
    /// Rebuild a recipe at the size its `generated.json` records and byte-compare the committed
    /// set (`L1286` per differing, missing or stray file), then run the validators. Writes nothing.
    Check {
        /// The recipe (`<name>.texture.yaml`).
        recipe: PathBuf,
        /// The directory holding the set. Defaults to `<recipe dir>/<name>/`.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// A TOML file overriding validator bounds and levels, as for `build`.
        #[arg(long, value_name = "TOML")]
        rules: Option<PathBuf>,
        /// Print one JSON report object.
        #[arg(long)]
        json: bool,
    },
    /// Evaluate a recipe once per root seed and report each seed's validators and the first seed
    /// that passes them all. Report-only: it writes nothing; set `seed:` in the recipe yourself,
    /// and preview a candidate with `build --seed N --out DIR`.
    Seeds {
        /// The recipe (`<name>.texture.yaml`).
        recipe: PathBuf,
        /// The seeds to try: a half-open range `A..B` or a comma list `3,7,11` (at most 256).
        #[arg(long, default_value = "0..16", value_parser = texture::parse_seeds)]
        seeds: texture::SeedList,
        /// Texels per side. Defaults to the size the committed `generated.json` records, else 1024.
        #[arg(long)]
        size: Option<u32>,
        /// A TOML file overriding validator bounds and levels, as for `build`.
        #[arg(long, value_name = "TOML")]
        rules: Option<PathBuf>,
        /// Print one JSON report object (no timings, so two runs are byte-identical).
        #[arg(long)]
        json: bool,
    },
    /// Copy an embedded `std:` recipe out of the binary so it can be edited. Never overwrites.
    Vendor {
        /// The recipe, `std:cobbles` or `cobbles`.
        name: String,
        /// Where to write it. Defaults to `./<name>.texture.yaml`.
        dest: Option<PathBuf>,
        /// Print one JSON object.
        #[arg(long)]
        json: bool,
    },
}

/// The `ludion lut` verbs. Each takes `--json` (exactly one compact JSON object on one stdout line;
/// diagnostics always render to stderr).
#[derive(Debug, Subcommand)]
enum LutCommand {
    /// Write the identity LUT: a table that leaves every colour unchanged, the starting point to
    /// grade in an external tool.
    Identity {
        /// Lattice points per axis: 17, 33 or 65.
        #[arg(long, default_value_t = 33)]
        size: u32,
        /// The `.cube` file to write.
        #[arg(long)]
        out: PathBuf,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
    /// Bake a scene's analytic grade (white balance, lift/gamma/gain, contrast, saturation) into a
    /// `.cube`. The LUT runs after the scene's own grade, so to reproduce the grade reset those
    /// fields to identity and point `PostProcess.lut` at the baked file.
    Bake {
        /// The scene whose `PostProcess` grade is baked.
        #[arg(long)]
        scene: PathBuf,
        /// The `.cube` file to write.
        #[arg(long)]
        out: PathBuf,
        /// Lattice points per axis: 17, 33 or 65.
        #[arg(long, default_value_t = 33)]
        size: u32,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
    /// Run a LUT over a PNG still: the in-engine operation (the LUT indexed by the sRGB-encoded
    /// colour, mixed by `--strength`), alpha kept.
    Apply {
        /// The `.cube` file to apply.
        #[arg(long)]
        lut: PathBuf,
        /// The PNG to grade.
        #[arg(long)]
        image: PathBuf,
        /// The PNG to write.
        #[arg(long)]
        out: PathBuf,
        /// Mix of the graded colour into the original, 0 to 1.
        #[arg(long, default_value_t = 1.0)]
        strength: f32,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
    /// Report how a LUT shifts six fixed colour bands (amber, cyan, green, magenta, skin, neutral
    /// grey): the change in hue, saturation and lightness, in HSL over the encoded sRGB colour.
    Stats {
        /// The `.cube` file to measure.
        lut: PathBuf,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
}

/// The `ludion mesh` texture-editing verbs. Both take `--json` (exactly one compact JSON object on
/// one stdout line; diagnostics always render to stderr).
#[derive(Debug, Subcommand)]
enum MeshCommand {
    /// Name the point on a mesh's surface under one pixel of a rendered model sheet.
    ///
    /// Rebuilds the exact camera `ludion render --model` draws that view's cell with and casts one
    /// CPU ray — no GPU, no device, nothing rendered here. It reads a SINGLE-CELL image, so render
    /// the sheet with `--views <one view>` and pass the same `--width`/`--height`; a multi-view
    /// sheet divides its cells and the pixel you read belongs to none of this camera's.
    ///
    /// `--pixel X,Y` names a pixel of that image, origin top-left, `+Y` down, and the ray is cast
    /// through that pixel's CENTRE. Integers and fractions are both accepted, and the same half-
    /// pixel offset applies to either.
    ///
    /// The point is reported in the mesh's own coordinates — the space a `kind: mesh-edit` region
    /// is written in — alongside the normalized position the cell actually drew, and the view is
    /// named by the world axis its camera sits on (`FRONT -Z`), never by the authoring-space
    /// nickname for the authored front.
    #[command(verbatim_doc_comment)]
    Pick {
        /// The mesh document to pick on (`kind: mesh`, e.g. `mesh-0.mesh.yaml`).
        mesh: PathBuf,
        /// Which view's cell the pixel was read off: `front`, `back`, `left`, `right`, `top`,
        /// `iso` or `game`.
        #[arg(long, value_name = "NAME")]
        view: String,
        /// The pixel to cast through, `X,Y` from the image's top-left corner.
        #[arg(long, value_name = "X,Y", value_parser = mesh::parse_pixel)]
        pixel: [f64; 2],
        /// The cell's width in pixels. Defaults to `render`'s own default.
        #[arg(long)]
        width: Option<u32>,
        /// The cell's height in pixels. Defaults to `render`'s own default.
        #[arg(long)]
        height: Option<u32>,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
    /// Derive the tree a `kind: mesh-edit` document owns: the edited mesh document plus one
    /// content-hash-named PNG per map its ops touched, written into the document's `out`
    /// directory and nowhere else. The source mesh and its committed maps are never rewritten.
    ///
    /// Prints the per-op report — texels painted, their share of the map, the model-space box of
    /// the surface they sit on, the triangles it spans — which is how an edit is verified without
    /// rendering anything. An op whose region misses the surface is an error (`L1177`). With
    /// `--view`, each op also reports that box as the pixels it covers in one sheet cell.
    Apply {
        /// The edit document to apply (`kind: mesh-edit`, e.g. `idol-lit.mesh-edit.yaml`).
        edit: PathBuf,
        /// Project each op's painted box into this view's single cell, as the pixel span it covers
        /// — through the exact camera `pick` reads; the names are `pick`'s.
        #[arg(long, value_name = "NAME")]
        view: Option<String>,
        /// The cell's width in pixels, with `--view`. Defaults to `render`'s own default, as in
        /// `pick`.
        #[arg(long, requires = "view")]
        width: Option<u32>,
        /// The cell's height in pixels, with `--view`. Defaults to `render`'s own default, as in
        /// `pick`.
        #[arg(long, requires = "view")]
        height: Option<u32>,
        /// Re-derive in memory and byte-compare against the committed tree instead of writing:
        /// exit 0 when identical, non-zero with one `DRIFT <path>` line per file that differs.
        /// The drift gate's form.
        #[arg(long)]
        check: bool,
        /// Print one JSON object instead of the default text.
        #[arg(long)]
        json: bool,
    },
}

/// The `check-scene --from` token set. One variant today, and an enum rather than a flag because
/// the axis is *which* viewpoint: a second one (a fixed pitch, another game's camera) adds a token
/// here without adding a flag.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliExposureView {
    /// A twin-stick follow camera: 58° down over the authored front.
    Game,
}

impl From<CliExposureView> for ludion_scene::ExposureView {
    fn from(view: CliExposureView) -> Self {
        match view {
            CliExposureView::Game => ludion_scene::ExposureView::Game,
        }
    }
}

/// The `--views` token set, mirroring `ludion_engine::ModelView` (which has no `ValueEnum` impl of
/// its own — that type lives in the engine crate and stays clap-free).
#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliModelView {
    Front,
    Back,
    Left,
    Right,
    Top,
    Iso,
    Game,
}

/// The `--debug-view` token set, mirroring `ludion_engine::DebugView` (the engine type stays
/// clap-free); the names are the engine's own spelling.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliDebugView {
    #[value(name = "ssr_confidence")]
    SsrConfidence,
    #[value(name = "ssr_footprint")]
    SsrFootprint,
    #[value(name = "probe_index")]
    ProbeIndex,
    #[value(name = "local_shadow_atlas")]
    LocalShadowAtlas,
    #[value(name = "sun_cascades")]
    SunCascades,
    #[value(name = "irradiance")]
    Irradiance,
    #[value(name = "exposure_meter")]
    ExposureMeter,
}

impl From<CliDebugView> for ludion_engine::DebugView {
    fn from(view: CliDebugView) -> Self {
        match view {
            CliDebugView::SsrConfidence => ludion_engine::DebugView::SsrConfidence,
            CliDebugView::SsrFootprint => ludion_engine::DebugView::SsrFootprint,
            CliDebugView::ProbeIndex => ludion_engine::DebugView::ProbeIndex,
            CliDebugView::LocalShadowAtlas => ludion_engine::DebugView::LocalShadowAtlas,
            CliDebugView::SunCascades => ludion_engine::DebugView::SunCascades,
            CliDebugView::Irradiance => ludion_engine::DebugView::Irradiance,
            CliDebugView::ExposureMeter => ludion_engine::DebugView::ExposureMeter,
        }
    }
}

/// The `--channel` token set, mirroring `ludion_engine::ModelChannel` — the [`CliModelView`] shape,
/// for the same reason (the engine type stays clap-free).
#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliModelChannel {
    Emissive,
}

impl From<CliModelChannel> for ludion_engine::ModelChannel {
    fn from(channel: CliModelChannel) -> Self {
        match channel {
            CliModelChannel::Emissive => ludion_engine::ModelChannel::Emissive,
        }
    }
}

impl From<CliModelView> for ludion_engine::ModelView {
    fn from(view: CliModelView) -> Self {
        match view {
            CliModelView::Front => ludion_engine::ModelView::Front,
            CliModelView::Back => ludion_engine::ModelView::Back,
            CliModelView::Left => ludion_engine::ModelView::Left,
            CliModelView::Right => ludion_engine::ModelView::Right,
            CliModelView::Top => ludion_engine::ModelView::Top,
            CliModelView::Iso => ludion_engine::ModelView::Iso,
            CliModelView::Game => ludion_engine::ModelView::Game,
        }
    }
}

/// Whether `render` was given `--width` or `--height` on the command line, as opposed to taking
/// their defaults. `--true-size` computes the cell from the subject and reads the requested size as
/// an aspect only, so an explicit one is a contradiction worth refusing — and clap cannot express
/// "conflicts with a *defaulted* argument", which is why this is a `ValueSource` check rather than a
/// `conflicts_with`.
fn render_size_was_requested(matches: &clap::ArgMatches) -> bool {
    let Some(render) = matches.subcommand_matches("render") else {
        return false;
    };
    ["width", "height"]
        .iter()
        .any(|name| render.value_source(name) == Some(clap::parser::ValueSource::CommandLine))
}

/// Runs one `ludion lut` verb.
fn run_lut(command: &LutCommand) -> std::process::ExitCode {
    match command {
        LutCommand::Identity { size, out, json } => lut::run_identity(*size, out, *json),
        LutCommand::Bake {
            scene,
            out,
            size,
            json,
        } => lut::run_bake(scene, out, *size, *json),
        LutCommand::Apply {
            lut,
            image,
            out,
            strength,
            json,
        } => lut::run_apply(lut, image, out, *strength, *json),
        LutCommand::Stats { lut, json } => lut::run_stats(lut, *json),
    }
}

/// Runs one `ludion mesh` verb.
fn run_mesh(command: &MeshCommand) -> std::process::ExitCode {
    match command {
        MeshCommand::Pick {
            mesh: model,
            view,
            pixel,
            width,
            height,
            json,
        } => mesh::run_pick(model, view, *pixel, *width, *height, *json),
        MeshCommand::Apply {
            edit,
            view,
            width,
            height,
            check,
            json,
        } => {
            let view = view.as_ref().map(|name| mesh::ApplyView {
                name: name.clone(),
                width: width.unwrap_or(mesh::DEFAULT_WIDTH),
                height: height.unwrap_or(mesh::DEFAULT_HEIGHT),
            });
            mesh::run_apply(edit, *check, *json, view)
        }
    }
}

/// Runs one `ludion gen` verb.
fn run_gen(command: &GenCommand) -> std::process::ExitCode {
    match command {
        GenCommand::Nodes {
            filter,
            class,
            url,
            json,
        } => gen_cmd::run_nodes(filter.as_deref(), class.as_deref(), url.as_deref(), *json),
        GenCommand::Check { recipe, url, json } => {
            gen_cmd::run_check(recipe, url.as_deref(), *json)
        }
        GenCommand::Run {
            recipe,
            count,
            seed,
            image,
            timeout,
            no_wait,
            url,
            json,
        } => gen_cmd::run_run(
            recipe,
            *count,
            *seed,
            image.as_deref(),
            *timeout,
            *no_wait,
            url.as_deref(),
            *json,
        ),
        GenCommand::Status { run_dir, url, json } => {
            gen_cmd::run_status(run_dir.as_deref(), url.as_deref(), *json)
        }
        GenCommand::Fetch { run_dir, url, json } => {
            gen_cmd::run_fetch(run_dir, url.as_deref(), *json)
        }
        GenCommand::Sheet {
            run_dir,
            cols,
            out,
            json,
        } => gen_cmd::run_sheet(run_dir, *cols, out.as_deref(), *json),
        GenCommand::Accept {
            run_dir,
            index,
            out,
            allow_lossy,
            json,
        } => gen_cmd::run_accept(run_dir, *index, out, *allow_lossy, *json),
    }
}

/// Refuses a per-machine knob given to a verb family that reads none of them, naming the first one
/// given — the family's own `--threads`/`--quality`/`--render-size` are hidden shadows that exist
/// only to land here, and the top-level spellings reach the same fields through clap's propagation.
fn refuse_knobs(cli: &Cli, family: &str, shadows: &Shadows<'_>) -> Option<std::process::ExitCode> {
    let given = if cli.threads.or(*shadows.threads).is_some() {
        "--threads"
    } else if cli.quality.is_some() || shadows.quality.is_some() {
        "--quality"
    } else if cli.render_size.is_some() || shadows.render_size.is_some() {
        "--render-size"
    } else {
        return None;
    };
    Some(crate::diagnostic::usage_error(format!(
        "`{given}` does not apply to `ludion {family}`: no {family} verb drives a sim or a render pass"
    )))
}

/// The hidden knob shadows a verb family carries, borrowed from its `Command` variant.
struct Shadows<'a> {
    threads: &'a Option<usize>,
    quality: &'a Option<String>,
    render_size: &'a Option<String>,
}

/// Runs the verbs that read none of `--threads`, `--quality` and `--render-size`, so a malformed
/// per-machine environment variable never blocks the help system, a scaffold or a verb that talks
/// to a ComfyUI server or edits a mesh. A knob given to `gen` or `mesh` is refused rather than
/// ignored. `None` means the verb drives a sim or a renderer and waits for the knobs to resolve.
fn knob_free(cli: &Cli) -> Option<std::process::ExitCode> {
    Some(match &cli.command {
        Command::Gen {
            command,
            threads,
            quality,
            render_size,
        } => {
            let shadows = Shadows {
                threads,
                quality,
                render_size,
            };
            refuse_knobs(cli, "gen", &shadows).unwrap_or_else(|| run_gen(command))
        }
        Command::Mesh {
            command,
            threads,
            quality,
            render_size,
        } => {
            let shadows = Shadows {
                threads,
                quality,
                render_size,
            };
            refuse_knobs(cli, "mesh", &shadows).unwrap_or_else(|| run_mesh(command))
        }
        Command::Lut {
            command,
            threads,
            quality,
            render_size,
        } => {
            let shadows = Shadows {
                threads,
                quality,
                render_size,
            };
            refuse_knobs(cli, "lut", &shadows).unwrap_or_else(|| run_lut(command))
        }
        Command::Explain { code } => match code {
            Some(code) => explain::run(code),
            None => explain::list(),
        },
        Command::Docs {
            topic,
            path,
            section,
        } => docs::run(topic.as_deref(), *path, section.as_deref()),
        Command::Devdocs {
            topic,
            path,
            section,
        } => docs::run_dev(topic.as_deref(), *path, section.as_deref()),
        Command::ImageStats {
            images,
            json,
            check,
        } => image_stats::run(images, *json, check.as_deref()),
        Command::New {
            refresh_guide: true,
            ..
        } => new::refresh_guide(),
        Command::New {
            name: Some(name), ..
        } => new::run(name),
        Command::New { name: None, .. } => {
            unreachable!("clap requires a name without --refresh-guide")
        }
        _ => return None,
    })
}

fn main() -> std::process::ExitCode {
    // `Cli::parse()` spelled out, because the `--true-size` gate needs to know which arguments were
    // *given* rather than defaulted, which only the raw matches record.
    let matches = <Cli as clap::CommandFactory>::command().get_matches();
    let cli = match <Cli as clap::FromArgMatches>::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    let sized = render_size_was_requested(&matches);

    if let Some(exit) = knob_free(&cli) {
        return exit;
    }

    // Resolved once, before any verb that reads a knob runs (`knob_free` answers the rest first),
    // so every verb that drives a sim — `serve`, `render`, `test`, `replay`, `sim-hash`, `profile`
    // — gets the same worker count from the same precedence chain: `--threads`, else
    // `LUDION_THREADS`, else this machine's capped parallelism.
    // (`dev` spawns the game as a child process, which inherits the environment and resolves it the
    // same way through `ludion::run`.)
    if let Err(invalid) = ludion_core::par::set_startup_threads(cli.threads) {
        eprintln!("{}", invalid.diagnostic());
        return std::process::ExitCode::FAILURE;
    }

    // The same posture for the same reason, one step further: this machine's render quality, from
    // `--quality`, else `LUDION_QUALITY`, else everything the scene authors. Resolved before any
    // verb that reads a knob, so a malformed setting refuses to start even on one that renders
    // little — a silently-ignored setting means rendering at a quality nobody chose. (`dev`
    // forwards none of the three knobs on this page: it spawns the game as a child process, which
    // inherits `LUDION_THREADS`, `LUDION_QUALITY` and `LUDION_RENDER_SIZE` from the environment and
    // resolves each for itself through `ludion::run`.)
    let quality = match ludion_engine::resolve_startup_quality(cli.quality.as_deref(), None) {
        Ok(quality) => quality,
        Err(invalid) => {
            eprintln!("{invalid}");
            return std::process::ExitCode::FAILURE;
        }
    };

    // And this machine's render extent, from `--render-size`, else `LUDION_RENDER_SIZE`, else the
    // engine's own default. Resolved here with its siblings so a malformed value refuses to start
    // on every verb that reads a knob, but *applied* by `serve` alone: an extent is what a live
    // stream actually delivers, and a per-machine environment variable that silently moved
    // `render`'s or `dump-graph`'s default would move committed goldens and sheets with it — the
    // two things this knob's safety rests on being immovable. Those verbs keep taking their extent
    // from their own explicit `--width`/`--height`.
    let render_size = match ludion_engine::resolve_startup_render_size(cli.render_size.as_deref()) {
        Ok(render_size) => render_size,
        Err(invalid) => {
            eprintln!("{invalid}");
            return std::process::ExitCode::FAILURE;
        }
    };

    match cli.command {
        Command::Explain { .. }
        | Command::Docs { .. }
        | Command::Devdocs { .. }
        | Command::ImageStats { .. }
        | Command::New { .. }
        | Command::Gen { .. }
        | Command::Mesh { .. }
        | Command::Lut { .. } => {
            unreachable!("answered by `knob_free` before the knobs resolve")
        }
        Command::CheckScene {
            file,
            mirror_x,
            slice,
            column,
            exposure,
            from,
            bounds,
            json,
            placement,
            ground_y,
            tolerance,
            sink,
            rows,
        } => check_scene::run(
            &file,
            &check_scene::VoxelViews {
                mirror_x,
                slice,
                column,
                exposure,
                from: from.map(Into::into).unwrap_or_default(),
            },
            &check_scene::SculptViews { bounds, json },
            placement.then(|| placement::PlacementOptions::new(ground_y, tolerance, sink)),
            rows,
        ),
        Command::SimHash {
            scene,
            script,
            frames,
            record,
        } => sim_hash::run(&scene, script.as_deref(), frames, record.as_deref()),
        Command::Trace {
            scene,
            script,
            frames,
            channels,
        } => trace::run(&scene, script.as_deref(), frames, &channels),
        Command::Replay { file, hashes } => replay::run(&file, hashes),
        Command::Render {
            scene,
            model,
            views,
            rig,
            sprite,
            atlas,
            font,
            out,
            width,
            height,
            dump_timings,
            dump_voxels,
            dump_gi,
            dump_probes,
            set,
            sweep,
            steps,
            frame,
            frames,
            debug_draw,
            debug_view,
            look_at,
            azimuth,
            pitch,
            distance,
            fov,
            scale,
            aa,
            true_size,
            clip,
            time,
            strip,
            crossfade,
            blend,
            layer,
            look,
            ik,
            hold,
            part,
            isolate,
            channel,
            onion,
        } => {
            if debug_draw.is_some() && frames.len() >= 2 {
                <Cli as clap::CommandFactory>::command()
                    .error(
                        clap::error::ErrorKind::ArgumentConflict,
                        "the argument '--debug-draw' cannot be used with a multi-value '--frames' \
                         strip: the strip renders through the contact-sheet machinery, which draws \
                         no debug lines — pass one '--frames N' to draw the world after N ticks",
                    )
                    .exit();
            }
            // A time strip reads left to right; a descending or repeated list would stitch cells
            // whose order silently disagrees with the numbers printed on them.
            if frames.windows(2).any(|pair| pair[0] >= pair[1]) {
                <Cli as clap::CommandFactory>::command()
                    .error(
                        clap::error::ErrorKind::InvalidValue,
                        format!(
                            "invalid value for '--frames <FRAMES>': values must be strictly \
                             ascending, e.g. '--frames 0,30,60' — got '{}'",
                            frames
                                .iter()
                                .map(u64::to_string)
                                .collect::<Vec<_>>()
                                .join(",")
                        ),
                    )
                    .exit();
            }
            if true_size && sized {
                <Cli as clap::CommandFactory>::command()
                    .error(
                        clap::error::ErrorKind::ArgumentConflict,
                        "the argument '--true-size' cannot be used with '--width'/'--height': \
                         true-size computes the cell size from the model's declared scale, and \
                         reads the requested size as an aspect ratio only",
                    )
                    .exit();
            }
            // `--dump-voxels`/`--dump-gi` alone need no `--out`; `--dump-timings` always renders
            // through the GPU and has nothing to time without one.
            if dump_timings.is_some() && out.is_none() {
                <Cli as clap::CommandFactory>::command()
                    .error(
                        clap::error::ErrorKind::MissingRequiredArgument,
                        "the following required argument was not provided: --out <OUT>\n\n  \
                         --dump-timings needs the full render `--out` produces; `--dump-voxels`/\
                         `--dump-gi` alone do not require one",
                    )
                    .exit();
            }
            // A time strip is one stitched sheet; `--dump-voxels`/`--dump-gi` lift clap's `--out`
            // requirement, so the strip has to ask for it itself.
            if frames.len() >= 2 && out.is_none() {
                <Cli as clap::CommandFactory>::command()
                    .error(
                        clap::error::ErrorKind::MissingRequiredArgument,
                        "the following required argument was not provided: --out <OUT>\n\n  a \
                         `--frames` strip stitches one sheet, so it has no default output name",
                    )
                    .exit();
            }
            // A lineup has no single model to name its output after, and silently inventing one
            // (the first model's stem? a joined stem?) would be a policy nobody asked for.
            if model.len() > 1 && out.is_none() {
                <Cli as clap::CommandFactory>::command()
                    .error(
                        clap::error::ErrorKind::MissingRequiredArgument,
                        "the following required argument was not provided: --out <OUT>\n\n  a \
                         lineup of several --model paths names no single model, so it has no \
                         default output name",
                    )
                    .exit();
            }
            let views: Vec<ludion_engine::ModelView> = views
                .into_iter()
                .map(ludion_engine::ModelView::from)
                .collect();
            let models: Vec<&std::path::Path> = model.iter().map(PathBuf::as_path).collect();
            render::run(&render::Request {
                scene: scene.as_deref(),
                models: &models,
                views: &views,
                rig: rig.as_deref(),
                sprite: sprite.as_deref(),
                atlas: atlas.as_deref(),
                font: font.as_deref(),
                out: out.as_deref(),
                width,
                height,
                dump_timings: dump_timings.as_deref(),
                dump_voxels: dump_voxels.as_deref(),
                dump_gi: dump_gi.as_deref(),
                dump_probes: dump_probes.as_deref(),
                set: &set,
                sweep: sweep.as_deref(),
                steps,
                frame: frame.as_deref(),
                frames: &frames,
                debug_draw: debug_draw.as_deref(),
                debug_view: debug_view.map(Into::into),
                framing: framing::FramingArgs {
                    look_at: look_at.as_deref(),
                    azimuth,
                    pitch,
                    distance,
                    fov,
                },
                scale,
                anti_aliasing: aa.unwrap_or_default(),
                true_size,
                clip: clip.as_deref(),
                time,
                strip,
                crossfade: crossfade.as_deref(),
                blend: blend.as_deref(),
                layers: &layer,
                look: look.as_deref(),
                ik: &ik,
                hold,
                part: part.as_deref(),
                isolate,
                channel: channel.map(Into::into),
                onion,
                quality: &quality,
            })
        }
        Command::BakeProbes { scene, probe, json } => {
            bake_probes::run(&scene, probe.as_deref(), &quality, json)
        }
        Command::DumpGraph {
            scene,
            width,
            height,
            scale,
            aa,
        } => dump_graph::run(
            &scene,
            width,
            height,
            scale,
            aa.unwrap_or_default(),
            &quality,
        ),
        Command::Test { dir, bless, filter } => test::run(&dir, bless, filter.as_deref()),
        Command::Serve {
            scene,
            addr,
            record,
            paused,
            window,
            dev_shaders,
            scale,
            probes,
            aa,
        } => serve::run(
            &scene,
            &addr,
            record.as_deref(),
            paused,
            window,
            dev_shaders,
            scale,
            aa,
            quality,
            render_size,
            probes.as_deref(),
        ),
        Command::Import {
            input,
            out,
            allow_lossy,
            lod,
            quantize,
            name,
        } => import::run(&input, &out, allow_lossy, lod, quantize, name),
        Command::Sync {
            project,
            json,
            targets,
            no_cook,
        } => sync::run(
            &project,
            json,
            &cook::CookRequest {
                enabled: !no_cook,
                extra_targets: targets
                    .iter()
                    .filter_map(|name| ludion_assets::cooked::CookTarget::from_name(name))
                    .collect(),
            },
            &quality,
        ),
        Command::Profile {
            scene,
            attach,
            frames,
            json,
            check,
            scale,
            aa,
            perfetto,
        } => match attach {
            Some(url) => profile::run_attach(
                &url,
                frames,
                json.as_deref(),
                check.as_deref(),
                perfetto.as_deref(),
            ),
            None => {
                let scene = scene.expect("clap requires --scene unless --attach is given");
                profile::run(
                    &scene,
                    frames,
                    json.as_deref(),
                    check.as_deref(),
                    scale,
                    aa,
                    perfetto.as_deref(),
                    quality,
                )
            }
        },
        Command::Export {
            target,
            out,
            project,
            debug,
            replays,
            run,
            app_args,
        } => export::run(
            target,
            &out,
            &project,
            &export::Options {
                debug,
                replays: &replays,
                run,
                app_args: &app_args,
            },
        ),
        Command::Dev {
            scene,
            addr,
            window,
            probes,
            aa,
        } => dev::run(
            scene.as_deref(),
            &addr,
            window,
            probes.as_deref() == Some("rebake"),
            aa,
        ),
        Command::GenerateMap {
            maps,
            out,
            check,
            kit,
            seed,
            mirror,
            prefab,
        } => generate_map::run(
            &maps,
            out.as_deref(),
            check,
            kit.as_deref(),
            seed,
            mirror.as_deref(),
            prefab,
        ),
        Command::GenerateAtlas {
            sources,
            out_dir,
            check,
        } => generate_atlas::run(&sources, out_dir.as_deref(), check),
        Command::MapCheck {
            maps,
            anchor,
            radius,
            at,
            traverse,
            speed,
            kit,
            seed,
            mirror,
        } => map_check::run(
            &maps,
            anchor.as_deref(),
            radius,
            at.as_deref(),
            traverse,
            speed,
            kit.as_deref(),
            seed,
            mirror.as_deref(),
        ),
        Command::MapDrift {
            root,
            dirs,
            json,
            deny_warnings,
            allow,
        } => map_drift::run(root.as_deref(), &dirs, json, deny_warnings, &allow),
        Command::Voxel { command } => match command {
            VoxelCommand::New {
                out,
                size,
                name,
                scale,
                palette_from,
            } => voxel::run_new(&out, &size, name.as_deref(), scale, palette_from.as_deref()),
            VoxelCommand::Apply {
                model,
                ops,
                dry_run,
            } => voxel::run_apply(&model, &ops, dry_run),
            VoxelCommand::Upscale { model, factor, out } => {
                voxel::run_upscale(&model, factor, out.as_deref())
            }
            VoxelCommand::Diff { a, b } => voxel::run_diff(&a, &b),
            VoxelCommand::FromJson {
                input,
                out,
                check,
                validate,
            } => voxel::run_from_json(input.as_deref(), out.as_deref(), check.as_deref(), validate),
        },
        Command::Texture { command } => match command {
            TextureCommand::Build {
                recipe,
                size,
                out,
                rules,
                seed,
                preview,
                preview_out,
                no_render,
                json,
            } => {
                let preview = preview.then_some(texture::Preview {
                    dir: preview_out.as_deref(),
                    render: !no_render,
                });
                texture::run_build(
                    &recipe,
                    size,
                    out.as_deref(),
                    rules.as_deref(),
                    preview,
                    seed,
                    json,
                )
            }
            TextureCommand::Check {
                recipe,
                out,
                rules,
                json,
            } => texture::run_check(&recipe, out.as_deref(), rules.as_deref(), json),
            TextureCommand::Seeds {
                recipe,
                seeds,
                size,
                rules,
                json,
            } => texture::run_seeds(&recipe, &seeds.0, size, rules.as_deref(), json),
            TextureCommand::Vendor { name, dest, json } => {
                texture::run_vendor(&name, dest.as_deref(), json)
            }
        },
        Command::Sculpt { command } => match command {
            SculptCommand::New {
                out,
                name,
                palette_from,
            } => sculpt::run_new(&out, name.as_deref(), palette_from.as_deref()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aa_flag_lists_exactly_the_modes_and_refuses_the_rest() {
        let parser = aa_value_parser();
        let listed: Vec<_> = clap::builder::TypedValueParser::possible_values(&parser)
            .expect("a possible-values parser lists its values")
            .map(|value| value.get_name().to_string())
            .collect();
        assert_eq!(
            listed,
            ludion_engine::AntiAliasing::ALL.map(|mode| mode.name().to_string())
        );

        assert!(
            Cli::try_parse_from(["ludion", "dump-graph", "--scene", "x", "--aa", "bogus"]).is_err()
        );
        let parsed = Cli::try_parse_from(["ludion", "dump-graph", "--scene", "x", "--aa", "smaa"])
            .expect("a listed mode parses");
        assert!(matches!(
            parsed.command,
            Command::DumpGraph {
                aa: Some(ludion_engine::AntiAliasing::Smaa),
                ..
            }
        ));
    }

    #[test]
    fn profile_attach_refuses_aa() {
        assert!(
            Cli::try_parse_from(["ludion", "profile", "--attach", "u", "--aa", "smaa"]).is_err()
        );
    }
}
