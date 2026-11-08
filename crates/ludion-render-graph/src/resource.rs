//! Virtual resource descriptors — the wgpu-free half of the frame graph.
//!
//! A pass never names a `wgpu::Texture`; it names a [`ResourceId`], a plain integer handle into the
//! graph's resource table. The *description* of a resource — its size, pixel format, and intended
//! usage — is expressed with the small enums in this module ([`TextureFormat`], [`TextureUsage`],
//! [`Extent2d`]), not with wgpu types. This is the acknowledged ash-migration seam
//! (`engine-architecture.md` §2): the declarative interface stays free of the execution backend, so
//! swapping wgpu for ash later would replace [`crate::execute`] wholesale and leave this module,
//! the scheduler, and the aliaser untouched.
//!
//! The mapping *to* wgpu lives entirely in [`crate::execute`]; the `to_wgpu*` helpers here are the
//! single crossing point and are deliberately trivial.

use serde::Serialize;

/// A handle to a virtual resource in a [`RenderGraph`](crate::RenderGraph).
///
/// Plain, `Copy`, comparable id — an index into the graph's resource table, optionally narrowed to
/// one mip level with [`mip`](ResourceId::mip). Passes reference resources only through these
/// handles, never through backend textures.
///
/// # Example
/// ```
/// use ludion_render_graph::{RenderGraph, TextureDesc};
///
/// let mut graph = RenderGraph::new();
/// let a: ludion_render_graph::ResourceId = graph.transient("a", TextureDesc::color(64, 64));
/// let b = graph.transient("b", TextureDesc::color(64, 64));
/// assert_ne!(a, b);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ResourceId {
    pub(crate) index: usize,
    pub(crate) mip: Option<u32>,
}

impl ResourceId {
    /// A whole-resource handle for table slot `index`.
    pub(crate) fn whole(index: usize) -> Self {
        ResourceId { index, mip: None }
    }

    /// The raw table index behind this handle.
    pub fn index(self) -> usize {
        self.index
    }

    /// This handle narrowed to mip `level` of the resource: the one way to attach, write, or read a
    /// single level of a mipped transient.
    ///
    /// # Panics
    /// If the handle already names a mip.
    ///
    /// # Example
    /// ```
    /// use ludion_render_graph::{RenderGraph, TextureDesc};
    ///
    /// let mut graph = RenderGraph::new();
    /// let pyramid = graph.transient("pyramid", TextureDesc::color(64, 64).with_mip_levels(3));
    /// assert_eq!(pyramid.mip_level(), None);
    /// assert_eq!(pyramid.mip(2).mip_level(), Some(2));
    /// assert_eq!(pyramid.mip(2).index(), pyramid.index());
    /// ```
    pub fn mip(self, level: u32) -> ResourceId {
        assert!(
            self.mip.is_none(),
            "resource handle already names mip {}",
            self.mip.unwrap()
        );
        ResourceId {
            index: self.index,
            mip: Some(level),
        }
    }

    /// The mip level this handle names, or `None` for the whole resource.
    pub fn mip_level(self) -> Option<u32> {
        self.mip
    }

    /// Whether two accesses touch common texels: same resource, and either is whole or both name
    /// the same mip. Dependency edges use this, never `==`.
    pub(crate) fn overlaps(self, other: ResourceId) -> bool {
        self.index == other.index
            && (self.mip.is_none() || other.mip.is_none() || self.mip == other.mip)
    }
}

