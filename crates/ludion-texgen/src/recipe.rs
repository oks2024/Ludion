//! Parsing and validating one `kind: texture` document, and instantiating it with parameters.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, pointer_token, value_phrase};
use ludion_core::suggest::nearest;
use serde_json::{Map, Value, json};

use crate::ops::{Op, ParamValue, Scope, parse_hex, parse_step};

/// The map an `outputs` entry feeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKey {
    /// Albedo, color.
    BaseColor,
    /// Roughness, gray.
    Roughness,
    /// Metallic, gray; defaults to 0.
    Metallic,
    /// Tangent-space normal, color; defaults to the height's normal at depth 0.05.
    Normal,
    /// Ambient occlusion, gray; defaults to 1.
    Occlusion,
    /// Emissive, color; omitted when absent.
    Emissive,
    /// Height, gray, written as a 16-bit PNG.
    Height,
}

impl OutputKey {
    /// Every key, in the order the docs list them.
    pub const ALL: [OutputKey; 7] = [
        OutputKey::BaseColor,
        OutputKey::Roughness,
        OutputKey::Metallic,
        OutputKey::Normal,
        OutputKey::Occlusion,
        OutputKey::Emissive,
        OutputKey::Height,
    ];

    /// The key as written in `outputs:`.
    pub fn name(self) -> &'static str {
        match self {
            OutputKey::BaseColor => "base_color",
            OutputKey::Roughness => "roughness",
            OutputKey::Metallic => "metallic",
            OutputKey::Normal => "normal",
            OutputKey::Occlusion => "occlusion",
            OutputKey::Emissive => "emissive",
            OutputKey::Height => "height",
        }
    }

    /// Whether the map is one channel.
    pub fn is_gray(self) -> bool {
        !matches!(
            self,
            OutputKey::BaseColor | OutputKey::Normal | OutputKey::Emissive
        )
    }

    fn required(self) -> bool {
        matches!(
            self,
            OutputKey::BaseColor | OutputKey::Roughness | OutputKey::Height
        )
    }
}

/// One document, validated against its parameter defaults.
#[derive(Clone, Debug)]
pub struct Recipe {
    /// The recipe seed.
    pub seed: u64,
    /// Parameter names and defaults, in document order.
    pub params: Vec<(String, ParamValue)>,
    steps: Vec<(String, Map<String, Value>)>,
    outputs: Vec<(OutputKey, String)>,
}

/// One step of an [`Instance`]: a validated op and the steps it reads.
#[derive(Clone, Debug)]
pub struct Step {
    /// The step's name.
    pub name: String,
    /// The validated op.
    pub op: Op,
    /// Indices of the steps the op reads, in [`Op::inputs`] order.
    pub inputs: Vec<usize>,
}

impl Step {
    /// The authored step this one came from: a `layer` step's `surface.roughness` is `surface`.
    pub fn document_name(&self) -> &str {
        document_name(&self.name)
    }
}

fn document_name(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}

/// A recipe with parameters bound: typed ops, resolved references, an evaluation order.
#[derive(Clone, Debug)]
pub struct Instance {
    /// Steps in document order.
    pub steps: Vec<Step>,
    /// Step indices in evaluation order (dependencies first, ties by document order).
    pub order: Vec<usize>,
    /// Which step feeds which map.
    pub outputs: Vec<(OutputKey, usize)>,
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "seed": { "type": "integer", "minimum": 0 },
            "params": { "type": "object" },
            "steps": { "type": "object" },
            "outputs": { "type": "object" },
        },
        "additionalProperties": false,
    })
}

fn structure(message: String, path: String, help: String) -> Diagnostic {
    Diagnostic::new(code::TEXTURE_STRUCTURE, message)
        .with_path(path)
        .with_help(help)
}

/// Parses and validates a recipe document, collecting every problem.
///
/// Op bodies are validated here against the parameters' defaults; [`Recipe::instantiate`] repeats
/// the check with a sub-recipe's bound values.
///
/// # Errors
/// `L1001`–`L1009` strict-YAML, `L1119`/`L1120` envelope and top-level fields, `L1283`–`L1285`.
pub fn parse(source: &str) -> Result<Recipe, Vec<Diagnostic>> {
    let doc = ludion_scene::check_doc(source, "texture", &schema())?;
    let mut diags = Vec::new();
    let seed = doc.get("seed").and_then(Value::as_u64).unwrap_or(0);
    let params = read_params(doc.get("params"), &mut diags);
    let steps = read_steps(doc.get("steps"), &mut diags);
    let outputs = read_outputs(doc.get("outputs"), &mut diags);
    if !diags.is_empty() {
        return Err(diags);
    }
    let recipe = Recipe {
        seed,
        params,
        steps,
        outputs,
    };
    recipe.instantiate(&recipe.defaults())?;
    Ok(recipe)
}

