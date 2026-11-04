//! Pass scheduling and transient aliasing — the deterministic, CPU-only core of the frame graph.
//!
//! [`schedule`] turns a [`RenderGraph`] into a [`Schedule`]: it derives a stable execution order
//! from the pass dependency edges (reads, and declaration order between passes that write the same
//! subresource), rejects cycles and reads of never-written resources with L-coded
//! diagnostics, then assigns transient resources to a minimal set of physical textures by lifetime
//! (transient aliasing). None of this touches the GPU — a [`Schedule`] is fully inspectable via
//! [`crate::dump`] before a device is ever created, which is what makes the graph testable without
//! an adapter.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::graph::{PassDecl, RenderGraph, ResourceDecl, ResourceKind};
use crate::resource::{Extent2d, TextureDesc, TextureFormat, TextureUsage};

/// One physical texture the executor will allocate, and the virtual resources bound to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhysicalTexture {
    /// Stable physical id (its index in [`Aliasing::physicals`]).
    pub id: usize,
    /// The physical texture's size.
    pub extent: Extent2d,
    /// The physical texture's z-slice count when it backs a 3D resource; `None` for a 2D one. Only
    /// resources that agree on this share a physical texture.
    pub depth_slices: Option<u32>,
    /// The physical texture's mip level count; 1 for an unmipped texture.
    pub mip_levels: u32,
    /// The physical texture's format.
    pub format: TextureFormat,
    /// The union of the usages of every resource bound here.
    pub usages: Vec<TextureUsage>,
    /// The resource indices bound to this physical texture, in assignment order.
    pub resources: Vec<usize>,
}

/// The transient-aliasing result: which physical texture backs each virtual resource.
///
/// Comparable because it is the identity of a frame's GPU textures: two schedules with equal
/// aliasing can be executed against one allocation (see `PhysicalCache`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Aliasing {
    /// `assignment[resource_index]` is the id of the physical texture backing that resource.
    pub assignment: Vec<usize>,
    /// The physical textures, in id order.
    pub physicals: Vec<PhysicalTexture>,
}

/// An executable, dumpable plan: the graph's resources and passes, a concrete execution order, and
/// the transient-aliasing assignment.
#[derive(Clone, Debug)]
pub struct Schedule {
    pub(crate) resources: Vec<ResourceDecl>,
    pub(crate) passes: Vec<PassDecl>,
    /// Pass indices (into `passes`) in execution order.
    pub(crate) order: Vec<usize>,
    pub(crate) aliasing: Aliasing,
}

impl Schedule {
    /// The pass names in execution order.
    pub fn order_names(&self) -> Vec<&str> {
        self.order
            .iter()
            .map(|&p| self.passes[p].name.as_str())
            .collect()
    }

    /// The number of physical textures the aliaser allocated. With no aliasing this equals the
    /// resource count; every alias reduces it by one.
    pub fn physical_texture_count(&self) -> usize {
        self.aliasing.physicals.len()
    }

    pub(crate) fn resources(&self) -> &[ResourceDecl] {
        &self.resources
    }
}

/// The half-open lifetime of a resource, as positions in the execution order.
#[derive(Clone, Copy, Debug)]
struct Lifetime {
    first: usize,
    last: usize,
}

/// Schedules a [`RenderGraph`]: validates it, derives execution order, and computes transient
/// aliasing.
///
/// # Errors
/// - `L2001` if the pass dependency graph contains a cycle.
/// - `L2002` if a pass reads a resource that no pass writes, or reads one several passes write when
///   none is declared before it.
///
/// # Example
/// ```
/// use ludion_render_graph::{PassKind, RenderGraph, TextureDesc};
///
/// let mut graph = RenderGraph::new();
/// let scene = graph.transient("scene", TextureDesc::color(64, 64));
/// let out = graph.output("final", TextureDesc::present_target(64, 64));
/// graph.add_pass("gradient", PassKind::Gradient, scene);
/// graph.add_pass("present", PassKind::Vignette { source: scene }, out);
///
/// let schedule = ludion_render_graph::schedule(&graph).unwrap();
/// assert_eq!(schedule.order_names(), ["gradient", "present"]);
/// ```
pub fn schedule(graph: &RenderGraph) -> Result<Schedule, Diagnostic> {
    let resources = graph.resources();
    let passes = graph.passes();

    validate_reads(resources, passes)?;
    let order = topo_order(passes)?;
    let lifetimes = lifetimes(resources.len(), passes, &order);
    let aliasing = alias(resources, &lifetimes);

    Ok(Schedule {
        resources: resources.to_vec(),
        passes: passes.to_vec(),
        order,
        aliasing,
    })
}

/// The passes whose writes overlap `read`, when the read is **contested**: at least two distinct
/// passes write subresources that overlap the read *and* each other. Disjoint mip writers are not
/// contested, so a whole read of a mip pyramid stays order-free. `None` for an uncontested read.
fn contested_writers(passes: &[PassDecl], read: crate::resource::ResourceId) -> Option<Vec<usize>> {
    let writers: Vec<(usize, Vec<crate::resource::ResourceId>)> = passes
        .iter()
        .enumerate()
        .filter_map(|(index, pass)| {
            let hits: Vec<_> = pass
                .writes()
                .into_iter()
                .filter(|w| w.overlaps(read))
                .collect();
            (!hits.is_empty()).then_some((index, hits))
        })
        .collect();
    let contested = writers.iter().enumerate().any(|(i, (_, a))| {
        writers[i + 1..]
            .iter()
            .any(|(_, b)| a.iter().any(|x| b.iter().any(|y| x.overlaps(*y))))
    });
    contested.then(|| writers.into_iter().map(|(index, _)| index).collect())
}

