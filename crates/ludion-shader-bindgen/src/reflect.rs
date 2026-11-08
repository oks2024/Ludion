//! WGSL → wgpu-free reflection model, driven by naga's front-end, validator, and Layouter.
//!
//! The unit of reflection is a [`ShaderUnit`]: an optional preamble prepended to a file's source
//! (Plan/shader-bindgen.md decision 15 — compose-then-reflect, so the reflector sees exactly the
//! string `create_shader_module` sees). [`reflect`] parses, validates, and lays out each unit, then
//! unifies same-named structs / resources / consts across all units, erroring on any disagreement
//! (decisions 4, 13).

use std::collections::{BTreeMap, BTreeSet};

use naga::proc::{Alignment, Layouter};
use naga::valid::{Capabilities, ModuleInfo, ValidationFlags, Validator};

use crate::error::ReflectError;
use crate::model::{
    BindGroupLayout, BindingKind, BindingResource, BufferSpace, EntryPoint, FieldLayout,
    Reflection, ScalarConst, ScalarKind, ShaderStage, StorageAccess, StorageFormat, StructLayout,
    TextureDim, TextureSample, TypeRef, VertexFormat, VertexInput,
};

/// A named WGSL source to reflect. The reflected string is `preamble` (if any) then a newline then
/// `source` — exactly the composition a pipeline would build for a shared-snippet prelude (the seam a
/// future `common.wgsl` uses). The repo shaders are currently all raw (no preamble).
#[derive(Clone, Debug)]
pub struct ShaderUnit {
    /// Logical name used in output ordering and error messages (e.g. `"mesh.wgsl"`).
    pub name: String,
    /// Text prepended before `source`, or `None` for a raw file.
    pub preamble: Option<String>,
    pub source: String,
}

impl ShaderUnit {
    /// A raw unit with no preamble.
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            preamble: None,
            source: source.into(),
        }
    }

    /// A unit whose `source` is composed after `preamble` (decision 15).
    pub fn with_preamble(
        name: impl Into<String>,
        preamble: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            preamble: Some(preamble.into()),
            source: source.into(),
        }
    }

    /// The composed string handed to naga — what `create_shader_module` would see.
    fn composed(&self) -> String {
        match &self.preamble {
            Some(p) => format!("{p}\n{}", self.source),
            None => self.source.clone(),
        }
    }
}

/// Reflect a set of shader units into one unified, deterministically-ordered model.
///
/// Units are reflected in sorted-name order (decision 7) so unification — and therefore which unit
/// a conflict names first — is stable regardless of input order.
pub fn reflect(units: &[ShaderUnit]) -> Result<Reflection, ReflectError> {
    let mut sorted: Vec<&ShaderUnit> = units.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let per_unit: Vec<UnitReflection> = sorted
        .iter()
        .map(|u| reflect_unit(u))
        .collect::<Result<_, _>>()?;

    unify(per_unit)
}

/// The reflection of a single unit, in WGSL declaration order.
struct UnitReflection {
    name: String,
    structs: Vec<StructLayout>,
    bindings: Vec<BindingResource>,
    consts: Vec<ScalarConst>,
    entry_points: Vec<EntryPoint>,
    /// The unit's bind groups: one per distinct `@group` index, each with its member resource names
    /// in ascending binding order. Unified across units in [`unify`] by the member-name set.
    groups: Vec<UnitBindGroup>,
}

/// One `@group` of a single unit — the raw material for cross-unit BGL unification.
struct UnitBindGroup {
    group: u32,
    /// Member resource variable names, ascending by binding index.
    members: Vec<String>,
}

fn align_u32(a: Alignment) -> u32 {
    // `Alignment: Mul<u32, Output = u32>` — the only conversion naga exposes.
    a * 1u32
}

