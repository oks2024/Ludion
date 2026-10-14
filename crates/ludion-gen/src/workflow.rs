//! API-format ComfyUI graphs: parsing, the title index, override application, and the
//! `/object_info` validation passes.
//!
//! The API format is the flat one `POST /prompt` accepts — `{"<id>": {"class_type", "inputs",
//! "_meta": {"title"}}}` — and is the only format a workflow file may be committed in. The
//! browser's default save is the UI/litegraph format (a `nodes` array and a `links` array), which
//! is refused with [`code::WORKFLOW_NOT_API`] naming the **File → Export (API)** menu item that
//! writes the other one.
//!
//! A recipe never rewires a graph: [`apply`] writes leaf values into nodes picked by `_meta.title`,
//! because a title survives a re-export from the UI while a numeric node id need not.

use std::collections::{BTreeMap, BTreeSet};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest::nearest;
use ludion_scene::{GenerationDocument, InputValue, ScalarValue};
use serde_json::{Map, Value};

/// The two input names ComfyUI samplers spell their noise seed with. A recipe's base seed is
/// written into every one the recipe does not set itself.
const SEED_INPUTS: [&str; 2] = ["seed", "noise_seed"];

/// What a graph's output nodes write. A graph produces exactly one kind; none, or both, is
/// [`code::GENERATION_OUTPUT_KIND`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputKind {
    /// `SaveImage` and its family, including the animated encoders.
    Image,
    /// `SaveGLB` / `Save3D` and their family.
    Mesh,
}

impl OutputKind {
    /// The spelling `run.json` and the CLI's `--json` objects use.
    pub fn as_str(self) -> &'static str {
        match self {
            OutputKind::Image => "image",
            OutputKind::Mesh => "mesh",
        }
    }
}

/// One node of an API-format graph.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    /// The node's id — the key it sits under in the graph object.
    pub id: String,
    /// The node's class, the key into `/object_info`.
    pub class_type: String,
    /// The node's `_meta.title`, which is what a recipe's overrides key on.
    pub title: Option<String>,
    /// Leaf values and `[node_id, slot]` links, by input name.
    pub inputs: Map<String, Value>,
}

impl GraphNode {
    /// How a diagnostic names this node: its title when it has one, its id otherwise, always with
    /// its class, so a reader can find it in either the recipe or the graph file.
    pub fn label(&self) -> String {
        match &self.title {
            Some(title) => format!("`{title}` ({})", self.class_type),
            None => format!("node {} ({})", self.id, self.class_type),
        }
    }
}

/// A parsed API-format graph, its nodes in a stable id order.
///
/// A graph produced by [`apply`] also remembers which seed inputs the recipe set itself, so
/// [`with_seed`] can rewrite the rest for a `--count` variant without overwriting an authored one.
#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    nodes: Vec<GraphNode>,
    pinned_seeds: BTreeSet<(String, String)>,
}