/// Pixel dimensions of a render target's base slice (color, depth, or one z-slice of a volume). A
/// 3D resource carries its slice count on [`TextureDesc::depth_slices`]; array textures are out of
/// scope.
///
/// # Example
/// ```
/// use ludion_render_graph::Extent2d;
/// let e = Extent2d { width: 1920, height: 1080 };
/// assert_eq!(e.width * e.height, 2_073_600);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub struct Extent2d {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// The pixel format of a virtual texture.
///
/// Deliberately tiny — this enum is the mapping point to `wgpu::TextureFormat`. New formats are
/// added as passes demand them, one obvious variant each.
///
/// The reflection split's extra colour targets reuse existing variants: world normal + roughness
/// is [`Rgba16Float`](TextureFormat::Rgba16Float) (8-bit octahedral normals band in sharp
/// reflections, and 16-bit unorm formats are not renderable on the WebGPU floor); F0 + specular
/// occlusion is [`Rgba8UnormSrgb`](TextureFormat::Rgba8UnormSrgb) (sRGB gives the dielectric F0
/// range ~0.02–0.05 about five times the codes of linear 8-bit; alpha stays linear). Together with
/// the scene's `Rgba16Float` that is 24 render-target bytes per sample, under the WebGPU floor's
/// 32.
///
/// [`R32Float`](TextureFormat::R32Float) is a single-channel 32-bit float, **unfilterable**, used
/// for depth pyramids: standard-Z depth in fp16 has 2^-11 spacing near 1, where the scene lives.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureFormat {
    /// 8-bit-per-channel RGBA in the sRGB color space — the final, read-back presentation format.
    Rgba8UnormSrgb,
    /// 16-bit float RGBA — the HDR working format for the scene and bloom passes, where colors
    /// exceed 1.0 before tonemapping. Never read back directly (the tonemap pass resolves to
    /// [`Rgba8UnormSrgb`](TextureFormat::Rgba8UnormSrgb)).
    Rgba16Float,
    /// 32-bit float, one channel — a depth pyramid's format. Renderable on the WebGPU floor but
    /// unfilterable: read it with `textureLoad`.
    R32Float,
    /// 32-bit float RGBA, unfilterable: read with `textureLoad`; a compute pass's exact-f32 output.
    Rgba32Float,
    /// 16-bit float, two channels, blendable on the WebGPU floor — the transparent layer target
    /// (weighted distance and coverage).
    Rg16Float,
    /// 8-bit normalized, one channel, blendable — a per-pixel scalar mask in [0, 1].
    R8Unorm,
    /// 32-bit float depth — the scene pass's depth attachment, and (once a pass samples it) a shadow
    /// map. A depth format is never a color attachment and never read back for presentation; it is
    /// attached with [`TextureUsage::RenderAttachment`] and, when a later pass samples it,
    /// [`TextureUsage::TextureBinding`].
    Depth32Float,
}

impl TextureFormat {
    /// The size in bytes of one texel in this format.
    pub fn bytes_per_texel(self) -> u32 {
        match self {
            TextureFormat::Rgba8UnormSrgb => 4,
            TextureFormat::Rgba16Float => 8,
            TextureFormat::R32Float => 4,
            TextureFormat::Rgba32Float => 16,
            TextureFormat::Rg16Float => 4,
            TextureFormat::R8Unorm => 1,
            TextureFormat::Depth32Float => 4,
        }
    }

    /// The lower-case token used for this format in `dump-graph` JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            TextureFormat::Rgba8UnormSrgb => "rgba8unorm_srgb",
            TextureFormat::Rgba16Float => "rgba16float",
            TextureFormat::R32Float => "r32float",
            TextureFormat::Rgba32Float => "rgba32float",
            TextureFormat::Rg16Float => "rg16float",
            TextureFormat::R8Unorm => "r8unorm",
            TextureFormat::Depth32Float => "depth32_float",
        }
    }

    /// Maps to the concrete `wgpu` format. The single crossing point into the backend.
    pub(crate) fn to_wgpu(self) -> wgpu::TextureFormat {
        match self {
            TextureFormat::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            TextureFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
            TextureFormat::R32Float => wgpu::TextureFormat::R32Float,
            TextureFormat::Rgba32Float => wgpu::TextureFormat::Rgba32Float,
            TextureFormat::Rg16Float => wgpu::TextureFormat::Rg16Float,
            TextureFormat::R8Unorm => wgpu::TextureFormat::R8Unorm,
            TextureFormat::Depth32Float => wgpu::TextureFormat::Depth32Float,
        }
    }
}