/// The passes writing what `read` reads, split into one group per mip the read overlaps (one group
/// when the read and every write are unmipped). A whole-resource write belongs to every group.
fn writer_groups(passes: &[PassDecl], read: crate::resource::ResourceId) -> Vec<Vec<usize>> {
    let hits: Vec<(usize, crate::resource::ResourceId)> = passes
        .iter()
        .enumerate()
        .flat_map(|(index, pass)| {
            pass.writes()
                .into_iter()
                .filter(|w| w.overlaps(read))
                .map(move |w| (index, w))
        })
        .collect();
    let mut mips: Vec<Option<u32>> = hits
        .iter()
        .filter_map(|(_, w)| w.mip_level().map(Some))
        .collect();
    mips.sort_unstable();
    mips.dedup();
    if mips.is_empty() {
        mips.push(None);
    }
    mips.into_iter()
        .map(|mip| {
            let probe = crate::resource::ResourceId {
                index: read.index(),
                mip,
            };
            let mut group: Vec<usize> = hits
                .iter()
                .filter(|(_, w)| w.overlaps(probe))
                .map(|(index, _)| *index)
                .collect();
            group.dedup();
            group
        })
        .collect()
}

/// Rejects any pass that reads a resource no pass ever writes (`L2002`). A mip read needs a write
/// overlapping that mip; a whole read of a mipped resource needs every mip covered. A contested
/// read (several passes write what it reads) also needs a writer declared before the reader, since
/// declaration order is the write order.
fn validate_reads(resources: &[ResourceDecl], passes: &[PassDecl]) -> Result<(), Diagnostic> {
    let writes: Vec<_> = passes.iter().flat_map(PassDecl::writes).collect();
    for (consumer, pass) in passes.iter().enumerate() {
        for read in pass.reads() {
            if let Some(writers) = contested_writers(passes, read)
                && writers.iter().all(|&w| w >= consumer)
            {
                let name = &resources[read.index()].name;
                return Err(Diagnostic::new(
                    code::RENDER_GRAPH_UNWRITTEN_READ,
                    format!(
                        "pass `{}` reads `{name}`, which several passes write, but none is \
                         declared before it",
                        pass.name
                    ),
                )
                .with_help(
                    "with several writers, declaration order is the write order: declare a \
                     pass that writes this resource before the pass that reads it",
                ));
            }
            let levels = resources[read.index()].desc.mip_levels;
            let wanted: Vec<Option<u32>> = match read.mip_level() {
                Some(level) => vec![Some(level)],
                None if levels > 1 => (0..levels).map(Some).collect(),
                None => vec![None],
            };
            let missing = wanted.into_iter().find(|&mip| {
                let probe = crate::resource::ResourceId {
                    index: read.index(),
                    mip,
                };
                !writes.iter().any(|w| w.overlaps(probe))
            });
            let Some(missing) = missing else { continue };
            let name = &resources[read.index()].name;
            let message = match (missing, read.mip_level()) {
                (Some(level), None) => format!(
                    "pass `{}` reads every mip of `{name}`, but no pass writes `{name}@mip{level}`",
                    pass.name
                ),
                (Some(level), Some(_)) => format!(
                    "pass `{}` reads `{name}@mip{level}`, but no pass writes it",
                    pass.name
                ),
                (None, _) => {
                    format!(
                        "pass `{}` reads resource `{name}`, but no pass writes it",
                        pass.name
                    )
                }
            };
            return Err(
                Diagnostic::new(code::RENDER_GRAPH_UNWRITTEN_READ, message).with_help(
                    "add a pass that writes this resource before the pass that reads it, or \
                 correct the read to name a resource that is produced",
                ),
            );
        }
    }
    Ok(())
}

