//! The declarative frame graph: virtual resources and passes with their read/write declarations.
//!
//! This is pure CPU data with no wgpu in sight. A [`RenderGraph`] is built, then [`schedule`](fn@crate::schedule)d into
//! an executable, dumpable [`Schedule`](crate::schedule::Schedule); the schedule is what
//! [`crate::execute`] runs on the GPU and what [`crate::dump`] serializes.

use crate::resource::{ResourceId, TextureDesc, TextureUsage};

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
    /// attachment, one storage-texture write, and reads resolved exactly as a render pass's are. Its
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
/// A pass writes at most one color target, at most one depth attachment (multi-*color*-target passes
/// are deferred), and at most one storage texture. All three are optional so the graph can express
/// every shape the engine needs: a color-only post pass ([`color`](PassDecl::color) set), the scene
/// pass (color + depth), a depth-only shadow pass (only `depth`), and a compute pass (only
/// [`storage`](PassDecl::storage)). Its reads are stored on [`reads`](PassDecl::reads) — derived from
/// the kind for built-ins, declared explicitly for externally-recorded passes. The scheduler treats
/// the depth attachment and the storage write exactly like the color target, so a later pass that
/// samples either orders after this one.
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
    /// The color resource this pass renders into, if any. `None` for a depth-only or compute pass.
    pub color: Option<ResourceId>,
    /// The depth resource this pass renders into, if any.
    pub depth: Option<ResourceId>,
    /// The storage texture this pass writes, if any — set only by [`PassKind::Compute`]. A storage
    /// write is not an attachment: it is bound into the recorder's own bind group.
    pub storage: Option<ResourceId>,
}

impl PassDecl {
    /// The resources this pass reads.
    pub fn reads(&self) -> Vec<ResourceId> {
        self.reads.clone()
    }

    /// The pass's color attachment, if it has one.
    pub fn color_target(&self) -> Option<ResourceId> {
        self.color
    }

    /// The pass's depth attachment, if it has one.
    pub fn depth_target(&self) -> Option<ResourceId> {
        self.depth
    }

    /// The storage texture this pass writes, if it has one (a compute pass).
    pub fn storage_target(&self) -> Option<ResourceId> {
        self.storage
    }

