//! Resolving sub-recipes: the [`Resolver`] seam, the filesystem resolver, and [`load`].

use std::collections::BTreeMap;
use std::path::PathBuf;

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, pointer_token};
use ludion_core::suggest::nearest;

use crate::hash::mix;
use crate::ops::{Const, FillOutput, Op, ParamValue, Slot};
use crate::picture::{Picture, decode};
use crate::recipe::{Instance, OutputKey, Recipe, parse};
use crate::stdlib;

/// A recipe document a [`Resolver`] found.
#[derive(Clone, Debug)]
pub struct Source {
    /// The lexically normalized path: the cycle key, and the `from` of references inside it.
    pub id: String,
    /// The document text.
    pub text: String,
}

/// Finds the documents and image files a recipe's paths name.
pub trait Resolver {
    /// Reads `reference`, relative to the document `from` (`None` for the root's own references).
    ///
    /// # Errors
    /// The reason the reference cannot be read, as one line.
    fn read(&self, reference: &str, from: Option<&str>) -> Result<Source, String>;

    /// Reads the bytes of the image file `reference` names, relative to the document `from`;
    /// returns the normalized id with them.
    ///
    /// # Errors
    /// The reason the file cannot be read, as one line.
    fn read_bytes(&self, reference: &str, from: Option<&str>) -> Result<(String, Vec<u8>), String>;
}

/// Resolves references against a base directory, relative to the referencing file.
#[derive(Clone, Debug)]
pub struct FsResolver {
    base: PathBuf,
}

impl FsResolver {
    /// A resolver for a root document that lives in `base`.
    pub fn new(base: impl Into<PathBuf>) -> Self {
        FsResolver { base: base.into() }
    }
}

/// The id of `reference` relative to the document `from`, refused when it leaves the root
/// document's directory (`..` past it, a drive or scheme prefix, an absolute path).
///
/// # Errors
/// The refusal as a phrase finishing "the path …".
pub fn confined_id(reference: &str, from: Option<&str>) -> Result<String, String> {
    let id = ludion_core::paths::sibling(from.unwrap_or(""), reference);
    match ludion_core::paths::content_refusal(&id) {
        None => Ok(id),
        Some(refusal) => Err(format!("the path {}", refusal.describe())),
    }
}

impl Resolver for FsResolver {
    fn read(&self, reference: &str, from: Option<&str>) -> Result<Source, String> {
        let id = confined_id(reference, from)?;
        let text = std::fs::read_to_string(self.base.join(&id))
            .map_err(|e| format!("cannot read recipe `{id}`: {e}"))?;
        Ok(Source { id, text })
    }

    fn read_bytes(&self, reference: &str, from: Option<&str>) -> Result<(String, Vec<u8>), String> {
        if reference.starts_with("std:") {
            return Err("an image cannot come from std:".into());
        }
        let id = confined_id(reference, from)?;
        let bytes = std::fs::read(self.base.join(&id))
            .map_err(|e| format!("cannot read image `{id}`: {e}"))?;
        Ok((id, bytes))
    }
}

/// A sub-recipe a `recipe` step evaluates, and which of its steps the step yields.
#[derive(Clone, Debug)]
pub struct Child {
    /// The loaded sub-recipe.
    pub node: Loaded,
    /// Index of the sub-recipe step the `recipe` step yields.
    pub output: usize,
    /// Index of the sub-recipe `input` step each binding feeds, in the step's `inputs` order.
    pub inputs: Vec<usize>,
}

/// A recipe and everything it references, validated and typed: evaluating it cannot fail.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// The document's id; `<root>` for the root.
    pub id: String,
    /// The seed this recipe evaluates with.
    pub seed: u64,
    /// The validated steps.
    pub instance: Instance,
    /// Channels (1 gray, 3 color) of each step's image.
    pub channels: Vec<usize>,
    /// Loaded sub-recipes by the index of the `recipe` step.
    pub children: BTreeMap<usize, Child>,
    /// Decoded files by the index of the `image` step.
    pub pictures: BTreeMap<usize, Picture>,
}

fn tag(id: Option<&str>, mut d: Diagnostic) -> Diagnostic {
    if let Some(id) = id {
        d.message = format!("in `{id}`: {}", d.message);
    }
    d
}