/// Kahn topological sort with a deterministic tie-break: among passes whose dependencies are all
/// scheduled, the one declared earliest (smallest index) runs next. Rejects cycles (`L2001`).
///
/// Edges: a read depends on the passes that write what it reads. When several passes write
/// overlapping subresources, declaration order is the write order — a later writer depends on an
/// earlier one (write-after-write), a contested read depends on the latest writer declared before
/// the reader only, and the first writer declared after the reader depends on the reader
/// (write-after-read). A read with at most one overlapping writer, or with disjoint mip writers,
/// depends on all of them and nothing else orders them.
fn topo_order(passes: &[PassDecl]) -> Result<Vec<usize>, Diagnostic> {
    let n = passes.len();

    // Deduplicated, because the in-degree below is decremented once per (consumer, producer) pair:
    // a pass reading two resources from one producer — `composite` reading the scene pass's colour
    // *and* its depth — would otherwise carry an in-degree it can never work off, and report
    // itself as a cycle.
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); n];
    fn add(deps: &mut [Vec<usize>], consumer: usize, producer: usize) {
        if !deps[consumer].contains(&producer) {
            deps[consumer].push(producer);
        }
    }
    for (consumer, pass) in passes.iter().enumerate() {
        for read in pass.reads() {
            // Contention is judged per overlapping mip: writers of different mips never order each
            // other, so a read of a pyramid orders against the latest earlier writer of each mip.
            for group in writer_groups(passes, read) {
                if group.len() < 2 {
                    for &producer in &group {
                        add(&mut deps, consumer, producer);
                    }
                    continue;
                }
                // A pass reading what it writes stays a self-dependency (`L2001`).
                if group.contains(&consumer) {
                    add(&mut deps, consumer, consumer);
                }
                if let Some(&before) = group.iter().filter(|&&w| w < consumer).max() {
                    add(&mut deps, consumer, before);
                }
                if let Some(&after) = group.iter().filter(|&&w| w > consumer).min() {
                    add(&mut deps, after, consumer);
                }
            }
        }
    }
    for (later, pass) in passes.iter().enumerate() {
        for (earlier, other) in passes.iter().enumerate().take(later) {
            let overlap = pass
                .writes()
                .iter()
                .any(|w| other.writes().iter().any(|e| e.overlaps(*w)));
            if overlap {
                add(&mut deps, later, earlier);
            }
        }
    }

    let mut in_degree: Vec<usize> = deps.iter().map(Vec::len).collect();
    let mut order = Vec::with_capacity(n);
    // Re-scan for the smallest ready pass each step: n is tiny (a handful of passes), and this
    // keeps the tie-break trivially "declaration order" without a separate sorted structure.
    while order.len() < n {
        let next = (0..n).find(|&p| in_degree[p] == 0 && !order.contains(&p));
        let Some(p) = next else {
            return Err(cycle_diagnostic(passes, &order));
        };
        order.push(p);
        for (consumer, dep_list) in deps.iter().enumerate() {
            if dep_list.contains(&p) {
                in_degree[consumer] -= 1;
            }
        }
    }
    Ok(order)
}

/// Builds the `L2001` cycle diagnostic, naming the passes still tangled in the cycle.
fn cycle_diagnostic(passes: &[PassDecl], scheduled: &[usize]) -> Diagnostic {
    let stuck: Vec<&str> = (0..passes.len())
        .filter(|p| !scheduled.contains(p))
        .map(|p| passes[p].name.as_str())
        .collect();
    Diagnostic::new(
        code::RENDER_GRAPH_CYCLE,
        format!(
            "the render graph has a dependency cycle among passes: {}",
            stuck.join(", ")
        ),
    )
    .with_help(
        "a pass cannot (transitively) read a resource it also produces; break the cycle by \
         splitting the shared resource or reordering the reads and writes",
    )
}

/// Computes each resource's lifetime as `[first_use, last_use]` positions in `order`. Resources no
/// pass touches get an empty lifetime (`first > last`) and are given their own physical texture.
fn lifetimes(resource_count: usize, passes: &[PassDecl], order: &[usize]) -> Vec<Lifetime> {
    let mut lifetimes = vec![
        Lifetime {
            first: usize::MAX,
            last: 0,
        };
        resource_count
    ];
    for (position, &pass_idx) in order.iter().enumerate() {
        let pass = &passes[pass_idx];
        let mut touch = |resource: usize| {
            let lt = &mut lifetimes[resource];
            lt.first = lt.first.min(position);
            lt.last = lt.last.max(position);
        };
        for write in pass.writes() {
            touch(write.index());
        }
        for read in pass.reads() {
            touch(read.index());
        }
    }
    lifetimes
}

/// Greedy lifetime-based aliasing. Transients are processed in `(first_use, resource_index)` order;
/// each is assigned to the first compatible physical texture that is free (its previous occupant's
/// last use precedes this resource's first use), or a fresh physical if none is free. Output
/// resources always get a dedicated physical — they must survive to be read back.
fn alias(resources: &[ResourceDecl], lifetimes: &[Lifetime]) -> Aliasing {
    let mut assignment = vec![usize::MAX; resources.len()];
    let mut physicals: Vec<PhysicalTexture> = Vec::new();

    // Outputs and untouched resources first, each on its own physical, so their ids are stable and
    // they never take part in aliasing.
    for (idx, resource) in resources.iter().enumerate() {
        let untouched = lifetimes[idx].first == usize::MAX;
        if resource.kind == ResourceKind::Output || untouched {
            assignment[idx] = new_physical(&mut physicals, idx, &resource.desc);
        }
    }

    // Transients in deterministic lifetime order.
    let mut transients: Vec<usize> = resources
        .iter()
        .enumerate()
        .filter(|(idx, r)| r.kind == ResourceKind::Transient && lifetimes[*idx].first != usize::MAX)
        .map(|(idx, _)| idx)
        .collect();
    transients.sort_by_key(|&idx| (lifetimes[idx].first, idx));

    // Tracks, per physical, the last-use position of its current occupant so we know when it frees.
    let mut phys_last_use: Vec<usize> = physicals.iter().map(|_| usize::MAX).collect();

    for idx in transients {
        let desc = &resources[idx].desc;
        let life = lifetimes[idx];

        let reuse = physicals.iter().position(|p| {
            phys_last_use[p.id] != usize::MAX
                && phys_last_use[p.id] < life.first
                && desc.alias_compatible(&PhysicalTexture::probe_desc(p))
        });

        match reuse {
            Some(pos) => {
                let phys = &mut physicals[pos];
                phys.resources.push(idx);
                phys.usages = TextureDesc::union_usages(&phys.usages, &desc.usages);
                assignment[idx] = phys.id;
                phys_last_use[phys.id] = life.last;
            }
            None => {
                let id = new_physical(&mut physicals, idx, desc);
                phys_last_use.push(life.last);
                debug_assert_eq!(phys_last_use.len(), physicals.len());
                assignment[idx] = id;
            }
        }
    }

    Aliasing {
        assignment,
        physicals,
    }
}