/// One intended use of a virtual texture. A resource declares a set of these; the executor unions
/// the sets of resources that share a physical texture (see [`Aliasing`](crate::Aliasing)) into the physical
/// texture's real `wgpu::TextureUsages`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureUsage {
    /// The texture is drawn into as a render-pass color attachment.
    RenderAttachment,
    /// The texture is sampled by a later pass's fragment shader.
    TextureBinding,
    /// The texture is bound as a writable storage texture by a compute pass
    /// ([`PassKind::Compute`](crate::PassKind)).
    StorageBinding,
    /// The texture is the source of a copy (e.g. the final target, copied out for readback).
    CopySrc,
}

impl TextureUsage {
    /// The lower-case token used in `dump-graph` JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            TextureUsage::RenderAttachment => "render_attachment",
            TextureUsage::TextureBinding => "texture_binding",
            TextureUsage::StorageBinding => "storage_binding",
            TextureUsage::CopySrc => "copy_src",
        }
    }

    fn to_wgpu(self) -> wgpu::TextureUsages {
        match self {
            TextureUsage::RenderAttachment => wgpu::TextureUsages::RENDER_ATTACHMENT,
            TextureUsage::TextureBinding => wgpu::TextureUsages::TEXTURE_BINDING,
            TextureUsage::StorageBinding => wgpu::TextureUsages::STORAGE_BINDING,
            TextureUsage::CopySrc => wgpu::TextureUsages::COPY_SRC,
        }
    }
}

/// A handle to a graph-owned buffer in a [`RenderGraph`](crate::RenderGraph)'s buffer table.
///
/// A separate type from [`ResourceId`]: a buffer can never be attached or sampled as a texture.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BufferId(pub(crate) usize);

impl BufferId {
    /// The raw table index behind this handle.
    pub fn index(self) -> usize {
        self.0
    }
}

/// One intended use of a graph buffer; a buffer declares a set of these.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferUsage {
    /// Bound as a storage buffer (read or read-write) by a compute or render pass.
    Storage,
    /// Bound as a vertex or instance buffer.
    Vertex,
    /// The source of an indirect draw or dispatch.
    Indirect,
    /// The source of a copy.
    CopySrc,
    /// The destination of a copy or `clear_buffer`.
    CopyDst,
}

impl BufferUsage {
    /// The lower-case token used in `dump-graph` JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            BufferUsage::Storage => "storage",
            BufferUsage::Vertex => "vertex",
            BufferUsage::Indirect => "indirect",
            BufferUsage::CopySrc => "copy_src",
            BufferUsage::CopyDst => "copy_dst",
        }
    }

    fn to_wgpu(self) -> wgpu::BufferUsages {
        match self {
            BufferUsage::Storage => wgpu::BufferUsages::STORAGE,
            BufferUsage::Vertex => wgpu::BufferUsages::VERTEX,
            BufferUsage::Indirect => wgpu::BufferUsages::INDIRECT,
            BufferUsage::CopySrc => wgpu::BufferUsages::COPY_SRC,
            BufferUsage::CopyDst => wgpu::BufferUsages::COPY_DST,
        }
    }
}

/// A graph buffer's description: byte size and usage set.
///
/// A graph buffer is frame-transient and graph-owned: the executor allocates it, it never aliases
/// another buffer, and every read needs a writer in the graph. The executor caches the allocation
/// across frames, keyed by the description, so **round capacities up** (for example to a power of
/// two): a size change reallocates. A reused buffer **keeps the previous frame's bytes**; a consumer
/// that needs zeros clears the buffer in its own pass (`encoder.clear_buffer`, which needs
/// [`BufferUsage::CopyDst`]).
///
/// # Example
/// ```
/// use ludion_render_graph::{BufferDesc, BufferUsage};
///
/// let indirect_args = BufferDesc::new(20, &[BufferUsage::Storage, BufferUsage::Indirect]);
/// let instances = BufferDesc::new(64 * 1024, &[BufferUsage::Vertex, BufferUsage::Storage]);
/// let counter = BufferDesc::new(4, &[BufferUsage::Storage, BufferUsage::CopySrc]);
/// assert_eq!(instances.usages, [BufferUsage::Storage, BufferUsage::Vertex]);
/// assert_eq!((indirect_args.size, counter.size), (20, 4));
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BufferDesc {
    /// Size in bytes; a non-zero multiple of 4.
    pub size: u64,
    /// The usages, sorted and deduplicated so `dump-graph` output is stable.
    pub usages: Vec<BufferUsage>,
}

