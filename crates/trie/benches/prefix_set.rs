use criterion::{
    black_box, criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use proptest::{
    prelude::*,
    strategy::{Strategy, ValueTree},
    test_runner::{basic_result_cache, TestRunner},
};
use reth_primitives::trie::Nibbles;
use reth_trie::prefix_set::PrefixSet;
use std::collections::BTreeSet;

pub trait PrefixSetAbstraction: Default {
    fn insert(&mut self, key: Nibbles);
    fn contains(&mut self, key: Nibbles) -> bool;
}

impl PrefixSetAbstraction for PrefixSet {
    fn insert(&mut self, key: Nibbles) {
        self.insert(key)
    }

    fn contains(&mut self, key: Nibbles) -> bool {
        PrefixSet::contains(self, key)
    }
}

pub fn prefix_set_lookups(c: &mut Criterion) {
    let mut group = c.benchmark_group("Prefix Set Lookups");

    for size in [10, 100, 1_000, 10_000] {
        let test_data = generate_test_data(size);

        use implementations::*;
        prefix_set_bench::<BTreeAnyPrefixSet>(
            &mut group,
            "`BTreeSet` with `Iterator::any` lookup",
            test_data.clone(),
        );
        prefix_set_bench::<BTreeRangeLastCheckedPrefixSet>(
            &mut group,
            "`BTreeSet` with `BTreeSet::range` lookup",
            test_data.clone(),
        );
        prefix_set_bench::<VecCursorPrefixSet>(
            &mut group,
            "`Vec` with custom cursor lookup",
            test_data.clone(),
        );
        prefix_set_bench::<VecBinarySearchPrefixSet>(
            &mut group,
            "`Vec` with binary search lookup",
            test_data.clone(),
        );
    }
}

fn prefix_set_bench<T: PrefixSetAbstraction>(
    group: &mut BenchmarkGroup<WallTime>,
    description: &str,
    (preload, input, expected): (Vec<Nibbles>, Vec<Nibbles>, Vec<bool>),
) {
    let setup = || {
        let mut prefix_set = T::default();
        for key in preload.iter() {
            prefix_set.insert(key.clone());
        }
        (prefix_set, input.clone(), expected.clone())
    };

    let group_id = format!(
        "prefix set | preload size: {} | input size: {} | {}",
        preload.len(),
        input.len(),
        description
    );
    group.bench_function(group_id, |b| {
        b.iter_with_setup(setup, |(mut prefix_set, input, expected)| {
            for (idx, key) in input.into_iter().enumerate() {
                let result = black_box(prefix_set.contains(key));
                assert_eq!(result, expected[idx]);
            }
        });
    });
}

fn generate_test_data(size: usize) -> (Vec<Nibbles>, Vec<Nibbles>, Vec<bool>) {
    use prop::collection::vec;

    let config = ProptestConfig { result_cache: basic_result_cache, ..Default::default() };
    let mut runner = TestRunner::new(config);

    let mut preload = vec(vec(any::<u8>(), 32), size).new_tree(&mut runner).unwrap().current();
    preload.dedup();
    preload.sort();
    let preload = preload.into_iter().map(|hash| Nibbles::from(&hash[..])).collect::<Vec<_>>();

    let mut input = vec(vec(any::<u8>(), 0..=32), size).new_tree(&mut runner).unwrap().current();
    input.dedup();
    input.sort();
    let input = input.into_iter().map(|bytes| Nibbles::from(&bytes[..])).collect::<Vec<_>>();

    let expected = input
        .iter()
        .map(|prefix| preload.iter().any(|key| key.has_prefix(prefix)))
        .collect::<Vec<_>>();
    (preload, input, expected)
}

criterion_group!(prefix_set, prefix_set_lookups);
criterion_main!(prefix_set);

mod implementations {
    use super::*;
    use std::ops::Bound;

    #[derive(Default)]
    pub struct BTreeAnyPrefixSet {
        keys: BTreeSet<Nibbles>,
    }

    impl PrefixSetAbstraction for BTreeAnyPrefixSet {
        fn contains(&mut self, key: Nibbles) -> bool {
            self.keys.iter().any(|k| k.has_prefix(&key))
        }

        fn insert(&mut self, key: Nibbles) {
            self.keys.insert(key);
        }
    }

    #[derive(Default)]
    pub struct BTreeRangeLastCheckedPrefixSet {
        keys: BTreeSet<Nibbles>,
        last_checked: Option<Nibbles>,
    }

    impl PrefixSetAbstraction for BTreeRangeLastCheckedPrefixSet {
        fn contains(&mut self, prefix: Nibbles) -> bool {
            let range = match self.last_checked.as_ref() {
                // presumably never hit
                Some(last) if &prefix < last => (Bound::Unbounded, Bound::Excluded(last)),
                Some(last) => (Bound::Included(last), Bound::Unbounded),
                None => (Bound::Unbounded, Bound::Unbounded),
            };
            for key in self.keys.range(range) {
                if key.has_prefix(&prefix) {
                    self.last_checked = Some(prefix);
                    return true
                }

                if key > &prefix {
                    self.last_checked = Some(prefix);
                    return false
                }
            }

            false
        }

        fn insert(&mut self, key: Nibbles) {
            self.keys.insert(key);
        }
    }

    #[derive(Default)]
    pub struct VecBinarySearchPrefixSet {
        keys: Vec<Nibbles>,
        sorted: bool,
    }

    impl PrefixSetAbstraction for VecBinarySearchPrefixSet {
        fn contains(&mut self, prefix: Nibbles) -> bool {
            if !self.sorted {
                self.keys.sort();
                self.sorted = true;
            }

            match self.keys.binary_search(&prefix) {
                Ok(_) => true,
                Err(idx) => match self.keys.get(idx) {
                    Some(key) => key.has_prefix(&prefix),
                    None => false, // prefix > last key
                },
            }
        }

        fn insert(&mut self, key: Nibbles) {
            self.sorted = false;
            self.keys.push(key);
        }
    }

    #[derive(Default)]
    pub struct VecCursorPrefixSet {
        keys: Vec<Nibbles>,
        sorted: bool,
        index: usize,
    }

    impl PrefixSetAbstraction for VecCursorPrefixSet {
        fn contains(&mut self, prefix: Nibbles) -> bool {
            if !self.sorted {
                self.keys.sort();
                self.sorted = true;
            }

            let prefix = prefix;

            while self.index > 0 && self.keys[self.index] > prefix {
                self.index -= 1;
            }

            for (idx, key) in self.keys[self.index..].iter().enumerate() {
                if key.has_prefix(&prefix) {
                    self.index += idx;
                    return true
                }

                if key > &prefix {
                    self.index += idx;
                    return false
                }
            }

            false
        }

        fn insert(&mut self, nibbles: Nibbles) {
            self.sorted = false;
            self.keys.push(nibbles);
        }
    }

    #[derive(Default)]
    pub struct VecBinarySearchWithLastFoundPrefixSet {
        keys: Vec<Nibbles>,
        last_found_idx: usize,
        sorted: bool,
    }

    impl PrefixSetAbstraction for VecBinarySearchWithLastFoundPrefixSet {
        fn contains(&mut self, prefix: Nibbles) -> bool {
            if !self.sorted {
                self.keys.sort();
                self.sorted = true;
            }

            while self.last_found_idx > 0 && self.keys[self.last_found_idx] > prefix {
                self.last_found_idx -= 1;
            }

            match self.keys[self.last_found_idx..].binary_search(&prefix) {
                Ok(_) => true,
                Err(idx) => match self.keys.get(idx) {
                    Some(key) => {
                        self.last_found_idx = idx;
                        key.has_prefix(&prefix)
                    }
                    None => false, // prefix > last key
                },
            }
        }

        fn insert(&mut self, key: Nibbles) {
            self.sorted = false;
            self.keys.push(key);
        }
    }
}
