//! Paired-action documents: `kind: paired` — two or more voxel-assembly roles that align and play
//! one clip each, with named events and the part contacts the choreography must reach
//! (`Plan/paired-animations.md` decisions 15–21).
//!
//! ```yaml
//! format: 1
//! kind: paired
//! name: choke
//! roles:
//!   instigator:
//!     assembly: humanoid.assembly.yaml
//!     clip: carry
//!   target:
//!     assembly: humanoid.assembly.yaml
//!     clip: idle
//! align:
//!   reference: instigator
//!   moves: instigator
//!   ticks: 12
//!   offsets:
//!     target:
//!       translation:
//!         - 0.0
//!         - 0.0
//!         - -0.6
//!       yaw_deg: 180.0
//! events:
//!   - name: grab
//!     t: 0.25
//! contacts:
//!   - a: instigator/lower-arm-r
//!     b: target/head
//!     at: grab
//!     max_gap: 1.0
//! ```
//!
//! `align.offsets.<role>` places a role in the reference role's frame (−Z forward); `moves` names the
//! role that closes the distance, or `split`. A role is a clip role (`clip`) or `align_only: true`.
//! A role's `scale` is its entity's uniform `Transform` scale (default 1).
//!
//! **This module stays IO-free** and checks the file alone: structure (`L1752`) and data consistency
//! (`L1753`). Checking against the role assemblies is the engine's `check_paired_files` (`L1754`).

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::number::fits_f32;
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::fields::parse_number;
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, MapEntry, Node, NodeValue};

/// The paired document format version this crate reads and writes.
pub const PAIRED_FORMAT_VERSION: u64 = 1;

const PAIRED_KEYS: [&str; 7] = [
    "format", "kind", "name", "roles", "align", "events", "contacts",
];
const ROLE_KEYS: [&str; 5] = ["assembly", "clip", "align_only", "posture", "scale"];
const ALIGN_KEYS: [&str; 4] = ["reference", "moves", "ticks", "offsets"];
const OFFSET_KEYS: [&str; 2] = ["translation", "yaw_deg"];
const EVENT_KEYS: [&str; 2] = ["name", "t"];
const CONTACT_KEYS: [&str; 4] = ["a", "b", "at", "max_gap"];

const STRUCTURE: DiagnosticCode = code::PAIRED_DOCUMENT_STRUCTURE;
const DATA: DiagnosticCode = code::PAIRED_DATA_CONSISTENCY;

/// One participant: an assembly and the clip it plays, or an assembly that only aligns.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedRole {
    /// The role name, such as `instigator`.
    pub name: String,
    /// The assembly document, relative to the paired document's directory.
    pub assembly: String,
    /// The assembly clip this role plays; `None` for an `align_only` role.
    pub clip: Option<String>,
    /// A role that aligns but plays no clip (a search spot, a prop).
    pub align_only: bool,
    /// The posture (`Plan/posture.md`) the role must be in, as a free name; `None` for any.
    pub posture: Option<String>,
    /// The role entity's uniform `Transform` scale; positive, 1 by default.
    pub scale: f64,
}

/// Which role closes the distance during the warp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairedMoves {
    /// Only the named role moves.
    Role(String),
    /// Every non-reference role and the reference meet in the middle.
    Split,
}

/// Where a non-reference role ends up, in the reference role's frame.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedOffset {
    /// The role this offset places.
    pub role: String,
    /// The position in metres, in the reference entity's frame (−Z forward).
    pub translation: [f64; 3],
    /// The yaw in degrees, relative to the reference entity.
    pub yaw_deg: f64,
}

/// The align (warp) block.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedAlign {
    /// The role every offset is relative to.
    pub reference: String,
    /// Who closes the distance.
    pub moves: PairedMoves,
    /// The warp length in fixed ticks; at least 1.
    pub ticks: u64,
    /// One offset per non-reference role.
    pub offsets: Vec<PairedOffset>,
}

/// A named moment of the action, in gameplay seconds since it started at speed 1.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedEvent {
    /// The event name; W5's runtime fires it as `paired:<name>`.
    pub name: String,
    /// Seconds since the action started.
    pub t: f64,
}

/// A part pair that must be close at an event.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedContact {
    /// The first side as `(role, part)`.
    pub a: (String, String),
    /// The second side as `(role, part)`.
    pub b: (String, String),
    /// The event name at whose time the gap is measured.
    pub at: String,
    /// The largest accepted gap, in the first role's cells.
    pub max_gap: f64,
}

/// A parsed, structurally valid paired document.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedDocument {
    /// The action name; events fire as `paired:<name>`.
    pub name: String,
    /// The participants, in authored order (the canonical writer sorts them).
    pub roles: Vec<PairedRole>,
    /// The align block.
    pub align: PairedAlign,
    /// The named moments, ascending in `t`.
    pub events: Vec<PairedEvent>,
    /// The part contacts to verify.
    pub contacts: Vec<PairedContact>,
}

// --- Parsing (`L1752`) --------------------------------------------------------------------------

fn structure(pointer: &str, message: String, help: &str) -> Diagnostic {
    Diagnostic::new(STRUCTURE, message)
        .with_path(pointer.to_string())
        .with_help(help.to_string())
}

fn expect_mapping<'a>(
    node: &'a Node,
    pointer: &str,
    what: &str,
    help: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a Vec<MapEntry>> {
    match &node.value {
        NodeValue::Mapping(entries) => Some(entries),
        other => {
            diagnostics.push(structure(
                pointer,
                format!(
                    "{what} must be a mapping, found {} (line {})",
                    other.type_phrase(),
                    node.line
                ),
                help,
            ));
            None
        }
    }
}