impl Graph {
    /// Parses API-format JSON.
    ///
    /// `source_label` names the file in the message — the diagnostic carries no JSON pointer here,
    /// since the fault is in the workflow file rather than in the recipe document.
    ///
    /// # Errors
    /// [`code::WORKFLOW_NOT_API`] when the text is not JSON, is not an object of nodes, or is a
    /// UI/litegraph export.
    pub fn parse(source: &str, source_label: &str) -> Result<Graph, Diagnostic> {
        let value: Value = serde_json::from_str(source).map_err(|e| {
            not_api(format!(
                "`{source_label}` is not valid JSON: {e}; a workflow file is the API-format graph \
                 ComfyUI's File -> Export (API) writes"
            ))
        })?;

        let Value::Object(object) = value else {
            return Err(not_api(format!(
                "`{source_label}` is a JSON {}, but an API-format graph is an object keyed by node \
                 id",
                ludion_core::diagnostics::value_phrase(&value)
            )));
        };

        // The UI/litegraph save is the likeliest first-run mistake: it is an object too, but its
        // nodes live in an array under `nodes` rather than being the object's own keys.
        if object.contains_key("nodes") || object.contains_key("links") {
            return Err(not_api(format!(
                "`{source_label}` is a UI (litegraph) workflow, not the API format: it has a \
                 top-level `nodes`/`links` array instead of one entry per node id"
            )));
        }

        if object.is_empty() {
            return Err(not_api(format!("`{source_label}` holds no nodes")));
        }

        let mut nodes = Vec::with_capacity(object.len());
        for (id, node) in object {
            let Some(node) = node.as_object() else {
                return Err(not_api(format!(
                    "`{source_label}`: node `{id}` is a {}, but every API-format node is an object \
                     with `class_type` and `inputs`",
                    ludion_core::diagnostics::value_phrase(&node)
                )));
            };
            let Some(class_type) = node.get("class_type").and_then(Value::as_str) else {
                return Err(not_api(format!(
                    "`{source_label}`: node `{id}` has no `class_type`, so this is not an \
                     API-format graph"
                )));
            };
            let inputs = match node.get("inputs") {
                None => Map::new(),
                Some(Value::Object(inputs)) => inputs.clone(),
                Some(other) => {
                    return Err(not_api(format!(
                        "`{source_label}`: node `{id}`'s `inputs` is a {}, but it must be an object \
                         of input name -> value",
                        ludion_core::diagnostics::value_phrase(other)
                    )));
                }
            };
            let title = node
                .get("_meta")
                .and_then(Value::as_object)
                .and_then(|meta| meta.get("title"))
                .and_then(Value::as_str)
                .map(str::to_string);
            nodes.push(GraphNode {
                id: id.clone(),
                class_type: class_type.to_string(),
                title,
                inputs,
            });
        }
        nodes.sort_by(|a, b| id_key(&a.id).cmp(&id_key(&b.id)));

        Ok(Graph {
            nodes,
            pinned_seeds: BTreeSet::new(),
        })
    }

    /// Every node, in node-id order (numeric ids first, ascending; then the rest lexicographically).
    pub fn nodes(&self) -> &[GraphNode] {
        &self.nodes
    }

    /// The node with this id, if the graph has one.
    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// Every `_meta.title` in the graph, sorted and deduplicated — what an unresolved override
    /// lists back at the author.
    pub fn titles(&self) -> Vec<&str> {
        let mut titles: Vec<&str> = self
            .nodes
            .iter()
            .filter_map(|node| node.title.as_deref())
            .collect();
        titles.sort_unstable();
        titles.dedup();
        titles
    }

