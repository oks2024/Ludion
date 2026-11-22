//! `dump-graph` JSON: a stable, deterministic description of a scheduled graph.
//!
//! The output is built entirely from ordered `Vec`s and `serde` structs (no hash maps), so field
//! order and content are fixed byte-for-byte given the same graph — suitable for snapshot testing
//! without a GPU. It records every pass (name, kind, reads, writes), every virtual resource (its
//! descriptor), the derived execution order, and the transient-aliasing assignment. Graph buffers add a
//! top-level `buffers` array and per-pass `buffer_reads`/`buffer_writes`, each omitted when empty so
//! a graph without buffers dumps the bytes it dumped before buffers existed. Secondary views add a
//! trailing `views` array of `{ name, graph }` (each `graph` this same shape), omitted when empty.

use serde::Serialize;

use crate::resource::{Extent2d, TextureFormat, TextureUsage};
use crate::schedule::Schedule;

#[derive(Serialize)]
struct GraphDump<'a> {
    passes: Vec<PassDump<'a>>,
    resources: Vec<ResourceDump<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    buffers: Vec<BufferDump<'a>>,
    schedule: Vec<&'a str>,
    aliasing: AliasingDump<'a>,
    /// Secondary views' own graphs; omitted when empty so a view-free graph dumps its old bytes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    views: Vec<ViewDump<'a>>,
}

#[derive(Serialize)]
struct ViewDump<'a> {
    name: &'a str,
    graph: GraphDump<'a>,
}

#[derive(Serialize)]
struct PassDump<'a> {
    name: &'a str,
    kind: &'a str,
    reads: Vec<String>,
    writes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    buffer_reads: Vec<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    buffer_writes: Vec<&'a str>,
}

#[derive(Serialize)]
struct BufferDump<'a> {
    name: &'a str,
    size: u64,
    usages: Vec<&'a str>,
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
    /// Present only for a mipped resource, for the same reason as `depth`.
    #[serde(skip_serializing_if = "is_one")]
    mips: u32,
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
    #[serde(skip_serializing_if = "is_one")]
    mips: u32,
    usages: Vec<&'a str>,
    resources: Vec<&'a str>,
}

#[derive(Serialize)]
struct AssignmentDump<'a> {
    resource: &'a str,
    physical: usize,
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

/// A handle's dump token: the resource name, with `@mip{k}` for a single-level handle.
fn handle_str(resources: &[crate::graph::ResourceDecl], id: crate::resource::ResourceId) -> String {
    let name = &resources[id.index()].name;
    match id.mip_level() {
        Some(level) => format!("{name}@mip{level}"),
        None => name.clone(),
    }
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
        self.dump_graph_json_with_views(&[])
    }

    /// Like [`Schedule::dump_graph_json`], with one nested graph per `(name, schedule)` view appended
    /// under a trailing top-level `views` key (omitted when `views` is empty).
    ///
    /// # Example
    /// ```
    /// let (graph, _out) = ludion_render_graph::build_graph(64, 64);
    /// let schedule = ludion_render_graph::schedule(&graph).unwrap();
    /// let json = schedule.dump_graph_json_with_views(&[("radar", &schedule)]);
    /// assert!(json.contains("\"views\""));
    /// ```
    pub fn dump_graph_json_with_views(&self, views: &[(&str, &Schedule)]) -> String {
        let mut dump = self.graph_dump();
        dump.views = views
            .iter()
            .map(|&(name, schedule)| ViewDump {
                name,
                graph: schedule.graph_dump(),
            })
            .collect();
        let mut json = serde_json::to_string_pretty(&dump)
            .expect("dump structs are plain data and always serialize");
        json.push('\n');
        json
    }

    fn graph_dump(&self) -> GraphDump<'_> {
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
                        .map(|&r| handle_str(resources, r))
                        .collect(),
                    writes: pass
                        .writes()
                        .iter()
                        .map(|&w| handle_str(resources, w))
                        .collect(),
                    buffer_reads: pass
                        .buffer_reads
                        .iter()
                        .map(|b| self.buffers[b.index()].name.as_str())
                        .collect(),
                    buffer_writes: pass
                        .buffer_writes
                        .iter()
                        .map(|b| self.buffers[b.index()].name.as_str())
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
                mips: r.desc.mip_levels,
                usages: usage_strs(&r.desc.usages),
            })
            .collect();

        let buffer_dumps = self
            .buffers
            .iter()
            .map(|b| BufferDump {
                name: &b.name,
                size: b.desc.size,
                usages: b.desc.usages.iter().map(|u| u.as_str()).collect(),
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
                    mips: p.mip_levels,
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

        GraphDump {
            passes,
            resources: resource_dumps,
            buffers: buffer_dumps,
            schedule,
            aliasing: AliasingDump {
                physical_texture_count: self.aliasing.physicals.len(),
                physical_textures,
                assignment,
            },
            views: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{build_graph, schedule};

    #[test]
    fn no_views_dumps_the_plain_bytes() {
        let (graph, _) = build_graph(64, 64);
        let s = schedule(&graph).unwrap();
        assert_eq!(s.dump_graph_json_with_views(&[]), s.dump_graph_json());
        assert!(!s.dump_graph_json().contains("\"views\""));
    }

    #[test]
    fn a_view_appends_a_trailing_key_and_leaves_the_rest() {
        let (graph, _) = build_graph(64, 64);
        let s = schedule(&graph).unwrap();
        let (vgraph, _) = build_graph(32, 32);
        let v = schedule(&vgraph).unwrap();
        let plain = s.dump_graph_json();
        let with = s.dump_graph_json_with_views(&[("radar", &v)]);
        let open = plain.strip_suffix("\n}\n").unwrap();
        assert!(with.starts_with(open));
        let parsed: serde_json::Value = serde_json::from_str(&with).unwrap();
        assert_eq!(parsed["views"][0]["name"], "radar");
        let nested: serde_json::Value = serde_json::from_str(&v.dump_graph_json()).unwrap();
        assert_eq!(parsed["views"][0]["graph"], nested);
    }
}