fn expect_sequence<'a>(
    node: &'a Node,
    pointer: &str,
    what: &str,
    help: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a Vec<Node>> {
    match &node.value {
        NodeValue::Sequence(items) => Some(items),
        other => {
            diagnostics.push(structure(
                pointer,
                format!(
                    "{what} must be a sequence, found {} (line {})",
                    other.type_phrase(),
                    node.line
                ),
                help,
            ));
            None
        }
    }
}

fn missing(pointer: &str, line: usize, what: &str, key: &str, help: &str) -> Diagnostic {
    structure(
        pointer,
        format!("{what} is missing the required key `{key}` (line {line})"),
        help,
    )
}

fn string_field(node: &Node, pointer: &str, what: &str, d: &mut Vec<Diagnostic>) -> Option<String> {
    envelope::expect_string(node, pointer, what, STRUCTURE, d)
}

fn number_field(node: &Node, pointer: &str, what: &str, d: &mut Vec<Diagnostic>) -> Option<f64> {
    parse_number(node, pointer, what, STRUCTURE, d)
}

/// Parses strict-subset YAML into a [`PairedDocument`], checking structure/shape only (`L1752`).
pub(crate) fn parse(source: &str) -> Result<PairedDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![structure(
            "",
            "the paired document is empty; a paired action needs `format: 1`, `kind: paired`, \
                 a `name`, `roles` and `align`"
                .to_string(),
            "see `ludion docs paired-actions` for a complete example",
        )]);
    };
    let NodeValue::Mapping(entries) = &root.value else {
        return Err(vec![structure(
            "",
            format!(
                "the paired document root must be a mapping, found {} (line {})",
                root.value.type_phrase(),
                root.line
            ),
            "start the file with the top-level keys `format:` and `kind: paired`",
        )]);
    };

    let mut diagnostics = Vec::new();
    let (mut format_seen, mut kind_seen) = (false, false);
    let (mut name, mut roles, mut align) = (None, None, None);
    let (mut events, mut contacts) = (Vec::new(), Vec::new());

    for entry in entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    STRUCTURE,
                    STRUCTURE,
                    PAIRED_FORMAT_VERSION,
                    "the paired document",
                    "paired",
                    "paired",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    STRUCTURE,
                    "paired",
                    "a paired document",
                    &mut diagnostics,
                );
            }
            "name" => {
                name = string_field(&entry.node, "/name", "the action name", &mut diagnostics);
            }
            "roles" => roles = parse_roles(&entry.node, &mut diagnostics),
            "align" => align = parse_align(&entry.node, &mut diagnostics),
            "events" => {
                if let Some(parsed) = parse_events(&entry.node, &mut diagnostics) {
                    events = parsed;
                }
            }
            "contacts" => {
                if let Some(parsed) = parse_contacts(&entry.node, &mut diagnostics) {
                    contacts = parsed;
                }
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a paired document",
                PAIRED_KEYS,
                STRUCTURE,
            )),
        }
    }

    let top =
        |key: &str, help: &str| envelope::missing_key(key, help, STRUCTURE, "paired document");
    if !format_seen {
        diagnostics.push(top("format", "add `format: 1` as the first line"));
    }
    if !kind_seen {
        diagnostics.push(top("kind", "add `kind: paired`"));
    }
    let present = |key: &str| entries.iter().any(|entry| entry.key == key);
    if !present("name") {
        diagnostics.push(top(
            "name",
            "add `name:`; the runtime's events fire as `paired:<name>`",
        ));
    }
    if !present("roles") {
        diagnostics.push(top("roles", "add `roles:` with two or more named roles"));
    }
    if !present("align") {
        diagnostics.push(top(
            "align",
            "add `align:` with `reference`, `moves`, `ticks` and `offsets`",
        ));
    }

    match (diagnostics.is_empty(), name, roles, align) {
        (true, Some(name), Some(roles), Some(align)) => Ok(PairedDocument {
            name,
            roles,
            align,
            events,
            contacts,
        }),
        _ => Err(diagnostics),
    }
}

