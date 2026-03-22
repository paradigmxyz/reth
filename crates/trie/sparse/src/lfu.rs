use core::{fmt, hash};

use alloc::vec::Vec;
use alloy_primitives::map::HashMap;

/// Maximum frequency value. Frequencies are capped here to bound bucket storage.
const LFU_MAX_FREQ: u16 = 255;

/// Per-entry metadata stored in the LFU lookup map.
#[derive(Debug, Clone, Copy)]
struct LfuEntryMeta {
    /// Current frequency (1..=`LFU_MAX_FREQ`).
    freq: u16,
    /// Index of this key within `buckets[freq]`.
    pos: usize,
}

/// Bucketed LFU cache with O(1) eviction and O(1) touch operations.
///
/// Generic over the key type `K`.
#[derive(Debug)]
pub(crate) struct BucketedLfu<K> {
    capacity: usize,
    /// Maps each key to its frequency and bucket position.
    entries: HashMap<K, LfuEntryMeta>,
    /// `buckets[f]` holds all keys currently at frequency `f`.
    /// Index 0 is unused; valid frequencies are 1..=`LFU_MAX_FREQ`.
    buckets: Vec<Vec<K>>,
    /// Smallest non-empty frequency bucket.
    min_freq: u16,
}

impl<K> Default for BucketedLfu<K> {
    fn default() -> Self {
        Self::new(0)
    }
}

impl<K> BucketedLfu<K> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::default(),
            buckets: (0..=LFU_MAX_FREQ).map(|_| Vec::new()).collect(),
            min_freq: 1,
        }
    }
}

impl<K: fmt::Debug + Copy + Eq + hash::Hash> BucketedLfu<K> {
    /// Updates the capacity and evicts the least-frequently-used entries that exceed it.
    ///
    /// Entries accumulate via [`Self::touch`] over time. This method trims the cache
    /// so that only the `capacity` hottest entries are retained.
    pub(crate) fn decay_and_evict(&mut self, capacity: usize) {
        self.capacity = capacity;

        if self.capacity == 0 {
            self.entries.clear();
            for b in &mut self.buckets {
                b.clear();
            }
            self.min_freq = 1;
            return;
        }

        while self.entries.len() > self.capacity {
            self.evict_one();
        }
    }

    /// Evicts a single entry from the lowest-frequency bucket.
    fn evict_one(&mut self) {
        if self.entries.is_empty() {
            self.min_freq = 1;
            return;
        }

        // Advance min_freq to the next non-empty bucket.
        while (self.min_freq as usize) < self.buckets.len() &&
            self.buckets[self.min_freq as usize].is_empty()
        {
            self.min_freq += 1;
        }

        if (self.min_freq as usize) >= self.buckets.len() {
            self.min_freq = 1;
            return;
        }

        if let Some(key) = self.buckets[self.min_freq as usize].pop() {
            self.entries.remove(&key);

            while (self.min_freq as usize) < self.buckets.len() &&
                self.buckets[self.min_freq as usize].is_empty()
            {
                self.min_freq += 1;
            }

            if (self.min_freq as usize) >= self.buckets.len() {
                self.min_freq = 1;
            }
        }
    }

    /// Records a key touch. O(1) amortized.
    pub(crate) fn touch(&mut self, key: K) {
        if self.capacity == 0 {
            return;
        }

        if let Some(meta) = self.entries.get(&key).copied() {
            debug_assert_eq!(self.buckets[meta.freq as usize][meta.pos], key);

            let old_freq = meta.freq as usize;
            let new_freq = meta.freq.saturating_add(1).min(LFU_MAX_FREQ);

            if new_freq as usize != old_freq {
                // Remove from old bucket via swap_remove and fix the swapped element's pos.
                self.buckets[old_freq].swap_remove(meta.pos);
                if let Some(&moved_key) = self.buckets[old_freq].get(meta.pos) {
                    self.entries.get_mut(&moved_key).expect("moved key must exist").pos = meta.pos;
                }

                // Insert into new bucket.
                let new_pos = self.buckets[new_freq as usize].len();
                self.buckets[new_freq as usize].push(key);

                let entry = self.entries.get_mut(&key).expect("key must exist");
                entry.freq = new_freq;
                entry.pos = new_pos;

                // Update min_freq if old bucket is now empty.
                if self.buckets[old_freq].is_empty() && old_freq == self.min_freq as usize {
                    self.min_freq = new_freq;
                }
            }
        } else {
            // Evict if at capacity before inserting.
            if self.entries.len() >= self.capacity {
                self.evict_one();
            }

            // New entry at frequency 1.
            let pos = self.buckets[1].len();
            self.buckets[1].push(key);
            self.entries.insert(key, LfuEntryMeta { freq: 1, pos });
            self.min_freq = 1;
        }
    }

