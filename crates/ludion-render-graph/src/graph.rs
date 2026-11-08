//! The declarative frame graph: virtual resources and passes with their read/write declarations.
//!
//! This is pure CPU data with no wgpu in sight. A [`RenderGraph`] is built, then [`schedule`](fn@crate::schedule)d into
//! an executable, dumpable [`Schedule`](crate::schedule::Schedule); the schedule is what
//! [`crate::execute`] runs on the GPU and what [`crate::dump`] serializes.

use crate::resource::{BufferDesc, BufferId, BufferUsage, ResourceId, TextureDesc, TextureUsage};

/// Whether a resource is an internal scratch texture the graph fully owns, or the external
/// presentation target read back at the end of the frame.
///
/// Only [`ResourceKind::Transient`] resources participate in aliasing; the [`ResourceKind::Output`]
/// target always gets its own dedicated physical texture, because it must survive to be copied out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResourceKind {
    /// An internal texture, alias-eligible against other transients with disjoint lifetimes.
    Transient,
    /// The final render target, copied out for readback; never aliased.
    Output,
}

/// A declared virtual resource: its name, description, and kind.
#[derive(Clone, Debug)]
pub struct ResourceDecl {
    /// The resource's stable, human-readable name (also its `dump-graph` key).
    pub name: String,
    /// The resource's virtual texture description.
    pub desc: TextureDesc,
    /// Whether the resource is transient or the output target.
    pub kind: ResourceKind,
}

/// A declared graph buffer: its name and description. See [`BufferDesc`] for the lifetime rules.
#[derive(Clone, Debug)]
pub struct BufferDecl {
    /// The buffer's stable, human-readable name (also its `dump-graph` key and wgpu label).
    pub name: String,
    /// The buffer's size and usages.
    pub desc: BufferDesc,
}

/// A handle to a declared pass, returned by every `add_*` so buffer reads can be attached with
/// [`RenderGraph::read_buffers`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PassId(usize);

/// What a pass actually draws. Plain data — the executor ([`crate::execute`]) matches on it to pick
/// a pipeline and bindings. Keeping the *work* as a data enum (rather than a wgpu closure) is what
/// lets the declarative graph stay backend-free: a pass's inputs are [`ResourceId`]s here and become
/// texture views only inside the executor.
///
/// New pass types are added as one variant plus one match arm in the executor — regular and
/// grep-able, no genericity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PassKind {
    /// Fill the target with a deterministic procedural gradient. Reads nothing. Used by the fixed
    /// proving graph ([`crate::build_graph`]).
    Gradient,
    /// Sample `source` full-screen and apply a fixed vignette into the target. The proving graph's
    /// one sampling-pass shape; chaining it several times gives the graph real dependency edges and
    /// transient lifetimes to alias.
    Vignette {
        /// The texture this pass samples.
        source: ResourceId,
    },
    /// A pass whose GPU work is recorded by an external [`PassRecorder`](crate::PassRecorder), not by
    /// this crate. The graph still schedules it and aliases its resources from the declared reads and
    /// write; only the draw recording lives elsewhere (in `ludion-renderer`). `label` is a stable
    /// token used to dispatch the recorder and to name the pass kind in `dump-graph`.
    ///
    /// The reads of an external pass are *not* carried in the kind (a `Vec` would cost `Copy`); they
    /// are declared alongside the pass via [`RenderGraph::add_external_pass`] and stored on the
    /// [`PassDecl`].
    External {
        /// A stable, static dispatch/label token (e.g. `"scene"`, `"tonemap"`).
        label: &'static str,
    },
    /// A pass that dispatches **compute** work instead of drawing: no color target and no depth
    /// attachment, one storage write (a texture, or buffers), and reads resolved exactly as a render pass's are. Its
    /// GPU work is recorded by the same external [`PassRecorder`](crate::PassRecorder), which opens
    /// the compute pass with [`RecordedPass::begin_compute`](crate::RecordedPass::begin_compute) and
    /// issues its own dispatch — the graph stays ignorant of workgroup shapes.
    ///
    /// Ordering against later passes comes from the ordinary read edge: a pass that reads the
    /// storage resource this one writes is scheduled after it.
    Compute {
        /// A stable, static dispatch/label token (e.g. `"gi_inject"`).
        label: &'static str,
    },
}