fn reflect_unit(unit: &ShaderUnit) -> Result<UnitReflection, ReflectError> {
    let src = unit.composed();

    let module = naga::front::wgsl::parse_str(&src).map_err(|e| ReflectError::Parse {
        unit: unit.name.clone(),
        message: e.emit_to_string(&src),
    })?;

    let info = Validator::new(ValidationFlags::default(), Capabilities::default())
        .validate(&module)
        .map_err(|e| ReflectError::Validate {
            unit: unit.name.clone(),
            message: e.emit_to_string(&src),
        })?;

    let mut layouter = Layouter::default();
    layouter
        .update(module.to_ctx())
        .map_err(|e| ReflectError::Layout {
            unit: unit.name.clone(),
            message: e.to_string(),
        })?;

    let structs = reflect_structs(unit, &module, &layouter)?;
    let bindings = reflect_bindings(unit, &module, &info, &layouter)?;
    let consts = reflect_consts(unit, &module);
    let entry_points = reflect_entry_points(unit, &module)?;
    let groups = unit_groups(&bindings);

    Ok(UnitReflection {
        name: unit.name.clone(),
        structs,
        bindings,
        consts,
        entry_points,
        groups,
    })
}

/// Groups a unit's bindings by `@group` index, each member list ascending by binding.
fn unit_groups(bindings: &[BindingResource]) -> Vec<UnitBindGroup> {
    let mut by_group: BTreeMap<u32, Vec<(u32, String)>> = BTreeMap::new();
    for b in bindings {
        by_group
            .entry(b.group)
            .or_default()
            .push((b.binding, b.name.clone()));
    }
    by_group
        .into_iter()
        .map(|(group, mut members)| {
            members.sort_by_key(|(binding, _)| *binding);
            UnitBindGroup {
                group,
                members: members.into_iter().map(|(_, name)| name).collect(),
            }
        })
        .collect()
}

/// Struct handles reachable from a uniform/storage global, transitively through array elements and
/// struct members. This is exactly the host-shareable (buffer-backed) set — it excludes vertex I/O
/// structs (`VsIn`/`VsOut`), which differ per shader and must not be unified by name.
fn buffer_struct_handles(module: &naga::Module) -> BTreeSet<naga::Handle<naga::Type>> {
    let mut seeds: Vec<naga::Handle<naga::Type>> = Vec::new();
    for (_, gv) in module.global_variables.iter() {
        if matches!(
            gv.space,
            naga::AddressSpace::Uniform | naga::AddressSpace::Storage { .. }
        ) {
            seeds.push(gv.ty);
        }
    }

    let mut out = BTreeSet::new();
    let mut stack = seeds;
    while let Some(handle) = stack.pop() {
        match &module.types[handle].inner {
            naga::TypeInner::Struct { members, .. } => {
                if out.insert(handle) {
                    for m in members {
                        stack.push(m.ty);
                    }
                }
            }
            naga::TypeInner::Array { base, .. } => stack.push(*base),
            _ => {}
        }
    }
    out
}

fn reflect_structs(
    unit: &ShaderUnit,
    module: &naga::Module,
    layouter: &Layouter,
) -> Result<Vec<StructLayout>, ReflectError> {
    let wanted = buffer_struct_handles(module);
    let mut out = Vec::new();
    // `types.iter()` yields handles in declaration order.
    for (handle, ty) in module.types.iter() {
        if !wanted.contains(&handle) {
            continue;
        }
        let naga::TypeInner::Struct { members, .. } = &ty.inner else {
            continue;
        };
        let Some(name) = &ty.name else { continue };
        let layout = layouter[handle];
        let mut fields = Vec::with_capacity(members.len());
        for m in members {
            let field_name = m.name.clone().unwrap_or_default();
            let ty = type_ref(unit, module, m.ty)?;
            // A `#[repr(C)]` POD cannot mirror a trailing runtime-sized array, and a fixed-size
            // stand-in would silently be the wrong type on the CPU side — so refuse, and name the
            // shape that works instead.
            if matches!(ty, TypeRef::Array { count: None, .. }) {
                return Err(ReflectError::RuntimeSizedMember {
                    unit: unit.name.clone(),
                    struct_name: name.clone(),
                    field: field_name,
                });
            }
            fields.push(FieldLayout {
                name: field_name,
                offset: m.offset,
                size: layouter[m.ty].size,
                ty,
            });
        }
        out.push(StructLayout {
            name: name.clone(),
            size: layout.size,
            alignment: align_u32(layout.alignment),
            fields,
            units: vec![unit.name.clone()],
        });
    }
    Ok(out)
}