fn read_params(value: Option<&Value>, diags: &mut Vec<Diagnostic>) -> Vec<(String, ParamValue)> {
    let mut out = Vec::new();
    let Some(Value::Object(map)) = value else {
        return out;
    };
    for (name, v) in map {
        let parsed = match v {
            Value::Number(n) => n.as_f64().filter(|x| x.is_finite()).map(ParamValue::Number),
            Value::String(s) => parse_hex(s).map(ParamValue::Color),
            _ => None,
        };
        match parsed {
            Some(p) => out.push((name.clone(), p)),
            None => diags.push(structure(
                format!(
                    "parameter `{name}` must be a number or a `#rrggbb` color, found {}",
                    value_phrase(v)
                ),
                format!("/params/{}", pointer_token(name)),
                format!("write `{name}: 1.0` or `{name}: \"#808080\"`"),
            )),
        }
    }
    out
}

fn read_steps(
    value: Option<&Value>,
    diags: &mut Vec<Diagnostic>,
) -> Vec<(String, Map<String, Value>)> {
    let mut out = Vec::new();
    let Some(value) = value else {
        diags.push(structure(
            "the recipe has no `steps`".into(),
            "/steps".into(),
            "add `steps:` with at least one named op".into(),
        ));
        return out;
    };
    let Value::Object(map) = value else {
        return out;
    };
    for (name, body) in map {
        if name.contains('.') {
            diags.push(structure(
                format!("step name `{name}` contains a `.`"),
                format!("/steps/{}", pointer_token(name)),
                "rename it — `.` separates a layer step from its maps".into(),
            ));
            continue;
        }
        match body {
            Value::Object(b) => out.push((name.clone(), b.clone())),
            other => diags.push(structure(
                format!(
                    "step `{name}` must be a mapping, found {}",
                    value_phrase(other)
                ),
                format!("/steps/{}", pointer_token(name)),
                "write the step as `op: ...` plus the op's fields".into(),
            )),
        }
    }
    out
}

fn read_outputs(value: Option<&Value>, diags: &mut Vec<Diagnostic>) -> Vec<(OutputKey, String)> {
    let mut out = Vec::new();
    let Some(value) = value else {
        diags.push(structure(
            "the recipe has no `outputs`".into(),
            "/outputs".into(),
            "add `outputs:` naming a step for `base_color`, `roughness` and `height`".into(),
        ));
        return out;
    };
    let Value::Object(map) = value else {
        return out;
    };
    let names: Vec<&str> = OutputKey::ALL.iter().map(|k| k.name()).collect();
    for (key, v) in map {
        let Some(k) = OutputKey::ALL.iter().find(|k| k.name() == key) else {
            let help = match nearest(key, names.iter().copied()) {
                Some(near) => format!("did you mean `{near}`?"),
                None => format!("the outputs are: {}", names.join(", ")),
            };
            diags.push(structure(
                format!("`{key}` is not an output of a texture recipe"),
                format!("/outputs/{}", pointer_token(key)),
                help,
            ));
            continue;
        };
        match v {
            Value::String(step) => out.push((*k, step.clone())),
            other => diags.push(structure(
                format!(
                    "output `{key}` must name a step, found {}",
                    value_phrase(other)
                ),
                format!("/outputs/{key}"),
                format!("write `{key}: <step name>`"),
            )),
        }
    }
    for k in OutputKey::ALL.iter().filter(|k| k.required()) {
        if !out.iter().any(|(o, _)| o == k) {
            diags.push(structure(
                format!("the recipe has no `{}` output", k.name()),
                "/outputs".into(),
                format!("add `{}: <step name>` under `outputs:`", k.name()),
            ));
        }
    }
    out
}

impl Recipe {
    /// The parameters at their defaults.
    pub fn defaults(&self) -> Scope {
        self.params.iter().cloned().collect()
    }