/// Parses the root recipe and every sub-recipe it references, once per use.
///
/// # Errors
/// Every diagnostic of every document; one raised inside a sub-recipe starts ``in `<id>`: ``.
pub fn load(root: &str, resolver: &dyn Resolver) -> Result<Loaded, Vec<Diagnostic>> {
    let recipe = parse(root)?;
    let seed = recipe.seed;
    load_parsed(&recipe, resolver, seed)
}

/// [`load`] with the root seed replaced by `seed`; every op and sub-recipe seed derives from it as
/// usual.
///
/// # Errors
/// As [`load`].
pub fn load_with_seed(
    root: &str,
    resolver: &dyn Resolver,
    seed: u64,
) -> Result<Loaded, Vec<Diagnostic>> {
    load_parsed(&parse(root)?, resolver, seed)
}

fn load_parsed(
    recipe: &Recipe,
    resolver: &dyn Resolver,
    seed: u64,
) -> Result<Loaded, Vec<Diagnostic>> {
    node(
        resolver,
        None,
        recipe,
        recipe.defaults(),
        seed,
        &mut Vec::new(),
    )
}

fn node(
    resolver: &dyn Resolver,
    id: Option<&str>,
    recipe: &Recipe,
    bindings: BTreeMap<String, ParamValue>,
    seed: u64,
    stack: &mut Vec<String>,
) -> Result<Loaded, Vec<Diagnostic>> {
    let instance = recipe
        .instantiate(&bindings)
        .map_err(|ds| ds.into_iter().map(|d| tag(id, d)).collect::<Vec<_>>())?;
    if let Some(id) = id {
        stack.push(id.to_string());
    }
    let mut diags = Vec::new();
    let mut children = BTreeMap::new();
    let mut pictures = BTreeMap::new();
    for &i in &instance.order {
        let step = &instance.steps[i];
        if let Op::Image { path, srgb } = &step.op {
            let at = format!("/steps/{}/path", pointer_token(&step.name));
            let read = if id.is_some_and(stdlib::is_std) {
                Err("a stdlib recipe cannot load an image".to_string())
            } else {
                confined_id(path, id).and_then(|_| resolver.read_bytes(path, id))
            };
            match read {
                Err(reason) => diags.push(tag(
                    id,
                    Diagnostic::new(
                        code::TEXTURE_GRAPH,
                        format!("step `{}` cannot load `{path}`: {reason}", step.name),
                    )
                    .with_path(at)
                    .with_help("check the path is relative to this recipe and the file exists"),
                )),
                Ok((file, bytes)) => match decode(&bytes, *srgb) {
                    Ok(picture) => {
                        pictures.insert(i, picture);
                    }
                    Err(reason) => diags.push(tag(
                        id,
                        Diagnostic::new(
                            code::TEXTURE_DATA,
                            format!(
                                "step `{}`: `{file}` is not a usable PNG: {reason}",
                                step.name
                            ),
                        )
                        .with_path(at)
                        .with_help("use a PNG at most 8192 pixels on a side"),
                    )),
                },
            }
            continue;
        }
        let Op::Recipe {
            path,
            output,
            params,
            inputs,
            seed: op_seed,
        } = &step.op
        else {
            continue;
        };
        let at = |field: &str| format!("/steps/{}/{field}", pointer_token(&step.name));
        let src = match source(resolver, id, path) {
            Ok(src) => src,
            Err(reason) => {
                diags.push(tag(
                    id,
                    Diagnostic::new(
                        code::TEXTURE_GRAPH,
                        format!("step `{}` cannot load `{path}`: {reason}", step.name),
                    )
                    .with_path(at("path"))
                    .with_help(
                        "check the path is relative to this recipe, or a `std:<name>` recipe",
                    ),
                ));
                continue;
            }
        };
        if let Some(pos) = stack.iter().position(|s| *s == src.id) {
            let mut chain: Vec<&str> = stack[pos..].iter().map(String::as_str).collect();
            chain.push(&src.id);
            diags.push(tag(
                id,
                Diagnostic::new(
                    code::TEXTURE_GRAPH,
                    format!(
                        "recipe `{}` references itself: {}",
                        src.id,
                        chain.join(" → ")
                    ),
                )
                .with_path(at("path"))
                .with_help("remove one reference in the chain"),
            ));
            continue;
        }
        let child = match parse(&src.text) {
            Ok(child) => child,
            Err(ds) => {
                diags.extend(ds.into_iter().map(|d| tag(Some(&src.id), d)));
                continue;
            }
        };
        let mut child_bindings = child.defaults();
        let mut bound_ok = true;
        for (name, value) in params {
            let params_at = format!("params/{}", pointer_token(name));
            match child.params.iter().find(|(n, _)| n == name) {
                None => {
                    bound_ok = false;
                    let known: Vec<&str> = child.params.iter().map(|(n, _)| n.as_str()).collect();
                    let help = match nearest(name, known.iter().copied()) {
                        Some(near) => format!("did you mean `{near}`?"),
                        None if known.is_empty() => {
                            "that recipe declares no parameters".to_string()
                        }
                        None => format!("its parameters are: {}", known.join(", ")),
                    };
                    diags.push(tag(
                        id,
                        Diagnostic::new(
                            code::TEXTURE_STRUCTURE,
                            format!("`{}` has no parameter `{name}`", src.id),
                        )
                        .with_path(at(&params_at))
                        .with_help(help),
                    ));
                }
                Some((_, default))
                    if std::mem::discriminant(default) != std::mem::discriminant(value) =>
                {
                    bound_ok = false;
                    diags.push(tag(
                        id,
                        Diagnostic::new(
                            code::TEXTURE_DATA,
                            format!("parameter `{name}` of `{}` has the wrong type", src.id),
                        )
                        .with_path(at(&params_at))
                        .with_help("bind a number to a numeric parameter and a `#rrggbb` color to a color one"),
                    ));
                }
                Some(_) => {
                    child_bindings.insert(name.clone(), *value);
                }
            }
        }
        if !bound_ok {
            continue;
        }
        let child_seed = mix(mix(seed, child.seed), u64::from(*op_seed));
        let loaded = match node(
            resolver,
            Some(&src.id),
            &child,
            child_bindings,
            child_seed,
            stack,
        ) {
            Ok(loaded) => loaded,
            Err(ds) => {
                diags.extend(ds);
                continue;
            }
        };
        let bound = match bind_inputs(&loaded, inputs) {
            Ok(bound) => bound,
            Err(failures) => {
                diags.extend(failures.into_iter().map(|(field, message, help)| {
                    tag(
                        id,
                        Diagnostic::new(code::TEXTURE_GRAPH, message)
                            .with_path(at(&field))
                            .with_help(help),
                    )
                }));
                continue;
            }
        };
        match loaded.instance.steps.iter().position(|s| s.name == *output) {
            Some(index) => {
                children.insert(
                    i,
                    Child {
                        node: loaded,
                        output: index,
                        inputs: bound,
                    },
                );
            }
            None => {
                let names: Vec<&str> = loaded
                    .instance
                    .steps
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect();
                let help = match nearest(output, names.iter().copied()) {
                    Some(near) => format!("did you mean `{near}`?"),
                    None => format!("its steps are: {}", names.join(", ")),
                };
                diags.push(tag(
                    id,
                    Diagnostic::new(
                        code::TEXTURE_GRAPH,
                        format!("`{}` has no step `{output}`", src.id),
                    )
                    .with_path(at("output"))
                    .with_help(help),
                ));
            }
        }
    }
    if id.is_some() {
        stack.pop();
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    let channels = infer(&instance, &children, &pictures)
        .map_err(|ds| ds.into_iter().map(|d| tag(id, d)).collect::<Vec<_>>())?;
    Ok(Loaded {
        id: id.unwrap_or("<root>").to_string(),
        seed,
        instance,
        channels,
        children,
        pictures,
    })
}

/// Reads the document `path` names: an embedded `std:` recipe, or whatever the resolver finds.
fn source(resolver: &dyn Resolver, from: Option<&str>, path: &str) -> Result<Source, String> {
    stdlib::std_policy(from, path)?;
    let Some(name) = path.strip_prefix("std:") else {
        confined_id(path, from)?;
        return resolver.read(path, from);
    };
    match stdlib::get(name) {
        Some(text) => Ok(Source {
            id: path.to_string(),
            text: text.to_string(),
        }),
        None => {
            let hint = match nearest(name, stdlib::names()) {
                Some(near) => format!("did you mean `std:{near}`?"),
                None => format!(
                    "the stdlib has: {}",
                    stdlib::names().collect::<Vec<_>>().join(", ")
                ),
            };
            Err(format!("no `std:{name}` in the embedded stdlib; {hint}"))
        }
    }
}

/// Maps each binding of a `recipe` step to the `input` step of `child` it feeds. A failure is
/// `(field pointer tail, message, help)`.
fn bind_inputs(
    child: &Loaded,
    bindings: &[(String, String)],
) -> Result<Vec<usize>, Vec<(String, String, String)>> {
    let steps = &child.instance.steps;
    let declared: Vec<&str> = steps
        .iter()
        .filter(|s| matches!(s.op, Op::Input { .. }))
        .map(|s| s.name.as_str())
        .collect();
    let mut bound = Vec::new();
    let mut failures = Vec::new();
    for (key, _) in bindings {
        let field = format!("inputs/{}", pointer_token(key));
        match steps.iter().position(|s| s.name == *key) {
            None => {
                let help = match nearest(key, declared.iter().copied()) {
                    Some(near) => format!("did you mean `{near}`?"),
                    None if declared.is_empty() => "that recipe declares no inputs".to_string(),
                    None => format!("its inputs are: {}", declared.join(", ")),
                };
                failures.push((
                    field,
                    format!("`{}` has no step `{key}` to bind", child.id),
                    help,
                ));
            }
            Some(index) if !matches!(steps[index].op, Op::Input { .. }) => {
                let help = if declared.is_empty() {
                    "that recipe declares no inputs".to_string()
                } else {
                    format!("its inputs are: {}", declared.join(", "))
                };
                failures.push((
                    field,
                    format!("`{key}` of `{}` is not an `input` step", child.id),
                    help,
                ));
            }
            Some(index) => bound.push(index),
        }
    }
    if failures.is_empty() {
        Ok(bound)
    } else {
        Err(failures)
    }
}

/// Infers each step's channel count and checks every slot's gray/color requirement.
fn infer(
    instance: &Instance,
    children: &BTreeMap<usize, Child>,
    pictures: &BTreeMap<usize, Picture>,
) -> Result<Vec<usize>, Vec<Diagnostic>> {
    let mut diags = Vec::new();
    let mut channels = vec![1usize; instance.steps.len()];
    for &i in &instance.order {
        let step = &instance.steps[i];
        let ch = |k: usize| channels[step.inputs[k]];
        let mut need = |field: &str, want: usize, found: usize| {
            if found == want {
                return;
            }
            let (want_s, found_s) = if want == 1 {
                ("gray", "color")
            } else {
                ("color", "gray")
            };
            let help = if want == 1 {
                "turn it gray with a `grayscale` step"
            } else {
                "turn it into color with a `colorize` step"
            };
            diags.push(
                Diagnostic::new(
                    code::TEXTURE_DATA,
                    format!(
                        "step `{}` needs a {want_s} `{field}`, but that step is {found_s}",
                        step.name
                    ),
                )
                .with_path(format!(
                    "/steps/{}/{field}",
                    pointer_token(step.document_name())
                ))
                .with_help(help),
            );
        };
        channels[i] = match &step.op {
            Op::Constant(c) => match c {
                crate::ops::Const::Gray(_) => 1,
                crate::ops::Const::Color(_) => 3,
            },
            Op::Gradient { .. } | Op::Noise(_) => 1,
            Op::TileSampler(spec) => {
                if spec.pattern.is_some() {
                    need("pattern", 1, ch(0));
                }
                1
            }
            Op::FloodFill { output, .. } => {
                need("input", 1, ch(0));
                if *output == FillOutput::Uv { 3 } else { 1 }
            }
            Op::Distance { .. }
            | Op::Band { .. }
            | Op::AmbientOcclusion { .. }
            | Op::Curvature { .. } => {
                need("input", 1, ch(0));
                1
            }
            Op::Image { .. } => pictures.get(&i).map_or(1, |p| p.channels),
            Op::Layer { map, base, coats } => {
                let mut next = step.inputs.iter();
                let slot = |slot: &Slot, next: &mut std::slice::Iter<'_, usize>| match slot {
                    Slot::Step(_) => channels[*next.next().expect("a step slot has an input")],
                    Slot::Const(Const::Gray(_)) => 1,
                    Slot::Const(Const::Color(_)) => 3,
                };
                let at = |field: String| {
                    format!("/steps/{}/{field}", pointer_token(step.document_name()))
                };
                let mut width = slot(base, &mut next);
                let mut report = |field: String, what: &str, help: &str| {
                    let path = at(field);
                    // Every map of a layer step checks the same coats: one report per coat.
                    if diags.iter().any(|d| {
                        d.code == code::TEXTURE_DATA && d.path.as_deref() == Some(path.as_str())
                    }) {
                        return;
                    }
                    diags.push(
                        Diagnostic::new(
                            code::TEXTURE_DATA,
                            format!(
                                "step `{}` needs a gray {what}, but it is color",
                                step.document_name()
                            ),
                        )
                        .with_path(path)
                        .with_help(help.to_string()),
                    );
                };
                if map.is_gray() && width != 1 {
                    report(
                        format!("base/{}", map.name()),
                        map.name(),
                        "use a gray step or a number",
                    );
                }
                for coat in coats {
                    let mask = channels[*next.next().expect("a coat has a mask input")];
                    if mask != 1 {
                        report(
                            format!("layers/{}/mask", coat.index),
                            "mask",
                            "turn it gray with a `grayscale` step",
                        );
                    }
                    let found = slot(&coat.value, &mut next);
                    if map.is_gray() && found != 1 {
                        report(
                            format!("layers/{}/{}", coat.index, map.name()),
                            map.name(),
                            "use a gray step or a number",
                        );
                    }
                    width = width.max(found);
                }
                width
            }
            Op::Blend { mask, .. } => {
                if mask.is_some() {
                    need("mask", 1, ch(2));
                }
                ch(0).max(ch(1))
            }
            Op::Levels { .. }
            | Op::Curve { .. }
            | Op::Clamp { .. }
            | Op::Remap { .. }
            | Op::Posterize { .. }
            | Op::Warp { .. }
            | Op::Blur { .. } => ch(0),
            Op::Colorize { .. } => {
                need("input", 1, ch(0));
                3
            }
            Op::Grayscale { .. } => {
                need("input", 3, ch(0));
                1
            }
            Op::HsvShift { .. } => {
                need("input", 3, ch(0));
                3
            }
            Op::NormalFromHeight { .. } => {
                need("input", 1, ch(0));
                3
            }
            Op::Input { channels, .. } => *channels,
            Op::Recipe { inputs, .. } => children.get(&i).map_or(1, |c| {
                for (k, (key, _)) in inputs.iter().enumerate() {
                    let want = c.node.channels[c.inputs[k]];
                    let found = ch(k);
                    if want != found {
                        let (want_s, found_s, help) = if want == 1 {
                            ("gray", "color", "turn it gray with a `grayscale` step")
                        } else {
                            ("color", "gray", "turn it into color with a `colorize` step")
                        };
                        diags.push(
                            Diagnostic::new(
                                code::TEXTURE_DATA,
                                format!(
                                    "input `{key}` of `{}` is {want_s}, but step `{}` is {found_s}",
                                    c.node.id, inputs[k].1
                                ),
                            )
                            .with_path(format!(
                                "/steps/{}/inputs/{}",
                                pointer_token(&step.name),
                                pointer_token(key)
                            ))
                            .with_help(help),
                        );
                    }
                }
                c.node.channels[c.output]
            }),
        };
    }
    for (key, i) in &instance.outputs {
        let found = channels[*i];
        let ok = if key.is_gray() {
            found == 1
        } else if *key == OutputKey::Normal {
            found == 3
        } else {
            true
        };
        if !ok {
            let (want, got, help) = if key.is_gray() {
                ("gray", "color", "turn it gray with a `grayscale` step")
            } else {
                (
                    "color",
                    "gray",
                    "build the normal with `normal_from_height`",
                )
            };
            diags.push(
                Diagnostic::new(
                    code::TEXTURE_DATA,
                    format!(
                        "output `{}` must be {want}, but step `{}` is {got}",
                        key.name(),
                        instance.steps[*i].name
                    ),
                )
                .with_path(format!("/outputs/{}", key.name()))
                .with_help(help),
            );
        }
    }
    if diags.is_empty() {
        Ok(channels)
    } else {
        Err(diags)
    }
}