/// Map a naga data type to a wgpu-free [`TypeRef`]. Only data types appear in struct members /
/// array elements; anything else in that context is a reflection bug and errors.
fn type_ref(
    unit: &ShaderUnit,
    module: &naga::Module,
    handle: naga::Handle<naga::Type>,
) -> Result<TypeRef, ReflectError> {
    let ty = &module.types[handle];
    Ok(match &ty.inner {
        naga::TypeInner::Scalar(s) => TypeRef::Scalar(scalar_kind(unit, *s)?),
        naga::TypeInner::Vector { size, scalar } => TypeRef::Vector {
            components: *size as u8,
            scalar: scalar_kind(unit, *scalar)?,
        },
        naga::TypeInner::Matrix {
            columns,
            rows,
            scalar,
        } => TypeRef::Matrix {
            columns: *columns as u8,
            rows: *rows as u8,
            scalar: scalar_kind(unit, *scalar)?,
        },
        naga::TypeInner::Atomic(s) => TypeRef::Atomic(scalar_kind(unit, *s)?),
        naga::TypeInner::Array { base, size, stride } => TypeRef::Array {
            element: Box::new(type_ref(unit, module, *base)?),
            count: match size {
                naga::ArraySize::Constant(n) => Some(n.get()),
                naga::ArraySize::Pending(_) | naga::ArraySize::Dynamic => None,
            },
            stride: *stride,
        },
        naga::TypeInner::Struct { .. } => TypeRef::Struct(
            ty.name
                .clone()
                .ok_or_else(|| unsupported_type(unit, "anonymous struct"))?,
        ),
        _ => return Err(unsupported_type(unit, "non-data type in a data context")),
    })
}

fn scalar_kind(unit: &ShaderUnit, s: naga::Scalar) -> Result<ScalarKind, ReflectError> {
    use naga::ScalarKind as K;
    Ok(match (s.kind, s.width) {
        (K::Float, 2) => ScalarKind::F16,
        (K::Float, 4) => ScalarKind::F32,
        (K::Float, 8) => ScalarKind::F64,
        (K::Sint, 4) => ScalarKind::I32,
        (K::Sint, 8) => ScalarKind::I64,
        (K::Uint, 4) => ScalarKind::U32,
        (K::Uint, 8) => ScalarKind::U64,
        (K::Bool, _) => ScalarKind::Bool,
        _ => return Err(unsupported_type(unit, "abstract or unsupported scalar")),
    })
}

fn unsupported_type(unit: &ShaderUnit, what: &str) -> ReflectError {
    ReflectError::Layout {
        unit: unit.name.clone(),
        message: format!("unsupported type: {what}"),
    }
}

fn reflect_bindings(
    unit: &ShaderUnit,
    module: &naga::Module,
    info: &ModuleInfo,
    layouter: &Layouter,
) -> Result<Vec<BindingResource>, ReflectError> {
    let mut out = Vec::new();
    for (handle, gv) in module.global_variables.iter() {
        let Some(rb) = &gv.binding else { continue };
        let kind = binding_kind(unit, module, layouter, gv)?;
        let visibility = binding_visibility(module, info, handle);
        out.push(BindingResource {
            name: gv.name.clone().unwrap_or_default(),
            group: rb.group,
            binding: rb.binding,
            kind,
            visibility,
            units: vec![unit.name.clone()],
        });
    }
    Ok(out)
}

fn binding_kind(
    unit: &ShaderUnit,
    module: &naga::Module,
    layouter: &Layouter,
    gv: &naga::GlobalVariable,
) -> Result<BindingKind, ReflectError> {
    Ok(match gv.space {
        naga::AddressSpace::Uniform => BindingKind::Buffer {
            address_space: BufferSpace::Uniform,
            min_size: layouter[gv.ty].size,
            atomic: reaches_atomic(module, gv.ty),
        },
        // `min_size` for a runtime-sized array is one element's stride: naga's Layouter sizes a
        // dynamically-sized array as exactly one element, which is also what wgpu's
        // `min_binding_size` asks for — the smallest buffer the shader can legally read.
        naga::AddressSpace::Storage { access } => BindingKind::Buffer {
            address_space: BufferSpace::Storage {
                read_only: !access.contains(naga::StorageAccess::STORE),
            },
            min_size: layouter[gv.ty].size,
            atomic: reaches_atomic(module, gv.ty),
        },
        naga::AddressSpace::Handle => match &module.types[gv.ty].inner {
            naga::TypeInner::Image {
                dim,
                arrayed,
                class: naga::ImageClass::Storage { format, access },
            } => BindingKind::StorageTexture {
                dim: texture_dim(unit, *dim, *arrayed)?,
                format: storage_format(unit, *format)?,
                access: storage_access(*access),
            },
            naga::TypeInner::Image {
                dim,
                arrayed,
                class,
            } => BindingKind::Texture {
                dim: texture_dim(unit, *dim, *arrayed)?,
                sample: texture_sample(unit, gv.name.as_deref(), *class)?,
                multisampled: matches!(
                    class,
                    naga::ImageClass::Sampled { multi: true, .. }
                        | naga::ImageClass::Depth { multi: true }
                ),
            },
            naga::TypeInner::Sampler { comparison } => BindingKind::Sampler {
                comparison: *comparison,
            },
            _ => {
                return Err(unsupported_type(
                    unit,
                    "handle binding is neither image nor sampler",
                ));
            }
        },
        _ => {
            return Err(ReflectError::Layout {
                unit: unit.name.clone(),
                message: format!("resource in unsupported address space {:?}", gv.space),
            });
        }
    })
}