    /// The one node carrying this `_meta.title`.
    ///
    /// # Errors
    /// [`code::WORKFLOW_TITLE_UNRESOLVED`] when no node or more than one node carries it; the
    /// message lists every title in the graph.
    pub fn find_title(&self, title: &str) -> Result<&GraphNode, Diagnostic> {
        let matches: Vec<&GraphNode> = self
            .nodes
            .iter()
            .filter(|node| node.title.as_deref() == Some(title))
            .collect();
        match matches.as_slice() {
            [one] => Ok(one),
            [] => {
                let mut help = String::new();
                if let Some(near) = nearest(title, self.titles()) {
                    help.push_str(&format!("nearest title is `{near}`; "));
                }
                help.push_str(
                    "overrides key on a node's `_meta.title` — rename the node in ComfyUI \
                     (right-click -> Title) and re-export in API format, or change the recipe to a \
                     title the graph already has",
                );
                Err(Diagnostic::new(
                    code::WORKFLOW_TITLE_UNRESOLVED,
                    format!(
                        "no node in the workflow is titled `{title}`; the graph's titles are {}",
                        title_list(&self.titles())
                    ),
                )
                .with_help(help))
            }
            several => Err(Diagnostic::new(
                code::WORKFLOW_TITLE_UNRESOLVED,
                format!(
                    "{} nodes in the workflow are titled `{title}` (ids {}), so an override cannot \
                     say which one it means",
                    several.len(),
                    several
                        .iter()
                        .map(|node| node.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .with_help(
                "give each node a distinct title in ComfyUI (right-click -> Title) and re-export \
                 the workflow in API format",
            )),
        }
    }

    /// What this graph's output nodes write.
    ///
    /// # Errors
    /// [`code::GENERATION_OUTPUT_KIND`] when the graph has no output node, or writes images and a
    /// mesh in one run.
    pub fn output_kind(&self) -> Result<OutputKind, Diagnostic> {
        let mut images: Vec<&GraphNode> = Vec::new();
        let mut meshes: Vec<&GraphNode> = Vec::new();
        for node in &self.nodes {
            match classify_output(&node.class_type) {
                Some(OutputKind::Image) => images.push(node),
                Some(OutputKind::Mesh) => meshes.push(node),
                None => {}
            }
        }
        match (images.is_empty(), meshes.is_empty()) {
            (false, true) => Ok(OutputKind::Image),
            (true, false) => Ok(OutputKind::Mesh),
            (true, true) => Err(Diagnostic::new(
                code::GENERATION_OUTPUT_KIND,
                "the workflow has no output node, so a run would produce no file",
            )
            .with_help(
                "add a `SaveImage` (images) or `SaveGLB` (mesh) node to the graph and re-export it \
                 in API format; a `PreviewImage` writes only a temporary file a run never downloads",
            )),
            (false, false) => Err(Diagnostic::new(
                code::GENERATION_OUTPUT_KIND,
                format!(
                    "the workflow writes images ({}) and a mesh ({}) in one run, and a recipe \
                     produces exactly one kind",
                    node_list(&images),
                    node_list(&meshes)
                ),
            )
            .with_help(
                "split the graph into an image workflow and a mesh workflow, each with its own \
                 recipe — the image loop is the cheap one to iterate",
            )),
        }
    }

    /// The graph as the JSON `POST /prompt` takes, nodes in id order.
    pub fn to_value(&self) -> Value {
        let mut object = Map::new();
        for node in &self.nodes {
            let mut entry = Map::new();
            entry.insert(
                "class_type".to_string(),
                Value::from(node.class_type.clone()),
            );
            entry.insert("inputs".to_string(), Value::Object(node.inputs.clone()));
            if let Some(title) = &node.title {
                let mut meta = Map::new();
                meta.insert("title".to_string(), Value::from(title.clone()));
                entry.insert("_meta".to_string(), Value::Object(meta));
            }
            object.insert(node.id.clone(), Value::Object(entry));
        }
        Value::Object(object)
    }
}

/// A 16-hex content hash of the graph's canonical JSON (object keys sorted at every depth), so two
/// runs that submitted the same graph carry the same hash whatever order the file spelled its keys
/// in.
pub fn graph_hash(graph: &Graph) -> String {
    let canonical = canonical_json(&graph.to_value());
    format!("{:016x}", ludion_assets::content_hash(canonical.as_bytes()))
}

/// Where a recipe binds the `images:` entry `name`: every `(node title, input name)` whose override
/// is `{ image: <name> }`, in the document's sorted order.
pub fn image_bindings(doc: &GenerationDocument, name: &str) -> Vec<(String, String)> {
    let mut bound = Vec::new();
    for (title, entries) in &doc.inputs {
        for (input, value) in entries {
            if matches!(value, InputValue::ImageRef(reference) if reference == name) {
                bound.push((title.clone(), input.clone()));
            }
        }
    }
    bound
}

/// One [`code::GENERATION_IMAGE_UNBOUND`] per `images:` entry no `inputs:` override binds — the
/// workflow would run its own baked-in image for it. Offline: reads the document alone.
pub fn unbound_images(doc: &GenerationDocument) -> Vec<Diagnostic> {
    doc.images
        .iter()
        .filter(|(name, _)| image_bindings(doc, name).is_empty())
        .map(|(name, _)| {
            Diagnostic::new(
                code::GENERATION_IMAGE_UNBOUND,
                format!(
                    "image `{name}` is declared under `images:` but no `inputs:` override binds \
                     it; the workflow would use its own baked-in default image"
                ),
            )
            .with_path(format!("/images/{}", pointer_token(name)))
            .with_help(format!(
                "add an override like `<Node Title>: image: {{ image: {name} }}` under `inputs:`, \
                 or delete the unused `images:` entry"
            ))
        })
        .collect()
}

/// Writes a recipe's overrides into the graph and resolves its image references, then writes the
/// recipe's base seed into every unpinned seed input.
///
/// `uploaded` maps an `images:` entry name to the server-side filename `POST /upload/image`
/// returned, which is what a `LoadImage` node's input actually holds.
///
/// # Errors
/// [`code::WORKFLOW_TITLE_UNRESOLVED`] per unresolved override title, and
/// [`code::GENERATION_DATA`] for an image reference with no uploaded filename, and
/// [`code::GENERATION_IMAGE_UNBOUND`] per declared image no override binds. Every override is
/// attempted, so one call reports every fault.
pub fn apply(
    graph: &Graph,
    doc: &GenerationDocument,
    uploaded: &BTreeMap<String, String>,
) -> Result<Graph, Vec<Diagnostic>> {
    let mut diagnostics = unbound_images(doc);
    let mut edits: Vec<(String, String, Value)> = Vec::new();
    let mut pinned: BTreeSet<(String, String)> = BTreeSet::new();

    for (title, entries) in &doc.inputs {
        let node = match graph.find_title(title) {
            Ok(node) => node,
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                continue;
            }
        };
        for (input, value) in entries {
            let resolved = match value {
                InputValue::Scalar(scalar) => scalar_value(scalar),
                InputValue::ImageRef(name) => match uploaded.get(name) {
                    Some(filename) => Value::from(filename.clone()),
                    None => {
                        diagnostics.push(
                            Diagnostic::new(
                                code::GENERATION_DATA,
                                format!(
                                    "`{input}` on {} references the image `{name}`, which was never \
                                     uploaded",
                                    node.label()
                                ),
                            )
                            .with_path(format!(
                                "/inputs/{}/{}",
                                pointer_token(title),
                                pointer_token(input)
                            ))
                            .with_help(
                                "declare the image under the recipe's `images:` key, as \
                                 `<name>: <relative path>`",
                            ),
                        );
                        continue;
                    }
                },
            };
            if SEED_INPUTS.contains(&input.as_str()) {
                pinned.insert((node.id.clone(), input.clone()));
            }
            edits.push((node.id.clone(), input.clone(), resolved));
        }
    }

    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let mut applied = graph.clone();
    for (id, input, value) in edits {
        if let Some(node) = applied.nodes.iter_mut().find(|node| node.id == id) {
            node.inputs.insert(input, value);
        }
    }
    applied.pinned_seeds = pinned;
    Ok(with_seed(&applied, doc.seed).0)
}

/// What [`with_seed`] did about a run's seed, which decides the seed a candidate is labelled with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeedUse {
    /// Some input took the run's seed.
    Written(u64),
    /// Nothing was written, and every literal seed input holds this one value.
    Pinned(u64),
    /// Nothing was written and no single value is known: a seed fed by a link, pins that disagree,
    /// or no seed input at all.
    Unknown,
}

impl SeedUse {
    /// The seed the job ran at, when it is known.
    pub fn seed(self) -> Option<u64> {
        match self {
            Self::Written(seed) | Self::Pinned(seed) => Some(seed),
            Self::Unknown => None,
        }
    }
}

/// Writes `seed` into every existing `seed`/`noise_seed` input the recipe did not set itself — the
/// one difference between candidate `i` and candidate `i + 1` of a `--count` run — and reports
/// which seed the graph then runs at.
pub fn with_seed(graph: &Graph, seed: u64) -> (Graph, SeedUse) {
    let pinned = graph.pinned_seeds.clone();
    let mut out = graph.clone();
    let mut written = false;
    for node in &mut out.nodes {
        for name in SEED_INPUTS {
            if !node.inputs.contains_key(name) {
                continue;
            }
            if pinned.contains(&(node.id.clone(), name.to_string())) {
                continue;
            }
            // A `[node_id, slot]` link feeds this seed from elsewhere in the graph; overwriting it
            // would rewire the graph, which is the workflow's business, never the recipe's.
            if node.inputs.get(name).is_some_and(Value::is_array) {
                continue;
            }
            node.inputs.insert(name.to_string(), Value::from(seed));
            written = true;
        }
    }
    if written {
        return (out, SeedUse::Written(seed));
    }
    let mut values = Vec::new();
    for node in &out.nodes {
        for name in SEED_INPUTS {
            match node.inputs.get(name) {
                // A linked seed's value is unknowable here, so it spoils any single pin.
                Some(value) if value.is_array() => values.push(None),
                Some(value) => values.push(value.as_u64()),
                None => {}
            }
        }
    }
    values.dedup();
    let used = match values.as_slice() {
        [Some(value)] => SeedUse::Pinned(*value),
        _ => SeedUse::Unknown,
    };
    (out, used)
}

/// Validates the graph and a recipe's overrides against the server's `/object_info` schema — the
/// live half of `gen check`, and what turns a 400 from a machine into a local diagnostic.
///
/// Reports [`code::WORKFLOW_UNKNOWN_NODE`] for a class the server does not have (with the nearest
/// installed one), and [`code::WORKFLOW_INPUT_INVALID`] for a required input neither set nor linked,
/// an override naming an input the node does not have, and an override whose value does not match
/// the input's declared type or enum.
pub fn validate(graph: &Graph, doc: &GenerationDocument, object_info: &Value) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let Some(classes) = object_info.as_object() else {
        return vec![
            Diagnostic::new(
                code::COMFY_REJECTED,
                "the server's `/object_info` reply is not an object of node classes",
            )
            .with_help("check that the address points at a ComfyUI server and not another service"),
        ];
    };

