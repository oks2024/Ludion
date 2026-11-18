//! Off-mesh links: directed edges between two grid nodes that [`find_path_linked`](crate::find_path_linked)
//! searches beside the grid's own links (a ladder, a vault, a door).

use crate::grid::NodeId;

/// One off-mesh link between two nodes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OffMeshLink {
    /// The node the traversal starts at.
    pub from: NodeId,
    /// The node the traversal ends at.
    pub to: NodeId,
    /// The total traversal cost, in the grid's cost unit (metres).
    pub cost: f32,
    /// Whether the link can also be crossed from `to` to `from`.
    pub bidirectional: bool,
    /// The caller's handle for the link, reported back on a path.
    pub tag: u32,
}

/// One directed edge of a [`NavLinks`] table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkEdge {
    /// The node the edge arrives at.
    pub to: NodeId,
    /// The traversal cost.
    pub cost: f32,
    /// The link's tag.
    pub tag: u32,
}

/// A table of off-mesh edges, indexed by their start node.
#[derive(Clone, Debug, Default)]
pub struct NavLinks {
    /// Start node of each edge, parallel to `edges`; sorted by `(from, tag, to)`.
    starts: Vec<u32>,
    edges: Vec<LinkEdge>,
}

impl NavLinks {
    /// Builds the table: a bidirectional link adds both directions; a link from a node to itself is skipped.
    ///
    /// ```
    /// use ludion_nav::{NavLinks, NodeId, OffMeshLink};
    ///
    /// let links = NavLinks::new(vec![OffMeshLink {
    ///     from: NodeId(0),
    ///     to: NodeId(5),
    ///     cost: 2.0,
    ///     bidirectional: true,
    ///     tag: 7,
    /// }]);
    /// assert_eq!(links.len(), 2);
    /// assert_eq!(links.out_edges(NodeId(5))[0].to, NodeId(0));
    /// ```
    pub fn new(links: Vec<OffMeshLink>) -> Self {
        let mut pairs: Vec<(u32, LinkEdge)> = Vec::with_capacity(links.len() * 2);
        for l in links {
            if l.from == l.to {
                continue;
            }
            let edge = |to| LinkEdge {
                to,
                cost: l.cost,
                tag: l.tag,
            };
            pairs.push((l.from.0, edge(l.to)));
            if l.bidirectional {
                pairs.push((l.to.0, edge(l.from)));
            }
        }
        pairs.sort_by_key(|(from, e)| (*from, e.tag, e.to.0));
        NavLinks {
            starts: pairs.iter().map(|(f, _)| *f).collect(),
            edges: pairs.into_iter().map(|(_, e)| e).collect(),
        }
    }

    /// Whether the table has no edge.
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// The number of directed edges.
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// The edges leaving `node`, in `(tag, to)` order.
    pub fn out_edges(&self, node: NodeId) -> &[LinkEdge] {
        let lo = self.starts.partition_point(|f| *f < node.0);
        let hi = self.starts.partition_point(|f| *f <= node.0);
        &self.edges[lo..hi]
    }
}