/// Whether `ty` is, or holds through arrays and struct members, an `atomic<…>`.
fn reaches_atomic(module: &naga::Module, ty: naga::Handle<naga::Type>) -> bool {
    match &module.types[ty].inner {
        naga::TypeInner::Atomic(_) => true,
        naga::TypeInner::Array { base, .. } | naga::TypeInner::BindingArray { base, .. } => {
            reaches_atomic(module, *base)
        }
        naga::TypeInner::Struct { members, .. } => {
            members.iter().any(|m| reaches_atomic(module, m.ty))
        }
        _ => false,
    }
}

fn texture_dim(
    unit: &ShaderUnit,
    dim: naga::ImageDimension,
    arrayed: bool,
) -> Result<TextureDim, ReflectError> {
    Ok(match (dim, arrayed) {
        (naga::ImageDimension::D1, false) => TextureDim::D1,
        (naga::ImageDimension::D2, false) => TextureDim::D2,
        (naga::ImageDimension::D3, false) => TextureDim::D3,
        (naga::ImageDimension::Cube, false) => TextureDim::Cube,
        (naga::ImageDimension::D2, true) => TextureDim::D2Array,
        (naga::ImageDimension::Cube, true) => TextureDim::CubeArray,
        (naga::ImageDimension::D1 | naga::ImageDimension::D3, true) => {
            return Err(unsupported_type(unit, "arrayed 1D or 3D texture"));
        }
    })
}

fn storage_format(
    unit: &ShaderUnit,
    format: naga::StorageFormat,
) -> Result<StorageFormat, ReflectError> {
    use naga::StorageFormat as F;
    Ok(match format {
        F::Rgba8Unorm => StorageFormat::Rgba8Unorm,
        F::Rgba16Float => StorageFormat::Rgba16Float,
        F::Rgba32Float => StorageFormat::Rgba32Float,
        F::R32Float => StorageFormat::R32Float,
        F::R32Uint => StorageFormat::R32Uint,
        F::R32Sint => StorageFormat::R32Sint,
        F::Rg32Float => StorageFormat::Rg32Float,
        other => {
            return Err(unsupported_type(
                unit,
                &format!("storage texture format {other:?}"),
            ));
        }
    })
}

fn storage_access(access: naga::StorageAccess) -> StorageAccess {
    match (
        access.contains(naga::StorageAccess::LOAD),
        access.contains(naga::StorageAccess::STORE),
    ) {
        (true, true) => StorageAccess::ReadWrite,
        (false, true) => StorageAccess::WriteOnly,
        _ => StorageAccess::ReadOnly,
    }
}

fn texture_sample(
    unit: &ShaderUnit,
    name: Option<&str>,
    class: naga::ImageClass,
) -> Result<TextureSample, ReflectError> {
    Ok(match class {
        // WGSL cannot express non-filterable float: filterable unless the global is listed in
        // `UNFILTERABLE_TEXTURES` (an `R32Float` binding).
        naga::ImageClass::Sampled {
            kind: naga::ScalarKind::Float,
            ..
        } => TextureSample::Float {
            filterable: !name.is_some_and(|n| crate::UNFILTERABLE_TEXTURES.contains(&n)),
        },
        naga::ImageClass::Sampled {
            kind: naga::ScalarKind::Sint,
            ..
        } => TextureSample::Sint,
        naga::ImageClass::Sampled {
            kind: naga::ScalarKind::Uint,
            ..
        } => TextureSample::Uint,
        naga::ImageClass::Depth { .. } => TextureSample::Depth,
        _ => return Err(unsupported_type(unit, "external texture")),
    })
}

