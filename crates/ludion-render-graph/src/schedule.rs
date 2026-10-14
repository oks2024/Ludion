//! Pass scheduling and transient aliasing — the deterministic, CPU-only core of the frame graph.
//!
//! [`schedule`] turns a [`RenderGraph`] into a [`Schedule`]: it derives a stable execution order
//! from the pass dependency edges, rejects cycles and reads of never-written resources with L-coded
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
/// - `L2002` if a pass reads a resource that no pass writes.
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

/// Rejects any pass that reads a resource no pass ever writes (`L2002`).
fn validate_reads(resources: &[ResourceDecl], passes: &[PassDecl]) -> Result<(), Diagnostic> {
    let mut written = vec![false; resources.len()];
    for pass in passes {
        for write in pass.writes() {
            written[write.index()] = true;
        }
    }
    for pass in passes {
        for read in pass.reads() {
            if !written[read.index()] {
                let name = &resources[read.index()].name;
                return Err(Diagnostic::new(
                    code::RENDER_GRAPH_UNWRITTEN_READ,
                    format!(
                        "pass `{}` reads resource `{name}`, but no pass writes it",
                        pass.name
                    ),
                )
                .with_help(
                    "add a pass that writes this resource before the pass that reads it, or \
                     correct the read to name a resource that is produced",
                ));
            }
        }
    }
    Ok(())
}

/// Kahn topological sort with a deterministic tie-break: among passes whose dependencies are all
/// scheduled, the one declared earliest (smallest index) runs next. Rejects cycles (`L2001`).
fn topo_order(passes: &[PassDecl]) -> Result<Vec<usize>, Diagnostic> {
    let n = passes.len();

    // A pass depends on the **set** of passes that write a resource it reads. Deduplicated, because
    // the in-degree below is decremented once per (consumer, producer) pair: a pass reading two
    // resources from one producer — `composite` reading the scene pass's colour *and* its depth —
    // would otherwise carry an in-degree it can never work off, and report itself as a cycle.
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (consumer, pass) in passes.iter().enumerate() {
        for read in pass.reads() {
            for (producer, other) in passes.iter().enumerate() {
                if other.writes().contains(&read) && !deps[consumer].contains(&producer) {
                    deps[consumer].push(producer);
                }
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
        graph.add_external_pass_with_depth("scene", "scene", vec![], color, depth);
        graph.add_external_pass("post", "post", vec![color, depth], out);
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
        graph.add_external_pass_with_depth("scene", "scene", vec![], color, depth);
        graph.add_external_pass("tonemap", "tonemap", vec![color], out);
        let schedule = super::schedule(&graph).unwrap();
        assert_ne!(
            physical_of(&schedule, color),
            physical_of(&schedule, depth),
            "a depth attachment never shares a physical with a color transient",
        );
    }

    #[test]
    fn depth_only_pass_orders_before_a_later_sampler() {
        // The future shadow-map shape: a depth-only pass writes a depth resource that a later pass
        // samples. The scheduler must derive the shadow→scene edge from the depth write, exactly as
        // it does for a color write — proving depth resources can be read by a later pass.
        let mut graph = RenderGraph::new();
        let shadow_map = graph.transient("shadow_map", TextureDesc::depth(16, 16));
        let scene = graph.transient("scene", TextureDesc::hdr_color(8, 8));
        let out = graph.output("final", TextureDesc::present_target(8, 8));
        graph.add_external_depth_pass("shadow", "shadow", vec![], shadow_map);
        graph.add_external_pass("scene", "scene", vec![shadow_map], scene);
        graph.add_external_pass("tonemap", "tonemap", vec![scene], out);
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
        graph.add_external_pass("shade", "shade", vec![volume], out);
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
        graph.add_external_pass("shade", "shade", vec![volume], out);
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
        graph.add_external_pass("shade", "shade", vec![volume], flat); // volume's last use: pos 1
        graph.add_external_pass("present", "present", vec![flat], out);
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
        graph.add_external_pass("shade", "shade", vec![volume], out);
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
        let scene = ResourceId(0);
        let veiled = ResourceId(1);
        let bloom = ResourceId(2);
        assert_eq!(physical_of(&schedule, scene), physical_of(&schedule, bloom));
        assert_ne!(
            physical_of(&schedule, scene),
            physical_of(&schedule, veiled)
        );
    }
}
