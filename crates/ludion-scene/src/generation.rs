//! Generation recipes (`kind: generation`) and the provenance block a generated asset carries.
//!
//! A recipe names a committed image-generation workflow and overrides its leaf inputs by node
//! title: `workflow` (a path relative to the recipe's own directory), a base `seed`, a candidate
//! `count`, a map of named `images` to upload, and `inputs`, a map of node title → input name →
//! value. A recipe never rewires the graph — the one structured value it can spell is
//! `{image: <name>}`, a reference into its own `images` map — so an edge stays the workflow's
//! business.
//!
//! [`Origin`] is the other half: the provenance map an accepted asset keeps, with two carriers and
//! one writer — an optional `origin:` key on a [`crate::MeshDocument`], and a `kind: origin`
//! sidecar document beside a generated image, which has no text place of its own.
//!
//! Three things the signatures do not say:
//!
//! - **This module reads no file.** `workflow` and every `images` path are checked for shape, never
//!   for existence: resolving them is the caller's job, against the recipe's own directory (the
//!   sculpt `shape: voxel` rule), so this crate stays filesystem-agnostic.
//! - **The image reference is block style, like every other mapping here.** The strict YAML subset
//!   rejects a non-empty flow collection, so the reference is written as a nested `image:` key, not
//!   as `{image: subject}`.
//! - **[`parse_origin`] takes its diagnostic code from the caller**, the [`crate::envelope`]
//!   pattern: an `origin:` inside a mesh document reports under the mesh's own `L1110`/`L1111`
//!   while the traversal producing the diagnostic stays single-sourced here.
//!
//! The codes past the document layer belong to the generation client, which owns the workflow file,
//! the server and the run: [`ludion_core::codes::code::WORKFLOW_NOT_API`],
//! [`ludion_core::codes::code::WORKFLOW_UNKNOWN_NODE`],
//! [`ludion_core::codes::code::WORKFLOW_INPUT_INVALID`],
//! [`ludion_core::codes::code::WORKFLOW_TITLE_UNRESOLVED`],
//! [`ludion_core::codes::code::COMFY_UNREACHABLE`], [`ludion_core::codes::code::COMFY_REJECTED`],
//! [`ludion_core::codes::code::GENERATION_RUN_FAILED`],
//! [`ludion_core::codes::code::GENERATION_RUN_TIMED_OUT`],
//! [`ludion_core::codes::code::GENERATION_OUTPUT_KIND`] and
//! [`ludion_core::codes::code::GENERATION_ACCEPT_REFUSED`]. None is raised here.

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, MapEntry, Node, NodeValue, pointer_token};

/// The generation recipe format version this crate reads and writes.
pub const GENERATION_FORMAT_VERSION: u64 = 1;

/// The `kind: origin` sidecar format version this crate reads and writes.
pub const ORIGIN_FORMAT_VERSION: u64 = 1;

/// `count` when a recipe omits it: one candidate per run.
pub const GENERATION_DEFAULT_COUNT: u32 = 1;

/// The largest `count` a recipe or `--count` may ask for: each candidate is one generation job on
/// the ComfyUI server.
pub const GENERATION_MAX_COUNT: u32 = 64;

const STRUCTURE: DiagnosticCode = code::GENERATION_STRUCTURE;
const DATA: DiagnosticCode = code::GENERATION_DATA;

const GENERATION_KEYS: [&str; 8] = [
    "format", "kind", "name", "workflow", "seed", "count", "images", "inputs",
];

/// The provenance block's key set, which is also its canonical emit order.
const ORIGIN_KEYS: [&str; 13] = [
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
];

/// The sidecar document's key set: the envelope, then [`ORIGIN_KEYS`].
const ORIGIN_DOCUMENT_KEYS: [&str; 15] = [
    "format",
    "kind",
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
];

/// One image entry's key set, which is also its emit order.
const ORIGIN_IMAGE_KEYS: [&str; 3] = ["name", "path", "hash"];

/// A parsed, structurally valid generation recipe.
#[derive(Clone, Debug, PartialEq)]
pub struct GenerationDocument {
    /// Optional recipe name. `None` means "the file stem", which is what names the run folder.
    pub name: Option<String>,
    /// The workflow this recipe drives, a path relative to the recipe document's own directory.
    pub workflow: String,
    /// The base seed. Candidate `i` of a `count`-wide run uses `seed + i`, so a candidate's index
    /// and its seed are the same fact.
    pub seed: u64,
    /// How many candidates one run produces.
    pub count: u32,
    /// Named committed images the run uploads before submitting, `(name, relative path)`, sorted by
    /// name.
    pub images: Vec<(String, String)>,
    /// Leaf-input overrides: `(node title, [(input name, value)])`, both levels sorted by key.
    pub inputs: Vec<(String, Vec<(String, InputValue)>)>,
}

impl Default for GenerationDocument {
    fn default() -> Self {
        Self {
            name: None,
            workflow: String::new(),
            seed: 0,
            count: GENERATION_DEFAULT_COUNT,
            images: Vec::new(),
            inputs: Vec::new(),
        }
    }
}

/// One override value a recipe sets on a workflow node's input.
#[derive(Clone, Debug, PartialEq)]
pub enum InputValue {
    /// A plain value, passed verbatim into the node's `inputs`.
    Scalar(ScalarValue),
    /// `{image: <name>}` — a reference into the document's own `images` map, which resolves to the
    /// server-side name the upload returned.
    ImageRef(String),
}

/// A scalar override value. Integers keep their own variant because a node input that declares an
/// integer (a width, a step count) rejects the float spelling a shared `f64` would emit.
#[derive(Clone, Debug, PartialEq)]
pub enum ScalarValue {
    /// `true` / `false`.
    Bool(bool),
    /// An integer, emitted without a decimal point.
    Integer(i64),
    /// A non-integer number, emitted in ryu shortest-round-trip form.
    Number(f64),
    /// A string.
    Text(String),
}