impl PassKind {
    /// The resources a *built-in* kind reads, derived from the kind. Externally-recorded passes
    /// (render and compute alike) carry their reads on the [`PassDecl`] instead (see
    /// [`PassDecl::reads`]), so this returns empty for them.
    pub fn intrinsic_reads(self) -> Vec<ResourceId> {
        match self {
            PassKind::Gradient | PassKind::External { .. } | PassKind::Compute { .. } => Vec::new(),
            PassKind::Vignette { source } => vec![source],
        }
    }

    /// The lower-case token used for this pass kind in `dump-graph` JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            PassKind::Gradient => "gradient",
            PassKind::Vignette { .. } => "vignette",
            PassKind::External { label } | PassKind::Compute { label } => label,
        }
    }

    /// Whether this kind dispatches compute work rather than drawing.
    pub fn is_compute(self) -> bool {
        matches!(self, PassKind::Compute { .. })
    }
}

/// A declared pass: a name, the work it does, and the resources it writes.
///
/// A pass writes any number of color targets (one extent between them and the depth attachment), at
/// most one depth attachment, and at most one storage texture; a compute pass may instead write graph
/// buffers. All are optional so the graph can
/// express every shape the engine needs: a color-only post pass ([`color`](PassDecl::color) set),
/// the scene pass (color + depth), a multi-target pass (several colors), a depth-only shadow pass
/// (only `depth`), and a compute pass (only [`storage`](PassDecl::storage)). Its reads are stored on
/// [`reads`](PassDecl::reads) — derived from the kind for built-ins, declared explicitly for
/// externally-recorded passes. The scheduler treats the depth attachment and the storage write
/// exactly like the color target, so a later pass that samples either orders after this one. Passes
/// that write the same subresource run in declaration order.
#[derive(Clone, Debug)]
pub struct PassDecl {
    /// The pass's stable, human-readable name.
    pub name: String,
    /// What the pass draws.
    pub kind: PassKind,
    /// The resources this pass samples. For built-in kinds this is derived from
    /// [`PassKind::intrinsic_reads`]; for externally-recorded passes it is supplied at declaration
    /// time. Either way it is the single source of truth the scheduler and aliaser consult.
    pub reads: Vec<ResourceId>,
    /// The color resources this pass renders into, in attachment order. Empty for a depth-only or
    /// compute pass.
    pub color: Vec<ResourceId>,
    /// The depth resource this pass renders into, if any.
    pub depth: Option<ResourceId>,
    /// The storage texture this pass writes, if any — set only by [`PassKind::Compute`]. A storage
    /// write is not an attachment: it is bound into the recorder's own bind group.
    pub storage: Option<ResourceId>,
    /// The graph buffers this pass reads (vertex, indirect or storage source), set only through
    /// [`RenderGraph::read_buffers`].
    pub buffer_reads: Vec<BufferId>,
    /// The graph buffers this pass writes — set only by
    /// [`RenderGraph::add_external_buffer_compute_pass`].
    pub buffer_writes: Vec<BufferId>,
}

impl PassDecl {
    /// The resources this pass reads.
    pub fn reads(&self) -> Vec<ResourceId> {
        self.reads.clone()
    }

    /// The pass's color attachments, in attachment order.
    pub fn color_targets(&self) -> &[ResourceId] {
        &self.color
    }

    /// The pass's depth attachment, if it has one.
    pub fn depth_target(&self) -> Option<ResourceId> {
        self.depth
    }

    /// The storage texture this pass writes, if it has one (a compute pass).
    pub fn storage_target(&self) -> Option<ResourceId> {
        self.storage
    }