    let mut installed: Vec<&str> = classes.keys().map(String::as_str).collect();
    installed.sort_unstable();

    for node in graph.nodes() {
        let Some(spec) = classes.get(&node.class_type) else {
            let mut help = String::new();
            if let Some(near) = nearest(&node.class_type, installed.iter().copied()) {
                help.push_str(&format!("nearest installed class is `{near}`; "));
            }
            help.push_str(
                "`ludion gen nodes <filter>` lists the classes this server does have — a custom \
                 node pack the graph was authored with may not be installed here",
            );
            diagnostics.push(
                Diagnostic::new(
                    code::WORKFLOW_UNKNOWN_NODE,
                    format!(
                        "the server has no node class `{}` (used by {})",
                        node.class_type,
                        node.label()
                    ),
                )
                .with_help(help),
            );
            continue;
        };

        for (name, _) in required_inputs(spec) {
            if !node.inputs.contains_key(&name) {
                diagnostics.push(
                    Diagnostic::new(
                        code::WORKFLOW_INPUT_INVALID,
                        format!(
                            "{} needs the input `{name}`, and the graph neither sets it nor links \
                             anything to it",
                            node.label()
                        ),
                    )
                    .with_help(
                        "open the workflow in ComfyUI, give the input a value or a connection, and \
                         re-export it in API format",
                    ),
                );
            }
        }
    }