/// The provenance of a generated asset: which recipe, seed and workflow produced it.
///
/// Carried by a [`crate::MeshDocument`]'s `origin:` key and by a `kind: origin` sidecar beside a
/// generated image. An asset carrying one is otherwise entirely ordinary — content, hand-editable
/// from the moment it is accepted.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Origin {
    /// The generator that produced the asset.
    pub tool: String,
    /// The generator's own version string, as it reported it.
    pub comfy_version: String,
    /// The recipe document, relative to the project root.
    pub recipe: String,
    /// A content hash of the recipe file as the run read it; absent in blocks accepted before it
    /// existed.
    pub recipe_hash: Option<String>,
    /// The run this candidate came from.
    pub run_id: String,
    /// The date of the run.
    pub date: String,
    /// The seed this candidate's job ran at: the run's base seed plus the job's position, or the
    /// recipe's pin. Absent when the run could not know it (a seed fed by a link, or pins that
    /// disagree).
    pub seed: Option<u64>,
    /// The workflow that was submitted, relative to the project root.
    pub workflow: String,
    /// A content hash of the workflow file as submitted, so a later edit to it is detectable.
    pub workflow_hash: String,
    /// Every model file the resolved graph loaded: any `*_name` string input ending in a model
    /// extension.
    pub checkpoints: Vec<String>,
    /// Every image the run uploaded, sorted by name — what an image-to-image or image-to-mesh asset
    /// was made *from*, which no other field records.
    pub images: Vec<OriginImage>,
    /// The text of the encoders whose output reaches a `positive` input, joined in node order — the
    /// one field a human reads a year later.
    pub prompt: String,
    /// The recipe's resolved overrides, snapshotted in the same shape
    /// [`GenerationDocument::inputs`] holds.
    pub inputs: Vec<(String, Vec<(String, InputValue)>)>,
}

/// One image a generated asset was conditioned on.
///
/// The name is the recipe's own (what an `image:` reference spells), the path is project-relative,
/// and the hash is of the bytes that were uploaded — so an edit to the source image afterwards is
/// detectable, exactly as [`Origin::workflow_hash`] makes an edited workflow detectable.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct OriginImage {
    /// The recipe's name for the image.
    pub name: String,
    /// The file, relative to the project root.
    pub path: String,
    /// A content hash of the uploaded bytes.
    pub hash: String,
}

// --- Parse --------------------------------------------------------------------------------------