    /// Every resource this pass writes — color targets in declared order, then depth attachment, then
    /// storage texture, whichever are present. The scheduler and aliaser consult this for dependency edges and
    /// lifetimes; the fixed order makes `dump-graph`'s `writes` array canonical.
    pub fn writes(&self) -> Vec<ResourceId> {
        let mut writes = Vec::new();
        writes.extend(self.color.iter().copied());
        writes.extend(self.depth);
        writes.extend(self.storage);
        writes
    }
}

/// A frame graph under construction: a list of virtual resources and the passes that read and write
/// them.
///
/// Build it with [`transient`](RenderGraph::transient) / [`output`](RenderGraph::output) to declare
/// resources and [`add_pass`](RenderGraph::add_pass) to declare passes, then call
/// [`schedule`](fn@crate::schedule) to derive execution order and transient aliasing.
///
/// # Example
/// ```
/// use ludion_render_graph::{PassKind, RenderGraph, TextureDesc};
///
/// let mut graph = RenderGraph::new();
/// let scene = graph.transient("scene", TextureDesc::color(64, 64));
/// let final_target = graph.output("final", TextureDesc::present_target(64, 64));
/// graph.add_pass("gradient", PassKind::Gradient, scene);
/// graph.add_pass("present", PassKind::Vignette { source: scene }, final_target);
///
/// let schedule = ludion_render_graph::schedule(&graph).expect("acyclic, fully written");
/// assert_eq!(schedule.order_names(), ["gradient", "present"]);
/// ```
#[derive(Clone, Debug, Default)]
pub struct RenderGraph {
    resources: Vec<ResourceDecl>,
    buffers: Vec<BufferDecl>,
    passes: Vec<PassDecl>,
}

impl RenderGraph {
    /// Creates an empty graph.
    pub fn new() -> Self {
        RenderGraph::default()
    }

    /// Declares a transient (internal, alias-eligible) resource and returns its handle.
    pub fn transient(&mut self, name: impl Into<String>, desc: TextureDesc) -> ResourceId {
        self.push_resource(name.into(), desc, ResourceKind::Transient)
    }

    /// Declares the output (presentation) resource and returns its handle. A graph may declare more
    /// than one output-kind resource, but the executor reads back exactly the one named as the
    /// graph's target when it renders.
    ///
    /// # Panics
    /// If `desc` is a 3D or mipped texture: readback packs one 2D image, so neither can be an output.
    pub fn output(&mut self, name: impl Into<String>, desc: TextureDesc) -> ResourceId {
        let name = name.into();
        assert!(
            desc.depth_slices.is_none(),
            "output resource `{name}` is a 3D texture; readback produces one 2D image, so a volume \
             cannot be a graph output",
        );
        assert!(
            desc.mip_levels == 1,
            "output resource `{name}` has {} mip levels; readback packs one image, so an output \
             cannot be mipped",
            desc.mip_levels,
        );
        self.push_resource(name, desc, ResourceKind::Output)
    }

    /// Rejects a 3D resource in a position that must be a flat texture — a color or depth
    /// attachment. A volume is written by a compute pass and sampled; it is never attached. This is
    /// a construction bug in the caller, the same class as indexing an empty
    /// [`RecordedPass::colors`](crate::RecordedPass::colors) on a depth-only pass.
    fn require_flat(&self, id: ResourceId, role: &str) {
        let decl = &self.resources[id.index()];
        assert!(
            decl.desc.depth_slices.is_none(),
            "resource `{}` is a 3D texture and cannot be a pass {role}",
            decl.name,
        );
    }

    /// Rejects a mip index past the resource's chain.
    fn require_mip_in_range(&self, id: ResourceId) {
        let decl = &self.resources[id.index()];
        if let Some(level) = id.mip_level() {
            assert!(
                level < decl.desc.mip_levels,
                "resource `{}` has {} mip level(s); mip {level} does not exist",
                decl.name,
                decl.desc.mip_levels,
            );
        }
    }

