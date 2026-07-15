// JS `Map`/`new Map()` iterates in INSERTION order, and computeWeaponAccuracyStats/
// computePlayerStats/computeTickAggregates all build their output ARRAYS by iterating such a Map
// (`[...someMap.values()]` / `[...someMap.entries()]`). Array element order is part of what the
// parity harness fingerprints (canonicalize only sorts OBJECT keys, not array order -- array
// order is real, meaningful output order here, same as `rounds`/`events` in compute_events), so
// Rust must reproduce insertion order exactly. A plain `HashMap`/`BTreeMap` does not (unordered /
// key-sorted respectively) -- this tiny Vec+HashMap combo does, with no new crate dependency
// (all these maps hold at most a few dozen entries -- weapons/players per match -- so the O(1)
// amortized insert via the HashMap index, O(n) into_entries() drain, is more than fast enough).
use std::collections::HashMap;
use std::hash::Hash;

pub struct OrderedMap<K: Eq + Hash + Clone, V> {
  order: Vec<K>,
  map: HashMap<K, V>,
}

impl<K: Eq + Hash + Clone, V> OrderedMap<K, V> {
  pub fn new() -> Self {
    Self { order: Vec::new(), map: HashMap::new() }
  }

  pub fn contains_key(&self, k: &K) -> bool {
    self.map.contains_key(k)
  }

  pub fn get(&self, k: &K) -> Option<&V> {
    self.map.get(k)
  }

  pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
    self.map.get_mut(k)
  }

  pub fn entry_or_insert_with(&mut self, k: K, f: impl FnOnce() -> V) -> &mut V {
    if !self.map.contains_key(&k) {
      self.order.push(k.clone());
      self.map.insert(k.clone(), f());
    }
    self.map.get_mut(&k).unwrap()
  }

  pub fn insert(&mut self, k: K, v: V) {
    if !self.map.contains_key(&k) {
      self.order.push(k.clone());
    }
    self.map.insert(k, v);
  }

  /// Drains in insertion order.
  pub fn into_entries(mut self) -> Vec<(K, V)> {
    self.order.into_iter().map(|k| { let v = self.map.remove(&k).unwrap(); (k, v) }).collect()
  }

  pub fn into_values(self) -> Vec<V> {
    self.into_entries().into_iter().map(|(_, v)| v).collect()
  }
}
