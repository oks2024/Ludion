//! A per-node overlay on a [`NavGrid`](crate::NavGrid): dynamic blockers marked without re-sampling.

/// What an overlay does to one node.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum NodeMark {
    /// Untouched.
    #[default]
    None,
    /// Entering the node from an unmarked-by-cost node adds this many metres.
    Cost(f32),
    /// Passable for A*, never entered by flow fields, line walks or string pulls; the tag names the
    /// blocker that stops a routed walker.
    Stop(u32),
    /// Cannot be entered.
    Block,
}

/// Dense per-node marks. Precedence where marks overlap: `Block` > `Stop` > `Cost`.
///
/// ```
/// use ludion_nav::{NavOverlay, NodeMark};
///
/// let mut overlay = NavOverlay::new(3);
/// overlay.cost(0, 2.0);
/// overlay.cost(0, 3.0);
/// overlay.stop(1, 7);
/// overlay.block(1);
/// assert_eq!(overlay.mark(0), NodeMark::Cost(3.0));
/// assert_eq!(overlay.mark(1), NodeMark::Block);
/// assert!(!overlay.is_empty());
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct NavOverlay {
    marks: Vec<NodeMark>,
    marked: bool,
}

impl NavOverlay {
    /// An empty overlay for a grid of `node_count` nodes.
    pub fn new(node_count: usize) -> NavOverlay {
        NavOverlay {
            marks: vec![NodeMark::None; node_count],
            marked: false,
        }
    }

    /// The node count this overlay was built for.
    pub fn node_count(&self) -> usize {
        self.marks.len()
    }

    /// Whether no node has been marked.
    pub fn is_empty(&self) -> bool {
        !self.marked
    }

    /// The mark on node `n`; out of range is [`NodeMark::None`].
    pub fn mark(&self, n: u32) -> NodeMark {
        self.marks.get(n as usize).copied().unwrap_or_default()
    }

    /// Blocks node `n`: it beats every other mark.
    pub fn block(&mut self, n: u32) {
        if let Some(m) = self.marks.get_mut(n as usize) {
            *m = NodeMark::Block;
            self.marked = true;
        }
    }

    /// Marks node `n` as a stop with `tag`: it beats a cost, loses to a block, and the lowest tag wins.
    pub fn stop(&mut self, n: u32, tag: u32) {
        if let Some(m) = self.marks.get_mut(n as usize) {
            *m = match *m {
                NodeMark::Block => NodeMark::Block,
                NodeMark::Stop(t) => NodeMark::Stop(t.min(tag)),
                _ => NodeMark::Stop(tag),
            };
            self.marked = true;
        }
    }

    /// Prices entering node `n` at `metres`: it loses to a block or a stop, and the largest cost wins.
    pub fn cost(&mut self, n: u32, metres: f32) {
        if let Some(m) = self.marks.get_mut(n as usize) {
            *m = match *m {
                NodeMark::Cost(c) => NodeMark::Cost(c.max(metres)),
                NodeMark::None => NodeMark::Cost(metres),
                other => other,
            };
            self.marked = true;
        }
    }
}