    /// Rejects a handle that cannot be a render attachment: a volume, a mip past the chain, or a
    /// whole handle to a mipped resource (wgpu attachments cover exactly one level, so a write must
    /// name which).
    fn require_attachable(&self, id: ResourceId, role: &str) {
        self.require_flat(id, role);
        self.require_mip_in_range(id);
        let decl = &self.resources[id.index()];
        assert!(
            decl.desc.mip_levels == 1 || id.mip_level().is_some(),
            "resource `{}` has {} mip levels; a pass {role} must name one with `.mip(level)`",
            decl.name,
            decl.desc.mip_levels,
        );
    }

    /// The pixel extent an attachment handle covers: the named mip's, or the base level's.
    fn attachment_extent(&self, id: ResourceId) -> crate::resource::Extent2d {
        self.resources[id.index()]
            .desc
            .mip_extent(id.mip_level().unwrap_or(0))
    }

    /// Rejects colour/depth attachments of differing extents (wgpu requires one per pass) and a
    /// resource attached twice among the colours.
    fn require_one_extent(&self, name: &str, colors: &[ResourceId], depth: Option<ResourceId>) {
        let mut attachments = colors.iter().chain(depth.iter());
        let Some(first) = attachments.next() else {
            return;
        };
        let extent = self.attachment_extent(*first);
        for id in attachments {
            assert!(
                self.attachment_extent(*id) == extent,
                "pass `{name}` attaches `{}` at {}x{} but `{}` at {}x{}; one pass's attachments \
                 must share an extent",
                self.resources[first.index()].name,
                extent.width,
                extent.height,
                self.resources[id.index()].name,
                self.attachment_extent(*id).width,
                self.attachment_extent(*id).height,
            );
        }
        for (i, color) in colors.iter().enumerate() {
            assert!(
                !colors[..i].contains(color),
                "pass `{name}` lists color target `{}` twice",
                self.resources[color.index()].name,
            );
        }
    }

    fn require_valid_reads(&self, reads: &[ResourceId]) {
        for read in reads {
            self.require_mip_in_range(*read);
        }
    }

    /// Rejects a compute pass whose storage target was not declared writable. Without
    /// [`TextureUsage::StorageBinding`] the physical texture is created without the wgpu usage and
    /// the recorder's bind group fails at record time, far from the declaration that caused it.
    fn require_storage(&self, id: ResourceId) {
        self.require_mip_in_range(id);
        let decl = &self.resources[id.index()];
        assert!(
            decl.desc.mip_levels == 1 || id.mip_level().is_some(),
            "compute pass storage target `{}` has {} mip levels; name one with `.mip(level)`",
            decl.name,
            decl.desc.mip_levels,
        );
        assert!(
            decl.desc.usages.contains(&TextureUsage::StorageBinding),
            "compute pass storage target `{}` does not declare TextureUsage::StorageBinding; \
             declare it with TextureDesc::hdr_storage_volume, or add the usage",
            decl.name,
        );
    }

    fn push_resource(&mut self, name: String, desc: TextureDesc, kind: ResourceKind) -> ResourceId {
        let id = ResourceId::whole(self.resources.len());
        self.resources.push(ResourceDecl { name, desc, kind });
        id
    }

    /// Declares a built-in pass that draws `kind` into `target`. Reads are derived from `kind`.
    ///
    /// # Panics
    /// If `target` is not attachable (a 3D resource, or a mipped one without a mip).
    pub fn add_pass(
        &mut self,
        name: impl Into<String>,
        kind: PassKind,
        target: ResourceId,
    ) -> PassId {
        self.require_attachable(target, "color target");
        let reads = kind.intrinsic_reads();
        self.passes.push(PassDecl {
            name: name.into(),
            kind,
            reads,
            color: vec![target],
            depth: None,
            storage: None,
            buffer_reads: Vec::new(),
            buffer_writes: Vec::new(),
        });
        PassId(self.passes.len() - 1)
    }

