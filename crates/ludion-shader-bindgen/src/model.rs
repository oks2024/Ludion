//! The wgpu-free reflection model (Plan/shader-bindgen.md decision 6).
//!
//! Every layout fact the generator needs is expressed with plain Rust types here — no `naga` or
//! `wgpu` type escapes into this module's public surface. A backend switch (the architecture doc's
//! ash seam) rewrites the *emitter*, never this model. Ordering is deterministic throughout: the
//! top-level [`Reflection`] holds `Vec`s sorted by a stable key (decision 7), and field/member
//! order within a struct is memory-offset order as declared in the WGSL.

use core::fmt;

/// The complete reflection of a set of shader units, unified across all of them.
///
/// Same-named structs, resources, and consts appearing in more than one unit are merged into a
/// single entry; a disagreement is a hard error raised during reflection, never a silent last-wins
/// (decisions 4, 13). Every `Vec` is in deterministic order (decision 7).
#[derive(Clone, Debug, PartialEq)]
pub struct Reflection {
    /// Every reflected unit's name, sorted — the provenance the emitter names in its header.
    pub units: Vec<String>,
    /// User-defined structs, unified by name across units, sorted by name.
    pub structs: Vec<StructLayout>,
    /// Bound resources (uniform/storage buffers, textures, samplers), unified by resource variable
    /// name across units (see the module docs on the unification key), sorted by `(group, binding,
    /// name)`.
    pub bindings: Vec<BindingResource>,
    /// Module-scope scalar-integer `const`s (e.g. a preamble's `LIGHT_COUNT`), unified by name,
    /// sorted by name.
    pub consts: Vec<ScalarConst>,
    /// Pipeline entry points, one per `@vertex`/`@fragment`/`@compute` function, in `(unit, name)`
    /// order. Vertex inputs are populated only for the vertex stage.
    pub entry_points: Vec<EntryPoint>,
    /// Bind group layouts, unified across units by the set of member resource names (decision 13),
    /// sorted by generated name. Each references entries in [`Reflection::bindings`] by name.
    pub bind_groups: Vec<BindGroupLayout>,
}

impl Reflection {
    /// The struct with this name, if reflected.
    pub fn struct_named(&self, name: &str) -> Option<&StructLayout> {
        self.structs.iter().find(|s| s.name == name)
    }

    /// The resource bound to this variable name, if reflected.
    pub fn binding_named(&self, name: &str) -> Option<&BindingResource> {
        self.bindings.iter().find(|b| b.name == name)
    }

    /// The value of a reflected scalar-integer const, if present.
    pub fn const_value(&self, name: &str) -> Option<i128> {
        self.consts.iter().find(|c| c.name == name).map(|c| c.value)
    }

    /// The bind group layout with this generated name, if reflected.
    pub fn bind_group_named(&self, name: &str) -> Option<&BindGroupLayout> {
        self.bind_groups.iter().find(|g| g.name == name)
    }
}

/// A bind group layout: the resources sharing one `@group` index, unified across shader units by the
/// **set of member resource names** (Plan/shader-bindgen.md decision 13). Distinct name-sets are
/// distinct layouts even when structurally identical — `sprite_tex`/`sprite_sampler` (group 2) and
/// `atlas_tex`/`atlas_sampler` (group 1) reflect as two layouts, not one, so the generated BGL tracks
/// the WGSL's named resources rather than leaning on wgpu's structural bind-group compatibility. A
/// group whose name-set differs (blur's `{src_tex, src_sampler, params}` vs threshold's
/// `{src_tex, src_sampler}`) is a different layout and falls out of the key naturally.
#[derive(Clone, Debug, PartialEq)]
pub struct BindGroupLayout {
    /// Deterministic identifier: the member resource names in ascending binding order, joined by `_`
    /// (e.g. `src_tex_src_sampler`). The generated `create_<name>_bgl` function derives from it.
    pub name: String,
    /// The `@group` index — identical across every unit that declares this layout (a resource name's
    /// group is pinned by binding unification, so a shared name-set implies a shared group).
    pub group: u32,
    /// Member resource variable names, in ascending binding order. Each names an entry in
    /// [`Reflection::bindings`] carrying its kind and merged visibility.
    pub members: Vec<String>,
    /// Shader units that declare this layout (provenance for the emitted doc comment), sorted.
    pub units: Vec<String>,
}