    /// Returns an iterator over all retained keys.
    #[cfg(any(test, feature = "std"))]
    pub(crate) fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.keys()
    }

    /// Returns the number of retained keys.
    #[cfg(any(test, feature = "std"))]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::{BTreeMap, BTreeSet};
    use proptest::prelude::*;

    #[derive(Clone, Copy, Debug)]
    enum Op {
        SetCapacity(usize),
        Touch(u8),
    }

    #[derive(Debug, Default)]
    struct ModelLfu {
        capacity: usize,
        entries: BTreeMap<u8, u16>,
    }

    impl ModelLfu {
        fn apply(&mut self, op: Op) {
            match op {
                Op::SetCapacity(capacity) => self.set_capacity(capacity),
                Op::Touch(key) => self.touch(key),
            }
        }

        fn set_capacity(&mut self, capacity: usize) {
            self.capacity = capacity;

            if capacity == 0 {
                self.entries.clear();
                return;
            }

            while self.entries.len() > self.capacity {
                self.evict_one();
            }
        }

        fn touch(&mut self, key: u8) {
            if self.capacity == 0 {
                return;
            }

            if let Some(freq) = self.entries.get_mut(&key) {
                *freq = freq.saturating_add(1).min(LFU_MAX_FREQ);
                return;
            }

            if self.entries.len() >= self.capacity {
                self.evict_one();
            }

            self.entries.insert(key, 1);
        }

        fn evict_one(&mut self) {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(key, freq)| (**freq, **key))
                .map(|(key, _)| *key)
                .expect("model eviction requires a live entry");
            self.entries.remove(&victim);
        }

        fn has_unique_frequencies(&self) -> bool {
            let mut freqs = BTreeSet::new();
            self.entries.values().all(|freq| freqs.insert(*freq))
        }

        fn snapshot(&self) -> BTreeMap<u8, u16> {
            self.entries.clone()
        }
    }

    fn apply_real(lfu: &mut BucketedLfu<u8>, op: Op) {
        match op {
            Op::SetCapacity(capacity) => lfu.decay_and_evict(capacity),
            Op::Touch(key) => lfu.touch(key),
        }
    }

    fn snapshot(lfu: &BucketedLfu<u8>) -> BTreeMap<u8, u16> {
        lfu.entries.iter().map(|(key, meta)| (*key, meta.freq)).collect()
    }

    fn assert_valid(lfu: &BucketedLfu<u8>) {
        let mut seen = BTreeSet::new();
        let mut actual_min_freq = None;

        for freq in 1..lfu.buckets.len() {
            for (pos, key) in lfu.buckets[freq].iter().copied().enumerate() {
                assert!(seen.insert(key), "duplicate key {key} in buckets");

                let meta =
                    lfu.entries.get(&key).unwrap_or_else(|| panic!("missing entry for {key}"));
                assert_eq!(meta.freq as usize, freq, "wrong frequency for key {key}");
                assert_eq!(meta.pos, pos, "wrong position for key {key}");

                actual_min_freq.get_or_insert(freq as u16);
            }
        }

        assert_eq!(seen.len(), lfu.entries.len(), "bucket/entry count mismatch");

        for (key, meta) in &lfu.entries {
            assert_ne!(meta.freq, 0, "zero frequency for key {key}");
            assert!(
                meta.pos < lfu.buckets[meta.freq as usize].len(),
                "position out of bounds for key {key}"
            );
            assert_eq!(
                lfu.buckets[meta.freq as usize][meta.pos], *key,
                "bucket position mismatch for key {key}"
            );
        }

        assert_eq!(lfu.min_freq, actual_min_freq.unwrap_or(1), "min_freq mismatch");
    }

    fn is_safe_op(model: &ModelLfu, op: Op) -> bool {
        match op {
            Op::SetCapacity(capacity) => {
                capacity >= model.entries.len() || model.has_unique_frequencies()
            }
            Op::Touch(key) => {
                if model.capacity == 0 {
                    return true;
                }

                let is_new_key = !model.entries.contains_key(&key);
                let would_evict = is_new_key && model.entries.len() >= model.capacity;
                !would_evict || model.has_unique_frequencies()
            }
        }
    }

    fn sanitize_trace(raw_ops: Vec<Op>) -> Vec<Op> {
        let mut model = ModelLfu::default();
        let mut safe_ops = Vec::with_capacity(raw_ops.len());

        for op in raw_ops {
            if is_safe_op(&model, op) {
                model.apply(op);
                safe_ops.push(op);
            }
        }

        safe_ops
    }

    fn raw_trace_strategy() -> impl Strategy<Value = Vec<Op>> {
        prop::collection::vec(
            prop_oneof![(0usize..=4).prop_map(Op::SetCapacity), (0u8..=5).prop_map(Op::Touch),],
            0..256,
        )
        .prop_map(sanitize_trace)
    }

    fn check_trace(ops: &[Op]) {
        let mut real = BucketedLfu::new(0);
        let mut model = ModelLfu::default();

        for (step, &op) in ops.iter().enumerate() {
            apply_real(&mut real, op);
            model.apply(op);

            assert_valid(&real);
            assert_eq!(snapshot(&real), model.snapshot(), "mismatch at step {step}: {op:?}");
        }
    }

    #[test]
    fn bucketed_lfu_zero_capacity_touches_are_ignored() {
        let mut lfu = BucketedLfu::default();

        lfu.touch(1);
        lfu.touch(2);

        assert_valid(&lfu);
        assert!(lfu.entries.is_empty());
        assert_eq!(lfu.min_freq, 1);
    }

    #[test]
    fn bucketed_lfu_retouch_does_not_duplicate_keys() {
        check_trace(&[Op::SetCapacity(2), Op::Touch(1), Op::Touch(1), Op::Touch(1)]);
    }

    #[test]
    fn bucketed_lfu_shrink_keeps_hottest_keys() {
        check_trace(&[
            Op::SetCapacity(4),
            Op::Touch(1),
            Op::Touch(2),
            Op::Touch(2),
            Op::Touch(3),
            Op::Touch(3),
            Op::Touch(3),
            Op::Touch(4),
            Op::Touch(4),
            Op::Touch(4),
            Op::Touch(4),
            Op::SetCapacity(2),
        ]);
    }

    #[test]
    fn bucketed_lfu_touch_saturates_frequency() {
        let mut lfu = BucketedLfu::new(1);

        lfu.touch(1);
        for _ in 0..(LFU_MAX_FREQ as usize + 16) {
            lfu.touch(1);
        }

        assert_valid(&lfu);
        assert_eq!(lfu.entries[&1].freq, LFU_MAX_FREQ);
        assert_eq!(lfu.buckets[LFU_MAX_FREQ as usize], vec![1]);
    }

    #[test]
    fn bucketed_lfu_tie_eviction_keeps_new_key_and_one_old_key_is_dropped() {
        let mut lfu = BucketedLfu::new(2);

        lfu.touch(1);
        lfu.touch(2);
        lfu.touch(3);

        assert_valid(&lfu);
        assert_eq!(lfu.entries.len(), 2);
        assert!(lfu.entries.contains_key(&3));
        assert_eq!(
            [lfu.entries.contains_key(&1), lfu.entries.contains_key(&2)]
                .into_iter()
                .filter(|present| *present)
                .count(),
            1
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn bucketed_lfu_model_safe_proptest_traces(ops in raw_trace_strategy()) {
            check_trace(&ops);
        }
    }
}