    /// Declares an externally-recorded pass ([`PassKind::External`]): it samples `reads` and writes
    /// the `colors` targets, and its GPU work is recorded by the [`PassRecorder`](crate::PassRecorder)
    /// passed to [`Gpu::execute`](crate::Gpu::execute). `label` is the recorder's dispatch token and
    /// the pass kind shown in `dump-graph`.
    ///
    /// This is how `ludion-renderer` builds its pipeline: the graph owns scheduling, aliasing,
    /// and the JSON dump, while the renderer owns the pipelines and draw calls — the declarative
    /// interface stays free of wgpu types.
    ///
    /// # Panics
    /// If `colors` is empty (use [`add_external_pass_with_depth`](RenderGraph::add_external_pass_with_depth)
    /// for a depth-only pass), a target is not attachable, or the targets differ in extent.
    pub fn add_external_pass(
        &mut self,
        name: impl Into<String>,
        label: &'static str,
        reads: Vec<ResourceId>,
        colors: Vec<ResourceId>,
    ) -> PassId {
        let name = name.into();
        assert!(
            !colors.is_empty(),
            "pass `{name}` declares no color target; a depth-only pass uses add_external_pass_with_depth",
        );
        self.push_external(name, label, reads, colors, None)
    }

    /// Declares an externally-recorded pass that writes `colors` and a `depth` attachment (the
    /// renderer's scene pass). `colors` may be empty: that is the depth-only shape, a shadow map. The
    /// recorder attaches everything when it opens its render pass; the graph owns and aliases the depth
    /// texture like any other resource, and the scheduler orders a later pass that samples `depth`
    /// after this one.
    ///
    /// # Panics
    /// If an attachment is not attachable, or the attachments differ in extent.
    pub fn add_external_pass_with_depth(
        &mut self,
        name: impl Into<String>,
        label: &'static str,
        reads: Vec<ResourceId>,
        colors: Vec<ResourceId>,
        depth: ResourceId,
    ) -> PassId {
        self.push_external(name.into(), label, reads, colors, Some(depth))
    }

    fn push_external(
        &mut self,
        name: String,
        label: &'static str,
        reads: Vec<ResourceId>,
        colors: Vec<ResourceId>,
        depth: Option<ResourceId>,
    ) -> PassId {
        for color in &colors {
            self.require_attachable(*color, "color target");
        }
        if let Some(depth) = depth {
            self.require_attachable(depth, "depth attachment");
        }
        self.require_one_extent(&name, &colors, depth);
        self.require_valid_reads(&reads);
        self.passes.push(PassDecl {
            name,
            kind: PassKind::External { label },
            reads,
            color: colors,
            depth,
            storage: None,
            buffer_reads: Vec::new(),
            buffer_writes: Vec::new(),
        });
        PassId(self.passes.len() - 1)
    }

    /// Declares an externally-recorded **compute** pass ([`PassKind::Compute`]): it samples `reads`,
    /// writes the storage texture `storage`, and has no attachments. The executor opens the compute
    /// pass; the recorder sets its pipeline and bind groups and issues the dispatch, so workgroup
    /// shapes stay entirely with the consumer.
    ///
    /// Ordering is the graph's ordinary read edge — a later pass that samples `storage` is scheduled
    /// after this one — and the pass carries a timestamp pair like any other, so it appears in
    /// `dump-timings` and the profiler under its own name.
    ///
    /// # Panics
    /// If `storage` does not declare [`TextureUsage::StorageBinding`], or is mipped without a mip.
    ///
    /// # Example
    /// ```
    /// use ludion_render_graph::{RenderGraph, TextureDesc};
    ///
    /// let mut graph = RenderGraph::new();
    /// let volume = graph.transient("volume", TextureDesc::hdr_storage_volume(8, 8, 8));
    /// let out = graph.output("final", TextureDesc::present_target(16, 16));
    /// graph.add_external_compute_pass("inject", "inject", vec![], volume);
    /// graph.add_external_pass("shade", "shade", vec![volume], vec![out]);
    ///
    /// let schedule = ludion_render_graph::schedule(&graph).unwrap();
    /// assert_eq!(schedule.order_names(), ["inject", "shade"]);
    /// ```
    pub fn add_external_compute_pass(
        &mut self,
        name: impl Into<String>,
        label: &'static str,
        reads: Vec<ResourceId>,
        storage: ResourceId,
    ) -> PassId {
        self.require_storage(storage);
        self.require_valid_reads(&reads);
        self.passes.push(PassDecl {
            name: name.into(),
            kind: PassKind::Compute { label },
            reads,
            color: Vec::new(),
            depth: None,
            storage: Some(storage),
            buffer_reads: Vec::new(),
            buffer_writes: Vec::new(),
        });
        PassId(self.passes.len() - 1)
    }