/// Allocates a fresh physical texture backing `resource_idx` and returns its id.
fn new_physical(
    physicals: &mut Vec<PhysicalTexture>,
    resource_idx: usize,
    desc: &TextureDesc,
) -> usize {
    let id = physicals.len();
    physicals.push(PhysicalTexture {
        id,
        extent: desc.extent,
        depth_slices: desc.depth_slices,
        mip_levels: desc.mip_levels,
        format: desc.format,
        usages: desc.usages.clone(),
        resources: vec![resource_idx],
    });
    id
}

impl PhysicalTexture {
    /// A description carrying just the fields alias-compatibility examines (size + slice count +
    /// format); usages are irrelevant to compatibility, so a stand-in is fine.
    fn probe_desc(&self) -> TextureDesc {
        TextureDesc {
            extent: self.extent,
            depth_slices: self.depth_slices,
            mip_levels: self.mip_levels,
            format: self.format,
            usages: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::graph::{PassKind, RenderGraph};
    use crate::resource::{ResourceId, TextureDesc};

    /// Resolves `physical id backing this resource` after scheduling.
    fn physical_of(schedule: &super::Schedule, resource: ResourceId) -> usize {
        schedule.aliasing.assignment[resource.index()]
    }

    #[test]
    fn order_is_dependency_topological() {
        let (graph, _output) = crate::build_graph(64, 64);
        let schedule = super::schedule(&graph).unwrap();
        assert_eq!(
            schedule.order_names(),
            ["gradient", "vignette", "bleed", "present"],
        );
    }

    #[test]
    fn independent_passes_break_ties_by_declaration_order() {
        // Two passes with no dependency between them: the scheduler must keep declaration order.
        let mut graph = RenderGraph::new();
        let a = graph.transient("a", TextureDesc::color(8, 8));
        let b = graph.transient("b", TextureDesc::color(8, 8));
        graph.add_pass("first", PassKind::Gradient, a);
        graph.add_pass("second", PassKind::Gradient, b);
        let schedule = super::schedule(&graph).unwrap();
        assert_eq!(schedule.order_names(), ["first", "second"]);
    }

    #[test]
    fn cycle_is_rejected() {
        // pA reads y (written by pB) and writes x; pB reads x (written by pA) and writes y.
        let mut graph = RenderGraph::new();
        let x = graph.transient("x", TextureDesc::color(8, 8));
        let y = graph.transient("y", TextureDesc::color(8, 8));
        graph.add_pass("pA", PassKind::Vignette { source: y }, x);
        graph.add_pass("pB", PassKind::Vignette { source: x }, y);
        let error = super::schedule(&graph).unwrap_err();
        assert_eq!(error.code.as_str(), "L2001");
        assert!(error.message.contains("pA") && error.message.contains("pB"));
    }

    /// One pass reading **two** resources written by the same producer is one dependency, not two.
    /// The in-degree is worked off once per (consumer, producer) pair, so counting the producer
    /// twice would leave the consumer permanently blocked and reported as a cycle. This is the shape
    /// the frame graph's `composite` takes once it reads the scene pass's colour *and* its depth.
    #[test]
    fn two_reads_from_one_producer_are_one_dependency() {
        let mut graph = RenderGraph::new();
        let color = graph.transient("color", TextureDesc::color(8, 8));
        let depth = graph.transient("depth", TextureDesc::sampled_depth(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_pass_with_depth("scene", "scene", vec![], vec![color], depth);
        graph.add_external_pass("post", "post", vec![color, depth], vec![out]);
        let schedule = super::schedule(&graph).expect("two reads of one producer is not a cycle");
        assert_eq!(schedule.order_names(), ["scene", "post"]);
    }

    #[test]
    fn read_of_unwritten_resource_is_rejected() {
        let mut graph = RenderGraph::new();
        let unwritten = graph.transient("ghost", TextureDesc::color(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_pass("consume", PassKind::Vignette { source: unwritten }, out);
        let error = super::schedule(&graph).unwrap_err();
        assert_eq!(error.code.as_str(), "L2002");
        assert!(error.message.contains("ghost"));
    }

    #[test]
    fn overlapping_lifetimes_do_not_alias() {
        // a is alive across the pass that first writes b, so their lifetimes overlap: no aliasing.
        let mut graph = RenderGraph::new();
        let a = graph.transient("a", TextureDesc::color(8, 8));
        let b = graph.transient("b", TextureDesc::color(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_pass("make_a", PassKind::Gradient, a);
        graph.add_pass("a_to_b", PassKind::Vignette { source: a }, b); // reads a, writes b
        graph.add_pass("b_to_out", PassKind::Vignette { source: b }, out);
        let schedule = super::schedule(&graph).unwrap();
        assert_ne!(
            physical_of(&schedule, a),
            physical_of(&schedule, b),
            "overlapping transients must not share a physical texture",
        );
    }

    #[test]
    fn disjoint_lifetimes_alias_when_compatible() {
        // a lives [0,1]; c lives [2,3]; disjoint and same descriptor → they share a physical.
        let mut graph = RenderGraph::new();
        let a = graph.transient("a", TextureDesc::color(8, 8));
        let b = graph.transient("b", TextureDesc::color(8, 8));
        let c = graph.transient("c", TextureDesc::color(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_pass("make_a", PassKind::Gradient, a);
        graph.add_pass("a_to_b", PassKind::Vignette { source: a }, b); // a read here (last use pos 1)
        graph.add_pass("make_c", PassKind::Vignette { source: b }, c); // c written pos 2
        graph.add_pass("c_to_out", PassKind::Vignette { source: c }, out); // c read pos 3
        let schedule = super::schedule(&graph).unwrap();
        assert_eq!(
            physical_of(&schedule, a),
            physical_of(&schedule, c),
            "disjoint, compatible transients should alias onto one physical texture",
        );
        assert_ne!(
            physical_of(&schedule, a),
            physical_of(&schedule, b),
            "a and b overlap and must stay separate",
        );
    }

    #[test]
    fn disjoint_but_incompatible_sizes_do_not_alias() {
        // a lives [0,1], c lives [2,3] (disjoint) but different sizes → cannot share.
        let mut graph = RenderGraph::new();
        let a = graph.transient("a", TextureDesc::color(8, 8));
        let b = graph.transient("b", TextureDesc::color(8, 8));
        let c = graph.transient("c", TextureDesc::color(16, 16));
        let out = graph.output("final", TextureDesc::present_target(16, 16));
        graph.add_pass("make_a", PassKind::Gradient, a);
        graph.add_pass("a_to_b", PassKind::Vignette { source: a }, b);
        graph.add_pass("make_c", PassKind::Vignette { source: b }, c);
        graph.add_pass("c_to_out", PassKind::Vignette { source: c }, out);
        let schedule = super::schedule(&graph).unwrap();
        assert_ne!(
            physical_of(&schedule, a),
            physical_of(&schedule, c),
            "incompatible descriptors must not alias even with disjoint lifetimes",
        );
    }

    #[test]
    fn depth_attachment_gets_its_own_physical_and_is_a_write() {
        // A scene pass writing a color target plus a depth attachment: the depth resource is a write
        // (so a graph reading an unwritten depth would be L2002, not here), and it never aliases a
        // color transient because the formats differ.
        let mut graph = RenderGraph::new();
        let color = graph.transient("color", TextureDesc::hdr_color(8, 8));
        let depth = graph.transient("depth", TextureDesc::depth(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_pass_with_depth("scene", "scene", vec![], vec![color], depth);
        graph.add_external_pass("tonemap", "tonemap", vec![color], vec![out]);
        let schedule = super::schedule(&graph).unwrap();
        assert_ne!(
            physical_of(&schedule, color),
            physical_of(&schedule, depth),
            "a depth attachment never shares a physical with a color transient",
        );
    }

    #[test]
    fn depth_only_pass_orders_before_a_later_sampler() {
        // The shadow-map shape: a depth-only pass writes a depth resource that a later pass
        // samples. The scheduler must derive the shadow→scene edge from the depth write, exactly as
        // it does for a color write — proving depth resources can be read by a later pass.
        let mut graph = RenderGraph::new();
        let shadow_map = graph.transient("shadow_map", TextureDesc::depth(16, 16));
        let scene = graph.transient("scene", TextureDesc::hdr_color(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_pass_with_depth("shadow", "shadow", vec![], vec![], shadow_map);
        graph.add_external_pass("scene", "scene", vec![shadow_map], vec![scene]);
        graph.add_external_pass("tonemap", "tonemap", vec![scene], vec![out]);
        let schedule = super::schedule(&graph).unwrap();
        assert_eq!(schedule.order_names(), ["shadow", "scene", "tonemap"]);
    }

    /// A compute pass's storage write is an ordering edge like any other: the pass that samples the
    /// volume runs after the pass that fills it, even though it was *declared* first — so the order
    /// comes from the read edge, not from declaration order.
    #[test]
    fn compute_pass_orders_before_the_pass_that_reads_its_volume() {
        let mut graph = RenderGraph::new();
        let volume = graph.transient("volume", TextureDesc::hdr_storage_volume(8, 8, 4));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_pass("shade", "shade", vec![volume], vec![out]);
        graph.add_external_compute_pass("inject", "inject", vec![], volume);
        let schedule = super::schedule(&graph).unwrap();
        assert_eq!(schedule.order_names(), ["inject", "shade"]);
    }

    /// A volume nobody writes is an unwritten read (`L2002`) exactly as a flat texture would be —
    /// the storage write is what satisfies the reader, and nothing else does.
    #[test]
    fn read_of_an_unwritten_volume_is_rejected() {
        let mut graph = RenderGraph::new();
        let volume = graph.transient("volume", TextureDesc::hdr_storage_volume(8, 8, 4));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_pass("shade", "shade", vec![volume], vec![out]);
        let error = super::schedule(&graph).unwrap_err();
        assert_eq!(error.code.as_str(), "L2002");
        assert!(error.message.contains("volume"));
    }

    /// A volume and a flat texture of the same width, height, and format still never share a
    /// physical texture: the slice count is part of alias compatibility, so a 2D and a 3D resource
    /// are incompatible however disjoint their lifetimes are.
    #[test]
    fn a_volume_never_aliases_a_flat_texture() {
        let mut graph = RenderGraph::new();
        let volume = graph.transient("volume", TextureDesc::hdr_storage_volume(8, 8, 4));
        let flat = graph.transient("flat", TextureDesc::hdr_color(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_compute_pass("inject", "inject", vec![], volume);
        graph.add_external_pass("shade", "shade", vec![volume], vec![flat]); // volume's last use: pos 1
        graph.add_external_pass("present", "present", vec![flat], vec![out]);
        let schedule = super::schedule(&graph).unwrap();
        assert_ne!(
            physical_of(&schedule, volume),
            physical_of(&schedule, flat),
            "a 3D and a 2D resource must never share one physical texture",
        );
        let volume_physical = &schedule.aliasing.physicals[physical_of(&schedule, volume)];
        assert_eq!(volume_physical.depth_slices, Some(4));
    }

    /// The dump names a volume's slice count, and says nothing at all about depth for a flat
    /// resource — which is what keeps every committed graph snapshot byte-identical.
    #[test]
    fn dump_reports_depth_only_for_a_volume() {
        let mut graph = RenderGraph::new();
        let volume = graph.transient("volume", TextureDesc::hdr_storage_volume(8, 8, 4));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_compute_pass("inject", "inject", vec![], volume);
        graph.add_external_pass("shade", "shade", vec![volume], vec![out]);
        let json = super::schedule(&graph).unwrap().dump_graph_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["resources"][0]["depth"], serde_json::json!(4));
        assert_eq!(
            value["resources"][0]["usages"],
            serde_json::json!(["texture_binding", "storage_binding"]),
        );
        assert!(
            value["resources"][1].get("depth").is_none(),
            "a flat resource must not gain a depth key: {json}",
        );
        assert_eq!(value["passes"][0]["writes"], serde_json::json!(["volume"]));
    }

    /// The executor caches a frame's physical textures keyed on the whole [`Aliasing`], so equality
    /// has to mean "the same GPU textures serve this schedule": stable for the same graph, and
    /// different the moment a descriptor moves.
    #[test]
    fn aliasing_equality_discriminates_the_physical_allocation() {
        let (graph, _output) = crate::build_graph(64, 64);
        let (again, _) = crate::build_graph(64, 64);
        let (resized, _) = crate::build_graph(64, 32);
        let a = super::schedule(&graph).unwrap();
        assert_eq!(a.aliasing, super::schedule(&again).unwrap().aliasing);
        assert_ne!(a.aliasing, super::schedule(&resized).unwrap().aliasing);

        // A graph with the same physical *descriptors* but a different resource→physical assignment
        // is not the same allocation either — the assignment is what hands a pass its texture.
        let mut swapped = RenderGraph::new();
        let first = swapped.transient("first", TextureDesc::color(8, 8));
        let second = swapped.transient("second", TextureDesc::color(8, 8));
        let out = swapped.output("final", TextureDesc::present_target(8, 8));
        swapped.add_pass("make_first", PassKind::Gradient, first);
        swapped.add_pass("make_second", PassKind::Gradient, second);
        swapped.add_pass("a", PassKind::Vignette { source: first }, out);
        let mut reordered = RenderGraph::new();
        let one = reordered.transient("first", TextureDesc::color(8, 8));
        let two = reordered.transient("second", TextureDesc::color(8, 8));
        let out2 = reordered.output("final", TextureDesc::present_target(8, 8));
        reordered.add_pass("make_second", PassKind::Gradient, two);
        reordered.add_pass("make_first", PassKind::Gradient, one);
        reordered.add_pass("a", PassKind::Vignette { source: two }, out2);
        assert_ne!(
            super::schedule(&swapped).unwrap().aliasing,
            super::schedule(&reordered).unwrap().aliasing,
        );
    }

    #[test]
    fn proving_graph_aliases_three_transients_into_two_physicals() {
        let (graph, _output) = crate::build_graph(64, 64);
        let schedule = super::schedule(&graph).unwrap();
        // scene, veiled, bloom, final -> 3 physical textures (scene and bloom alias).
        assert_eq!(schedule.physical_texture_count(), 3);
        let scene = ResourceId::whole(0);
        let veiled = ResourceId::whole(1);
        let bloom = ResourceId::whole(2);
        assert_eq!(physical_of(&schedule, scene), physical_of(&schedule, bloom));
        assert_ne!(
            physical_of(&schedule, scene),
            physical_of(&schedule, veiled)
        );
    }

    /// A 4-mip pyramid: seed writes mip 0, each `down_k` reads mip k-1 and writes mip k, `consume`
    /// reads the whole pyramid. Declared in reverse; the per-mip edges alone order it.
    fn pyramid_graph() -> RenderGraph {
        let mut graph = RenderGraph::new();
        let pyramid = graph.transient("pyramid", TextureDesc::color(16, 16).with_mip_levels(4));
        let out = graph.output("final", TextureDesc::present_target(16, 16));
        graph.add_external_pass("consume", "consume", vec![pyramid], vec![out]);
        for k in (1..4).rev() {
            graph.add_external_pass(
                format!("down_{k}"),
                "down",
                vec![pyramid.mip(k - 1)],
                vec![pyramid.mip(k)],
            );
        }
        graph.add_external_pass("seed", "seed", vec![], vec![pyramid.mip(0)]);
        graph
    }

    #[test]
    fn per_mip_edges_order_a_pyramid() {
        let schedule = super::schedule(&pyramid_graph()).unwrap();
        assert_eq!(
            schedule.order_names(),
            ["seed", "down_1", "down_2", "down_3", "consume"],
        );
    }

    #[test]
    fn a_whole_read_needs_every_mip_written() {
        let mut graph = RenderGraph::new();
        let pyramid = graph.transient("pyramid", TextureDesc::color(16, 16).with_mip_levels(4));
        let out = graph.output("final", TextureDesc::present_target(16, 16));
        for k in 0..3 {
            graph.add_external_pass(format!("w{k}"), "w", vec![], vec![pyramid.mip(k)]);
        }
        graph.add_external_pass("consume", "consume", vec![pyramid], vec![out]);
        let err = super::schedule(&graph).unwrap_err();
        assert_eq!(
            err.code,
            ludion_core::codes::code::RENDER_GRAPH_UNWRITTEN_READ
        );
        assert!(err.message.contains("`pyramid@mip3`"), "{}", err.message);
    }

    #[test]
    fn reading_the_whole_while_writing_a_mip_is_a_cycle() {
        let mut graph = RenderGraph::new();
        let pyramid = graph.transient("pyramid", TextureDesc::color(16, 16).with_mip_levels(2));
        graph.add_external_pass("seed", "seed", vec![], vec![pyramid.mip(0)]);
        graph.add_external_pass("loop", "loop", vec![pyramid], vec![pyramid.mip(1)]);
        let err = super::schedule(&graph).unwrap_err();
        assert_eq!(err.code, ludion_core::codes::code::RENDER_GRAPH_CYCLE);
    }

    #[test]
    fn a_multi_target_pass_writes_colors_then_depth_and_orders_its_readers() {
        let mut graph = RenderGraph::new();
        let scene = graph.transient("scene", TextureDesc::hdr_color(8, 8));
        let normal = graph.transient("normal", TextureDesc::hdr_color(8, 8));
        let depth = graph.transient("depth", TextureDesc::depth(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_pass("use_normal", "use", vec![normal], vec![out]);
        graph.add_external_pass_with_depth("opaque", "opaque", vec![], vec![scene, normal], depth);
        assert_eq!(graph.passes()[1].writes(), vec![scene, normal, depth]);
        let schedule = super::schedule(&graph).unwrap();
        assert_eq!(schedule.order_names(), ["opaque", "use_normal"]);
    }

    #[test]
    fn mip_count_is_part_of_alias_compatibility() {
        let mut graph = RenderGraph::new();
        let flat = graph.transient("flat", TextureDesc::color(8, 8));
        let mipped = graph.transient("mipped", TextureDesc::color(8, 8).with_mip_levels(4));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        let out2 = graph.output("final2", TextureDesc::present_target(8, 8));
        // `flat` is dead before `mipped` is born, so only the mip count keeps them apart.
        graph.add_pass("make_flat", PassKind::Gradient, flat);
        graph.add_external_pass("flat_out", "f", vec![flat], vec![out]);
        graph.add_external_pass("seed", "s", vec![], vec![mipped.mip(0)]);
        for k in 1..4 {
            graph.add_external_pass(
                format!("d{k}"),
                "d",
                vec![mipped.mip(k - 1)],
                vec![mipped.mip(k)],
            );
        }
        graph.add_external_pass("mip_out", "o", vec![mipped], vec![out2]);
        let schedule = super::schedule(&graph).unwrap();
        assert_ne!(physical_of(&schedule, flat), physical_of(&schedule, mipped));
    }

    #[test]
    fn the_dump_names_mips_and_lists_every_color_target() {
        let json = super::schedule(&pyramid_graph()).unwrap().dump_graph_json();
        assert!(json.contains("\"pyramid@mip2\""), "{json}");
        assert!(json.contains("\"mips\": 4"), "{json}");

        let mut graph = RenderGraph::new();
        let scene = graph.transient("scene", TextureDesc::hdr_color(8, 8));
        let normal = graph.transient("normal", TextureDesc::hdr_color(8, 8));
        let depth = graph.transient("depth", TextureDesc::depth(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_pass_with_depth("opaque", "opaque", vec![], vec![scene, normal], depth);
        graph.add_external_pass("post", "post", vec![scene, normal], vec![out]);
        let dump: serde_json::Value =
            serde_json::from_str(&super::schedule(&graph).unwrap().dump_graph_json()).unwrap();
        assert_eq!(
            dump["passes"][0]["writes"],
            serde_json::json!(["scene", "normal", "depth"]),
        );
        assert!(!dump.to_string().contains("mips"));
    }

    /// Several writers of one resource run in declaration order, and a reader sees the last one.
    #[test]
    fn writers_of_one_resource_run_in_declaration_order() {
        let mut graph = RenderGraph::new();
        let x = graph.transient("x", TextureDesc::color(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_pass("a", "a", vec![], vec![x]);
        graph.add_external_pass("b", "b", vec![], vec![x]);
        graph.add_external_pass("c", "c", vec![x], vec![out]);
        let schedule = super::schedule(&graph).unwrap();
        assert_eq!(schedule.order_names(), ["a", "b", "c"]);
    }

    /// The scene split: opaque writes colour + depth, `mid` reads depth and writes colour, `late`
    /// writes colour + depth, `tail` reads both. Only the last writer feeds the reader.
    fn split_graph(mid_extra_read: bool) -> RenderGraph {
        let mut graph = RenderGraph::new();
        let s = graph.transient("s", TextureDesc::hdr_color(8, 8));
        let d = graph.transient("d", TextureDesc::depth(8, 8));
        let y = graph.transient("y", TextureDesc::color(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_pass_with_depth("opaque", "opaque", vec![], vec![s], d);
        let mid_reads = if mid_extra_read { vec![d, y] } else { vec![d] };
        graph.add_external_pass("mid", "mid", mid_reads, vec![s]);
        graph.add_external_pass_with_depth("late", "late", vec![], vec![s], d);
        graph.add_external_pass("tail", "tail", vec![s, d], vec![out]);
        if mid_extra_read {
            graph.add_external_pass("p", "p", vec![], vec![y]);
        }
        graph
    }

    #[test]
    fn a_contested_read_follows_the_latest_earlier_writer() {
        let schedule = super::schedule(&split_graph(false)).unwrap();
        assert_eq!(schedule.order_names(), ["opaque", "mid", "late", "tail"]);
    }

    /// Write-after-write on colour already orders `mid` before `late`; here `mid` writes elsewhere,
    /// so only the write-after-read edge on depth keeps `late` behind `mid` once `mid` waits on `p`.
    #[test]
    fn write_after_read_keeps_a_later_writer_behind_the_reader() {
        let mut graph = RenderGraph::new();
        let d = graph.transient("d", TextureDesc::depth(8, 8));
        let y = graph.transient("y", TextureDesc::color(8, 8));
        let z = graph.transient("z", TextureDesc::color(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_pass_with_depth("opaque", "opaque", vec![], vec![], d);
        graph.add_external_pass("mid", "mid", vec![d, y], vec![z]);
        graph.add_external_pass_with_depth("late", "late", vec![], vec![], d);
        graph.add_external_pass("p", "p", vec![], vec![y]);
        graph.add_external_pass("tail", "tail", vec![z, d], vec![out]);
        let names = super::schedule(&graph).unwrap().order_names().join(",");
        assert_eq!(names, "opaque,p,mid,late,tail");
        let split = super::schedule(&split_graph(true)).unwrap();
        let order = split.order_names();
        let at = |n: &str| order.iter().position(|&o| o == n).unwrap();
        assert!(at("mid") < at("late"), "{order:?}");
    }

    /// Mip0 is contested (a, b); mip1 has one writer `c`, which waits on `w`, declared after the
    /// reader. The reader must still wait for the latest earlier mip0 writer *and* for `c`.
    #[test]
    fn a_contested_read_orders_against_the_latest_writer_per_mip() {
        let mut graph = RenderGraph::new();
        let mut desc = TextureDesc::color(8, 8);
        desc.mip_levels = 2;
        let pyr = graph.transient("pyr", desc);
        let g = graph.transient("g", TextureDesc::color(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_pass("a", "a", vec![], vec![pyr.mip(0)]);
        graph.add_external_pass("b", "b", vec![], vec![pyr.mip(0)]);
        graph.add_external_pass("c", "c", vec![g], vec![pyr.mip(1)]);
        graph.add_external_pass("reader", "reader", vec![pyr], vec![out]);
        graph.add_external_pass("w", "w", vec![], vec![g]);
        let order = super::schedule(&graph).unwrap();
        let order = order.order_names();
        let at = |n: &str| order.iter().position(|&o| o == n).unwrap();
        assert!(at("b") < at("reader"), "{order:?}");
        assert!(at("c") < at("reader"), "{order:?}");
        assert!(at("w") < at("c"), "{order:?}");
        assert!(at("a") < at("b"), "{order:?}");
    }

    #[test]
    fn a_contested_read_declared_before_every_writer_is_rejected() {
        let mut graph = RenderGraph::new();
        let x = graph.transient("x", TextureDesc::color(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_pass("early", "early", vec![x], vec![out]);
        graph.add_external_pass("a", "a", vec![], vec![x]);
        graph.add_external_pass("b", "b", vec![], vec![x]);
        let err = super::schedule(&graph).unwrap_err();
        assert_eq!(err.code.as_str(), "L2002");
        assert!(
            err.message
                .contains("reads `x`, which several passes write, but none is declared before it"),
            "{}",
            err.message
        );
    }

    #[test]
    fn reading_and_writing_a_contested_resource_is_a_cycle() {
        let mut graph = RenderGraph::new();
        let x = graph.transient("x", TextureDesc::color(8, 8));
        graph.add_external_pass("a", "a", vec![], vec![x]);
        graph.add_external_pass("both", "both", vec![x], vec![x]);
        graph.add_external_pass("b", "b", vec![], vec![x]);
        let err = super::schedule(&graph).unwrap_err();
        assert_eq!(err.code.as_str(), "L2001");
    }
}