    /// Every resource this pass writes — color target, then depth attachment, then storage texture,
    /// whichever are present. The scheduler and aliaser consult this for dependency edges and
    /// lifetimes; the fixed order makes `dump-graph`'s `writes` array canonical.
    pub fn writes(&self) -> Vec<ResourceId> {
        let mut writes = Vec::new();
        writes.extend(self.color);
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
    /// If `desc` is a 3D texture: readback packs one 2D image, so a volume can never be an output.
    pub fn output(&mut self, name: impl Into<String>, desc: TextureDesc) -> ResourceId {
        let name = name.into();
        assert!(
            desc.depth_slices.is_none(),
            "output resource `{name}` is a 3D texture; readback produces one 2D image, so a volume \
             cannot be a graph output",
        );
        self.push_resource(name, desc, ResourceKind::Output)
    }

    /// Rejects a 3D resource in a position that must be a flat texture — a color or depth
    /// attachment. A volume is written by a compute pass and sampled; it is never attached. This is
    /// a construction bug in the caller, the same class as reaching
    /// [`RecordedPass::target`](crate::RecordedPass::target) on a depth-only pass.
    fn require_flat(&self, id: ResourceId, role: &str) {
        let decl = &self.resources[id.0];
        assert!(
            decl.desc.depth_slices.is_none(),
            "resource `{}` is a 3D texture and cannot be a pass {role}",
            decl.name,
        );
    }

    /// Rejects a compute pass whose storage target was not declared writable. Without
    /// [`TextureUsage::StorageBinding`] the physical texture is created without the wgpu usage and
    /// the recorder's bind group fails at record time, far from the declaration that caused it.
    fn require_storage(&self, id: ResourceId) {
        let decl = &self.resources[id.0];
        assert!(
            decl.desc.usages.contains(&TextureUsage::StorageBinding),
            "compute pass storage target `{}` does not declare TextureUsage::StorageBinding; \
             declare it with TextureDesc::hdr_storage_volume, or add the usage",
            decl.name,
        );
    }

    fn push_resource(&mut self, name: String, desc: TextureDesc, kind: ResourceKind) -> ResourceId {
        let id = ResourceId(self.resources.len());
        self.resources.push(ResourceDecl { name, desc, kind });
        id
    }

    /// Declares a built-in pass that draws `kind` into `target`. Reads are derived from `kind`.
    ///
    /// # Panics
    /// If `target` is a 3D resource.
    pub fn add_pass(&mut self, name: impl Into<String>, kind: PassKind, target: ResourceId) {
        self.require_flat(target, "color target");
        let reads = kind.intrinsic_reads();
        self.passes.push(PassDecl {
            name: name.into(),
            kind,
            reads,
            color: Some(target),
            depth: None,
            storage: None,
        });
    }

    /// Declares an externally-recorded pass ([`PassKind::External`]): it samples `reads` and writes
    /// `target`, and its GPU work is recorded by the [`PassRecorder`](crate::PassRecorder) passed to
    /// [`Gpu::execute`](crate::Gpu::execute). `label` is the recorder's dispatch token and the pass
    /// kind shown in `dump-graph`.
    ///
    /// This is how `ludion-renderer` builds its pipeline: the graph owns scheduling, aliasing,
    /// and the JSON dump, while the renderer owns the pipelines and draw calls — the declarative
    /// interface stays free of wgpu types.
    ///
    /// # Panics
    /// If `target` is a 3D resource.
    pub fn add_external_pass(
        &mut self,
        name: impl Into<String>,
        label: &'static str,
        reads: Vec<ResourceId>,
        target: ResourceId,
    ) {
        self.require_flat(target, "color target");
        self.passes.push(PassDecl {
            name: name.into(),
            kind: PassKind::External { label },
            reads,
            color: Some(target),
            depth: None,
            storage: None,
        });
    }

    /// Declares an externally-recorded pass that writes both a `color` target and a `depth`
    /// attachment (the renderer's scene pass). The recorder attaches both when it opens its render pass;
    /// the graph owns and aliases the depth texture like any other resource, and the scheduler orders
    /// a later pass that samples `depth` after this one.
    ///
    /// # Panics
    /// If `color` or `depth` is a 3D resource.
    pub fn add_external_pass_with_depth(
        &mut self,
        name: impl Into<String>,
        label: &'static str,
        reads: Vec<ResourceId>,
        color: ResourceId,
        depth: ResourceId,
    ) {
        self.require_flat(color, "color target");
        self.require_flat(depth, "depth attachment");
        self.passes.push(PassDecl {
            name: name.into(),
            kind: PassKind::External { label },
            reads,
            color: Some(color),
            depth: Some(depth),
            storage: None,
        });
    }

    /// Declares an externally-recorded *depth-only* pass: it writes `depth` and no color target (a
    /// shadow map is drawn this way). A later pass that samples the depth resource orders after this
    /// one, exactly as with a color dependency.
    ///
    /// # Panics
    /// If `depth` is a 3D resource.
    pub fn add_external_depth_pass(
        &mut self,
        name: impl Into<String>,
        label: &'static str,
        reads: Vec<ResourceId>,
        depth: ResourceId,
    ) {
        self.require_flat(depth, "depth attachment");
        self.passes.push(PassDecl {
            name: name.into(),
            kind: PassKind::External { label },
            reads,
            color: None,
            depth: Some(depth),
            storage: None,
        });
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
    /// If `storage` does not declare [`TextureUsage::StorageBinding`].
    ///
    /// # Example
    /// ```
    /// use ludion_render_graph::{RenderGraph, TextureDesc};
    ///
    /// let mut graph = RenderGraph::new();
    /// let volume = graph.transient("volume", TextureDesc::hdr_storage_volume(8, 8, 8));
    /// let out = graph.output("final", TextureDesc::present_target(16, 16));
    /// graph.add_external_compute_pass("inject", "inject", vec![], volume);
    /// graph.add_external_pass("shade", "shade", vec![volume], out);
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
    ) {
        self.require_storage(storage);
        self.passes.push(PassDecl {
            name: name.into(),
            kind: PassKind::Compute { label },
            reads,
            color: None,
            depth: None,
            storage: Some(storage),
        });
    }

    /// The declared resources, in declaration order.
    pub fn resources(&self) -> &[ResourceDecl] {
        &self.resources
    }

    /// The declared passes, in declaration order.
    pub fn passes(&self) -> &[PassDecl] {
        &self.passes
    }

    /// Looks up a resource declaration by handle.
    pub fn resource(&self, id: ResourceId) -> &ResourceDecl {
        &self.resources[id.0]
    }
}

#[cfg(test)]
mod tests {
    use super::{PassKind, RenderGraph};
    use crate::resource::TextureDesc;

    /// A compute pass writes its storage texture, so the pass's `writes` — the scheduler's and the
    /// dump's single source of truth — names it, with no color or depth attachment beside it.
    #[test]
    fn a_compute_pass_writes_only_its_storage_texture() {
        let mut graph = RenderGraph::new();
        let volume = graph.transient("volume", TextureDesc::hdr_storage_volume(4, 4, 4));
        graph.add_external_compute_pass("inject", "inject", vec![], volume);
        let pass = &graph.passes()[0];
        assert_eq!(pass.writes(), vec![volume]);
        assert_eq!(pass.color_target(), None);
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
}
