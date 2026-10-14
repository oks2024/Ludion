//! `dump-graph` JSON: a stable, deterministic description of a scheduled graph.
//!
//! The output is built entirely from ordered `Vec`s and `serde` structs (no hash maps), so field
//! order and content are fixed byte-for-byte given the same graph — suitable for snapshot testing
//! without a GPU. It records every pass (name, kind, reads, writes), every virtual resource (its
//! descriptor), the derived execution order, and the transient-aliasing assignment.

use serde::Serialize;

use crate::resource::{Extent2d, TextureFormat, TextureUsage};
use crate::schedule::Schedule;

#[derive(Serialize)]
struct GraphDump<'a> {
    passes: Vec<PassDump<'a>>,
    resources: Vec<ResourceDump<'a>>,
    schedule: Vec<&'a str>,
    aliasing: AliasingDump<'a>,
}

#[derive(Serialize)]
struct PassDump<'a> {
    name: &'a str,
    kind: &'a str,
    reads: Vec<&'a str>,
    writes: Vec<&'a str>,
}

#[derive(Serialize)]
struct ResourceDump<'a> {
    name: &'a str,
    kind: &'a str,
    format: &'a str,
    width: u32,
    height: u32,
    /// Present only for a 3D resource — a 2D one omits the key entirely, so every graph that
    /// declares no volume dumps exactly the bytes it dumped before volumes existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    depth: Option<u32>,
    usages: Vec<&'a str>,
}

#[derive(Serialize)]
struct AliasingDump<'a> {
    physical_texture_count: usize,
    physical_textures: Vec<PhysicalDump<'a>>,
    assignment: Vec<AssignmentDump<'a>>,
}

#[derive(Serialize)]
struct PhysicalDump<'a> {
    id: usize,
    format: &'a str,
    width: u32,
    height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    depth: Option<u32>,
    usages: Vec<&'a str>,
    resources: Vec<&'a str>,
}

#[derive(Serialize)]
struct AssignmentDump<'a> {
    resource: &'a str,
    physical: usize,
}

fn format_str(f: TextureFormat) -> &'static str {
    f.as_str()
}

fn usage_strs(usages: &[TextureUsage]) -> Vec<&'static str> {
    usages.iter().map(|u| u.as_str()).collect()
}

impl Schedule {
    /// Serializes this schedule to canonical `dump-graph` JSON (pretty-printed, trailing newline).
    ///
    /// The result is deterministic — identical graphs produce byte-identical output — so it is safe
    /// to snapshot-test. This is what `ludion dump-graph` prints; it requires no GPU.
    ///
    /// # Example
    /// ```
    /// let (graph, _out) = ludion_render_graph::build_graph(64, 64);
    /// let schedule = ludion_render_graph::schedule(&graph).unwrap();
    /// let json = schedule.dump_graph_json();
    /// assert!(json.contains("\"passes\""));
    /// assert!(json.ends_with('\n'));
    /// ```
    pub fn dump_graph_json(&self) -> String {
        let resources = self.resources();

        let passes = self
            .order
            .iter()
            .map(|&p| {
                let pass = &self.passes[p];
                PassDump {
                    name: &pass.name,
                    kind: pass.kind.as_str(),
                    reads: pass
                        .reads()
                        .iter()
                        .map(|r| resources[r.index()].name.as_str())
                        .collect(),
                    writes: pass
                        .writes()
                        .iter()
                        .map(|w| resources[w.index()].name.as_str())
                        .collect(),
                }
            })
            .collect();

        let resource_dumps = resources
            .iter()
            .map(|r| ResourceDump {
                name: &r.name,
                kind: match r.kind {
                    crate::graph::ResourceKind::Transient => "transient",
                    crate::graph::ResourceKind::Output => "output",
                },
                format: format_str(r.desc.format),
                width: r.desc.extent.width,
                height: r.desc.extent.height,
                depth: r.desc.depth_slices,
                usages: usage_strs(&r.desc.usages),
            })
            .collect();

        let schedule = self.order_names();

        let physical_textures = self
            .aliasing
            .physicals
            .iter()
            .map(|p| {
                let Extent2d { width, height } = p.extent;
                PhysicalDump {
                    id: p.id,
                    format: format_str(p.format),
                    width,
                    height,
                    depth: p.depth_slices,
                    usages: usage_strs(&p.usages),
                    resources: p
                        .resources
                        .iter()
                        .map(|&idx| resources[idx].name.as_str())
                        .collect(),
                }
            })
            .collect();

        let assignment = self
            .aliasing
            .assignment
            .iter()
            .enumerate()
            .map(|(idx, &phys)| AssignmentDump {
                resource: resources[idx].name.as_str(),
                physical: phys,
            })
            .collect();

        let dump = GraphDump {
            passes,
            resources: resource_dumps,
            schedule,
            aliasing: AliasingDump {
                physical_texture_count: self.aliasing.physicals.len(),
                physical_textures,
                assignment,
            },
        };

        let mut json = serde_json::to_string_pretty(&dump)
            .expect("dump structs are plain data and always serialize");
        json.push('\n');
        json
    }
}
