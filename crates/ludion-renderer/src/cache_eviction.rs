//! Whether a frame's resource-cache resolves drop the entries the frame does not name.

use std::collections::HashMap;

/// The cache policy of one `prepare_frame`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheEviction {
    /// A main frame: the caches hold its working set and nothing else, so a scene change cannot
    /// leak resources.
    Evict,
    /// A probe capture: its derived scene leaves out decals and reads a frozen world, so it adds what
    /// it names and removes nothing.
    Keep,
}

impl CacheEviction {
    /// Drops the entries of `cache` that `referenced` rejects, unless this policy keeps them.
    pub(crate) fn retain<K, V>(self, cache: &mut HashMap<K, V>, referenced: impl Fn(&K) -> bool) {
        if self == CacheEviction::Evict {
            cache.retain(|key, _| referenced(key));
        }
    }
}
