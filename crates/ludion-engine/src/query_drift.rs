//! Query-drift verification — the *measured* half of typed query declarations.
//!
//! A system declares the component queries it runs with
//! [`EngineBuilder::query`](crate::EngineBuilder::query); this module checks those declarations
//! against reality. It runs a game through a committed replay — or an arbitrary driver closure — with
//! [`Engine::begin_query_recording`] armed, then compares each system's **observed** query shapes
//! against its **declared** ones, producing a [`QueryDriftReport`]:
//!
//! - an observed shape a system never declared is an **error** (`L1126`) — the declaration is a lie;
//! - a declared shape the run never exercised is a **warning** (`L1127`) — a conditional branch may
//!   legitimately not fire in every replay.
//!
//! A shape is compared as a **set** of `(component, access)` pairs: tuple order, declaration order,
//! and the order queries were declared in do not matter, and a declared read (`&T`) where the body
//! took a write (`&mut T`) counts as *undeclared* (the access differs).
//!
//! # Example
//! ```
//! use ludion_engine::{check_query_drift_with, Engine};
//! use ludion_protocol::Command;
//! use ludion_protocol::command::SimStepParams;
//! use serde::{Deserialize, Serialize};
//! use schemars::JsonSchema;
//!
//! #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
//! struct Health { hp: f32 }
//!
//! fn regen(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
//!     for (h,) in world.query::<(&mut Health,)>() { h.hp += 1.0; }
//! }
//!
//! let mut engine = Engine::builder()
//!     .components(|r| r.register::<Health>("Health"))
//!     .system("regen", regen).query::<(&mut Health,)>()
//!     .build()
//!     .unwrap();
//! engine
//!     .load_inline_scene("format: 1\nentities:\n  - components:\n      Health:\n        hp: 1.0\n")
//!     .unwrap();
//!
//! let report = check_query_drift_with(&mut engine, |e| {
//!     e.execute(Command::SimStep(SimStepParams { ticks: 2, hash: false })).map(|_| ())
//! })
//! .unwrap();
//! assert!(report.is_clean()); // the one shape `regen` ran is exactly what it declared
//! ```

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::QueryObservations;

use crate::builder::EngineBuilder;
use crate::engine::Engine;

/// One query shape as compared by the verifier: the set of `(component name, is-write)` pairs a query
/// names, normalized so tuple/declaration order is irrelevant.
type NamedShape = BTreeSet<(String, bool)>;

/// The verdict of a query-drift check: [`errors`](QueryDriftReport::errors) (`L1126`,
/// observed-but-undeclared) and [`warnings`](QueryDriftReport::warnings) (`L1127`,
/// declared-but-never-observed). A run is [`is_clean`](QueryDriftReport::is_clean) when there are no
/// errors; warnings never fail a check.
#[derive(Debug, Default, Clone)]
pub struct QueryDriftReport {
    errors: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
}

impl QueryDriftReport {
    /// The `L1126` errors: shapes a system ran but never declared. Empty means every observed query
    /// was declared.
    pub fn errors(&self) -> &[Diagnostic] {
        &self.errors
    }

    /// The `L1127` warnings: shapes a system declared but the run never exercised (a conditional
    /// branch that did not fire, or a stale declaration).
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    /// Whether the check passed — no observed-but-undeclared shapes. Warnings do not affect this.
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}

impl fmt::Display for QueryDriftReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.errors.is_empty() && self.warnings.is_empty() {
            return write!(
                f,
                "query-drift check clean: every observed query was declared"
            );
        }
        writeln!(
            f,
            "query-drift check: {} error(s), {} warning(s)",
            self.errors.len(),
            self.warnings.len()
        )?;
        for e in &self.errors {
            writeln!(f, "  error [{}] {}", e.code, e.message)?;
        }
        for w in &self.warnings {
            writeln!(f, "  warning [{}] {}", w.code, w.message)?;
        }
        Ok(())
    }
}

/// Runs `drive` against `engine` with query recording enabled, then compares the observed query
/// shapes against `engine`'s declared ones, returning the [`QueryDriftReport`].
///
/// The driver-closure flavor: use it when you already hold a built engine and want to drive it with
/// arbitrary commands. For the common case — a committed replay — use [`check_query_drift_replay`].
///
/// # Errors
/// Any diagnostic `drive` returns.
pub fn check_query_drift_with(
    engine: &mut Engine,
    drive: impl FnOnce(&mut Engine) -> Result<(), Diagnostic>,
) -> Result<QueryDriftReport, Diagnostic> {
    engine.begin_query_recording();
    drive(engine)?;
    let observed = engine.take_query_observations();
    Ok(compare(engine, &observed))
}