    /// Declares a graph buffer and returns its handle. The executor allocates it (see
    /// [`BufferDesc`] for the lifetime and capacity rules); a pass writes it through
    /// [`add_external_buffer_compute_pass`](RenderGraph::add_external_buffer_compute_pass) and any
    /// pass reads it through [`read_buffers`](RenderGraph::read_buffers).
    pub fn buffer(&mut self, name: impl Into<String>, desc: BufferDesc) -> BufferId {
        self.buffers.push(BufferDecl {
            name: name.into(),
            desc,
        });
        BufferId(self.buffers.len() - 1)
    }

    /// Declares an externally-recorded **compute** pass that writes graph buffers: it samples
    /// `reads`, writes every buffer in `buffer_writes`, and has no attachments or storage texture.
    /// The recorder receives the buffers in declared order on
    /// [`RecordedPass::buffer_writes`](crate::RecordedPass::buffer_writes).
    ///
    /// # Panics
    /// If `buffer_writes` is empty, lists a buffer twice, or names a buffer without
    /// [`BufferUsage::Storage`].
    ///
    /// # Example
    /// ```
    /// use ludion_render_graph::{BufferDesc, BufferUsage, RenderGraph, TextureDesc};
    ///
    /// let mut graph = RenderGraph::new();
    /// let args = graph.buffer("args", BufferDesc::new(16, &[BufferUsage::Storage, BufferUsage::Indirect]));
    /// let out = graph.output("final", TextureDesc::present_target(16, 16));
    /// graph.add_external_buffer_compute_pass("cull", "cull", vec![], vec![args]);
    /// let draw = graph.add_external_pass("draw", "draw", vec![], vec![out]);
    /// graph.read_buffers(draw, &[args]);
    ///
    /// let schedule = ludion_render_graph::schedule(&graph).unwrap();
    /// assert_eq!(schedule.order_names(), ["cull", "draw"]);
    /// ```
    pub fn add_external_buffer_compute_pass(
        &mut self,
        name: impl Into<String>,
        label: &'static str,
        reads: Vec<ResourceId>,
        buffer_writes: Vec<BufferId>,
    ) -> PassId {
        let name = name.into();
        assert!(
            !buffer_writes.is_empty(),
            "buffer compute pass `{name}` writes no buffer",
        );
        for (i, buffer) in buffer_writes.iter().enumerate() {
            let decl = &self.buffers[buffer.index()];
            assert!(
                !buffer_writes[..i].contains(buffer),
                "buffer compute pass `{name}` lists buffer `{}` twice",
                decl.name,
            );
            assert!(
                decl.desc.usages.contains(&BufferUsage::Storage),
                "buffer compute pass `{name}` writes buffer `{}`, which does not declare \
                 BufferUsage::Storage; add the usage",
                decl.name,
            );
        }
        self.require_valid_reads(&reads);
        self.passes.push(PassDecl {
            name,
            kind: PassKind::Compute { label },
            reads,
            color: Vec::new(),
            depth: None,
            storage: None,
            buffer_reads: Vec::new(),
            buffer_writes,
        });
        PassId(self.passes.len() - 1)
    }