    for (title, entries) in &doc.inputs {
        // An unresolved title is `apply`'s report, not this pass's.
        let Ok(node) = graph.find_title(title) else {
            continue;
        };
        let Some(spec) = classes.get(&node.class_type) else {
            continue;
        };
        for (input, value) in entries {
            let pointer = format!("/inputs/{}/{}", pointer_token(title), pointer_token(input));
            let Some(declared) = input_spec(spec, input) else {
                let names = input_names(spec);
                let help = match nearest(input, names.iter().map(String::as_str)) {
                    Some(near) => format!("nearest input is `{near}`"),
                    None => format!("this node's inputs are {}", title_list_owned(&names)),
                };
                diagnostics.push(
                    Diagnostic::new(
                        code::WORKFLOW_INPUT_INVALID,
                        format!("{} has no input named `{input}`", node.label()),
                    )
                    .with_path(&pointer)
                    .with_help(help),
                );
                continue;
            };
            if let Some(diagnostic) = check_value(node, input, value, &declared) {
                diagnostics.push(diagnostic.with_path(&pointer));
            }
        }
    }

    diagnostics
}

/// The declared shape of one node input, as `/object_info` spells it.
#[derive(Clone, Debug, PartialEq)]
enum InputType {
    Int,
    Float,
    Text,
    Bool,
    /// A picklist, with its options when the server sent them.
    Combo(Vec<String>),
    /// A graph edge's type (`MODEL`, `LATENT`, …) — something a recipe can never set.
    Link(String),
}