/// The memory layout of a WGSL `struct`, as the naga Layouter (hence `create_shader_module`)
/// computes it: byte `size` including trailing padding, `alignment`, and every field with its
/// resolved `offset`.
#[derive(Clone, Debug, PartialEq)]
pub struct StructLayout {
    pub name: String,
    /// Byte size including trailing padding to `alignment` — what `size_of` on the Rust POD side
    /// must equal.
    pub size: u32,
    pub alignment: u32,
    /// Fields in declaration / offset-ascending order.
    pub fields: Vec<FieldLayout>,
    /// Shader units that declare this struct (provenance for the emitted doc comment), sorted.
    pub units: Vec<String>,
}

/// One field of a [`StructLayout`].
#[derive(Clone, Debug, PartialEq)]
pub struct FieldLayout {
    pub name: String,
    /// Byte offset from the start of the struct.
    pub offset: u32,
    /// Byte size of the field's own type (a following field may sit further along if the layouter
    /// inserted padding — compare successive `offset`s to see the gaps).
    pub size: u32,
    pub ty: TypeRef,
}

/// A reference to a WGSL type as it appears in a field or array element — enough to compare two
/// declarations for layout equality and to inform code generation, without pulling in naga types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeRef {
    Scalar(ScalarKind),
    Vector {
        components: u8,
        scalar: ScalarKind,
    },
    Matrix {
        columns: u8,
        rows: u8,
        scalar: ScalarKind,
    },
    Atomic(ScalarKind),
    Array {
        element: Box<TypeRef>,
        /// Element count; `None` for a runtime-sized array.
        count: Option<u32>,
        /// Byte stride between successive elements (uniform-address-space rounding already applied).
        stride: u32,
    },
    /// A named struct — its layout lives in [`Reflection::structs`] under this name.
    Struct(String),
}

impl fmt::Display for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeRef::Scalar(s) => write!(f, "{s}"),
            TypeRef::Vector { components, scalar } => write!(f, "vec{components}<{scalar}>"),
            TypeRef::Matrix {
                columns,
                rows,
                scalar,
            } => write!(f, "mat{columns}x{rows}<{scalar}>"),
            TypeRef::Atomic(s) => write!(f, "atomic<{s}>"),
            TypeRef::Array {
                element,
                count,
                stride,
            } => match count {
                Some(n) => write!(f, "array<{element}, {n}> (stride {stride})"),
                None => write!(f, "array<{element}> (stride {stride})"),
            },
            TypeRef::Struct(name) => write!(f, "{name}"),
        }
    }
}

/// The scalar kinds a reflected type can carry. WGSL-spellable kinds only; naga's abstract
/// int/float never survive validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarKind {
    F16,
    F32,
    F64,
    I32,
    U32,
    I64,
    U64,
    Bool,
}

impl fmt::Display for ScalarKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ScalarKind::F16 => "f16",
            ScalarKind::F32 => "f32",
            ScalarKind::F64 => "f64",
            ScalarKind::I32 => "i32",
            ScalarKind::U32 => "u32",
            ScalarKind::I64 => "i64",
            ScalarKind::U64 => "u64",
            ScalarKind::Bool => "bool",
        };
        f.write_str(s)
    }
}

/// A resource bound at `@group(g) @binding(b)`, with the stages that actually reference it.
#[derive(Clone, Debug, PartialEq)]
pub struct BindingResource {
    /// The WGSL global variable name — the unification identity across units.
    pub name: String,
    pub group: u32,
    pub binding: u32,
    pub kind: BindingKind,
    /// Stages whose entry points reference this resource (naga `GlobalUse`), deduped and sorted.
    /// This is the derived BGL visibility.
    pub visibility: Vec<ShaderStage>,
    /// Shader units that bind this resource (provenance), sorted.
    pub units: Vec<String>,
}