fn binding_visibility(
    module: &naga::Module,
    info: &ModuleInfo,
    handle: naga::Handle<naga::GlobalVariable>,
) -> Vec<ShaderStage> {
    let mut stages = BTreeSet::new();
    for (idx, ep) in module.entry_points.iter().enumerate() {
        let uses = info.get_entry_point(idx)[handle];
        if !uses.is_empty()
            && let Some(stage) = map_stage(ep.stage)
        {
            stages.insert(stage);
        }
    }
    stages.into_iter().collect()
}

fn map_stage(stage: naga::ShaderStage) -> Option<ShaderStage> {
    match stage {
        naga::ShaderStage::Vertex => Some(ShaderStage::Vertex),
        naga::ShaderStage::Fragment => Some(ShaderStage::Fragment),
        naga::ShaderStage::Compute => Some(ShaderStage::Compute),
        _ => None,
    }
}

fn reflect_consts(unit: &ShaderUnit, module: &naga::Module) -> Vec<ScalarConst> {
    let mut out = Vec::new();
    for (_, c) in module.constants.iter() {
        let Some(name) = &c.name else { continue };
        let naga::Expression::Literal(lit) = &module.global_expressions[c.init] else {
            continue;
        };
        if let Some((value, kind)) = int_literal(*lit) {
            out.push(ScalarConst {
                name: name.clone(),
                value,
                kind,
                units: vec![unit.name.clone()],
            });
        }
    }
    out
}

/// The scalar-integer value of a literal, or `None` for non-integer literals (floats, bools).
fn int_literal(lit: naga::Literal) -> Option<(i128, ScalarKind)> {
    use naga::Literal as L;
    Some(match lit {
        L::I32(v) => (v as i128, ScalarKind::I32),
        L::U32(v) => (v as i128, ScalarKind::U32),
        L::I64(v) => (v as i128, ScalarKind::I64),
        L::U64(v) => (v as i128, ScalarKind::U64),
        L::I16(v) => (v as i128, ScalarKind::I32),
        L::U16(v) => (v as i128, ScalarKind::U32),
        L::AbstractInt(v) => (v as i128, ScalarKind::I64),
        _ => return None,
    })
}

fn reflect_entry_points(
    unit: &ShaderUnit,
    module: &naga::Module,
) -> Result<Vec<EntryPoint>, ReflectError> {
    let mut out = Vec::new();
    for ep in &module.entry_points {
        let Some(stage) = map_stage(ep.stage) else {
            return Err(ReflectError::UnsupportedStage {
                unit: unit.name.clone(),
                stage: format!("{:?}", ep.stage).to_lowercase(),
            });
        };
        let inputs = if stage == ShaderStage::Vertex {
            vertex_inputs(unit, module, &ep.function)?
        } else {
            Vec::new()
        };
        out.push(EntryPoint {
            unit: unit.name.clone(),
            name: ep.name.clone(),
            stage,
            workgroup_size: ep.workgroup_size,
            inputs,
        });
    }
    Ok(out)
}

/// The `@location` inputs of a vertex entry point, gathered from both direct arguments and the
/// members of struct arguments (`fn vs_main(in: VsIn)`), sorted by location. Builtins are skipped.
fn vertex_inputs(
    unit: &ShaderUnit,
    module: &naga::Module,
    func: &naga::Function,
) -> Result<Vec<VertexInput>, ReflectError> {
    let mut out = Vec::new();
    for arg in &func.arguments {
        match &arg.binding {
            Some(naga::Binding::Location { location, .. }) => {
                out.push(VertexInput {
                    location: *location,
                    name: arg.name.clone().unwrap_or_default(),
                    format: vertex_format(unit, module, arg.ty)?,
                });
            }
            None => {
                // A struct argument carries its locations on its members.
                if let naga::TypeInner::Struct { members, .. } = &module.types[arg.ty].inner {
                    for m in members {
                        if let Some(naga::Binding::Location { location, .. }) = &m.binding {
                            out.push(VertexInput {
                                location: *location,
                                name: m.name.clone().unwrap_or_default(),
                                format: vertex_format(unit, module, m.ty)?,
                            });
                        }
                    }
                }
            }
            Some(naga::Binding::BuiltIn(_)) => {}
        }
    }
    out.sort_by_key(|i| i.location);
    Ok(out)
}