/// Checks one override against its declared type, `None` when it fits.
fn check_value(
    node: &GraphNode,
    input: &str,
    value: &InputValue,
    declared: &InputType,
) -> Option<Diagnostic> {
    let mismatch = |expected: &str, found: &str| {
        Some(
            Diagnostic::new(
                code::WORKFLOW_INPUT_INVALID,
                format!(
                    "`{input}` on {} expects {expected}, and the recipe gives {found}",
                    node.label()
                ),
            )
            .with_help(format!(
                "`ludion gen nodes --class {}` prints this node's inputs with their types",
                node.class_type
            )),
        )
    };

    match (declared, value) {
        (InputType::Int, InputValue::Scalar(ScalarValue::Integer(_))) => None,
        (InputType::Int, other) => mismatch("an integer (`INT`)", &found_phrase(other)),
        (
            InputType::Float,
            InputValue::Scalar(ScalarValue::Number(_) | ScalarValue::Integer(_)),
        ) => None,
        (InputType::Float, other) => mismatch("a number (`FLOAT`)", &found_phrase(other)),
        (InputType::Text, InputValue::Scalar(ScalarValue::Text(_)) | InputValue::ImageRef(_)) => {
            None
        }
        (InputType::Text, other) => mismatch("a string (`STRING`)", &found_phrase(other)),
        (InputType::Bool, InputValue::Scalar(ScalarValue::Bool(_))) => None,
        (InputType::Bool, other) => mismatch("`true` or `false` (`BOOLEAN`)", &found_phrase(other)),
        // An uploaded image's server-side filename is not in the options list the server sent
        // before the upload happened, so an image reference is taken on trust.
        (InputType::Combo(_), InputValue::ImageRef(_)) => None,
        (InputType::Combo(options), InputValue::Scalar(ScalarValue::Text(text))) => {
            if options.is_empty() || options.iter().any(|option| option == text) {
                return None;
            }
            let help = match nearest(text, options.iter().map(String::as_str)) {
                Some(near) => format!("nearest option is `{near}`"),
                None => format!(
                    "`ludion gen nodes --class {}` prints the full picklist this server offers",
                    node.class_type
                ),
            };
            Some(
                Diagnostic::new(
                    code::WORKFLOW_INPUT_INVALID,
                    format!(
                        "`{input}` on {} is a picklist and does not offer `{text}`; it offers {}",
                        node.label(),
                        title_list_owned(&truncated(options))
                    ),
                )
                .with_help(help),
            )
        }
        (InputType::Combo(_), other) => {
            mismatch("one of a picklist's strings", &found_phrase(other))
        }
        (InputType::Link(kind), _) => Some(
            Diagnostic::new(
                code::WORKFLOW_INPUT_INVALID,
                format!(
                    "`{input}` on {} is a graph edge (type `{kind}`), not a value a recipe can set",
                    node.label()
                ),
            )
            .with_help(
                "a recipe sets leaf inputs only; rewire the graph in ComfyUI and re-export it in \
                 API format",
            ),
        ),
    }
}

fn found_phrase(value: &InputValue) -> String {
    match value {
        InputValue::Scalar(ScalarValue::Bool(v)) => format!("`{v}`"),
        InputValue::Scalar(ScalarValue::Integer(v)) => format!("the integer `{v}`"),
        InputValue::Scalar(ScalarValue::Number(v)) => format!("the number `{v}`"),
        InputValue::Scalar(ScalarValue::Text(v)) => format!("the string `{v}`"),
        InputValue::ImageRef(name) => format!("the image reference `{name}`"),
    }
}

