//! Concurrent lookup with visited-bit SIEVE eviction and byte-weighted admission.

use super::CacheEntry;
use alloy_primitives::{map::DefaultHashBuilder, Bytes};
use parking_lot::Mutex;
use reth_primitives_traits::dashmap::DashMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// Hits only set a visited bit; misses serialize queue maintenance separately from lookups.
#[derive(Debug)]
pub(super) struct SieveCache<S> {
    entries: DashMap<Bytes, Entry<S>, DefaultHashBuilder>,
    queue: Mutex<Queue>,
    capacity: usize,
}

impl<S: Clone> SieveCache<S> {
    pub(super) fn new(capacity: usize) -> Self {
        Self { entries: DashMap::default(), queue: Mutex::default(), capacity }
    }

    pub(super) fn get(&self, input: &[u8]) -> Option<CacheEntry<S>> {
        let entry = self.entries.get(input)?;
        entry.visited.store(true, Ordering::Relaxed);
        Some(entry.value.clone())
    }

    pub(super) fn insert(&self, input: Bytes, value: CacheEntry<S>) -> usize {
        // Charge at least one byte so even empty inputs and outputs cannot grow without bound.
        let weight = input.len().saturating_add(value.output.bytes.len()).max(1);
        let mut queue = self.queue.lock();
        if weight > self.capacity {
            return queue.len
        }

        let existing = self.entries.get(input.as_ref()).map(|entry| entry.slot);
        let slot = if let Some(slot) = existing {
            let node = queue.nodes[slot].as_mut().expect("live cache entry has a queue node");
            let previous = node.weight;
            node.weight = weight;
            queue.weight = queue.weight - previous + weight;
            slot
        } else {
            queue.push(input.clone(), weight)
        };
        self.entries
            .insert(input, Entry { value, visited: AtomicBool::new(existing.is_some()), slot });

        while queue.weight > self.capacity {
            let slot = queue.hand.or(queue.oldest).expect("overweight queue is nonempty");
            let node = queue.nodes[slot].as_ref().expect("hand points to a live queue node");
            let next = node.next.or(queue.oldest);
            let key = node.key.clone();
            let visited = self
                .entries
                .get(key.as_ref())
                .is_some_and(|entry| entry.visited.swap(false, Ordering::Relaxed));
            if visited {
                queue.hand = next;
            } else {
                self.entries.remove(key.as_ref());
                queue.remove(slot);
            }
        }
        queue.len
    }
}

#[derive(Debug)]
struct Entry<S> {
    value: CacheEntry<S>,
    visited: AtomicBool,
    slot: usize,
}

#[derive(Debug, Default)]
struct Queue {
    nodes: Vec<Option<Node>>,
    free: Vec<usize>,
    oldest: Option<usize>,
    newest: Option<usize>,
    hand: Option<usize>,
    weight: usize,
    len: usize,
}

impl Queue {
    fn push(&mut self, key: Bytes, weight: usize) -> usize {
        let node = Node { key, weight, previous: self.newest, next: None };
        let slot = if let Some(slot) = self.free.pop() {
            self.nodes[slot] = Some(node);
            slot
        } else {
            self.nodes.push(Some(node));
            self.nodes.len() - 1
        };
        if let Some(newest) = self.newest {
            self.nodes[newest].as_mut().expect("newest node is live").next = Some(slot);
        } else {
            self.oldest = Some(slot);
        }
        self.newest = Some(slot);
        self.weight += weight;
        self.len += 1;
        slot
    }

    fn remove(&mut self, slot: usize) {
        let node = self.nodes[slot].take().expect("removed node is live");
        if let Some(previous) = node.previous {
            self.nodes[previous].as_mut().expect("previous node is live").next = node.next;
        } else {
            self.oldest = node.next;
        }
        if let Some(next) = node.next {
            self.nodes[next].as_mut().expect("next node is live").previous = node.previous;
        } else {
            self.newest = node.previous;
        }
        self.hand = node.next.or(self.oldest);
        self.weight -= node.weight;
        self.len -= 1;
        self.free.push(slot);
    }
}

#[derive(Debug)]
struct Node {
    key: Bytes,
    weight: usize,
    previous: Option<usize>,
    next: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_revm::precompile::{PrecompileOutput, PrecompileStatus};
    use std::{sync::Arc, thread};

    fn value(bytes: Bytes) -> CacheEntry<u8> {
        CacheEntry {
            output: PrecompileOutput {
                status: PrecompileStatus::Success,
                gas_used: 1,
                state_gas_used: 0,
                state_gas_spilled: 0,
                reservoir: 0,
                gas_refunded: 0,
                bytes,
            },
            spec: 0,
        }
    }

    #[test]
    fn visited_entries_survive_without_reordering_the_queue() {
        let cache = SieveCache::new(3);
        for key in [b"a", b"b", b"c"] {
            cache.insert(Bytes::copy_from_slice(key), value(Bytes::new()));
        }
        assert!(cache.get(b"a").is_some());
        for (inserted, evicted) in [(b"d", b"b"), (b"e", b"c"), (b"f", b"d")] {
            assert_eq!(cache.insert(Bytes::copy_from_slice(inserted), value(Bytes::new())), 3);
            assert!(!cache.entries.contains_key(evicted.as_slice()));
        }
        assert!(cache.entries.contains_key(b"a".as_slice()));
    }

    #[test]
    fn replacement_and_admission_respect_byte_capacity() {
        let cache = SieveCache::new(4);
        cache.insert(Bytes::from_static(b"a"), value(Bytes::from_static(b"aa")));
        cache.insert(Bytes::from_static(b"b"), value(Bytes::from_static(b"b")));
        assert!(cache.get(b"a").is_none());
        assert_eq!(cache.insert(Bytes::from_static(b"b"), value(Bytes::from_static(b"bb"))), 1);
        assert_eq!(cache.queue.lock().weight, 3);
        assert_eq!(cache.insert(Bytes::from_static(b"large"), value(Bytes::new())), 1);
        assert!(cache.get(b"large").is_none());
        assert!(cache.get(b"b").is_some());
        assert_eq!(SieveCache::new(0).insert(Bytes::new(), value(Bytes::new())), 0);
    }

    #[test]
    fn concurrent_hits_and_replacements_preserve_values_and_queue_accounting() {
        let cache = Arc::new(SieveCache::new(64));
        let threads = (0..8)
            .map(|worker| {
                let cache = cache.clone();
                thread::spawn(move || {
                    for iteration in 0..2000 {
                        let key = Bytes::copy_from_slice(&[((worker + iteration) % 255) as u8]);
                        cache.insert(key.clone(), value(key.clone()));
                        if let Some(entry) = cache.get(&key) {
                            assert_eq!(entry.output.bytes, key);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        let queue = cache.queue.lock();
        assert!(queue.weight <= 64);
        assert_eq!(queue.len, cache.entries.len());
        let mut cursor = queue.oldest;
        let mut previous = None;
        let mut len = 0;
        let mut weight = 0;
        while let Some(slot) = cursor {
            let node = queue.nodes[slot].as_ref().unwrap();
            assert_eq!(node.previous, previous);
            assert_eq!(cache.entries.get(node.key.as_ref()).unwrap().slot, slot);
            weight += node.weight;
            len += 1;
            assert!(len <= queue.len);
            previous = cursor;
            cursor = node.next;
        }
        assert_eq!(previous, queue.newest);
        assert_eq!(len, queue.len);
        assert_eq!(weight, queue.weight);
    }
}