fn parse_roles(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<Vec<PairedRole>> {
    let entries = expect_mapping(
        node,
        "/roles",
        "`roles`",
        "write `roles` as a mapping of role name to `assembly:` / `clip:`",
        diagnostics,
    )?;
    let mut roles = Vec::with_capacity(entries.len());
    let mut ok = true;
    for entry in entries {
        match parse_role(entry, diagnostics) {
            Some(role) => roles.push(role),
            None => ok = false,
        }
    }
    ok.then_some(roles)
}

fn parse_role(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<PairedRole> {
    let pointer = format!("/roles/{}", entry.key);
    let fields = expect_mapping(
        &entry.node,
        &pointer,
        "a role",
        "write a role as `assembly:` plus a `clip:` or `align_only: true`",
        diagnostics,
    )?;
    let mut role = PairedRole {
        name: entry.key.clone(),
        assembly: String::new(),
        clip: None,
        align_only: false,
        posture: None,
        scale: 1.0,
    };
    let mut assembly_seen = false;
    let mut ok = true;
    for field in fields {
        let child = format!("{pointer}/{}", field.key);
        match field.key.as_str() {
            "assembly" => {
                assembly_seen = true;
                match string_field(&field.node, &child, "`assembly`", diagnostics) {
                    Some(value) => role.assembly = value,
                    None => ok = false,
                }
            }
            "clip" => match string_field(&field.node, &child, "`clip`", diagnostics) {
                Some(value) => role.clip = Some(value),
                None => ok = false,
            },
            "posture" => match string_field(&field.node, &child, "`posture`", diagnostics) {
                Some(value) => role.posture = Some(value),
                None => ok = false,
            },
            "align_only" => match &field.node.value {
                NodeValue::Scalar(Value::Bool(flag)) => role.align_only = *flag,
                other => {
                    ok = false;
                    diagnostics.push(structure(
                        &child,
                        format!(
                            "`align_only` must be true or false, found {} (line {})",
                            other.type_phrase(),
                            field.node.line
                        ),
                        "write `align_only: true`, or delete the key",
                    ));
                }
            },
            "scale" => match number_field(&field.node, &child, "`scale`", diagnostics) {
                Some(value) => role.scale = value,
                None => ok = false,
            },
            unknown => {
                ok = false;
                diagnostics.push(envelope::unknown_key(
                    unknown, field, &pointer, "a role", ROLE_KEYS, STRUCTURE,
                ));
            }
        }
    }
    if !assembly_seen {
        ok = false;
        diagnostics.push(missing(
            &pointer,
            entry.node.line,
            "a role",
            "assembly",
            "add `assembly:` naming the role's `.assembly.yaml`, relative to this file",
        ));
    }
    ok.then_some(role)
}

fn parse_align(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<PairedAlign> {
    let entries = expect_mapping(
        node,
        "/align",
        "`align`",
        "write `align` with `reference`, `moves`, `ticks` and `offsets`",
        diagnostics,
    )?;
    let (mut reference, mut moves, mut ticks, mut offsets) = (None, None, None, None);
    let mut ok = true;
    for entry in entries {
        let child = format!("/align/{}", entry.key);
        match entry.key.as_str() {
            "reference" => {
                reference = string_field(&entry.node, &child, "`reference`", diagnostics);
                ok &= reference.is_some();
            }
            "moves" => {
                moves = string_field(&entry.node, &child, "`moves`", diagnostics).map(|value| {
                    if value == "split" {
                        PairedMoves::Split
                    } else {
                        PairedMoves::Role(value)
                    }
                });
                ok &= moves.is_some();
            }
            "ticks" => match &entry.node.value {
                NodeValue::Scalar(Value::Number(n)) if n.as_u64().is_some() => {
                    ticks = n.as_u64();
                }
                other => {
                    ok = false;
                    diagnostics.push(structure(
                        &child,
                        format!(
                            "`ticks` must be a whole number, found {} (line {})",
                            other.type_phrase(),
                            entry.node.line
                        ),
                        "write `ticks: 12`, the warp length in fixed ticks",
                    ));
                }
            },
            "offsets" => {
                offsets = parse_offsets(&entry.node, diagnostics);
                ok &= offsets.is_some();
            }
            unknown => {
                ok = false;
                diagnostics.push(envelope::unknown_key(
                    unknown, entry, "/align", "`align`", ALIGN_KEYS, STRUCTURE,
                ));
            }
        }
    }
    for (key, present, help) in [
        (
            "reference",
            reference.is_some(),
            "name the role every offset is relative to",
        ),
        (
            "moves",
            moves.is_some(),
            "name the role that moves, or `split`",
        ),
        (
            "ticks",
            ticks.is_some(),
            "give the warp length, such as `ticks: 12`",
        ),
        (
            "offsets",
            offsets.is_some(),
            "give each non-reference role an offset",
        ),
    ] {
        if !present && !entries.iter().any(|entry| entry.key == key) {
            ok = false;
            diagnostics.push(missing("/align", node.line, "`align`", key, help));
        }
    }
    match (ok, reference, moves, ticks, offsets) {
        (true, Some(reference), Some(moves), Some(ticks), Some(offsets)) => Some(PairedAlign {
            reference,
            moves,
            ticks,
            offsets,
        }),
        _ => None,
    }
}

fn parse_offsets(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<Vec<PairedOffset>> {
    let entries = expect_mapping(
        node,
        "/align/offsets",
        "`offsets`",
        "write `offsets` as a mapping of role name to `translation:` / `yaw_deg:`",
        diagnostics,
    )?;
    let mut offsets = Vec::with_capacity(entries.len());
    let mut ok = true;
    for entry in entries {
        let pointer = format!("/align/offsets/{}", entry.key);
        let Some(fields) = expect_mapping(
            &entry.node,
            &pointer,
            "an offset",
            "write an offset as `translation:` (three numbers) and optional `yaw_deg:`",
            diagnostics,
        ) else {
            ok = false;
            continue;
        };
        let mut offset = PairedOffset {
            role: entry.key.clone(),
            translation: [0.0; 3],
            yaw_deg: 0.0,
        };
        let mut translation_seen = false;
        let mut entry_ok = true;
        for field in fields {
            let child = format!("{pointer}/{}", field.key);
            match field.key.as_str() {
                "translation" => {
                    translation_seen = true;
                    match parse_vec3(&field.node, &child, diagnostics) {
                        Some(value) => offset.translation = value,
                        None => entry_ok = false,
                    }
                }
                "yaw_deg" => match number_field(&field.node, &child, "`yaw_deg`", diagnostics) {
                    Some(value) => offset.yaw_deg = value,
                    None => entry_ok = false,
                },
                unknown => {
                    entry_ok = false;
                    diagnostics.push(envelope::unknown_key(
                        unknown,
                        field,
                        &pointer,
                        "an offset",
                        OFFSET_KEYS,
                        STRUCTURE,
                    ));
                }
            }
        }
        if !translation_seen {
            entry_ok = false;
            diagnostics.push(missing(
                &pointer,
                entry.node.line,
                "an offset",
                "translation",
                "add `translation:` with three numbers, one per line",
            ));
        }
        if entry_ok {
            offsets.push(offset);
        } else {
            ok = false;
        }
    }
    ok.then_some(offsets)
}

fn parse_vec3(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[f64; 3]> {
    let help = "write `translation` as three numbers, one per line";
    let items = expect_sequence(node, pointer, "`translation`", help, diagnostics)?;
    if items.len() != 3 {
        diagnostics.push(structure(
            pointer,
            format!(
                "`translation` must have exactly 3 components, found {} (line {})",
                items.len(),
                node.line
            ),
            help,
        ));
        return None;
    }
    let mut out = [0.0; 3];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match number_field(
            item,
            &format!("{pointer}/{index}"),
            "`translation`",
            diagnostics,
        ) {
            Some(value) => out[index] = value,
            None => ok = false,
        }
    }
    ok.then_some(out)
}

fn parse_events(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<Vec<PairedEvent>> {
    let items = expect_sequence(
        node,
        "/events",
        "`events`",
        "write `events` as a list of `- name:` / `t:` entries",
        diagnostics,
    )?;
    let mut events = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        let pointer = format!("/events/{index}");
        let Some(fields) = expect_mapping(
            item,
            &pointer,
            "an event",
            "write each event as `- name: grab` with `t: 0.25` beneath it",
            diagnostics,
        ) else {
            ok = false;
            continue;
        };
        let (mut name, mut t) = (None, None);
        let mut item_ok = true;
        for field in fields {
            let child = format!("{pointer}/{}", field.key);
            match field.key.as_str() {
                "name" => {
                    name = string_field(&field.node, &child, "`name`", diagnostics);
                    item_ok &= name.is_some();
                }
                "t" => {
                    t = number_field(&field.node, &child, "`t`", diagnostics);
                    item_ok &= t.is_some();
                }
                unknown => {
                    item_ok = false;
                    diagnostics.push(envelope::unknown_key(
                        unknown, field, &pointer, "an event", EVENT_KEYS, STRUCTURE,
                    ));
                }
            }
        }
        for (key, present) in [("name", name.is_some()), ("t", t.is_some())] {
            if !present && item_ok {
                item_ok = false;
                diagnostics.push(missing(
                    &pointer,
                    item.line,
                    "an event",
                    key,
                    "an event needs both `name` and `t`",
                ));
            }
        }
        match (item_ok, name, t) {
            (true, Some(name), Some(t)) => events.push(PairedEvent { name, t }),
            _ => ok = false,
        }
    }
    ok.then_some(events)
}

fn parse_contacts(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<Vec<PairedContact>> {
    let items = expect_sequence(
        node,
        "/contacts",
        "`contacts`",
        "write `contacts` as a list of `- a:` / `b:` / `at:` / `max_gap:` entries",
        diagnostics,
    )?;
    let mut contacts = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        let pointer = format!("/contacts/{index}");
        let Some(fields) = expect_mapping(
            item,
            &pointer,
            "a contact",
            "write each contact as `- a: role/part` with `b:`, `at:` and `max_gap:` beneath it",
            diagnostics,
        ) else {
            ok = false;
            continue;
        };
        let (mut a, mut b, mut at, mut max_gap) = (None, None, None, None);
        let mut item_ok = true;
        for field in fields {
            let child = format!("{pointer}/{}", field.key);
            match field.key.as_str() {
                "a" => {
                    a = string_field(&field.node, &child, "`a`", diagnostics);
                    item_ok &= a.is_some();
                }
                "b" => {
                    b = string_field(&field.node, &child, "`b`", diagnostics);
                    item_ok &= b.is_some();
                }
                "at" => {
                    at = string_field(&field.node, &child, "`at`", diagnostics);
                    item_ok &= at.is_some();
                }
                "max_gap" => {
                    max_gap = number_field(&field.node, &child, "`max_gap`", diagnostics);
                    item_ok &= max_gap.is_some();
                }
                unknown => {
                    item_ok = false;
                    diagnostics.push(envelope::unknown_key(
                        unknown,
                        field,
                        &pointer,
                        "a contact",
                        CONTACT_KEYS,
                        STRUCTURE,
                    ));
                }
            }
        }
        for (key, present) in [
            ("a", a.is_some()),
            ("b", b.is_some()),
            ("at", at.is_some()),
            ("max_gap", max_gap.is_some()),
        ] {
            if !present && item_ok {
                item_ok = false;
                diagnostics.push(missing(
                    &pointer,
                    item.line,
                    "a contact",
                    key,
                    "a contact needs `a`, `b`, `at` and `max_gap`",
                ));
            }
        }
        match (item_ok, a, b, at, max_gap) {
            (true, Some(a), Some(b), Some(at), Some(max_gap)) => contacts.push(PairedContact {
                a: split_side(&a),
                b: split_side(&b),
                at,
                max_gap,
            }),
            _ => ok = false,
        }
    }
    ok.then_some(contacts)
}

/// Splits `role/part` at the first `/`; a side without one keeps an empty part, which validation
/// reports.
fn split_side(side: &str) -> (String, String) {
    match side.split_once('/') {
        Some((role, part)) => (role.to_string(), part.to_string()),
        None => (side.to_string(), String::new()),
    }
}

// --- Validation (`L1753`) -----------------------------------------------------------------------

fn data(pointer: &str, message: String, help: &str) -> Diagnostic {
    Diagnostic::new(DATA, message)
        .with_path(pointer.to_string())
        .with_help(help.to_string())
}

/// Reports a non-finite or out-of-f32-range `value`; returns whether it was fine.
fn number_ok(value: f64, pointer: &str, what: &str, diagnostics: &mut Vec<Diagnostic>) -> bool {
    if value.is_finite() && fits_f32(value) {
        return true;
    }
    diagnostics.push(data(
        pointer,
        format!("{what} is {value}, which is not a finite f32-range number"),
        "keep every number finite and below 3.4e38 in magnitude",
    ));
    false
}

fn nonempty(value: &str, pointer: &str, what: &str, diagnostics: &mut Vec<Diagnostic>) {
    if value.is_empty() {
        diagnostics.push(data(
            pointer,
            format!("{what} is empty"),
            "name it, or delete the key",
        ));
    }
}

fn unknown_role_help(name: &str, roles: &[PairedRole]) -> String {
    match suggest::nearest(name, roles.iter().map(|role| role.name.as_str())) {
        Some(candidate) => format!("nearest role is `{candidate}`"),
        None => {
            let names: Vec<&str> = roles.iter().map(|role| role.name.as_str()).collect();
            format!("the roles are: {}", names.join(", "))
        }
    }
}

/// Validates the data-consistency rules of an already-parsed [`PairedDocument`] (`L1753`).
pub(crate) fn validate(doc: &PairedDocument) -> Vec<Diagnostic> {
    let mut d = Vec::new();
    nonempty(&doc.name, "/name", "the action name", &mut d);
    if doc.roles.len() < 2 {
        d.push(data(
            "/roles",
            format!(
                "a paired action needs at least 2 roles, found {}",
                doc.roles.len()
            ),
            "add a second role",
        ));
    }
    let has_role = |name: &str| doc.roles.iter().any(|role| role.name == name);
    validate_roles(doc, &mut d);
    validate_align(doc, has_role, &mut d);
    validate_events(doc, &mut d);
    validate_contacts(doc, has_role, &mut d);
    d
}

fn validate_roles(doc: &PairedDocument, d: &mut Vec<Diagnostic>) {
    for role in &doc.roles {
        let base = format!("/roles/{}", role.name);
        nonempty(&role.name, &base, "a role name", d);
        nonempty(&role.assembly, &format!("{base}/assembly"), "`assembly`", d);
        if let Some(clip) = &role.clip {
            nonempty(clip, &format!("{base}/clip"), "`clip`", d);
        }
        if let Some(posture) = &role.posture {
            nonempty(posture, &format!("{base}/posture"), "`posture`", d);
        }
        if number_ok(role.scale, &format!("{base}/scale"), "`scale`", d) && role.scale <= 0.0 {
            d.push(data(
                &format!("{base}/scale"),
                format!("`scale` is {}, but a scale has to be above 0", role.scale),
                "use the role entity's uniform `Transform` scale, such as 1.0",
            ));
        }
        match (&role.clip, role.align_only) {
            (Some(_), true) => d.push(data(
                &base,
                format!(
                    "role `{}` has both a `clip` and `align_only: true`",
                    role.name
                ),
                "an align-only role plays no clip: delete `clip` or `align_only`",
            )),
            (None, false) => d.push(data(
                &base,
                format!(
                    "role `{}` has neither a `clip` nor `align_only: true`",
                    role.name
                ),
                "add `clip:` naming one of the assembly's clips, or `align_only: true`",
            )),
            _ => {}
        }
    }
}

fn validate_align(doc: &PairedDocument, has_role: impl Fn(&str) -> bool, d: &mut Vec<Diagnostic>) {
    let align = &doc.align;
    if !has_role(&align.reference) {
        d.push(data(
            "/align/reference",
            format!(
                "`reference` names `{}`, which is not a role",
                align.reference
            ),
            &unknown_role_help(&align.reference, &doc.roles),
        ));
    }
    if let PairedMoves::Role(mover) = &align.moves
        && !has_role(mover)
    {
        d.push(data(
            "/align/moves",
            format!("`moves` names `{mover}`, which is not a role"),
            &format!("{}, or write `split`", unknown_role_help(mover, &doc.roles)),
        ));
    }
    if align.ticks == 0 {
        d.push(data(
            "/align/ticks",
            "`ticks` is 0, but the warp needs at least 1 tick".to_string(),
            "use a positive number of fixed ticks",
        ));
    }
    for offset in &align.offsets {
        let base = format!("/align/offsets/{}", offset.role);
        if offset.role == align.reference {
            d.push(data(
                &base,
                format!("`{}` is the reference, so it has no offset", offset.role),
                "delete this offset; the reference sits at the origin of its own frame",
            ));
        } else if !has_role(&offset.role) {
            d.push(data(
                &base,
                format!("an offset names `{}`, which is not a role", offset.role),
                &unknown_role_help(&offset.role, &doc.roles),
            ));
        }
        for (index, value) in offset.translation.iter().enumerate() {
            number_ok(
                *value,
                &format!("{base}/translation/{index}"),
                "a translation component",
                d,
            );
        }
        number_ok(offset.yaw_deg, &format!("{base}/yaw_deg"), "`yaw_deg`", d);
    }
    for role in &doc.roles {
        if role.name != align.reference && !align.offsets.iter().any(|o| o.role == role.name) {
            d.push(data(
                "/align/offsets",
                format!("role `{}` has no offset in `align.offsets`", role.name),
                "add an entry with `translation:` and optional `yaw_deg:`",
            ));
        }
    }
}

fn validate_events(doc: &PairedDocument, d: &mut Vec<Diagnostic>) {
    let mut previous: Option<f64> = None;
    for (index, event) in doc.events.iter().enumerate() {
        let base = format!("/events/{index}");
        nonempty(&event.name, &format!("{base}/name"), "an event name", d);
        if doc.events[..index]
            .iter()
            .any(|earlier| earlier.name == event.name)
        {
            d.push(data(
                &format!("{base}/name"),
                format!("event `{}` appears twice", event.name),
                "give each event a unique name",
            ));
        }
        if event.name == "end" {
            d.push(data(
                &format!("{base}/name"),
                "`end` is the implicit event fired when the action ends; rename it".to_owned(),
                "rename the event, for example `release`",
            ));
        }
        if !number_ok(event.t, &format!("{base}/t"), "`t`", d) {
            continue;
        }
        if event.t < 0.0 {
            d.push(data(
                &format!("{base}/t"),
                format!("`t` is {}, but an event time cannot be negative", event.t),
                "use 0 or a positive number of seconds",
            ));
        } else if previous.is_some_and(|before| event.t <= before) {
            d.push(data(
                &format!("{base}/t"),
                format!(
                    "event `{}` at {} does not come after the one before it",
                    event.name, event.t
                ),
                "list events in strictly ascending `t`",
            ));
        }
        previous = Some(event.t);
    }
}

fn validate_contacts(
    doc: &PairedDocument,
    has_role: impl Fn(&str) -> bool,
    d: &mut Vec<Diagnostic>,
) {
    for (index, contact) in doc.contacts.iter().enumerate() {
        let base = format!("/contacts/{index}");
        for (key, (role, part)) in [("a", &contact.a), ("b", &contact.b)] {
            let pointer = format!("{base}/{key}");
            if role.is_empty() || part.is_empty() {
                d.push(data(
                    &pointer,
                    format!("`{key}` is not `role/part`"),
                    "write both sides as a role name, a slash and a part name, such as `target/head`",
                ));
            } else if !has_role(role) {
                d.push(data(
                    &pointer,
                    format!("`{key}` names `{role}`, which is not a role"),
                    &unknown_role_help(role, &doc.roles),
                ));
            }
        }
        if contact.a.0 == contact.b.0 && !contact.a.0.is_empty() {
            d.push(data(
                &format!("{base}/b"),
                format!("both sides of the contact are on role `{}`", contact.a.0),
                "a contact joins a part of one role to a part of another",
            ));
        }
        if !doc.events.iter().any(|event| event.name == contact.at) {
            let help =
                match suggest::nearest(&contact.at, doc.events.iter().map(|e| e.name.as_str())) {
                    Some(candidate) => format!("nearest event is `{candidate}`"),
                    None => "add the event to `events`, or name an existing one".to_string(),
                };
            d.push(data(
                &format!("{base}/at"),
                format!("`at` names `{}`, which is not an event", contact.at),
                &help,
            ));
        }
        if number_ok(contact.max_gap, &format!("{base}/max_gap"), "`max_gap`", d)
            && contact.max_gap < 0.0
        {
            d.push(data(
                &format!("{base}/max_gap"),
                format!(
                    "`max_gap` is {}, but a gap cannot be negative",
                    contact.max_gap
                ),
                "use 0 or a positive number of cells",
            ));
        }
    }
}

// --- Canonical writer ---------------------------------------------------------------------------

/// Writes `doc` in canonical form: `format`, `kind`, `name`, `roles` and `offsets` sorted by name,
/// every `align` key spelled, `align_only` only when true, `scale` only when not 1, `events` and
/// `contacts` as authored and omitted when empty.
pub(crate) fn emit(doc: &PairedDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\nkind: paired\nname: ");
    out.push_str(&scalar_string(&doc.name));
    out.push_str("\nroles:\n");
    let mut roles: Vec<&PairedRole> = doc.roles.iter().collect();
    roles.sort_by(|a, b| a.name.cmp(&b.name));
    for role in roles {
        out.push_str(&format!("  {}:\n", scalar_string(&role.name)));
        out.push_str(&format!(
            "    assembly: {}\n",
            scalar_string(&role.assembly)
        ));
        if let Some(clip) = &role.clip {
            out.push_str(&format!("    clip: {}\n", scalar_string(clip)));
        }
        if role.align_only {
            out.push_str("    align_only: true\n");
        }
        if let Some(posture) = &role.posture {
            out.push_str(&format!("    posture: {}\n", scalar_string(posture)));
        }
        if role.scale != 1.0 {
            out.push_str(&format!("    scale: {}\n", format_float(role.scale)));
        }
    }
    out.push_str("align:\n");
    out.push_str(&format!(
        "  reference: {}\n",
        scalar_string(&doc.align.reference)
    ));
    match &doc.align.moves {
        PairedMoves::Split => out.push_str("  moves: split\n"),
        PairedMoves::Role(role) => out.push_str(&format!("  moves: {}\n", scalar_string(role))),
    }
    out.push_str(&format!("  ticks: {}\n  offsets:\n", doc.align.ticks));
    let mut offsets: Vec<&PairedOffset> = doc.align.offsets.iter().collect();
    offsets.sort_by(|a, b| a.role.cmp(&b.role));
    for offset in offsets {
        out.push_str(&format!(
            "    {}:\n      translation:\n",
            scalar_string(&offset.role)
        ));
        for value in offset.translation {
            out.push_str(&format!("        - {}\n", format_float(value)));
        }
        out.push_str(&format!(
            "      yaw_deg: {}\n",
            format_float(offset.yaw_deg)
        ));
    }
    if !doc.events.is_empty() {
        out.push_str("events:\n");
        for event in &doc.events {
            out.push_str(&format!(
                "  - name: {}\n    t: {}\n",
                scalar_string(&event.name),
                format_float(event.t)
            ));
        }
    }
    if !doc.contacts.is_empty() {
        out.push_str("contacts:\n");
        for contact in &doc.contacts {
            let side = |(role, part): &(String, String)| scalar_string(&format!("{role}/{part}"));
            out.push_str(&format!(
                "  - a: {}\n    b: {}\n    at: {}\n    max_gap: {}\n",
                side(&contact.a),
                side(&contact.b),
                scalar_string(&contact.at),
                format_float(contact.max_gap)
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = "\
format: 1
kind: paired
name: choke
roles:
  instigator:
    assembly: humanoid.assembly.yaml
    clip: carry
    posture: standing
    scale: 0.015625
  search:
    assembly: locker.assembly.yaml
    align_only: true
  target:
    assembly: humanoid.assembly.yaml
    clip: idle
align:
  reference: instigator
  moves: split
  ticks: 12
  offsets:
    search:
      translation:
        - 1.0
        - 0.0
        - 0.0
      yaw_deg: 0.0
    target:
      translation:
        - 0.0
        - 0.0
        - -0.6
      yaw_deg: 180.0
events:
  - name: grab
    t: 0.25
  - name: release
    t: 0.75
contacts:
  - a: instigator/lower-arm-r
    b: target/head
    at: grab
    max_gap: 1.0
";

    /// A two-role document with one line replaced or appended, for one-rule-per-test checks.
    fn base(extra_role: &str, align: &str, tail: &str) -> String {
        format!(
            "format: 1\nkind: paired\nname: x\nroles:\n  a:\n    assembly: a.yaml\n    clip: c\n  b:\n    assembly: b.yaml\n    clip: c\n{extra_role}align:\n{align}{tail}"
        )
    }

    const GOOD_ALIGN: &str = "  reference: a\n  moves: a\n  ticks: 5\n  offsets:\n    b:\n      translation:\n        - 0.0\n        - 0.0\n        - -1.0\n";

    fn found(source: &str) -> Vec<(String, String)> {
        let errors = match parse(source) {
            Err(errors) => errors,
            Ok(doc) => validate(&doc),
        };
        errors
            .iter()
            .map(|d| (d.code.to_string(), d.path.clone().unwrap_or_default()))
            .collect()
    }

    fn assert_finds(source: &str, code: &str, pointer: &str) {
        let all = found(source);
        assert!(
            all.contains(&(code.to_string(), pointer.to_string())),
            "{code} {pointer}: {all:?}\n{source}"
        );
    }

    #[test]
    fn an_event_named_end_is_reserved() {
        assert_finds(
            &base("", GOOD_ALIGN, "events:\n  - name: end\n    t: 0.5\n"),
            "L1753",
            "/events/0/name",
        );
    }

    #[test]
    fn a_full_document_parses_and_emits_as_a_fixed_point() {
        let doc = parse(FULL).expect("parses");
        assert_eq!(doc.roles.len(), 3);
        assert_eq!(doc.align.moves, PairedMoves::Split);
        assert_eq!(
            doc.contacts[0].b,
            ("target".to_string(), "head".to_string())
        );
        assert!(validate(&doc).is_empty());
        let emitted = emit(&doc);
        assert_eq!(emitted, FULL, "FULL is already canonical");
        assert_eq!(emit(&parse(&emitted).unwrap()), emitted);
    }

    #[test]
    fn the_writer_sorts_roles_and_offsets() {
        let source = FULL
            .replacen("  instigator:", "  zed:", 1)
            .replace("reference: instigator", "reference: zed")
            .replace("a: instigator/", "a: zed/");
        let doc = parse(&source).expect("parses");
        let emitted = emit(&doc);
        let zed = emitted.find("  zed:").expect("zed");
        let target = emitted.find("  target:").expect("target");
        assert!(target < zed, "roles sort by name:\n{emitted}");
    }

    #[test]
    fn structure_errors_carry_pointers() {
        let cases = [
            (base("", GOOD_ALIGN, "wat: 1\n"), "/wat"),
            (
                base("", &GOOD_ALIGN.replace("ticks: 5", "ticks: soon"), ""),
                "/align/ticks",
            ),
            (
                base(
                    "",
                    &GOOD_ALIGN.replace(
                        "      translation:\n        - 0.0\n        - 0.0\n        - -1.0\n",
                        "      translation:\n        - 0.0\n",
                    ),
                    "",
                ),
                "/align/offsets/b/translation",
            ),
            (
                base("", &format!("{GOOD_ALIGN}      spin: 1\n"), ""),
                "/align/offsets/b/spin",
            ),
            (base("", GOOD_ALIGN, "events:\n  - name: e\n"), "/events/0"),
            (
                base("", GOOD_ALIGN, "events:\n  - name: e\n    t: now\n"),
                "/events/0/t",
            ),
            (
                base(
                    "",
                    GOOD_ALIGN,
                    "contacts:\n  - a: a/x\n    b: b/y\n    at: e\n",
                ),
                "/contacts/0",
            ),
            (base("  c:\n    clip: x\n", GOOD_ALIGN, ""), "/roles/c"),
            (
                base("  c:\n    assembly: c.yaml\n    clyp: x\n", GOOD_ALIGN, ""),
                "/roles/c/clyp",
            ),
            (
                base(
                    "  c:\n    assembly: c.yaml\n    align_only: maybe\n",
                    GOOD_ALIGN,
                    "",
                ),
                "/roles/c/align_only",
            ),
            (
                "format: 1\nkind: paired\nname: x\nalign:\n  reference: a\n".to_string(),
                "",
            ),
        ];
        for (source, pointer) in cases {
            assert_finds(&source, "L1752", pointer);
        }
        assert!(
            found("format: 1\nkind: patrol\n")
                .iter()
                .all(|(code, _)| code == "L1752"),
            "a wrong kind is a structure error"
        );
    }

    #[test]
    fn data_errors_carry_pointers() {
        let with = |align: &str, tail: &str| base("", align, tail);
        let cases = [
            (
                "format: 1\nkind: paired\nname: x\nroles:\n  a:\n    assembly: a.yaml\n    clip: c\nalign:\n  reference: a\n  moves: a\n  ticks: 1\n  offsets:\n    z:\n      translation:\n        - 0.0\n        - 0.0\n        - 0.0\n".to_string(),
                "/roles",
            ),
            (
                with(&GOOD_ALIGN.replace("reference: a", "reference: aa"), ""),
                "/align/reference",
            ),
            (with(&GOOD_ALIGN.replace("moves: a", "moves: bb"), ""), "/align/moves"),
            (
                with(&format!("{GOOD_ALIGN}    z:\n      translation:\n        - 0.0\n        - 0.0\n        - 0.0\n"), ""),
                "/align/offsets/z",
            ),
            (
                with(&format!("{GOOD_ALIGN}    a:\n      translation:\n        - 0.0\n        - 0.0\n        - 0.0\n"), ""),
                "/align/offsets/a",
            ),
            (
                base("  c:\n    assembly: c.yaml\n    clip: k\n", GOOD_ALIGN, ""),
                "/align/offsets",
            ),
            (with(&GOOD_ALIGN.replace("ticks: 5", "ticks: 0"), ""), "/align/ticks"),
            (
                with(&GOOD_ALIGN.replace("- -1.0", "- 1.0e300"), ""),
                "/align/offsets/b/translation/2",
            ),
            (
                base("", GOOD_ALIGN, "").replace("    clip: c\n  b:", "    clip: c\n    scale: 0.0\n  b:"),
                "/roles/a/scale",
            ),
            (
                base("", GOOD_ALIGN, "").replace("    clip: c\n  b:", "    clip: c\n    align_only: true\n  b:"),
                "/roles/a",
            ),
            (
                base("", GOOD_ALIGN, "").replacen("    clip: c\n", "", 1),
                "/roles/a",
            ),
            (
                base("", GOOD_ALIGN, "").replacen("assembly: a.yaml", "assembly: \"\"", 1),
                "/roles/a/assembly",
            ),
            (
                base("", GOOD_ALIGN, "").replacen("clip: c", "clip: \"\"", 1),
                "/roles/a/clip",
            ),
            (
                base("", GOOD_ALIGN, "").replacen("clip: c", "clip: c\n    posture: \"\"", 1),
                "/roles/a/posture",
            ),
            (
                base("", GOOD_ALIGN, "").replace("name: x", "name: \"\""),
                "/name",
            ),
            (
                with(GOOD_ALIGN, "events:\n  - name: \"\"\n    t: 0.5\n"),
                "/events/0/name",
            ),
            (
                with(GOOD_ALIGN, "events:\n  - name: e\n    t: -0.5\n"),
                "/events/0/t",
            ),
            (
                with(GOOD_ALIGN, "events:\n  - name: e\n    t: 0.5\n  - name: f\n    t: 0.5\n"),
                "/events/1/t",
            ),
            (
                with(GOOD_ALIGN, "events:\n  - name: e\n    t: 0.5\n  - name: e\n    t: 0.9\n"),
                "/events/1/name",
            ),
            (
                with(GOOD_ALIGN, "events:\n  - name: e\n    t: 1.0e300\n"),
                "/events/0/t",
            ),
            (
                with(GOOD_ALIGN, "events:\n  - name: e\n    t: 0.5\ncontacts:\n  - a: a\n    b: b/y\n    at: e\n    max_gap: 1.0\n"),
                "/contacts/0/a",
            ),
            (
                with(GOOD_ALIGN, "events:\n  - name: e\n    t: 0.5\ncontacts:\n  - a: q/x\n    b: b/y\n    at: e\n    max_gap: 1.0\n"),
                "/contacts/0/a",
            ),
            (
                with(GOOD_ALIGN, "events:\n  - name: e\n    t: 0.5\ncontacts:\n  - a: a/x\n    b: a/y\n    at: e\n    max_gap: 1.0\n"),
                "/contacts/0/b",
            ),
            (
                with(GOOD_ALIGN, "events:\n  - name: e\n    t: 0.5\ncontacts:\n  - a: a/x\n    b: b/y\n    at: ee\n    max_gap: 1.0\n"),
                "/contacts/0/at",
            ),
            (
                with(GOOD_ALIGN, "events:\n  - name: e\n    t: 0.5\ncontacts:\n  - a: a/x\n    b: b/y\n    at: e\n    max_gap: -1.0\n"),
                "/contacts/0/max_gap",
            ),
        ];
        for (source, pointer) in cases {
            assert_finds(&source, "L1753", pointer);
        }
    }

    #[test]
    fn a_minimal_two_role_document_is_valid() {
        let doc = parse(&base("", GOOD_ALIGN, "")).expect("parses");
        assert!(validate(&doc).is_empty());
        assert!(doc.events.is_empty() && doc.contacts.is_empty());
        let emitted = emit(&doc);
        assert!(!emitted.contains("events:"), "{emitted}");
        assert_eq!(parse(&emitted).expect("re-parses"), doc);
    }
}