/// Reads one input's declared shape out of a class's `/object_info` entry.
fn input_spec(class: &Value, name: &str) -> Option<InputType> {
    let input = class.get("input")?;
    for group in ["required", "optional"] {
        if let Some(spec) = input.get(group).and_then(|g| g.get(name)) {
            return Some(input_type(spec));
        }
    }
    None
}

/// `[type, opts?]`, where `type` is a type name, a list of picklist options, or the newer
/// `["COMBO", {"options": [...]}]` spelling.
fn input_type(spec: &Value) -> InputType {
    let head = spec.get(0).unwrap_or(spec);
    if let Some(options) = head.as_array() {
        return InputType::Combo(option_strings(options));
    }
    match head.as_str() {
        Some("INT") => InputType::Int,
        Some("FLOAT") => InputType::Float,
        Some("STRING") => InputType::Text,
        Some("BOOLEAN") => InputType::Bool,
        Some("COMBO") => {
            let options = spec
                .get(1)
                .and_then(|opts| opts.get("options"))
                .and_then(Value::as_array)
                .map(|options| option_strings(options))
                .unwrap_or_default();
            InputType::Combo(options)
        }
        Some(other) => InputType::Link(other.to_string()),
        None => InputType::Link("unknown".to_string()),
    }
}

fn option_strings(options: &[Value]) -> Vec<String> {
    options
        .iter()
        .map(|option| match option.as_str() {
            Some(text) => text.to_string(),
            None => option.to_string(),
        })
        .collect()
}

/// Every required input of a class, in declaration order, with its declared shape.
fn required_inputs(class: &Value) -> Vec<(String, InputType)> {
    class
        .get("input")
        .and_then(|input| input.get("required"))
        .and_then(Value::as_object)
        .map(|required| {
            required
                .iter()
                .map(|(name, spec)| (name.clone(), input_type(spec)))
                .collect()
        })
        .unwrap_or_default()
}

/// Every input name a class declares, required then optional — the vocabulary a misspelled
/// override is matched against.
fn input_names(class: &Value) -> Vec<String> {
    let mut names = Vec::new();
    for group in ["required", "optional"] {
        if let Some(entries) = class
            .get("input")
            .and_then(|input| input.get(group))
            .and_then(Value::as_object)
        {
            names.extend(entries.keys().cloned());
        }
    }
    names
}

/// The output-node families, by class name. ComfyUI's savers are named rather than flagged in the
/// graph, so the name is what a graph-only pass can read.
fn classify_output(class: &str) -> Option<OutputKind> {
    if class.contains("SaveImage") || class == "SaveAnimatedWEBP" || class == "SaveAnimatedPNG" {
        return Some(OutputKind::Image);
    }
    if class.contains("SaveGLB") || class.contains("Save3D") {
        return Some(OutputKind::Mesh);
    }
    None
}

fn scalar_value(scalar: &ScalarValue) -> Value {
    match scalar {
        ScalarValue::Bool(v) => Value::from(*v),
        ScalarValue::Integer(v) => Value::from(*v),
        ScalarValue::Number(v) => Value::from(*v),
        ScalarValue::Text(v) => Value::from(v.clone()),
    }
}

fn not_api(message: String) -> Diagnostic {
    Diagnostic::new(code::WORKFLOW_NOT_API, message).with_help(
        "in ComfyUI use File -> Export (API) — the browser's plain save writes the UI format, \
         which `/prompt` does not accept — and commit that file",
    )
}

/// Numeric node ids sort as numbers so `10` follows `9`; anything else sorts after them by name.
fn id_key(id: &str) -> (u64, &str) {
    match id.parse::<u64>() {
        Ok(n) => (n, ""),
        Err(_) => (u64::MAX, id),
    }
}

fn node_list(nodes: &[&GraphNode]) -> String {
    nodes
        .iter()
        .map(|node| node.label())
        .collect::<Vec<_>>()
        .join(", ")
}