/// What kind of resource a binding is.
#[derive(Clone, Debug, PartialEq)]
pub enum BindingKind {
    Buffer {
        address_space: BufferSpace,
        /// The reflected byte size of the bound type — the emitted `min_binding_size`. A
        /// runtime-sized array counts as exactly one element (naga's own rule), so a
        /// `var<storage, read> xs: array<T>` binding reports one element's stride: the smallest
        /// buffer the shader can legally read, which is what `min_binding_size` means.
        min_size: u32,
    },
    Texture {
        dim: TextureDim,
        sample: TextureSample,
        multisampled: bool,
    },
    /// A `texture_storage_*` binding — read and/or written by a shader without a sampler.
    StorageTexture {
        dim: TextureDim,
        format: StorageFormat,
        access: StorageAccess,
    },
    Sampler {
        comparison: bool,
    },
}

/// The address space of a buffer binding. `Storage`'s `read_only` mirrors the WGSL access mode
/// (`var<storage, read>` vs `read_write`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferSpace {
    Uniform,
    Storage { read_only: bool },
}

/// Texture dimensionality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureDim {
    D1,
    D2,
    D3,
    Cube,
    D2Array,
    CubeArray,
}

/// The texel formats a storage-texture binding can declare. wgpu-free; the emitter maps each to its
/// `wgpu::TextureFormat`. A WGSL format outside this set is a reflection error naming it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageFormat {
    Rgba8Unorm,
    Rgba16Float,
    Rgba32Float,
    R32Float,
    R32Uint,
    R32Sint,
    Rg32Float,
}

/// How a shader accesses a storage texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageAccess {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

/// How a texture is sampled. `filterable` on `Float` is not expressible in WGSL, so it defaults to
/// `true` (matches every current binding). There is no per-binding override — a real non-filterable
/// binding is what would force one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureSample {
    Float { filterable: bool },
    Depth,
    Sint,
    Uint,
}

/// A module-scope scalar-integer `const`, captured so array bounds like `LIGHT_COUNT` are visible
/// to the generator even when they arrive through the compose-then-reflect preamble.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalarConst {
    pub name: String,
    /// The folded integer value. `i128` holds every WGSL integer width losslessly.
    pub value: i128,
    pub kind: ScalarKind,
    /// Shader units that declare this const (provenance), sorted.
    pub units: Vec<String>,
}

/// A pipeline entry point.
#[derive(Clone, Debug, PartialEq)]
pub struct EntryPoint {
    /// The shader unit that declares it — entry-point names (`vs_main`/`fs_main`) repeat across
    /// units, so this is part of the identity.
    pub unit: String,
    pub name: String,
    pub stage: ShaderStage,
    /// The declared `@workgroup_size`; meaningful for compute entry points only (`[0; 3]` otherwise).
    pub workgroup_size: [u32; 3],
    /// `@location` vertex inputs, sorted by location. Populated only for [`ShaderStage::Vertex`];
    /// other stages carry an empty list (a fragment's location inputs are interpolated varyings,
    /// not vertex-buffer attributes).
    pub inputs: Vec<VertexInput>,
}

/// One `@location(n)` input of a vertex entry point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VertexInput {
    pub location: u32,
    pub name: String,
    pub format: VertexFormat,
}

/// The vertex-attribute formats reflectable from a WGSL input type. Normalization / packing (unorm,
/// snorm, packed) is a CPU-side vertex-buffer decision WGSL does not express, so only the
/// scalar-kind × component-count shape is reflected (Plan/shader-bindgen.md, current-code facts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VertexFormat {
    Float32,
    Float32x2,
    Float32x3,
    Float32x4,
    Sint32,
    Sint32x2,
    Sint32x3,
    Sint32x4,
    Uint32,
    Uint32x2,
    Uint32x3,
    Uint32x4,
}

/// A pipeline stage. Only the graphics/compute stages the engine uses; naga's mesh/task stages are
/// rejected at reflection time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

impl fmt::Display for ShaderStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ShaderStage::Vertex => "vertex",
            ShaderStage::Fragment => "fragment",
            ShaderStage::Compute => "compute",
        };
        f.write_str(s)
    }
}