fn vertex_format(
    unit: &ShaderUnit,
    module: &naga::Module,
    handle: naga::Handle<naga::Type>,
) -> Result<VertexFormat, ReflectError> {
    use naga::ScalarKind as K;
    let (kind, components) = match &module.types[handle].inner {
        naga::TypeInner::Scalar(s) => (s.kind, 1u8),
        naga::TypeInner::Vector { size, scalar } => (scalar.kind, *size as u8),
        _ => {
            return Err(unsupported_type(
                unit,
                "vertex input is not a scalar or vector",
            ));
        }
    };
    Ok(match (kind, components) {
        (K::Float, 1) => VertexFormat::Float32,
        (K::Float, 2) => VertexFormat::Float32x2,
        (K::Float, 3) => VertexFormat::Float32x3,
        (K::Float, 4) => VertexFormat::Float32x4,
        (K::Sint, 1) => VertexFormat::Sint32,
        (K::Sint, 2) => VertexFormat::Sint32x2,
        (K::Sint, 3) => VertexFormat::Sint32x3,
        (K::Sint, 4) => VertexFormat::Sint32x4,
        (K::Uint, 1) => VertexFormat::Uint32,
        (K::Uint, 2) => VertexFormat::Uint32x2,
        (K::Uint, 3) => VertexFormat::Uint32x3,
        (K::Uint, 4) => VertexFormat::Uint32x4,
        _ => {
            return Err(unsupported_type(
                unit,
                "unsupported vertex input scalar kind",
            ));
        }
    })
}

// --- Cross-unit unification (decisions 4, 13) ----------------------------------------------------

fn unify(units: Vec<UnitReflection>) -> Result<Reflection, ReflectError> {
    // `(value, first-declaring unit)` accumulators keyed by name — BTreeMap keeps output sorted. The
    // value's own `units` field accrues every declaring unit (provenance); the paired `String` is the
    // first-declaring unit, named as `unit_a` in a conflict.
    let mut structs: BTreeMap<String, (StructLayout, String)> = BTreeMap::new();
    let mut bindings: BTreeMap<String, (BindingResource, String)> = BTreeMap::new();
    let mut consts: BTreeMap<String, (ScalarConst, String)> = BTreeMap::new();
    let mut entry_points: Vec<EntryPoint> = Vec::new();
    let mut all_units: BTreeSet<String> = BTreeSet::new();
    // Bind groups keyed by their member-name list (binding order) — the decision 13 unification key.
    // Binding unification pins each name's group, so a shared member-name list implies a shared group.
    let mut bind_groups: BTreeMap<Vec<String>, (u32, BTreeSet<String>)> = BTreeMap::new();

    for unit in units {
        all_units.insert(unit.name.clone());

        for g in unit.groups {
            let entry = bind_groups
                .entry(g.members)
                .or_insert_with(|| (g.group, BTreeSet::new()));
            entry.1.insert(unit.name.clone());
        }

        for s in unit.structs {
            match structs.get_mut(&s.name) {
                Some((existing, unit_a)) => {
                    if let Some(detail) = struct_conflict(existing, &s) {
                        return Err(ReflectError::StructConflict {
                            name: s.name.clone(),
                            unit_a: unit_a.clone(),
                            unit_b: unit.name.clone(),
                            detail,
                        });
                    }
                    existing.units.push(unit.name.clone());
                }
                None => {
                    structs.insert(s.name.clone(), (s, unit.name.clone()));
                }
            }
        }

        for b in unit.bindings {
            match bindings.get_mut(&b.name) {
                Some((existing, unit_a)) => {
                    if let Some(detail) = binding_conflict(existing, &b) {
                        return Err(ReflectError::BindingConflict {
                            name: b.name.clone(),
                            unit_a: unit_a.clone(),
                            unit_b: unit.name.clone(),
                            detail,
                        });
                    }
                    merge_visibility(&mut existing.visibility, &b.visibility);
                    existing.units.push(unit.name.clone());
                }
                None => {
                    bindings.insert(b.name.clone(), (b, unit.name.clone()));
                }
            }
        }

        for c in unit.consts {
            match consts.get_mut(&c.name) {
                Some((existing, unit_a)) => {
                    if existing.value != c.value || existing.kind != c.kind {
                        return Err(ReflectError::ConstConflict {
                            name: c.name.clone(),
                            unit_a: unit_a.clone(),
                            unit_b: unit.name.clone(),
                            detail: format!("{} vs {}", existing.value, c.value),
                        });
                    }
                    existing.units.push(unit.name.clone());
                }
                None => {
                    consts.insert(c.name.clone(), (c, unit.name.clone()));
                }
            }
        }

        entry_points.extend(unit.entry_points);
    }

    let mut structs: Vec<StructLayout> = structs.into_values().map(|(s, _)| s).collect();
    for s in &mut structs {
        s.units.sort();
        s.units.dedup();
    }

    let mut bindings: Vec<BindingResource> = bindings.into_values().map(|(b, _)| b).collect();
    for b in &mut bindings {
        b.units.sort();
        b.units.dedup();
    }
    bindings.sort_by(|a, b| (a.group, a.binding, &a.name).cmp(&(b.group, b.binding, &b.name)));

    let mut consts: Vec<ScalarConst> = consts.into_values().map(|(c, _)| c).collect();
    for c in &mut consts {
        c.units.sort();
        c.units.dedup();
    }

    entry_points.sort_by(|a, b| (&a.unit, &a.name).cmp(&(&b.unit, &b.name)));

    // Materialize the unified bind group layouts. The name derives from the member list (binding
    // order), and `BTreeMap` iteration is by that list, so `bind_groups` comes out deterministically
    // ordered; a final sort by generated name makes the emitter's ordering explicit.
    let mut bind_groups: Vec<BindGroupLayout> = bind_groups
        .into_iter()
        .map(|(members, (group, units))| BindGroupLayout {
            name: members.join("_"),
            group,
            members,
            units: units.into_iter().collect(),
        })
        .collect();
    bind_groups.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Reflection {
        units: all_units.into_iter().collect(),
        structs,
        bindings,
        consts,
        entry_points,
        bind_groups,
    })
}

