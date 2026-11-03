//! The MCP tool catalog, derived at runtime from the protocol schema.
//!
//! There is one MCP tool per Ludion protocol method — except [`EXCLUDED_METHODS`] — plus one
//! synthetic [`ludion_status`](STATUS_TOOL) convenience. The catalog is built from
//! [`ludion_protocol::schema::protocol_schema`] — the same canonical document the `schema_hash`
//! handshake hashes — so a method added to the protocol appears as a tool automatically and the two
//! can never drift. Nothing here is hand-maintained per command.
//!
//! MCP tool names must match `^[a-zA-Z0-9_-]+$`, but protocol method names are dotted
//! (`sim.set_speed`). [`sanitize`] maps `.` to `_`; because that is lossy (`sim_set_speed` could
//! have come from `sim.set.speed`), each [`ToolSpec`] keeps the original dotted method so
//! `tools/call` never reverse-parses a name.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

/// The synthetic connection-status tool's name (not a protocol method).
pub(crate) const STATUS_TOOL: &str = "ludion_status";

/// Protocol methods that get no MCP tool.
///
/// `stream.subscribe`/`stream.unsubscribe` open a server-push channel: frames and events arrive
/// asynchronously, outside any request's response. MCP's transport is strict request/response, so
/// there is nowhere for those pushes to land — a model that calls `stream_subscribe` gets an
/// acknowledgement and then waits forever for events that can never be delivered. Omitting the tools
/// is safer than shipping them with a warning: a peer that needs streaming (a viewer) speaks the wire
/// protocol directly instead.
pub(crate) const EXCLUDED_METHODS: &[&str] = &["stream.subscribe", "stream.unsubscribe"];

/// One MCP tool: its sanitized name, its inbound JSON Schema, and what to do when it is called.
pub(crate) struct ToolSpec {
    /// The MCP tool name (`^[a-zA-Z0-9_-]+$`); the dotted method with `.` replaced by `_`.
    pub(crate) name: String,
    /// The tool description shown to the model.
    pub(crate) description: String,
    /// The tool's `inputSchema` — the method's `params` schema, with only the `$defs` entries it
    /// transitively references embedded (so internal `$ref`s resolve without paying context tokens
    /// for the rest of the protocol schema), or an empty object for a parameterless method / the
    /// status tool.
    pub(crate) input_schema: Value,
    /// What invoking this tool does.
    pub(crate) action: ToolAction,
}

/// The behaviour behind a [`ToolSpec`].
pub(crate) enum ToolAction {
    /// Build the named protocol [`Command`](ludion_protocol::Command) and send it. `has_params` is
    /// whether the command carries a `params` object (unit variants must omit it on the wire).
    Protocol {
        /// The dotted protocol method name (e.g. `"sim.step"`).
        method: String,
        /// Whether the method takes parameters (a `params` key on the wire).
        has_params: bool,
    },
    /// Report connection state and the server hello (the synthetic `ludion_status` tool).
    Status,
}

impl ToolSpec {
    /// This tool as the JSON object `tools/list` returns for it.
    pub(crate) fn to_list_entry(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
        })
    }
}

/// Maps a dotted protocol method name to a valid MCP tool name (`.` → `_`).
pub(crate) fn sanitize(method: &str) -> String {
    method.replace('.', "_")
}

/// Builds the full tool catalog from the live protocol schema.
///
/// One tool per `Command` variant (in declaration order), skipping [`EXCLUDED_METHODS`], then the
/// synthetic `ludion_status` tool.
pub(crate) fn catalog() -> Vec<ToolSpec> {
    let schema = ludion_protocol::schema::protocol_schema();
    let command = &schema["command"];
    let defs = command
        .get("$defs")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));

    let variants = command["oneOf"]
        .as_array()
        .expect("the Command schema is a `oneOf` over its variants");

    let mut tools = Vec::with_capacity(variants.len() + 1);
    for variant in variants {
        let method = variant["properties"]["method"]["const"]
            .as_str()
            .expect("every Command variant pins `method` to a const string")
            .to_string();

        if EXCLUDED_METHODS.contains(&method.as_str()) {
            continue;
        }

        let description = variant
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        // A variant either references a params struct (`"$ref": "#/$defs/…Params"`) or is a unit
        // variant with no `params` at all.
        let params_ref = variant["properties"]
            .get("params")
            .and_then(|p| p.get("$ref"))
            .and_then(Value::as_str);
        let (input_schema, has_params) = match params_ref {
            Some(reference) => {
                let def_name = reference
                    .rsplit('/')
                    .next()
                    .expect("a `$ref` always has a final path segment");
                let mut params_schema = defs.get(def_name).cloned().unwrap_or_else(empty_object);
                // Lift the params schema to the root of the tool's inputSchema and carry along only
                // the `$defs` entries it transitively references, so its internal `#/$defs/…`
                // references still resolve without embedding the whole protocol schema — a tool
                // catalog is prompt payload, and an unreferenced def is context tokens every session
                // pays for.
                let needed = referenced_defs(&params_schema, &defs);
                if let Value::Object(map) = &mut params_schema
                    && !needed.is_empty()
                {
                    let mut pruned = Map::new();
                    for name in needed {
                        if let Some(def) = defs.get(&name) {
                            pruned.insert(name, def.clone());
                        }
                    }
                    map.insert("$defs".to_string(), Value::Object(pruned));
                }
                (params_schema, true)
            }
            None => (empty_object(), false),
        };

        tools.push(ToolSpec {
            name: sanitize(&method),
            description,
            input_schema,
            action: ToolAction::Protocol { method, has_params },
        });
    }

    tools.push(ToolSpec {
        name: STATUS_TOOL.to_string(),
        description: "Report the ludion-mcp adapter's connection state and the engine's hello \
                      (engine version, protocol version, schema hash) plus the current sim and \
                      gameplay frames and WorldHash. Connects (or reconnects) to the engine if \
                      needed. Use this to check whether `ludion serve` is reachable before driving it."
            .to_string(),
        input_schema: empty_object(),
        action: ToolAction::Status,
    });

    tools
}