/// Parses strict-subset YAML into a [`GenerationDocument`], checking structure (`L1161`); a value
/// whose *shape* is legal YAML but is not a value a recipe can spell (a map that is not the image
/// reference, a list) is the data code `L1162`, raised here because the type cannot survive to
/// [`validate`].
pub(crate) fn parse(source: &str) -> Result<GenerationDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                STRUCTURE,
                "the generation document is empty; a recipe needs at least `format: 1`, \
                 `kind: generation`, and `workflow`",
            )
            .with_path("")
            .with_help(
                "a minimal recipe is:\n  format: 1\n  kind: generation\n  workflow: \
                 ../workflows/txt2img.json",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                STRUCTURE,
                format!(
                    "the generation document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: generation`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = GenerationDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut workflow_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    STRUCTURE,
                    STRUCTURE,
                    GENERATION_FORMAT_VERSION,
                    "the generation document",
                    "recipe",
                    "generation",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    STRUCTURE,
                    "generation",
                    "a generation recipe",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the recipe name",
                    STRUCTURE,
                    &mut diagnostics,
                );
            }
            "workflow" => {
                workflow_seen = true;
                if let Some(path) = envelope::expect_string(
                    &entry.node,
                    "/workflow",
                    "the workflow path",
                    STRUCTURE,
                    &mut diagnostics,
                ) {
                    doc.workflow = path;
                }
            }
            "seed" => {
                if let Some(seed) = expect_seed(&entry.node, "/seed", &mut diagnostics) {
                    doc.seed = seed;
                }
            }
            "count" => {
                if let Some(count) = expect_u32(&entry.node, "/count", "count", &mut diagnostics) {
                    doc.count = count;
                }
            }
            "images" => {
                doc.images = parse_images(&entry.node, &mut diagnostics);
            }
            "inputs" => {
                doc.inputs = parse_inputs(&entry.node, "/inputs", &mut diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a generation recipe",
                GENERATION_KEYS,
                STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key(
            "format",
            "add `format: 1` as the first line",
            "generation document",
        ));
    }
    if !kind_seen {
        diagnostics.push(missing_key(
            "kind",
            "add `kind: generation`",
            "generation document",
        ));
    }
    if !workflow_seen {
        diagnostics.push(missing_key(
            "workflow",
            "add `workflow:` naming an API-format workflow, relative to this document's directory",
            "generation document",
        ));
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

fn missing_key(key: &str, help: &str, doc_noun: &str) -> Diagnostic {
    envelope::missing_key(key, help, STRUCTURE, doc_noun)
}

/// `seed`: a non-negative integer that fits in 64 bits. A non-integer is structure; an integer out
/// of that range is data, since only the value is wrong.
fn expect_seed(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<u64> {
    match &node.value {
        NodeValue::Scalar(Value::Number(n)) => match n.as_u64() {
            Some(seed) => Some(seed),
            None => {
                diagnostics.push(
                    Diagnostic::new(
                        DATA,
                        format!(
                            "`seed` is {n}, but a seed must be a non-negative integer that fits \
                             in 64 bits"
                        ),
                    )
                    .with_path(pointer)
                    .with_help("use a whole number from 0 upward; candidate `i` uses `seed + i`"),
                );
                None
            }
        },
        other => {
            diagnostics.push(
                Diagnostic::new(
                    STRUCTURE,
                    format!(
                        "`seed` must be a non-negative integer, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer)
                .with_help("write `seed: 7`; omit the key entirely for `seed: 0`"),
            );
            None
        }
    }
}

/// An integer key that fits in 32 bits (`count`). Range is this parse's business; the meaning of
/// the value (`count >= 1`) is [`validate`]'s.
fn expect_u32(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<u32> {
    let value = match &node.value {
        NodeValue::Scalar(Value::Number(n)) => n.as_u64().filter(|v| *v <= u64::from(u32::MAX)),
        _ => None,
    };
    match value {
        Some(value) => Some(value as u32),
        None => {
            diagnostics.push(
                Diagnostic::new(
                    STRUCTURE,
                    format!(
                        "`{what}` must be a non-negative integer that fits in 32 bits, found {} \
                         (line {})",
                        node.value.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer)
                .with_help(format!("write `{what}: 4`")),
            );
            None
        }
    }
}

/// Parses `images`: a mapping of name → relative path. Returned sorted by name, the order emit
/// writes.
fn parse_images(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Vec<(String, String)> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                STRUCTURE,
                format!(
                    "`images` must be a mapping of names to image paths, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path("/images")
            .with_help(
                "write `images` as one name per line:\n  images:\n    subject: \
                 ../renders/idol-front.png",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let pointer = format!("/images/{}", pointer_token(&entry.key));
        if let Some(path) = envelope::expect_string(
            &entry.node,
            &pointer,
            "an image path",
            STRUCTURE,
            diagnostics,
        ) {
            out.push((entry.key.clone(), path));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Parses an `inputs:` mapping — node title → input name → value — under `base`. Shared by the
/// recipe and by [`Origin`], whose snapshot has the same shape. Both levels come back sorted by
/// key.
fn parse_inputs(
    node: &Node,
    base: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<(String, Vec<(String, InputValue)>)> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                STRUCTURE,
                format!(
                    "`inputs` must be a mapping of node titles to their inputs, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(base)
            .with_help(
                "write `inputs` as one node title per line:\n  inputs:\n    Empty Latent:\n      \
                 width: 1024",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let title_pointer = format!("{base}/{}", pointer_token(&entry.key));
        let NodeValue::Mapping(fields) = &entry.node.value else {
            diagnostics.push(
                Diagnostic::new(
                    STRUCTURE,
                    format!(
                        "the inputs of node `{}` must be a mapping of input names to values, \
                         found {} (line {})",
                        entry.key,
                        entry.node.value.type_phrase(),
                        entry.node.line
                    ),
                )
                .with_path(&title_pointer)
                .with_help(
                    "a node title maps to its own inputs:\n    Empty Latent:\n      width: 1024",
                ),
            );
            continue;
        };
        let mut values = Vec::with_capacity(fields.len());
        for field in fields {
            let pointer = format!("{title_pointer}/{}", pointer_token(&field.key));
            if let Some(value) = parse_input_value(&field.node, &pointer, diagnostics) {
                values.push((field.key.clone(), value));
            }
        }
        values.sort_by(|a, b| a.0.cmp(&b.0));
        out.push((entry.key.clone(), values));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// One input value: a scalar, or the single structured form `{image: <name>}` written in block
/// style (the strict subset has no non-empty flow collections).
fn parse_input_value(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<InputValue> {
    match &node.value {
        NodeValue::Scalar(Value::String(s)) => {
            Some(InputValue::Scalar(ScalarValue::Text(s.clone())))
        }
        NodeValue::Scalar(Value::Bool(b)) => Some(InputValue::Scalar(ScalarValue::Bool(*b))),
        NodeValue::Scalar(Value::Number(n)) => Some(InputValue::Scalar(match n.as_i64() {
            Some(i) => ScalarValue::Integer(i),
            None => ScalarValue::Number(n.as_f64().unwrap_or(f64::NAN)),
        })),
        NodeValue::Mapping(entries) => {
            match entries.as_slice() {
                [entry] if entry.key == "image" => {
                    return envelope::expect_string(
                        &entry.node,
                        &format!("{pointer}/image"),
                        "an image reference",
                        STRUCTURE,
                        diagnostics,
                    )
                    .map(InputValue::ImageRef);
                }
                _ => {}
            }
            diagnostics.push(
                Diagnostic::new(
                    DATA,
                    format!(
                        "an input value may be a scalar or an `image:` reference, found another \
                         mapping (line {})",
                        node.line
                    ),
                )
                .with_path(pointer)
                .with_help(
                    "the one structured value is a reference into `images:`:\n      image:\n     \
                     \x20  image: subject\nwiring one node to another belongs in the workflow, not \
                     in a recipe",
                ),
            );
            None
        }
        NodeValue::Sequence(_) => {
            diagnostics.push(
                Diagnostic::new(
                    DATA,
                    format!(
                        "an input value may be a scalar or an `image:` reference, found a \
                         sequence (line {})",
                        node.line
                    ),
                )
                .with_path(pointer)
                .with_help("set one leaf input per key; a list is not a value a node input takes"),
            );
            None
        }
        NodeValue::Scalar(Value::Null) => {
            diagnostics.push(
                Diagnostic::new(
                    STRUCTURE,
                    format!("an input value must not be null (line {})", node.line),
                )
                .with_path(pointer)
                .with_help("give the input a string, number, boolean, or an `image:` reference"),
            );
            None
        }
        NodeValue::Scalar(_) => None,
    }
}

/// Parses a provenance block under `base` — a mesh document's `origin:` value.
///
/// `structure_code` comes from the caller (the [`crate::envelope`] pattern), so a mesh document
/// reports its own code while this traversal stays single-sourced.
pub(crate) fn parse_origin(
    node: &Node,
    base: &str,
    structure_code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Origin> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure_code,
                format!(
                    "`origin` must be a mapping, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(base)
            .with_help(format!(
                "an origin block spells: {}",
                ORIGIN_KEYS.join(", ")
            )),
        );
        return None;
    };
    let entries: Vec<&MapEntry> = entries.iter().collect();
    parse_origin_fields(
        &entries,
        base,
        structure_code,
        "an origin block",
        ORIGIN_KEYS,
        diagnostics,
    )
}

/// The provenance block's one field walk, over whichever entries the carrier hands it: a mesh
/// document's whole `origin:` mapping, or a sidecar's root minus its envelope keys.
///
/// `valid` is the key vocabulary an unknown key is matched against, which differs between the two
/// (only the sidecar's root may spell `format`/`kind`). Returns `None` when it reported anything,
/// so a partly-filled block never reaches a caller.
fn parse_origin_fields<const N: usize>(
    entries: &[&MapEntry],
    base: &str,
    structure_code: DiagnosticCode,
    context: &str,
    valid: [&'static str; N],
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Origin> {
    let before = diagnostics.len();
    let mut origin = Origin::default();
    let mut seen = [false; ORIGIN_KEYS.len()];
    for entry in entries {
        let pointer = format!("{base}/{}", pointer_token(&entry.key));
        let mut string_field = |what: &str| {
            envelope::expect_string(&entry.node, &pointer, what, structure_code, diagnostics)
                .unwrap_or_default()
        };
        match entry.key.as_str() {
            "tool" => {
                seen[0] = true;
                origin.tool = string_field("the generating tool");
            }
            "comfy_version" => {
                seen[1] = true;
                origin.comfy_version = string_field("the generator version");
            }
            "recipe" => {
                seen[2] = true;
                origin.recipe = string_field("the recipe path");
            }
            "recipe_hash" => {
                seen[3] = true;
                origin.recipe_hash = Some(string_field("the recipe hash"));
            }
            "run_id" => {
                seen[4] = true;
                origin.run_id = string_field("the run id");
            }
            "date" => {
                seen[5] = true;
                origin.date = string_field("the run date");
            }
            "seed" => {
                seen[6] = true;
                match &entry.node.value {
                    NodeValue::Scalar(Value::Number(n)) if n.as_u64().is_some() => {
                        origin.seed = Some(n.as_u64().expect("checked by the guard"));
                    }
                    other => diagnostics.push(
                        Diagnostic::new(
                            structure_code,
                            format!(
                                "`seed` must be a non-negative integer, found {} (line {})",
                                other.type_phrase(),
                                entry.node.line
                            ),
                        )
                        .with_path(&pointer)
                        .with_help("the seed is the one that produced this asset"),
                    ),
                }
            }
            "workflow" => {
                seen[7] = true;
                origin.workflow = string_field("the workflow path");
            }
            "workflow_hash" => {
                seen[8] = true;
                origin.workflow_hash = string_field("the workflow hash");
            }
            "checkpoints" => {
                seen[9] = true;
                origin.checkpoints =
                    parse_string_list(&entry.node, &pointer, structure_code, diagnostics);
            }
            "images" => {
                seen[10] = true;
                origin.images =
                    parse_origin_images(&entry.node, &pointer, structure_code, diagnostics);
            }
            "prompt" => {
                seen[11] = true;
                origin.prompt = string_field("the prompt");
            }
            "inputs" => {
                seen[12] = true;
                origin.inputs = parse_inputs(&entry.node, &pointer, diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                base,
                context,
                valid,
                structure_code,
            )),
        }
    }

    // `checkpoints`, `images` and `inputs` may be absent — an empty list says the same thing —
    // `recipe_hash` is absent from blocks accepted before it existed, and `seed` is absent when
    // the run could not know it. Every other key is a fact that makes provenance worth writing at
    // all, so each is required.
    for (index, key) in ORIGIN_KEYS.iter().enumerate() {
        if !seen[index]
            && !matches!(
                *key,
                "recipe_hash" | "seed" | "checkpoints" | "images" | "inputs"
            )
        {
            diagnostics.push(
                Diagnostic::new(
                    structure_code,
                    format!("the origin block is missing the required key `{key}`"),
                )
                .with_path(base)
                .with_help(format!(
                    "an origin block spells: {}",
                    ORIGIN_KEYS.join(", ")
                )),
            );
        }
    }

    (diagnostics.len() == before).then_some(origin)
}

fn parse_string_list(
    node: &Node,
    pointer: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<String> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "`checkpoints` must be a sequence of strings, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer)
            .with_help(
                "write one checkpoint per line:\n  checkpoints:\n    - sdxl_base.safetensors",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(text) = envelope::expect_string(
            item,
            &format!("{pointer}/{index}"),
            "a checkpoint name",
            code,
            diagnostics,
        ) {
            out.push(text);
        }
    }
    out
}

/// Parses an origin's `images:` — a sequence of entries, one per image the run uploaded. Returned
/// sorted by name, the order emit writes.
fn parse_origin_images(
    node: &Node,
    pointer: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<OriginImage> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "`images` must be a sequence of image entries, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer)
            .with_help(
                "write one entry per uploaded image:\n  images:\n    - name: subject\n      path: \
                 generation/references/idol.png\n      hash: \"0123456789abcdef\"",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(image) =
            parse_origin_image(item, &format!("{pointer}/{index}"), code, diagnostics)
        {
            out.push(image);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// One `images:` entry: a mapping with all three of `name`, `path` and `hash`.
fn parse_origin_image(
    node: &Node,
    pointer: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<OriginImage> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "an image entry must be a mapping with `name`, `path` and `hash`, found {} \
                     (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer)
            .with_help(
                "write the entry as keys under one `-`:\n    - name: subject\n      path: \
                 generation/references/idol.png\n      hash: \"0123456789abcdef\"",
            ),
        );
        return None;
    };
    let before = diagnostics.len();
    let mut image = OriginImage::default();
    let mut seen = [false; ORIGIN_IMAGE_KEYS.len()];
    for entry in entries {
        let field_pointer = format!("{pointer}/{}", pointer_token(&entry.key));
        let mut string_field = |what: &str| {
            envelope::expect_string(&entry.node, &field_pointer, what, code, diagnostics)
                .unwrap_or_default()
        };
        match entry.key.as_str() {
            "name" => {
                seen[0] = true;
                image.name = string_field("the image name");
            }
            "path" => {
                seen[1] = true;
                image.path = string_field("the image path");
            }
            "hash" => {
                seen[2] = true;
                image.hash = string_field("the image hash");
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                pointer,
                "an image entry",
                ORIGIN_IMAGE_KEYS,
                code,
            )),
        }
    }
    for (index, key) in ORIGIN_IMAGE_KEYS.iter().enumerate() {
        if !seen[index] {
            diagnostics.push(
                Diagnostic::new(
                    code,
                    format!("the image entry is missing the required key `{key}`"),
                )
                .with_path(pointer)
                .with_help(format!(
                    "an image entry spells: {}",
                    ORIGIN_IMAGE_KEYS.join(", ")
                )),
            );
        }
    }
    (diagnostics.len() == before).then_some(image)
}

/// Parses a `kind: origin` sidecar document — the envelope, then a provenance block at the root.
pub(crate) fn parse_origin_document(source: &str) -> Result<Origin, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                STRUCTURE,
                "the origin document is empty; a provenance sidecar needs at least `format: 1` \
                 and `kind: origin`",
            )
            .with_path("")
            .with_help("a sidecar starts with:\n  format: 1\n  kind: origin"),
        ]);
    };
    let NodeValue::Mapping(entries) = &root.value else {
        return Err(vec![
            Diagnostic::new(
                STRUCTURE,
                format!(
                    "the origin document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: origin`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut body = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    STRUCTURE,
                    STRUCTURE,
                    ORIGIN_FORMAT_VERSION,
                    "the origin document",
                    "sidecar",
                    "origin",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    STRUCTURE,
                    "origin",
                    "a provenance sidecar",
                    &mut diagnostics,
                );
            }
            _ => body.push(entry),
        }
    }
    if !format_seen {
        diagnostics.push(missing_key(
            "format",
            "add `format: 1` as the first line",
            "origin document",
        ));
    }
    if !kind_seen {
        diagnostics.push(missing_key("kind", "add `kind: origin`", "origin document"));
    }

    let origin = parse_origin_fields(
        &body,
        "",
        STRUCTURE,
        "an origin document",
        ORIGIN_DOCUMENT_KEYS,
        &mut diagnostics,
    );
    match origin {
        Some(origin) if diagnostics.is_empty() => Ok(origin),
        _ => Err(diagnostics),
    }
}

// --- Validate -----------------------------------------------------------------------------------

/// Validates a parsed recipe's data (`L1162`): `name` is a plain file stem, `workflow` and every
/// `images` path are checked for shape, never for existence — the caller resolves them against the
/// recipe's own directory.
pub(crate) fn validate(doc: &GenerationDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if let Some(name) = &doc.name
        && let Some(refusal) = ludion_core::paths::stem_refusal(name)
    {
        diagnostics.push(
            Diagnostic::new(DATA, format!("`name` {}", refusal.describe()))
                .with_path("/name")
                .with_help(
                    "write a plain file stem such as `idol-portrait`; it names the run folder \
                     under `generation/runs/`",
                ),
        );
    }

    if doc.workflow.trim().is_empty() {
        diagnostics.push(
            Diagnostic::new(
                DATA,
                "`workflow` is empty; a recipe names one workflow file",
            )
            .with_path("/workflow")
            .with_help(
                "give a path relative to this document's directory, e.g. \
                     `../workflows/txt2img.json`",
            ),
        );
    }
    if doc.count < 1 {
        diagnostics.push(
            Diagnostic::new(
                DATA,
                format!(
                    "`count` is {}, but a run produces at least 1 candidate",
                    doc.count
                ),
            )
            .with_path("/count")
            .with_help("set `count: 1` or higher, or omit the key entirely"),
        );
    } else if doc.count > GENERATION_MAX_COUNT {
        diagnostics.push(
            Diagnostic::new(
                DATA,
                format!(
                    "`count` is {}, but a run produces at most {GENERATION_MAX_COUNT} candidates",
                    doc.count
                ),
            )
            .with_path("/count")
            .with_help("lower `count`, or run the recipe several times with different seeds"),
        );
    }
    for (name, path) in &doc.images {
        let pointer = format!("/images/{}", pointer_token(name));
        if name.trim().is_empty() {
            diagnostics.push(
                Diagnostic::new(DATA, "an `images` entry has an empty name")
                    .with_path(&pointer)
                    .with_help("name each image so an input can reference it: `subject: ...`"),
            );
        }
        if path.trim().is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    DATA,
                    format!("the `images` entry `{name}` has an empty path"),
                )
                .with_path(&pointer)
                .with_help("give a path relative to this document's directory"),
            );
        }
    }
    validate_inputs(
        &doc.inputs,
        "/inputs",
        Some(&doc.images),
        DATA,
        &mut diagnostics,
    );

    diagnostics
}

/// Validates a provenance block's data at `base`: its snapshot's numbers must be finite, and every
/// `images` entry must be complete. An origin's image *references* name server-side uploads, not a
/// declared `images` map, so they are not resolved here. `code` comes from the caller, as
/// [`parse_origin`]'s does.
pub(crate) fn validate_origin_at(
    origin: &Origin,
    base: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (index, image) in origin.images.iter().enumerate() {
        let empty = ORIGIN_IMAGE_KEYS
            .into_iter()
            .zip([&image.name, &image.path, &image.hash])
            .find(|(_, value)| value.trim().is_empty());
        if let Some((key, _)) = empty {
            diagnostics.push(
                Diagnostic::new(
                    code,
                    format!("the `images` entry at index {index} has an empty `{key}`"),
                )
                .with_path(format!("{base}/images/{index}"))
                .with_help(
                    "an image entry records the recipe's name for the image, the file it uploaded, \
                     and a content hash of those bytes",
                ),
            );
        }
    }
    validate_inputs(
        &origin.inputs,
        &format!("{base}/inputs"),
        None,
        code,
        diagnostics,
    );
}

/// Every override value under `base` is finite, and — when `images` is `Some` — every image
/// reference names a declared entry.
fn validate_inputs(
    inputs: &[(String, Vec<(String, InputValue)>)],
    base: &str,
    images: Option<&[(String, String)]>,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (title, values) in inputs {
        for (name, value) in values {
            let pointer = format!("{base}/{}/{}", pointer_token(title), pointer_token(name));
            match value {
                InputValue::Scalar(ScalarValue::Number(n)) if !n.is_finite() => diagnostics.push(
                    Diagnostic::new(
                        code,
                        format!("`{title}` input `{name}` is {n}, which is not a finite number"),
                    )
                    .with_path(&pointer)
                    .with_help("a node input takes a finite number; check for a bad computation"),
                ),
                InputValue::ImageRef(reference) => {
                    let Some(images) = images else { continue };
                    if images.iter().any(|(declared, _)| declared == reference) {
                        continue;
                    }
                    let names: Vec<&str> = images.iter().map(|(n, _)| n.as_str()).collect();
                    let mut diagnostic = Diagnostic::new(
                        code,
                        format!(
                            "`{title}` input `{name}` references the image `{reference}`, which \
                             `images:` does not declare"
                        ),
                    )
                    .with_path(&pointer);
                    diagnostic = match suggest::nearest(reference, names.iter().copied()) {
                        Some(candidate) => {
                            diagnostic.with_help(format!("nearest match is `{candidate}`"))
                        }
                        None if names.is_empty() => diagnostic
                            .with_help("this recipe declares no `images:`; add the entry it needs"),
                        None => diagnostic
                            .with_help(format!("declared images are: {}", names.join(", "))),
                    };
                    diagnostics.push(diagnostic);
                }
                _ => {}
            }
        }
    }
}

// --- Emit ---------------------------------------------------------------------------------------

/// Writes `doc` in canonical form: `format`, `kind`, `name`, `workflow`, `seed`, `count`, `images`,
/// `inputs`, block style, maps sorted by key, ryu floats. `name` is written only when authored,
/// `count` only when it is not the default, and `images`/`inputs` only when non-empty.
pub(crate) fn emit(doc: &GenerationDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: generation\n");
    if let Some(name) = &doc.name {
        out.push_str(&format!("name: {}\n", scalar_string(name)));
    }
    out.push_str(&format!("workflow: {}\n", scalar_string(&doc.workflow)));
    out.push_str(&format!("seed: {}\n", doc.seed));
    if doc.count != GENERATION_DEFAULT_COUNT {
        out.push_str(&format!("count: {}\n", doc.count));
    }
    if !doc.images.is_empty() {
        let mut images = doc.images.clone();
        images.sort_by(|a, b| a.0.cmp(&b.0));
        out.push_str("images:\n");
        for (name, path) in &images {
            out.push_str(&format!(
                "  {}: {}\n",
                scalar_string(name),
                scalar_string(path)
            ));
        }
    }
    if !doc.inputs.is_empty() {
        emit_inputs(&mut out, "", &doc.inputs);
    }
    out
}

/// Writes an `inputs:` block at `indent` — the recipe's and [`Origin`]'s one writer.
fn emit_inputs(out: &mut String, indent: &str, inputs: &[(String, Vec<(String, InputValue)>)]) {
    let mut inputs = inputs.to_vec();
    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    out.push_str(&format!("{indent}inputs:\n"));
    for (title, values) in &inputs {
        out.push_str(&format!("{indent}  {}:", scalar_string(title)));
        if values.is_empty() {
            out.push_str(" {}\n");
            continue;
        }
        out.push('\n');
        let mut values = values.clone();
        values.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, value) in &values {
            let key = scalar_string(name);
            match value {
                InputValue::Scalar(scalar) => {
                    out.push_str(&format!("{indent}    {key}: {}\n", emit_scalar(scalar)));
                }
                InputValue::ImageRef(reference) => {
                    out.push_str(&format!("{indent}    {key}:\n"));
                    out.push_str(&format!(
                        "{indent}      image: {}\n",
                        scalar_string(reference)
                    ));
                }
            }
        }
    }
}

fn emit_scalar(value: &ScalarValue) -> String {
    match value {
        ScalarValue::Bool(b) => b.to_string(),
        ScalarValue::Integer(i) => i.to_string(),
        ScalarValue::Number(n) => format_float(*n),
        ScalarValue::Text(text) => scalar_string(text),
    }
}

/// Writes a provenance block's keys at `indent`, in [`ORIGIN_KEYS`] order. `checkpoints`, `images`
/// and `inputs` are written only when non-empty and `recipe_hash` only when present; every other key
/// always is.
pub(crate) fn emit_origin(out: &mut String, indent: &str, origin: &Origin) {
    out.push_str(&format!("{indent}tool: {}\n", scalar_string(&origin.tool)));
    out.push_str(&format!(
        "{indent}comfy_version: {}\n",
        scalar_string(&origin.comfy_version)
    ));
    out.push_str(&format!(
        "{indent}recipe: {}\n",
        scalar_string(&origin.recipe)
    ));
    if let Some(hash) = &origin.recipe_hash {
        out.push_str(&format!("{indent}recipe_hash: {}\n", scalar_string(hash)));
    }
    out.push_str(&format!(
        "{indent}run_id: {}\n",
        scalar_string(&origin.run_id)
    ));
    out.push_str(&format!("{indent}date: {}\n", scalar_string(&origin.date)));
    if let Some(seed) = origin.seed {
        out.push_str(&format!("{indent}seed: {seed}\n"));
    }
    out.push_str(&format!(
        "{indent}workflow: {}\n",
        scalar_string(&origin.workflow)
    ));
    out.push_str(&format!(
        "{indent}workflow_hash: {}\n",
        scalar_string(&origin.workflow_hash)
    ));
    if !origin.checkpoints.is_empty() {
        out.push_str(&format!("{indent}checkpoints:\n"));
        for checkpoint in &origin.checkpoints {
            out.push_str(&format!("{indent}  - {}\n", scalar_string(checkpoint)));
        }
    }
    if !origin.images.is_empty() {
        let mut images = origin.images.clone();
        images.sort_by(|a, b| a.name.cmp(&b.name));
        out.push_str(&format!("{indent}images:\n"));
        for image in &images {
            out.push_str(&format!(
                "{indent}  - name: {}\n",
                scalar_string(&image.name)
            ));
            out.push_str(&format!(
                "{indent}    path: {}\n",
                scalar_string(&image.path)
            ));
            out.push_str(&format!(
                "{indent}    hash: {}\n",
                scalar_string(&image.hash)
            ));
        }
    }
    out.push_str(&format!(
        "{indent}prompt: {}\n",
        scalar_string(&origin.prompt)
    ));
    if !origin.inputs.is_empty() {
        emit_inputs(out, indent, &origin.inputs);
    }
}

/// Writes a `kind: origin` sidecar document: the envelope, then the provenance block at the root.
pub(crate) fn emit_origin_document(origin: &Origin) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: origin\n");
    emit_origin(&mut out, "", origin);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECIPE: &str = "\
format: 1
kind: generation
name: idol-portrait
workflow: \"../workflows/txt2img.json\"
seed: 7
count: 4
images:
  subject: \"../renders/idol-front.png\"
inputs:
  Empty Latent:
    height: 1024
    width: 1024
  Load Subject:
    image:
      image: subject
  Positive Prompt:
    text: a weathered stone idol
";

    const ORIGIN_DOC: &str = "\
format: 1
kind: origin
tool: comfyui
comfy_version: \"0.3.61\"
recipe: generation/recipes/idol-portrait.gen.yaml
run_id: idol-portrait-0007
date: \"2026-09-17\"
seed: 9
workflow: generation/workflows/txt2img.json
workflow_hash: \"9f2c\"
checkpoints:
  - sdxl_base.safetensors
prompt: a weathered stone idol
inputs:
  Empty Latent:
    height: 1024
    width: 1024
";

    #[test]
    fn parses_a_full_recipe() {
        let doc = parse(RECIPE).expect("the recipe should parse");
        assert_eq!(doc.name.as_deref(), Some("idol-portrait"));
        assert_eq!(doc.workflow, "../workflows/txt2img.json");
        assert_eq!(doc.seed, 7);
        assert_eq!(doc.count, 4);
        assert_eq!(
            doc.images,
            vec![(
                "subject".to_string(),
                "../renders/idol-front.png".to_string()
            )]
        );
        assert_eq!(doc.inputs[1].1[0].1, InputValue::ImageRef("subject".into()));
        assert!(validate(&doc).is_empty());
    }

    #[test]
    fn a_name_that_is_not_a_plain_stem_is_l1162_at_name() {
        for bad in ["..", ".", "a/b", r"a\b", "C:x", "con", "nul.png"] {
            let mut doc = parse(RECIPE).expect("the recipe should parse");
            doc.name = Some(bad.to_string());
            let diagnostics = validate(&doc);
            assert_eq!(diagnostics.len(), 1, "{bad:?}: {diagnostics:?}");
            assert_eq!(diagnostics[0].code, DATA, "{bad:?}");
            assert_eq!(diagnostics[0].path.as_deref(), Some("/name"), "{bad:?}");
        }
        for good in ["idol-portrait", "v1.2"] {
            let mut doc = parse(RECIPE).expect("the recipe should parse");
            doc.name = Some(good.to_string());
            assert!(validate(&doc).is_empty(), "{good:?}");
        }
        let source = RECIPE.replace("name: idol-portrait", "name: \"..\"");
        let diagnostics = crate::check_generation(&source).expect_err("`..` is not a stem");
        assert_eq!(diagnostics[0].code, DATA);
        assert_eq!(diagnostics[0].path.as_deref(), Some("/name"));
    }

    #[test]
    fn emit_is_a_fixed_point() {
        let doc = parse(RECIPE).expect("the recipe should parse");
        assert_eq!(emit(&doc), RECIPE);
    }

    #[test]
    fn emit_is_byte_idempotent_on_a_non_canonical_document() {
        // Unsorted keys, a defaulted `count` spelled out loud, and an integer input.
        let source = "\
format: 1
kind: generation
workflow: w.json
seed: 0
count: 1
inputs:
  Zed:
    width: 8
  Alpha:
    on: true
";
        let once = emit(&parse(source).expect("parses"));
        let twice = emit(&parse(&once).expect("re-parses"));
        assert_eq!(once, twice);
        assert!(!once.contains("count:"), "the default count is not written");
        assert!(
            once.find("Alpha").unwrap() < once.find("Zed").unwrap(),
            "titles are sorted on emit"
        );
    }

    #[test]
    fn an_integer_input_keeps_its_integer_spelling() {
        let doc = parse(
            "format: 1\nkind: generation\nworkflow: w.json\ninputs:\n  N:\n    width: 1024\n",
        )
        .expect("parses");
        assert!(emit(&doc).contains("width: 1024\n"));
    }

    #[test]
    fn missing_workflow_is_l1161() {
        let diagnostics = parse("format: 1\nkind: generation\n").unwrap_err();
        assert!(diagnostics.iter().any(|d| d.code == STRUCTURE
            && d.message.contains("workflow")
            && d.path.as_deref() == Some("")));
    }

    #[test]
    fn bad_kind_is_l1161() {
        let diagnostics = parse("format: 1\nkind: mesh\nworkflow: w.json\n").unwrap_err();
        assert_eq!(diagnostics[0].code, STRUCTURE);
        assert_eq!(diagnostics[0].path.as_deref(), Some("/kind"));
    }

    #[test]
    fn unknown_key_suggests_nearest() {
        let diagnostics =
            parse("format: 1\nkind: generation\nworkflow: w.json\nworkflows: 2\n").unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/workflows"))
            .expect("unknown key diagnostic");
        assert_eq!(d.code, STRUCTURE);
        assert_eq!(d.help.as_deref(), Some("nearest match is `workflow`"));
    }

    #[test]
    fn a_non_integer_seed_is_l1161() {
        let diagnostics =
            parse("format: 1\nkind: generation\nworkflow: w.json\nseed: hello\n").unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/seed"))
            .expect("seed diagnostic");
        assert_eq!(d.code, STRUCTURE);
    }

    #[test]
    fn a_negative_seed_is_l1162() {
        let diagnostics =
            parse("format: 1\nkind: generation\nworkflow: w.json\nseed: -3\n").unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/seed"))
            .expect("seed diagnostic");
        assert_eq!(d.code, DATA);
    }

    #[test]
    fn a_zero_count_is_l1162() {
        let doc = parse("format: 1\nkind: generation\nworkflow: w.json\ncount: 0\n")
            .expect("count 0 parses; only its meaning is wrong");
        let diagnostics = validate(&doc);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DATA && d.path.as_deref() == Some("/count"))
        );
    }

    #[test]
    fn a_count_past_the_ceiling_is_l1162() {
        let source = |count: u32| {
            format!(
                "format: 1
kind: generation
workflow: w.json
count: {count}
"
            )
        };
        let at_ceiling = parse(&source(GENERATION_MAX_COUNT)).expect("parses");
        assert!(validate(&at_ceiling).is_empty());
        let past = parse(&source(GENERATION_MAX_COUNT + 1)).expect("parses");
        assert!(
            validate(&past)
                .iter()
                .any(|d| d.code == DATA && d.path.as_deref() == Some("/count"))
        );
    }

    #[test]
    fn an_empty_workflow_path_is_l1162() {
        let doc = parse("format: 1\nkind: generation\nworkflow: \"\"\n").expect("parses");
        assert!(
            validate(&doc)
                .iter()
                .any(|d| d.code == DATA && d.path.as_deref() == Some("/workflow"))
        );
    }

    #[test]
    fn a_non_finite_input_number_is_l1162() {
        let doc = GenerationDocument {
            workflow: "w.json".into(),
            inputs: vec![(
                "Sampler".into(),
                vec![(
                    "cfg".into(),
                    InputValue::Scalar(ScalarValue::Number(f64::NAN)),
                )],
            )],
            ..Default::default()
        };
        assert!(
            validate(&doc)
                .iter()
                .any(|d| d.code == DATA && d.path.as_deref() == Some("/inputs/Sampler/cfg"))
        );
    }

    #[test]
    fn an_undeclared_image_reference_is_l1162() {
        let source = "\
format: 1
kind: generation
workflow: w.json
images:
  subject: a.png
inputs:
  Load:
    image:
      image: subjekt
";
        let doc = parse(source).expect("parses");
        let d = validate(&doc)
            .into_iter()
            .find(|d| d.path.as_deref() == Some("/inputs/Load/image"))
            .expect("image reference diagnostic");
        assert_eq!(d.code, DATA);
        assert_eq!(d.help.as_deref(), Some("nearest match is `subject`"));
    }

    #[test]
    fn another_map_under_inputs_is_l1162() {
        let source = "format: 1\nkind: generation\nworkflow: w.json\ninputs:\n  N:\n    latent:\n      node: \"4\"\n";
        let diagnostics = parse(source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/inputs/N/latent"))
            .expect("structured value diagnostic");
        assert_eq!(d.code, DATA);
    }

    #[test]
    fn a_list_under_inputs_is_l1162() {
        let source =
            "format: 1\nkind: generation\nworkflow: w.json\ninputs:\n  N:\n    sizes:\n      - 1\n";
        let diagnostics = parse(source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/inputs/N/sizes"))
            .expect("sequence diagnostic");
        assert_eq!(d.code, DATA);
    }

    #[test]
    fn a_non_mapping_images_is_l1161() {
        let diagnostics =
            parse("format: 1\nkind: generation\nworkflow: w.json\nimages: []\n").unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/images"))
            .expect("images diagnostic");
        assert_eq!(d.code, STRUCTURE);
    }

    #[test]
    fn origin_document_round_trips() {
        let origin = parse_origin_document(ORIGIN_DOC).expect("the sidecar should parse");
        assert_eq!(origin.tool, "comfyui");
        assert_eq!(origin.seed, Some(9));
        assert_eq!(
            origin.checkpoints,
            vec!["sdxl_base.safetensors".to_string()]
        );
        assert_eq!(emit_origin_document(&origin), ORIGIN_DOC);
    }

    const ORIGIN_DOC_WITH_IMAGES: &str = "\
format: 1
kind: origin
tool: comfyui
comfy_version: \"0.3.61\"
recipe: generation/recipes/idol-mesh.gen.yaml
run_id: idol-mesh-0007
date: \"2026-09-17\"
seed: 400
workflow: generation/workflows/img2mesh.json
workflow_hash: \"9f2c\"
checkpoints:
  - trellis.safetensors
images:
  - name: subject
    path: generation/references/idol-reference.png
    hash: \"0123456789abcdef\"
prompt: \"\"
";

    #[test]
    fn an_origin_with_images_round_trips_with_the_key_after_checkpoints() {
        let origin = parse_origin_document(ORIGIN_DOC_WITH_IMAGES).expect("the sidecar parses");
        assert_eq!(
            origin.images,
            vec![OriginImage {
                name: "subject".to_string(),
                path: "generation/references/idol-reference.png".to_string(),
                hash: "0123456789abcdef".to_string(),
            }]
        );
        assert!(validate_origin_at_ok(&origin));
        let emitted = emit_origin_document(&origin);
        assert_eq!(emitted, ORIGIN_DOC_WITH_IMAGES);
        assert_eq!(
            emit_origin_document(&parse_origin_document(&emitted).expect("re-parses")),
            emitted
        );
        let checkpoints_at = emitted
            .find("checkpoints:")
            .expect("checkpoints is written");
        let images_at = emitted.find("images:").expect("images is written");
        let prompt_at = emitted.find("prompt:").expect("prompt is written");
        assert!(
            checkpoints_at < images_at && images_at < prompt_at,
            "{emitted}"
        );
    }

    #[test]
    fn a_recipe_hash_round_trips_right_after_the_recipe_and_is_optional() {
        let with_hash = ORIGIN_DOC.replacen(
            "recipe: generation/recipes/idol-portrait.gen.yaml\n",
            "recipe: generation/recipes/idol-portrait.gen.yaml\nrecipe_hash: \"0000000012345678\"\n",
            1,
        );
        assert_ne!(with_hash, ORIGIN_DOC, "the fixture names its recipe");
        let origin = parse_origin_document(&with_hash).expect("parses");
        assert_eq!(origin.recipe_hash.as_deref(), Some("0000000012345678"));
        assert_eq!(emit_origin_document(&origin), with_hash);

        let without = parse_origin_document(ORIGIN_DOC).expect("parses");
        assert_eq!(without.recipe_hash, None);
        assert_eq!(emit_origin_document(&without), ORIGIN_DOC);
    }

    #[test]
    fn a_seed_is_optional_and_null_is_refused() {
        let without_seed: String = ORIGIN_DOC
            .lines()
            .filter(|line| !line.starts_with("seed:"))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_ne!(without_seed, ORIGIN_DOC, "the fixture names a seed");
        let origin = parse_origin_document(&without_seed).expect("parses");
        assert_eq!(origin.seed, None);
        assert_eq!(emit_origin_document(&origin), without_seed);

        let null_seed = ORIGIN_DOC.replacen("seed: 9", "seed: null", 1);
        assert!(parse_origin_document(&null_seed).is_err());
    }

    /// An origin carrying no image is byte-identical to what it was before the key existed — the
    /// whole reason `images` is written only when non-empty.
    #[test]
    fn an_origin_without_images_writes_no_images_key() {
        let origin = parse_origin_document(ORIGIN_DOC).expect("parses");
        assert!(origin.images.is_empty());
        let emitted = emit_origin_document(&origin);
        assert!(!emitted.contains("images"), "{emitted}");
        assert_eq!(emitted, ORIGIN_DOC);
    }

    #[test]
    fn a_non_sequence_images_in_an_origin_is_l1161() {
        let source = ORIGIN_DOC.replace("checkpoints:\n  - sdxl_base.safetensors\n", "images: 4\n");
        let diagnostics = parse_origin_document(&source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/images"))
            .expect("images diagnostic");
        assert_eq!(d.code, STRUCTURE);
    }

    #[test]
    fn an_image_entry_missing_its_hash_is_l1161() {
        let source = ORIGIN_DOC_WITH_IMAGES.replace("    hash: \"0123456789abcdef\"\n", "");
        let diagnostics = parse_origin_document(&source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/images/0"))
            .expect("image entry diagnostic");
        assert_eq!(d.code, STRUCTURE);
        assert!(d.message.contains("`hash`"), "{}", d.message);
    }

    #[test]
    fn an_unknown_image_entry_key_suggests_the_nearest_match() {
        let source = ORIGIN_DOC_WITH_IMAGES.replace("    path: ", "    paths: ");
        let diagnostics = parse_origin_document(&source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/images/0/paths"))
            .expect("unknown key diagnostic");
        assert_eq!(d.help.as_deref(), Some("nearest match is `path`"));
    }

    #[test]
    fn an_image_entry_with_an_empty_path_is_l1162() {
        let mut origin = parse_origin_document(ORIGIN_DOC_WITH_IMAGES).expect("parses");
        origin.images[0].path = String::new();
        let mut diagnostics = Vec::new();
        validate_origin_at(&origin, "", DATA, &mut diagnostics);
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/images/0"))
            .expect("image data diagnostic");
        assert_eq!(d.code, DATA);
        assert!(d.message.contains("`path`"), "{}", d.message);
    }

    fn validate_origin_at_ok(origin: &Origin) -> bool {
        let mut diagnostics = Vec::new();
        validate_origin_at(origin, "", DATA, &mut diagnostics);
        diagnostics.is_empty()
    }

    #[test]
    fn an_origin_missing_a_required_key_is_l1161() {
        let source = "format: 1\nkind: origin\ntool: comfyui\n";
        let diagnostics = parse_origin_document(source).unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == STRUCTURE && d.message.contains("`recipe`"))
        );
    }

    #[test]
    fn an_origin_with_an_empty_checkpoint_list_omits_the_key() {
        let mut origin = parse_origin_document(ORIGIN_DOC).expect("parses");
        origin.checkpoints.clear();
        origin.inputs.clear();
        let text = emit_origin_document(&origin);
        assert!(!text.contains("checkpoints"));
        assert!(!text.contains("images"));
        assert!(!text.contains("inputs"));
        assert_eq!(
            emit_origin_document(&parse_origin_document(&text).expect("re-parses")),
            text
        );
    }
}