fn merge_visibility(into: &mut Vec<ShaderStage>, add: &[ShaderStage]) {
    for &s in add {
        if !into.contains(&s) {
            into.push(s);
        }
    }
    into.sort();
}

/// A human description of the first way two same-named struct layouts disagree, or `None` if equal.
fn struct_conflict(a: &StructLayout, b: &StructLayout) -> Option<String> {
    if a.fields.len() != b.fields.len() {
        return Some(format!(
            "field count {} vs {}",
            a.fields.len(),
            b.fields.len()
        ));
    }
    for (fa, fb) in a.fields.iter().zip(&b.fields) {
        if fa.name != fb.name {
            return Some(format!("field `{}` vs `{}`", fa.name, fb.name));
        }
        if fa.offset != fb.offset {
            return Some(format!(
                "field `{}` at offset {} vs {}",
                fa.name, fa.offset, fb.offset
            ));
        }
        if fa.ty != fb.ty {
            return Some(format!(
                "field `{}` type `{}` vs `{}`",
                fa.name, fa.ty, fb.ty
            ));
        }
        if fa.size != fb.size {
            return Some(format!(
                "field `{}` size {} vs {}",
                fa.name, fa.size, fb.size
            ));
        }
    }
    if a.size != b.size {
        return Some(format!("size {} vs {}", a.size, b.size));
    }
    if a.alignment != b.alignment {
        return Some(format!("alignment {} vs {}", a.alignment, b.alignment));
    }
    None
}

/// A human description of the first way two same-named bindings disagree, or `None` if equal
/// (visibility is merged, not compared — the same resource is legitimately used by different stages
/// in different units).
fn binding_conflict(a: &BindingResource, b: &BindingResource) -> Option<String> {
    if a.group != b.group || a.binding != b.binding {
        return Some(format!(
            "group {} binding {} vs group {} binding {}",
            a.group, a.binding, b.group, b.binding
        ));
    }
    if a.kind != b.kind {
        return Some(format!("kind {:?} vs {:?}", a.kind, b.kind));
    }
    None
}
