//! Order-stable collections.
//!
//! Sim-side code may not use `std::collections::HashMap` / `HashSet`: their iteration order depends
//! on a per-process random seed, which silently breaks determinism (two runs, or two processes,
//! visit entities in different orders and diverge). The strict `clippy.toml` in this crate makes
//! reaching for them a build error.
//!
//! [`StableMap`] and [`StableSet`] are thin newtypes over `BTreeMap` / `BTreeSet`, which iterate in
//! sorted key order — deterministic by construction. They expose only the surface the engine
//! actually needs; reach for the underlying type through [`StableMap::as_inner`] if something exotic
//! is required.
//!
//! **Sorted key order is the contract, not an implementation detail.** "Stable" here means *sorted*,
//! not *insertion-ordered*: `ludion-ecs` stores components in dense ascending `keys`/`values` columns and joins
//! queries with a sorted merge that is only correct on ascending columns — an insertion-ordered
//! backing store would silently drop matching rows, with no compile error anywhere. So the backing
//! store may only ever be swapped for another sorted-iteration map.

use std::collections::btree_map::{Entry, IntoIter as MapIntoIter, Iter, IterMut, Keys, Values};
use std::collections::btree_set::{IntoIter as SetIntoIter, Iter as SetIter};
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// A deterministic-iteration map, backed by [`BTreeMap`].
///
/// Iteration always visits keys in sorted order, independent of insertion order or process.
///
/// # Example
///
/// ```
/// use ludion_core::collections::StableMap;
///
/// let mut m = StableMap::new();
/// m.insert(2, "two");
/// m.insert(1, "one");
/// // Sorted key order, deterministically.
/// let keys: Vec<_> = m.keys().copied().collect();
/// assert_eq!(keys, vec![1, 2]);
/// ```
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(
    transparent,
    bound(deserialize = "K: Ord + Deserialize<'de>, V: Deserialize<'de>")
)]
pub struct StableMap<K, V>(BTreeMap<K, V>);

impl<K, V> StableMap<K, V> {
    /// Creates an empty map.
    pub const fn new() -> Self {
        StableMap(BTreeMap::new())
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Removes all entries.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Borrows the underlying `BTreeMap` (escape hatch for uncommon operations).
    pub fn as_inner(&self) -> &BTreeMap<K, V> {
        &self.0
    }

    /// Iterates entries in sorted key order.
    pub fn iter(&self) -> Iter<'_, K, V> {
        self.0.iter()
    }

    /// Iterates entries in sorted key order, with mutable values.
    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        self.0.iter_mut()
    }

    /// Iterates keys in sorted order.
    pub fn keys(&self) -> Keys<'_, K, V> {
        self.0.keys()
    }

    /// Iterates values in sorted-key order.
    pub fn values(&self) -> Values<'_, K, V> {
        self.0.values()
    }
}

impl<K: Ord, V> StableMap<K, V> {
    /// Inserts a key/value pair, returning the previous value if the key was present.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.0.insert(key, value)
    }

    /// Gets a reference to the value for `key`.
    pub fn get(&self, key: &K) -> Option<&V> {
        self.0.get(key)
    }

    /// Gets a mutable reference to the value for `key`.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.0.get_mut(key)
    }

    /// Removes `key`, returning its value if present.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.0.remove(key)
    }

    /// Whether `key` is present.
    pub fn contains_key(&self, key: &K) -> bool {
        self.0.contains_key(key)
    }

    /// Gets the entry for in-place manipulation.
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        self.0.entry(key)
    }
}

impl<K, V> Default for StableMap<K, V> {
    fn default() -> Self {
        StableMap::new()
    }
}

impl<K: Ord, V> FromIterator<(K, V)> for StableMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        StableMap(BTreeMap::from_iter(iter))
    }
}

impl<K: Ord, V> Extend<(K, V)> for StableMap<K, V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}

impl<K, V> IntoIterator for StableMap<K, V> {
    type Item = (K, V);
    type IntoIter = MapIntoIter<K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, K, V> IntoIterator for &'a StableMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a, K, V> IntoIterator for &'a mut StableMap<K, V> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}

/// A deterministic-iteration set, backed by [`BTreeSet`].
///
/// Iteration always visits elements in sorted order, independent of insertion order or process.
///
/// # Example
///
/// ```
/// use ludion_core::collections::StableSet;
///
/// let mut s = StableSet::new();
/// s.insert(3);
/// s.insert(1);
/// let items: Vec<_> = s.iter().copied().collect();
/// assert_eq!(items, vec![1, 3]);
/// ```
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(transparent, bound(deserialize = "T: Ord + Deserialize<'de>"))]
pub struct StableSet<T>(BTreeSet<T>);

impl<T> StableSet<T> {
    /// Creates an empty set.
    pub const fn new() -> Self {
        StableSet(BTreeSet::new())
    }

    /// Number of elements.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Removes all elements.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Borrows the underlying `BTreeSet` (escape hatch for uncommon operations).
    pub fn as_inner(&self) -> &BTreeSet<T> {
        &self.0
    }

    /// Iterates elements in sorted order.
    pub fn iter(&self) -> SetIter<'_, T> {
        self.0.iter()
    }
}

impl<T: Ord> StableSet<T> {
    /// Inserts `value`, returning `true` if it was newly added.
    pub fn insert(&mut self, value: T) -> bool {
        self.0.insert(value)
    }

    /// Removes `value`, returning `true` if it was present.
    pub fn remove(&mut self, value: &T) -> bool {
        self.0.remove(value)
    }

    /// Whether `value` is present.
    pub fn contains(&self, value: &T) -> bool {
        self.0.contains(value)
    }
}

impl<T> Default for StableSet<T> {
    fn default() -> Self {
        StableSet::new()
    }
}

impl<T: Ord> FromIterator<T> for StableSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        StableSet(BTreeSet::from_iter(iter))
    }
}

impl<T: Ord> Extend<T> for StableSet<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}

impl<T> IntoIterator for StableSet<T> {
    type Item = T;
    type IntoIter = SetIntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a StableSet<T> {
    type Item = &'a T;
    type IntoIter = SetIter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_iterates_in_sorted_order() {
        let mut m = StableMap::new();
        for k in [5, 1, 4, 2, 3] {
            m.insert(k, k * 10);
        }
        let collected: Vec<_> = m.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(collected, vec![(1, 10), (2, 20), (3, 30), (4, 40), (5, 50)]);
    }

    #[test]
    fn map_basic_ops() {
        let mut m: StableMap<&str, i32> = StableMap::new();
        assert!(m.is_empty());
        m.insert("a", 1);
        *m.entry("b").or_insert(0) += 5;
        assert_eq!(m.get(&"a"), Some(&1));
        assert_eq!(m.get(&"b"), Some(&5));
        assert!(m.contains_key(&"a"));
        assert_eq!(m.remove(&"a"), Some(1));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn set_iterates_in_sorted_order() {
        let s: StableSet<i32> = [9, 3, 7, 1].into_iter().collect();
        let collected: Vec<_> = s.iter().copied().collect();
        assert_eq!(collected, vec![1, 3, 7, 9]);
        assert!(s.contains(&7));
    }

    #[test]
    fn serde_is_transparent() {
        let m: StableMap<String, i32> = [("k".to_string(), 1)].into_iter().collect();
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json, r#"{"k":1}"#);
        let back: StableMap<String, i32> = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }
}