    /// Declares that `pass` (render or compute) reads `buffers`: the one way a pass names a buffer
    /// read. Appends, skipping buffers already declared. The scheduler orders the pass after the
    /// buffers' writers.
    pub fn read_buffers(&mut self, pass: PassId, buffers: &[BufferId]) {
        let reads = &mut self.passes[pass.0].buffer_reads;
        for buffer in buffers {
            if !reads.contains(buffer) {
                reads.push(*buffer);
            }
        }
    }

    /// The declared resources, in declaration order.
    pub fn resources(&self) -> &[ResourceDecl] {
        &self.resources
    }

    /// The declared buffers, in declaration order.
    pub fn buffers(&self) -> &[BufferDecl] {
        &self.buffers
    }

    /// The declared passes, in declaration order.
    pub fn passes(&self) -> &[PassDecl] {
        &self.passes
    }

    /// Looks up a resource declaration by handle.
    pub fn resource(&self, id: ResourceId) -> &ResourceDecl {
        &self.resources[id.index()]
    }
}

#[cfg(test)]
mod tests {
    use super::{PassKind, RenderGraph};
    use crate::resource::{BufferDesc, BufferUsage, TextureDesc};

    fn storage(size: u64) -> BufferDesc {
        BufferDesc::new(size, &[BufferUsage::Storage, BufferUsage::Indirect])
    }

    #[test]
    #[should_panic(expected = "zero bytes")]
    fn a_buffer_cannot_be_empty() {
        BufferDesc::new(0, &[BufferUsage::Storage]);
    }

    #[test]
    #[should_panic(expected = "not a multiple of 4")]
    fn a_buffer_size_must_be_word_aligned() {
        BufferDesc::new(6, &[BufferUsage::Storage]);
    }

    #[test]
    #[should_panic(expected = "at least one BufferUsage")]
    fn a_buffer_needs_a_usage() {
        BufferDesc::new(4, &[]);
    }

    #[test]
    fn buffer_usages_are_sorted_and_deduplicated() {
        let desc = BufferDesc::new(
            4,
            &[
                BufferUsage::Vertex,
                BufferUsage::Storage,
                BufferUsage::Vertex,
            ],
        );
        assert_eq!(desc.usages, [BufferUsage::Storage, BufferUsage::Vertex]);
    }

    #[test]
    #[should_panic(expected = "does not declare BufferUsage::Storage")]
    fn a_written_buffer_must_be_storage() {
        let mut graph = RenderGraph::new();
        let vertex = graph.buffer("v", BufferDesc::new(16, &[BufferUsage::Vertex]));
        graph.add_external_buffer_compute_pass("cull", "cull", vec![], vec![vertex]);
    }

    #[test]
    #[should_panic(expected = "writes no buffer")]
    fn a_buffer_compute_pass_writes_a_buffer() {
        let mut graph = RenderGraph::new();
        graph.add_external_buffer_compute_pass("cull", "cull", vec![], vec![]);
    }

    #[test]
    #[should_panic(expected = "lists buffer `a` twice")]
    fn a_buffer_is_written_once_per_pass() {
        let mut graph = RenderGraph::new();
        let a = graph.buffer("a", storage(16));
        graph.add_external_buffer_compute_pass("cull", "cull", vec![], vec![a, a]);
    }

    #[test]
    fn a_buffer_compute_pass_writes_only_its_buffers() {
        let mut graph = RenderGraph::new();
        let a = graph.buffer("a", storage(16));
        let b = graph.buffer("b", storage(16));
        graph.add_external_buffer_compute_pass("cull", "cull", vec![], vec![a, b]);
        let pass = &graph.passes()[0];
        assert_eq!(pass.buffer_writes, vec![a, b]);
        assert!(pass.writes().is_empty());
        assert!(pass.kind.is_compute());
        assert_eq!(graph.buffers().len(), 2);
    }

    #[test]
    fn read_buffers_appends_without_duplicates() {
        let mut graph = RenderGraph::new();
        let a = graph.buffer("a", storage(16));
        let b = graph.buffer("b", storage(16));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        let draw = graph.add_external_pass("draw", "draw", vec![], vec![out]);
        graph.read_buffers(draw, &[a, b]);
        graph.read_buffers(draw, &[b, a]);
        assert_eq!(graph.passes()[0].buffer_reads, vec![a, b]);
    }