    /// Validates every step with `bindings` substituted and resolves the step graph.
    ///
    /// # Errors
    /// `L1283`/`L1284` for an op body, `L1285` for an unknown step reference or a cycle.
    pub fn instantiate(&self, bindings: &Scope) -> Result<Instance, Vec<Diagnostic>> {
        let mut diags = Vec::new();
        let mut flat: Vec<(String, Op)> = Vec::new();
        for (name, body) in &self.steps {
            if let Some(expanded) = parse_step(name, body, bindings, &mut diags) {
                flat.extend(expanded);
            }
        }
        if !diags.is_empty() {
            return Err(diags);
        }
        let index: BTreeMap<String, usize> = flat
            .iter()
            .enumerate()
            .map(|(i, (n, _))| (n.clone(), i))
            .collect();
        let names: Vec<String> = flat.iter().map(|(n, _)| n.clone()).collect();
        let reference = |from: &str, field: &str, to: &str, path: String| {
            index.get(to).copied().ok_or_else(|| {
                let maps: Vec<&str> = names
                    .iter()
                    .map(String::as_str)
                    .filter(|n| n.strip_prefix(to).is_some_and(|rest| rest.starts_with('.')))
                    .collect();
                let help = if !maps.is_empty() {
                    format!(
                        "`{to}` is a layer step; name one of its maps: {}",
                        maps.join(", ")
                    )
                } else {
                    match nearest(to, names.iter().map(String::as_str)) {
                        Some(near) => format!("did you mean `{near}`?"),
                        None => format!("steps are: {}", names.join(", ")),
                    }
                };
                Diagnostic::new(
                    code::TEXTURE_GRAPH,
                    format!("{from} `{field}` names `{to}`, which is not a step"),
                )
                .with_path(path)
                .with_help(help)
            })
        };
        let mut steps = Vec::new();
        for (name, op) in flat {
            let doc = document_name(&name).to_string();
            let mut inputs = Vec::new();
            for (field, to) in op.inputs() {
                let path = format!("/steps/{}/{field}", pointer_token(&doc));
                match reference(&format!("step `{doc}`"), &field, to, path) {
                    Ok(i) => inputs.push(i),
                    // A layer's maps share its coats: one bad coat reference reports once.
                    Err(d) => {
                        if !diags.iter().any(|e| e.code == d.code && e.path == d.path) {
                            diags.push(d);
                        }
                    }
                }
            }
            steps.push(Step { name, op, inputs });
        }
        let mut outputs = Vec::new();
        for (key, to) in &self.outputs {
            let path = format!("/outputs/{}", key.name());
            match reference("outputs", key.name(), to, path) {
                Ok(i) => outputs.push((*key, i)),
                Err(d) => diags.push(d),
            }
        }
        if !diags.is_empty() {
            return Err(diags);
        }
        let order = topological_order(&steps).map_err(|cycle| {
            let names: Vec<&str> = cycle.iter().map(|i| steps[*i].document_name()).collect();
            let expanded: Vec<&str> = cycle.iter().map(|i| steps[*i].name.as_str()).collect();
            let help = if expanded == names {
                "break the cycle by pointing one of these steps at an earlier step".to_string()
            } else {
                format!(
                    "break the cycle by pointing one of these steps at an earlier step; between \
                     the layer's maps it runs {}",
                    expanded.join(" → ")
                )
            };
            vec![
                Diagnostic::new(
                    code::TEXTURE_GRAPH,
                    format!("the steps form a cycle: {}", names.join(" → ")),
                )
                .with_path(format!("/steps/{}", pointer_token(names[0])))
                .with_help(help),
            ]
        })?;
        Ok(Instance {
            steps,
            order,
            outputs,
        })
    }
}

/// Dependencies-first order, ties by document order; the error is a cycle `a, b, ..., a`.
fn topological_order(steps: &[Step]) -> Result<Vec<usize>, Vec<usize>> {
    fn visit(
        i: usize,
        steps: &[Step],
        state: &mut [u8],
        path: &mut Vec<usize>,
    ) -> Result<(), Vec<usize>> {
        match state[i] {
            2 => return Ok(()),
            1 => {
                let start = path.iter().position(|p| *p == i).unwrap_or(0);
                let mut cycle = path[start..].to_vec();
                cycle.push(i);
                return Err(cycle);
            }
            _ => {}
        }
        state[i] = 1;
        path.push(i);
        for dep in &steps[i].inputs {
            visit(*dep, steps, state, path)?;
        }
        path.pop();
        state[i] = 2;
        Ok(())
    }
    let mut state = vec![0u8; steps.len()];
    for i in 0..steps.len() {
        visit(i, steps, &mut state, &mut Vec::new())?;
    }
    let mut order = Vec::new();
    let mut emitted = vec![false; steps.len()];
    while order.len() < steps.len() {
        let next = (0..steps.len())
            .find(|i| !emitted[*i] && steps[*i].inputs.iter().all(|d| emitted[*d]))
            .expect("an acyclic graph always has a ready step");
        emitted[next] = true;
        order.push(next);
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "format: 1
kind: texture
params:
  scale: 6
steps:
  b:
    op: levels
    input: a
  a:
    op: noise
    type: value
    scale: $scale
outputs:
  base_color: b
  roughness: b
  height: a
";

    #[test]
    fn orders_dependencies_first() {
        let recipe = parse(GOOD).unwrap();
        let instance = recipe.instantiate(&recipe.defaults()).unwrap();
        assert_eq!(instance.order, vec![1, 0]);
    }

    #[test]
    fn bound_param_is_revalidated() {
        let recipe = parse(GOOD).unwrap();
        let mut bindings = recipe.defaults();
        bindings.insert("scale".into(), ParamValue::Number(0.0));
        let d = recipe.instantiate(&bindings).unwrap_err();
        assert_eq!(d[0].code, code::TEXTURE_DATA);
        assert_eq!(d[0].path.as_deref(), Some("/steps/a/scale"));
    }

    #[test]
    fn cycle_is_named() {
        let text = GOOD.replace("type: value", "type: value\n    # x").replace(
            "  a:\n    op: noise\n    type: value\n    # x\n    scale: $scale",
            "  a:\n    op: levels\n    input: b",
        );
        let d = parse(&text).unwrap_err();
        assert_eq!(d[0].code, code::TEXTURE_GRAPH);
        assert!(d[0].message.contains("b → a → b") || d[0].message.contains("a → b → a"));
    }
}