impl BufferDesc {
    /// A buffer of `size` bytes with the given usages.
    ///
    /// # Panics
    /// If `size` is zero or not a multiple of 4 (the WebGPU copy and storage alignment), or `usages`
    /// is empty.
    pub fn new(size: u64, usages: &[BufferUsage]) -> Self {
        assert!(size != 0, "a graph buffer cannot be zero bytes");
        assert!(
            size.is_multiple_of(4),
            "graph buffer size {size} is not a multiple of 4 (the WebGPU copy and storage alignment)",
        );
        assert!(
            !usages.is_empty(),
            "a graph buffer must declare at least one BufferUsage",
        );
        let mut usages = usages.to_vec();
        usages.sort();
        usages.dedup();
        BufferDesc { size, usages }
    }

    /// The union of the usages as wgpu flags. The single crossing point into the backend.
    pub(crate) fn wgpu_usages(&self) -> wgpu::BufferUsages {
        self.usages
            .iter()
            .fold(wgpu::BufferUsages::empty(), |acc, u| acc | u.to_wgpu())
    }
}

/// A virtual texture's full description: size, dimensionality, format, and the set of usages it
/// participates in.
///
/// Two descriptions are *alias-compatible* ([`TextureDesc::alias_compatible`]) when their size,
/// slice count, and format match; usage sets may differ and are unioned when the resources share a
/// physical texture.
///
/// # Example
/// ```
/// use ludion_render_graph::{TextureDesc, TextureFormat, TextureUsage};
///
/// let target = TextureDesc::color(1920, 1080);
/// assert_eq!(target.format, TextureFormat::Rgba8UnormSrgb);
/// assert!(target.usages.contains(&TextureUsage::RenderAttachment));
/// assert_eq!(target.depth_slices, None);
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TextureDesc {
    /// The texture's pixel dimensions (of one z-slice, for a volume).
    pub extent: Extent2d,
    /// The z-slice count when this resource is a **3D** texture; `None` for an ordinary 2D one.
    /// This `Option` *is* the dimensionality — there is no separate dimension enum, so a 2D and a 3D
    /// resource can never be confused for one another (or aliased onto one physical texture).
    pub depth_slices: Option<u32>,
    /// The texture's pixel format.
    pub format: TextureFormat,
    /// The mip level count; 1 for an unmipped texture. A mipped resource is written one level at a
    /// time (see [`ResourceId::mip`]).
    pub mip_levels: u32,
    /// The usages this virtual resource requires. Kept sorted and deduplicated by the constructors
    /// so `dump-graph` output is stable.
    pub usages: Vec<TextureUsage>,
}

impl TextureDesc {
    /// A color render target that is also sampleable by a downstream pass — the default shape of an
    /// intermediate (transient) texture in the graph.
    pub fn color(width: u32, height: u32) -> Self {
        TextureDesc {
            extent: Extent2d { width, height },
            depth_slices: None,
            mip_levels: 1,
            format: TextureFormat::Rgba8UnormSrgb,
            usages: vec![TextureUsage::RenderAttachment, TextureUsage::TextureBinding],
        }
    }

    /// An HDR (16-bit float) color target that is also sampleable downstream — the working format of
    /// the scene and bloom passes, where lit colors run past 1.0 before tonemapping.
    pub fn hdr_color(width: u32, height: u32) -> Self {
        TextureDesc {
            extent: Extent2d { width, height },
            depth_slices: None,
            mip_levels: 1,
            format: TextureFormat::Rgba16Float,
            usages: vec![TextureUsage::RenderAttachment, TextureUsage::TextureBinding],
        }
    }