    /// A compute pass writes its storage texture, so the pass's `writes` — the scheduler's and the
    /// dump's single source of truth — names it, with no color or depth attachment beside it.
    #[test]
    fn a_compute_pass_writes_only_its_storage_texture() {
        let mut graph = RenderGraph::new();
        let volume = graph.transient("volume", TextureDesc::hdr_storage_volume(4, 4, 4));
        graph.add_external_compute_pass("inject", "inject", vec![], volume);
        let pass = &graph.passes()[0];
        assert_eq!(pass.writes(), vec![volume]);
        assert!(pass.color_targets().is_empty());
        assert_eq!(pass.depth_target(), None);
        assert_eq!(pass.storage_target(), Some(volume));
        assert!(pass.kind.is_compute());
        assert_eq!(pass.kind.as_str(), "inject");
    }

    /// A storage target that was not declared writable is a construction bug: without the usage the
    /// physical texture is created unwritable and the failure surfaces at bind time instead.
    #[test]
    #[should_panic(expected = "does not declare TextureUsage::StorageBinding")]
    fn a_compute_pass_rejects_a_storage_target_that_is_not_writable() {
        let mut graph = RenderGraph::new();
        let flat = graph.transient("flat", TextureDesc::hdr_color(4, 4));
        graph.add_external_compute_pass("inject", "inject", vec![], flat);
    }

    /// A volume has no 2D view to attach: it is written by compute and sampled, never rendered into.
    #[test]
    #[should_panic(expected = "cannot be a pass color target")]
    fn a_volume_cannot_be_a_color_attachment() {
        let mut graph = RenderGraph::new();
        let volume = graph.transient("volume", TextureDesc::hdr_storage_volume(4, 4, 4));
        graph.add_pass("draw", PassKind::Gradient, volume);
    }

    /// Readback packs exactly one 2D image, so a volume can never be the graph's output.
    #[test]
    #[should_panic(expected = "readback produces one 2D image")]
    fn a_volume_cannot_be_the_graph_output() {
        let mut graph = RenderGraph::new();
        graph.output("final", TextureDesc::hdr_storage_volume(4, 4, 4));
    }

    fn pyramid() -> (RenderGraph, crate::resource::ResourceId) {
        let mut graph = RenderGraph::new();
        let pyramid = graph.transient("pyramid", TextureDesc::color(8, 8).with_mip_levels(4));
        (graph, pyramid)
    }

    #[test]
    #[should_panic(expected = "must name one with `.mip(level)`")]
    fn writing_a_mipped_resource_needs_a_mip() {
        let (mut graph, pyramid) = pyramid();
        graph.add_external_pass("w", "w", vec![], vec![pyramid]);
    }

    #[test]
    #[should_panic(expected = "mip 4 does not exist")]
    fn a_mip_past_the_chain_is_rejected() {
        let (mut graph, pyramid) = pyramid();
        graph.add_external_pass("w", "w", vec![], vec![pyramid.mip(4)]);
    }

    #[test]
    #[should_panic(expected = "must share an extent")]
    fn color_targets_of_one_pass_share_an_extent() {
        let mut graph = RenderGraph::new();
        let a = graph.transient("a", TextureDesc::color(8, 8));
        let b = graph.transient("b", TextureDesc::color(4, 4));
        graph.add_external_pass("w", "w", vec![], vec![a, b]);
    }

    #[test]
    #[should_panic(expected = "declares no color target")]
    fn an_external_pass_needs_a_color_target() {
        let mut graph = RenderGraph::new();
        graph.add_external_pass("w", "w", vec![], vec![]);
    }

    #[test]
    #[should_panic(expected = "exceed the full chain")]
    fn mip_levels_beyond_the_full_chain_are_rejected() {
        let _ = TextureDesc::color(8, 8).with_mip_levels(5);
    }
}
