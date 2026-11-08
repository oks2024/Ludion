//! Reflection errors — plain messages aimed at the agent editing WGSL (Plan/shader-bindgen.md
//! decision 8: no `L####` codes, this is engine-internal tooling). `thiserror` is not a workspace
//! dependency, so `Display` is hand-written.

use core::fmt;

/// Anything that can go wrong reflecting a set of shader units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReflectError {
    /// naga's WGSL front-end rejected the (composed) source. `message` is naga's own rendered
    /// diagnostic against the composed string.
    Parse { unit: String, message: String },
    /// naga's validator rejected the module.
    Validate { unit: String, message: String },
    /// The Layouter could not lay the module's types out (e.g. an unresolved override length).
    Layout { unit: String, message: String },
    /// A stage the engine does not target (naga's mesh/task/ray stages).
    UnsupportedStage { unit: String, stage: String },
    /// A buffer-backed struct whose member is a runtime-sized array — no `#[repr(C)]` POD mirrors it.
    RuntimeSizedMember {
        unit: String,
        struct_name: String,
        field: String,
    },
    /// The same struct name is declared with different layouts in two units.
    StructConflict {
        name: String,
        unit_a: String,
        unit_b: String,
        detail: String,
    },
    /// The same resource variable name is bound differently in two units.
    BindingConflict {
        name: String,
        unit_a: String,
        unit_b: String,
        detail: String,
    },
    /// The same const name has different values in two units.
    ConstConflict {
        name: String,
        unit_a: String,
        unit_b: String,
        detail: String,
    },
    /// An [`UNFILTERABLE_TEXTURES`](crate::UNFILTERABLE_TEXTURES) entry names no reflected float texture.
    UnfilterableNotFound { name: String },
    /// A buffer binding of an [`ATOMIC_FREE_UNITS`](crate::ATOMIC_FREE_UNITS) unit reaches an `atomic<…>`.
    AtomicInAtomicFreeUnit { unit: String, binding: String },
}

impl fmt::Display for ReflectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReflectError::Parse { unit, message } => {
                write!(f, "failed to parse `{unit}`: {message}")
            }
            ReflectError::Validate { unit, message } => {
                write!(f, "failed to validate `{unit}`: {message}")
            }
            ReflectError::Layout { unit, message } => {
                write!(f, "failed to lay out types in `{unit}`: {message}")
            }
            ReflectError::UnsupportedStage { unit, stage } => {
                write!(f, "`{unit}` has an unsupported `{stage}` entry point")
            }
            ReflectError::RuntimeSizedMember {
                unit,
                struct_name,
                field,
            } => write!(
                f,
                "struct `{struct_name}` in `{unit}` has the runtime-sized array member `{field}`; a \
                 Rust POD cannot mirror it — bind the array on its own \
                 (`var<storage, read> {field}: array<T>`) and keep the fixed members in a uniform"
            ),
            ReflectError::StructConflict {
                name,
                unit_a,
                unit_b,
                detail,
            } => write!(
                f,
                "struct `{name}` is defined in `{unit_a}` and `{unit_b}` with different layouts: {detail}"
            ),
            ReflectError::BindingConflict {
                name,
                unit_a,
                unit_b,
                detail,
            } => write!(
                f,
                "resource `{name}` is bound in `{unit_a}` and `{unit_b}` with different shapes: {detail}"
            ),
            ReflectError::ConstConflict {
                name,
                unit_a,
                unit_b,
                detail,
            } => write!(
                f,
                "const `{name}` has different values in `{unit_a}` and `{unit_b}`: {detail}"
            ),
            ReflectError::UnfilterableNotFound { name } => write!(
                f,
                "`UNFILTERABLE_TEXTURES` lists `{name}`, but no reflected shader binds a float texture \
                 by that name. Rename the entry with its binding, or delete it. Otherwise the renamed \
                 binding silently becomes filterable."
            ),
            ReflectError::AtomicInAtomicFreeUnit { unit, binding } => write!(
                f,
                "`{unit}` binds `{binding}` with an `atomic<…>` type, but its compaction must be stable \
                 (survivors keep input order): an atomic append orders them by thread timing. Count \
                 with a per-block scan instead (Plan/gpu-driven-culling.md decision 15)."
            ),
        }
    }
}

impl std::error::Error for ReflectError {}