    /// A two-channel 16-bit float color target that is also sampleable downstream — the transparent
    /// layer (see [`TextureFormat::Rg16Float`]).
    pub fn layer_color(width: u32, height: u32) -> Self {
        TextureDesc {
            extent: Extent2d { width, height },
            depth_slices: None,
            mip_levels: 1,
            format: TextureFormat::Rg16Float,
            usages: vec![TextureUsage::RenderAttachment, TextureUsage::TextureBinding],
        }
    }

    /// A single-channel 8-bit normalized color target that is also sampleable downstream — a
    /// per-pixel scalar mask (see [`TextureFormat::R8Unorm`]).
    pub fn mask_color(width: u32, height: u32) -> Self {
        TextureDesc {
            extent: Extent2d { width, height },
            depth_slices: None,
            mip_levels: 1,
            format: TextureFormat::R8Unorm,
            usages: vec![TextureUsage::RenderAttachment, TextureUsage::TextureBinding],
        }
    }

    /// A single-channel 32-bit float color target that is also sampleable downstream — a depth
    /// pyramid's level (see [`TextureFormat::R32Float`]).
    ///
    /// # Example
    /// ```
    /// use ludion_render_graph::{TextureDesc, TextureFormat};
    ///
    /// assert_eq!(TextureDesc::r32_float(8, 8).format, TextureFormat::R32Float);
    /// ```
    pub fn r32_float(width: u32, height: u32) -> Self {
        TextureDesc {
            extent: Extent2d { width, height },
            depth_slices: None,
            mip_levels: 1,
            format: TextureFormat::R32Float,
            usages: vec![TextureUsage::RenderAttachment, TextureUsage::TextureBinding],
        }
    }

    /// An HDR (16-bit float) **3D** texture a compute pass writes as a storage texture and a later
    /// pass samples — the shape of a probe-lattice volume. `depth` is the z-slice count; it is a real
    /// dimension, not an array layer.
    ///
    /// It carries [`StorageBinding`](TextureUsage::StorageBinding) (the compute write) and
    /// [`TextureBinding`](TextureUsage::TextureBinding) (the downstream read), and never
    /// [`RenderAttachment`](TextureUsage::RenderAttachment): a volume cannot be a pass attachment.
    ///
    /// # Example
    /// ```
    /// use ludion_render_graph::{TextureDesc, TextureUsage};
    ///
    /// let volume = TextureDesc::hdr_storage_volume(16, 8, 16);
    /// assert_eq!(volume.depth_slices, Some(16));
    /// assert!(volume.usages.contains(&TextureUsage::StorageBinding));
    /// ```
    pub fn hdr_storage_volume(width: u32, height: u32, depth: u32) -> Self {
        TextureDesc {
            extent: Extent2d { width, height },
            depth_slices: Some(depth),
            mip_levels: 1,
            format: TextureFormat::Rgba16Float,
            usages: vec![TextureUsage::TextureBinding, TextureUsage::StorageBinding],
        }
    }

    /// A 32-bit float RGBA **2D** texture a compute pass writes as a storage texture and a later
    /// pass reads with `textureLoad` (see [`TextureFormat::Rgba32Float`]) — a per-index table of exact
    /// f32 results, one texel each.
    ///
    /// It carries [`StorageBinding`](TextureUsage::StorageBinding) (the compute write) and
    /// [`TextureBinding`](TextureUsage::TextureBinding) (the downstream read), and never
    /// [`RenderAttachment`](TextureUsage::RenderAttachment).
    ///
    /// # Example
    /// ```
    /// use ludion_render_graph::{TextureDesc, TextureFormat, TextureUsage};
    ///
    /// let table = TextureDesc::rgba32_storage(32, 1);
    /// assert_eq!(table.format, TextureFormat::Rgba32Float);
    /// assert!(table.usages.contains(&TextureUsage::StorageBinding));
    /// ```
    pub fn rgba32_storage(width: u32, height: u32) -> Self {
        TextureDesc {
            extent: Extent2d { width, height },
            depth_slices: None,
            mip_levels: 1,
            format: TextureFormat::Rgba32Float,
            usages: vec![TextureUsage::TextureBinding, TextureUsage::StorageBinding],
        }
    }