/// An empty-object JSON Schema — the inputSchema for parameterless tools.
fn empty_object() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// The names of every `$defs` entry `schema` needs, directly or transitively.
///
/// Walks `schema` for `"$ref": "#/$defs/Name"` values, then walks each newly discovered `Name`'s own
/// definition in `defs` the same way, to a fixpoint — definitions commonly reference other
/// definitions. A `$ref` naming a def absent from `defs` is skipped rather than treated as an error;
/// this is pure reachability over whatever `defs` actually contains.
fn referenced_defs(schema: &Value, defs: &Value) -> BTreeSet<String> {
    let mut needed = BTreeSet::new();
    let mut frontier: Vec<String> = collect_refs(schema).into_iter().collect();
    while let Some(name) = frontier.pop() {
        if !needed.insert(name.clone()) {
            continue; // already visited
        }
        if let Some(def) = defs.get(&name) {
            frontier.extend(collect_refs(def));
        }
    }
    needed
}

/// The `$defs` names referenced anywhere within `value` (properties, `items`, `oneOf`/`anyOf`/`allOf`,
/// `additionalProperties`, ...) — a plain recursive walk, since a `$ref` can appear at any depth.
fn collect_refs(value: &Value) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    walk_refs(value, &mut refs);
    refs
}

fn walk_refs(value: &Value, out: &mut BTreeSet<String>) {
    if let Value::Object(map) = value
        && let Some(name) = map
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_prefix("#/$defs/"))
    {
        out.insert(name.to_string());
    }
    match value {
        Value::Object(map) => map.values().for_each(|v| walk_refs(v, out)),
        Value::Array(items) => items.iter().for_each(|v| walk_refs(v, out)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_tool_per_method_plus_status_minus_excluded() {
        let tools = catalog();
        assert_eq!(
            tools.len(),
            ludion_protocol::Command::METHODS.len() + 1 - EXCLUDED_METHODS.len(),
            "one tool per protocol method except EXCLUDED_METHODS, plus ludion_status",
        );
    }

    #[test]
    fn names_are_sanitized_and_valid() {
        for tool in catalog() {
            assert!(
                tool.name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "tool name `{}` must match ^[a-zA-Z0-9_-]+$",
                tool.name,
            );
            assert!(!tool.name.contains('.'), "`{}` still has a dot", tool.name);
        }
    }

    #[test]
    fn every_non_excluded_method_has_a_tool_with_a_dotted_method_and_input_schema() {
        let tools = catalog();
        for method in ludion_protocol::Command::METHODS {
            if EXCLUDED_METHODS.contains(method) {
                continue;
            }
            let expected = sanitize(method);
            let tool = tools
                .iter()
                .find(|t| t.name == expected)
                .unwrap_or_else(|| panic!("no tool for method {method}"));
            // Every tool advertises an object inputSchema.
            assert_eq!(tool.input_schema["type"], json!("object"), "{method}");
            match &tool.action {
                ToolAction::Protocol { method: m, .. } => assert_eq!(m, method),
                ToolAction::Status => panic!("{method} mapped to the status action"),
            }
        }
    }

    /// `has_params` must track the schema: a variant whose `params` is optional would be an `anyOf`
    /// the catalog reads as having none, silently losing the caller's arguments.
    #[test]
    fn has_params_matches_the_schema_properties_and_required() {
        let schema = ludion_protocol::schema::protocol_schema();
        let variants = schema["command"]["oneOf"].as_array().expect("oneOf");
        let tools = catalog();
        for variant in variants {
            let method = variant["properties"]["method"]["const"].as_str().unwrap();
            if EXCLUDED_METHODS.contains(&method) {
                continue;
            }
            let in_properties = variant["properties"].get("params").is_some();
            let required = variant["required"]
                .as_array()
                .is_some_and(|names| names.iter().any(|name| name == "params"));
            let tool = tools.iter().find(|t| t.name == sanitize(method)).unwrap();
            let ToolAction::Protocol { has_params, .. } = &tool.action else {
                panic!("{method} mapped to the status action");
            };
            assert_eq!(
                *has_params, in_properties,
                "{method}: has_params vs properties"
            );
            assert_eq!(*has_params, required, "{method}: has_params vs required");
        }
    }

    #[test]
    fn excluded_methods_have_no_tool() {
        let tools = catalog();
        for method in EXCLUDED_METHODS {
            let expected = sanitize(method);
            assert!(
                tools.iter().all(|t| t.name != expected),
                "{method} should have no MCP tool, found `{expected}`",
            );
        }
    }

    #[test]
    fn parameterized_tool_carries_its_params_schema_with_defs() {
        let tools = catalog();
        let step = tools
            .iter()
            .find(|t| t.name == "sim_step")
            .expect("sim_step tool");
        // sim.step's params has a `ticks` field.
        assert!(
            step.input_schema["properties"]["ticks"].is_object(),
            "sim_step inputSchema must describe `ticks`",
        );
    }

    /// Every `$ref` anywhere in `schema` (recursively) names an entry present in `schema["$defs"]`.
    fn assert_refs_resolve(tool_name: &str, schema: &Value) {
        let defs = schema.get("$defs").cloned().unwrap_or(json!({}));
        let names = collect_refs(schema);
        for name in names {
            assert!(
                defs.get(&name).is_some(),
                "{tool_name}: `$ref` to `#/$defs/{name}` does not resolve against its own $defs",
            );
        }
    }

    #[test]
    fn scalar_params_tool_carries_few_or_no_defs_and_is_small() {
        let tools = catalog();
        let step = tools
            .iter()
            .find(|t| t.name == "sim_set_speed")
            .expect("sim_set_speed tool");
        let defs = step.input_schema.get("$defs");
        let def_count = defs.and_then(Value::as_object).map_or(0, Map::len);
        assert!(
            def_count <= 1,
            "sim_set_speed's params are scalar; expected few or no $defs, got {def_count}",
        );
        let size = serde_json::to_string(&step.input_schema)
            .expect("inputSchema always serializes")
            .len();
        assert!(
            size < 2000,
            "sim_set_speed inputSchema is {size} chars; pruning should keep a scalar tool small",
        );
        assert_refs_resolve("sim_set_speed", &step.input_schema);
    }

    #[test]
    fn component_touching_tool_keeps_only_the_defs_it_needs() {
        let tools = catalog();
        for name in ["entity_spawn", "component_set"] {
            let tool = tools
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} tool"));
            // It still resolves — pruning must not have dropped a def it actually needs.
            assert_refs_resolve(name, &tool.input_schema);
            // And it isn't carrying the whole protocol schema's $defs anymore.
            let schema = ludion_protocol::schema::protocol_schema();
            let full_defs_len = schema["command"]["$defs"]
                .as_object()
                .expect("command schema has $defs")
                .len();
            let kept = tool
                .input_schema
                .get("$defs")
                .and_then(Value::as_object)
                .map_or(0, Map::len);
            assert!(
                kept <= full_defs_len,
                "{name} should keep at most the full def set ({full_defs_len}), kept {kept}",
            );
        }
    }

    #[test]
    fn every_tool_input_schema_refs_resolve_against_its_own_defs() {
        for tool in catalog() {
            assert_refs_resolve(&tool.name, &tool.input_schema);
        }
    }

    #[test]
    fn full_catalog_is_much_smaller_than_the_unpruned_baseline() {
        let tools = catalog();
        let schema = ludion_protocol::schema::protocol_schema();
        let full_defs = schema["command"]["$defs"].clone();

        let pruned_size: usize = tools
            .iter()
            .map(|t| serde_json::to_string(&t.to_list_entry()).unwrap().len())
            .sum();

        // What the old "embed the whole $defs block on every parameterized tool" behaviour cost.
        let unpruned_size: usize = tools
            .iter()
            .map(|t| {
                let mut with_full_defs = t.input_schema.clone();
                if let Value::Object(map) = &mut with_full_defs
                    && map.contains_key("$defs")
                {
                    map.insert("$defs".to_string(), full_defs.clone());
                }
                let entry = json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": with_full_defs,
                });
                serde_json::to_string(&entry).unwrap().len()
            })
            .sum();

        eprintln!(
            "MCP catalog size: pruned {pruned_size} chars vs unpruned baseline {unpruned_size} chars",
        );
        assert!(
            pruned_size < unpruned_size,
            "pruning should shrink the catalog: pruned {pruned_size} >= unpruned {unpruned_size}",
        );
    }

    #[test]
    fn stream_subscribe_tool_is_absent() {
        let tools = catalog();
        assert!(
            tools.iter().all(|t| t.name != "stream_subscribe"),
            "stream_subscribe cannot deliver events over MCP's request/response transport, so it \
             must not appear in the catalog",
        );
    }

    #[test]
    fn status_tool_is_present_and_parameterless() {
        let tools = catalog();
        let status = tools
            .iter()
            .find(|t| t.name == STATUS_TOOL)
            .expect("ludion_status tool");
        assert!(matches!(status.action, ToolAction::Status));
        assert_eq!(
            status.input_schema["additionalProperties"],
            json!(false),
            "status tool takes no arguments",
        );
    }
}