/// Runs `builder`'s engine through the recorded session in `bytes` (rooted at `project_root`) with
/// query recording enabled, then compares each system's observed query shapes against its declared
/// ones, returning the [`QueryDriftReport`]. The reusable game-test entry point: a game hands its own
/// `EngineBuilder` (registry + declared systems) and its committed `run.lrp`.
///
/// Recording accumulates across the district switches a replay may perform, so a cross-district run
/// verifies every district's systems, not just the last one's.
///
/// # Errors
/// Every error [`replay_hashes_with`](crate::replay_hashes_with) can raise (a bad replay, an
/// unresolved scene, a dispatch failure).
pub fn check_query_drift_replay(
    builder: EngineBuilder,
    bytes: &[u8],
    project_root: impl AsRef<Path>,
) -> Result<QueryDriftReport, Diagnostic> {
    let (mut engine, actions) = crate::replay::prepare_replay(
        builder,
        bytes,
        project_root,
        crate::replay::Keyframes::Skip,
    )?;
    engine.begin_query_recording();
    crate::replay::drive_actions_with(&mut engine, actions, &mut crate::replay::PlainDrive)?;
    let observed = engine.take_query_observations();
    Ok(compare(&engine, &observed))
}

/// Compares each system's declared query shapes against `observed`, resolving both sides to registry
/// component names so the comparison is name+access, order-free.
fn compare(engine: &Engine, observed: &QueryObservations) -> QueryDriftReport {
    let registry = engine.registry();
    let mut report = QueryDriftReport::default();

    for spec in engine.systems() {
        // Declared: each declared query tuple → a named (component, access) set.
        let declared: BTreeSet<NamedShape> = spec
            .declared_queries
            .iter()
            .map(|query| {
                query
                    .iter()
                    .map(|elem| {
                        let name = registry
                            .get_by_type_id(elem.type_id)
                            .map(|info| info.name().to_string())
                            .unwrap_or_else(|| elem.type_name.to_string());
                        (name, elem.write)
                    })
                    .collect()
            })
            .collect();

        // Observed: each recorded shape (TypeIds) → the same named set.
        let observed_shapes: BTreeSet<NamedShape> = observed
            .shapes(spec.name)
            .map(|shape| {
                shape
                    .iter()
                    .map(|(type_id, write)| {
                        let name = registry
                            .get_by_type_id(*type_id)
                            .map(|info| info.name().to_string())
                            .unwrap_or_else(|| {
                                observed
                                    .type_name(*type_id)
                                    .map_or_else(|| format!("{type_id:?}"), str::to_string)
                            });
                        (name, *write)
                    })
                    .collect()
            })
            .collect();

        for shape in &observed_shapes {
            if !declared.contains(shape) {
                report
                    .errors
                    .push(undeclared_error(spec.name, shape, &declared));
            }
        }
        for shape in &declared {
            if !observed_shapes.contains(shape) {
                report.warnings.push(unobserved_warning(spec.name, shape));
            }
        }
    }

    report
}

/// Formats a named shape as the `&T`/`&mut T` tuple a `.query::<...>()` would name, sorted by
/// component name for a stable message.
fn fmt_shape(shape: &NamedShape) -> String {
    let parts: Vec<String> = shape
        .iter()
        .map(|(name, write)| {
            if *write {
                format!("&mut {name}")
            } else {
                format!("&{name}")
            }
        })
        .collect();
    format!("({})", parts.join(", "))
}

fn undeclared_error(
    system: &str,
    observed: &NamedShape,
    declared: &BTreeSet<NamedShape>,
) -> Diagnostic {
    let declared_list = if declared.is_empty() {
        "none".to_string()
    } else {
        declared
            .iter()
            .map(fmt_shape)
            .collect::<Vec<_>>()
            .join(", ")
    };
    Diagnostic::new(
        code::QUERY_DRIFT_UNDECLARED,
        format!(
            "system `{system}` ran the query {} but never declared it (declared shapes: {declared_list})",
            fmt_shape(observed),
        ),
    )
    .with_help(format!(
        "add `.query::<{}>()` to this system's registration, matching the &T/&mut T access the body uses",
        fmt_shape(observed),
    ))
}

fn unobserved_warning(system: &str, declared: &NamedShape) -> Diagnostic {
    Diagnostic::new(
        code::QUERY_DRIFT_UNOBSERVED,
        format!(
            "system `{system}` declared the query {} but the verified run never exercised it",
            fmt_shape(declared),
        ),
    )
    .with_help(
        "if the branch is reachable, extend the replay/driver to exercise it; if the declaration is \
         stale (the system no longer runs that query), remove it; a legitimately-conditional query \
         needs no action",
    )
}