    /// The final presentation target: a color render target that is copied out for readback (hence
    /// [`TextureUsage::CopySrc`]) rather than sampled.
    pub fn present_target(width: u32, height: u32) -> Self {
        TextureDesc {
            extent: Extent2d { width, height },
            depth_slices: None,
            mip_levels: 1,
            format: TextureFormat::Rgba8UnormSrgb,
            usages: vec![TextureUsage::RenderAttachment, TextureUsage::CopySrc],
        }
    }

    /// A depth attachment ([`TextureFormat::Depth32Float`], [`TextureUsage::RenderAttachment`]) — the
    /// scene pass's depth buffer. Not sampled: nothing reads it, so its usage set is attachment-only
    /// and the recorder discards its store. A shadow map (a depth texture a *later* pass samples) is
    /// [`sampled_depth`](TextureDesc::sampled_depth) instead.
    pub fn depth(width: u32, height: u32) -> Self {
        TextureDesc {
            extent: Extent2d { width, height },
            depth_slices: None,
            mip_levels: 1,
            format: TextureFormat::Depth32Float,
            usages: vec![TextureUsage::RenderAttachment],
        }
    }

    /// A *sampled* depth texture ([`TextureFormat::Depth32Float`], both
    /// [`RenderAttachment`](TextureUsage::RenderAttachment) and
    /// [`TextureBinding`](TextureUsage::TextureBinding)) — a shadow map: written depth-only by one
    /// pass, then sampled (a comparison sampler) by a later pass. The store survives the frame
    /// (unlike the discard-on-store [`depth`](TextureDesc::depth) attachment) because a downstream
    /// pass reads it.
    pub fn sampled_depth(width: u32, height: u32) -> Self {
        TextureDesc {
            extent: Extent2d { width, height },
            depth_slices: None,
            mip_levels: 1,
            format: TextureFormat::Depth32Float,
            usages: vec![TextureUsage::RenderAttachment, TextureUsage::TextureBinding],
        }
    }

    /// This description with a `levels`-deep mip chain.
    ///
    /// # Panics
    /// If `levels` is 0, the texture is a volume, or `levels` exceeds the full chain for the extent.
    pub fn with_mip_levels(mut self, levels: u32) -> Self {
        assert!(levels >= 1, "a texture needs at least one mip level");
        assert!(self.depth_slices.is_none(), "a 3D texture cannot be mipped");
        let full = 32 - self.extent.width.max(self.extent.height).leading_zeros();
        assert!(
            levels <= full,
            "{levels} mip levels exceed the full chain ({full}) of a {}x{} texture",
            self.extent.width,
            self.extent.height,
        );
        self.mip_levels = levels;
        self
    }

    /// The pixel dimensions of mip `level`: each side halves per level, floored at 1.
    pub fn mip_extent(&self, level: u32) -> Extent2d {
        Extent2d {
            width: (self.extent.width >> level).max(1),
            height: (self.extent.height >> level).max(1),
        }
    }

    /// Whether two descriptions may share one physical texture: identical size, slice count, mip
    /// count, and format. Usage sets need not match — they are unioned by the aliaser.
    pub fn alias_compatible(&self, other: &TextureDesc) -> bool {
        self.extent == other.extent
            && self.depth_slices == other.depth_slices
            && self.mip_levels == other.mip_levels
            && self.format == other.format
    }

    /// The union of two usage lists, sorted and deduplicated — the physical texture's usage when
    /// two resources alias.
    pub(crate) fn union_usages(a: &[TextureUsage], b: &[TextureUsage]) -> Vec<TextureUsage> {
        let mut merged: Vec<TextureUsage> = a.iter().chain(b.iter()).copied().collect();
        merged.sort_unstable();
        merged.dedup();
        merged
    }

    /// The wgpu usage bitset for a physical texture built from `usages`.
    pub(crate) fn wgpu_usages(usages: &[TextureUsage]) -> wgpu::TextureUsages {
        usages
            .iter()
            .fold(wgpu::TextureUsages::empty(), |acc, u| acc | u.to_wgpu())
    }
}