fn title_list(titles: &[&str]) -> String {
    if titles.is_empty() {
        return "none — no node in the graph carries a `_meta.title`".to_string();
    }
    titles
        .iter()
        .map(|title| format!("`{title}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn title_list_owned(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("`{item}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A picklist can hold hundreds of checkpoint filenames; a diagnostic shows the first few.
fn truncated(options: &[String]) -> Vec<String> {
    const SHOWN: usize = 8;
    if options.len() <= SHOWN {
        return options.to_vec();
    }
    let mut shown: Vec<String> = options[..SHOWN].to_vec();
    shown.push(format!("… and {} more", options.len() - SHOWN));
    shown
}

pub(crate) use ludion_core::diagnostics::pointer_token;

/// JSON with every object's keys sorted at every depth — the byte form the graph hash is taken
/// over, so key order in the committed file never moves the hash.
pub(crate) fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(&mut out, value);
    out
}

fn write_canonical(out: &mut String, value: &Value) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::from(key.as_str()).to_string());
                out.push(':');
                write_canonical(out, &map[*key]);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(out, item);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(seed_input: &str) -> Graph {
        let source = format!(
            r#"{{
  "3": {{"class_type": "KSampler", "_meta": {{"title": "Sampler"}}, "inputs": {{ {seed_input} "steps": 20}}}},
  "7": {{"class_type": "SaveImage", "inputs": {{"images": ["3", 0]}}}},
  "8": {{"class_type": "PreviewImage", "inputs": {{"images": ["3", 0]}}}}
}}"#
        );
        Graph::parse(&source, "test.json").expect("the inline graph parses")
    }

    fn doc(inputs: &str) -> GenerationDocument {
        let source = format!("format: 1\nkind: generation\nworkflow: w.json\n{inputs}");
        ludion_scene::parse_generation(&source).expect("the inline recipe parses")
    }

    fn used(graph: &Graph, doc: &GenerationDocument, seed: u64) -> SeedUse {
        let applied = apply(graph, doc, &BTreeMap::new()).expect("applies");
        with_seed(&applied, seed).1
    }

    #[test]
    fn an_unpinned_literal_seed_takes_the_runs_seed() {
        let g = graph(r#""seed": 1,"#);
        assert_eq!(with_seed(&g, 9).1, SeedUse::Written(9));
        assert_eq!(used(&g, &doc(""), 4), SeedUse::Written(4));
    }

    #[test]
    fn a_pin_on_every_seed_input_records_the_pin() {
        let g = graph(r#""seed": 1,"#);
        let pinned = doc("inputs:\n  Sampler:\n    seed: 5\n");
        assert_eq!(used(&g, &pinned, 9), SeedUse::Pinned(5));
        assert_eq!(used(&g, &pinned, 9).seed(), Some(5));
    }

    #[test]
    fn a_linked_seed_is_unknown() {
        let g = graph(r#""seed": ["9", 0],"#);
        assert_eq!(used(&g, &doc(""), 9), SeedUse::Unknown);
        assert_eq!(SeedUse::Unknown.seed(), None);
    }

    #[test]
    fn pins_that_disagree_are_unknown() {
        let g = graph(r#""seed": 1, "noise_seed": 2,"#);
        let pinned = doc("inputs:\n  Sampler:\n    seed: 5\n    noise_seed: 6\n");
        assert_eq!(used(&g, &pinned, 9), SeedUse::Unknown);
    }

    #[test]
    fn a_graph_with_no_seed_input_is_unknown() {
        assert_eq!(used(&graph(""), &doc(""), 9), SeedUse::Unknown);
    }

    #[test]
    fn a_preview_image_is_not_an_output() {
        assert_eq!(graph("").output_kind().unwrap(), OutputKind::Image);
        let preview_only = Graph::parse(
            r#"{"1": {"class_type": "PreviewImage", "inputs": {}}}"#,
            "test.json",
        )
        .unwrap();
        let err = preview_only.output_kind().unwrap_err();
        assert_eq!(err.code, code::GENERATION_OUTPUT_KIND);
    }
}
