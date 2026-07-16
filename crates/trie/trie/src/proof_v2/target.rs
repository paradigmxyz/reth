use reth_trie_common::{Nibbles, ProofV2Target};

// A helper function for getting the largest prefix of the sub-trie which contains a particular
// target, based on its `min_len`.
//
// A target will only match nodes which share the target's prefix, where the target's prefix is
// the first `min_len` nibbles of its key. E.g. a target with `key` 0xabcd and `min_len` 2 will
// only match nodes with prefix 0xab.
//
// In general the target will only match within the sub-trie whose prefix is identical to the
// target's. However there is an exception:
//
// Given a trie with a node at 0xabc, there must be a branch at 0xab. A target with prefix 0xabc
// needs to match that node, but the branch at 0xab must be constructed order to know the node
// is at that path. Therefore the sub-trie prefix is the target prefix with a nibble truncated.
//
// For a target with an empty prefix (`min_len` of 0) we still use an empty sub-trie prefix;
// this will still construct the branch at the root node (if there is one). Targets with
// `min_len` of both 0 and 1 will therefore construct the root node, but only those with
// `min_len` of 0 will retain it.
#[inline]
pub(crate) fn sub_trie_prefix(target: &ProofV2Target) -> Nibbles {
    let mut sub_trie_prefix = target.key_nibbles;
    sub_trie_prefix.truncate(target.min_len.saturating_sub(1) as usize);
    sub_trie_prefix
}

// A helper function which returns the first path following a sub-trie in lexicographical order.
#[inline]
fn sub_trie_upper_bound(sub_trie_prefix: &Nibbles) -> Option<Nibbles> {
    sub_trie_prefix.next_without_prefix()
}

/// Describes a set of targets which all apply to a single sub-trie, ie a section of the overall
/// trie whose nodes all share a prefix.
pub(crate) struct SubTrieTargets<'a> {
    /// The prefix which all nodes in the sub-trie share. This is also the first node in the trie
    /// in lexicographic order.
    pub(crate) prefix: Nibbles,
    /// The targets belonging to this sub-trie. These will be sorted by their `key` field,
    /// lexicographically.
    pub(crate) targets: &'a [ProofV2Target],
    /// Will be true if at least one target in the set has a zero `min_len`.
    ///
    /// If this is true then `prefix.is_empty()`, though not necessarily vice-versa.
    pub(crate) retain_root: bool,
}

impl<'a> SubTrieTargets<'a> {
    // A helper function which returns the first path following a sub-trie in lexicographical order.
    #[inline]
    pub(crate) fn upper_bound(&self) -> Option<Nibbles> {
        sub_trie_upper_bound(&self.prefix)
    }
}

/// Given a set of [`ProofV2Target`]s, returns an iterator over those same [`ProofV2Target`]s
/// chunked by the sub-tries they apply to within the overall trie.
pub(crate) fn iter_sub_trie_targets(
    targets: &mut [ProofV2Target],
) -> impl Iterator<Item = SubTrieTargets<'_>> {
    // Sort globally by sub-trie prefix, then by target key. This makes equal-prefix targets
    // contiguous and already ordered for the `ProofCalculator`.
    targets.sort_unstable_by(|a, b| {
        sub_trie_prefix(a).cmp(&sub_trie_prefix(b)).then_with(|| a.key_nibbles.cmp(&b.key_nibbles))
    });

    // Chunk targets by exact sub-trie prefix. Prefixes nested below a previous prefix are still
    // processed as their own sub-trie.
    let target_chunks =
        targets.chunk_by_mut(|current, next| sub_trie_prefix(current) == sub_trie_prefix(next));

    // Map the chunks to the return type.
    target_chunks.map(move |targets| {
        let prefix = sub_trie_prefix(&targets[0]);
        // Targets with `min_len` 0 and 1 share the empty prefix, so key ordering cannot indicate
        // whether the root should be retained.
        let retain_root = targets.iter().any(|target| target.min_len == 0);
        SubTrieTargets { prefix, targets, retain_root }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn test_iter_sub_trie_targets() {
        // Helper to create nibbles from hex string (each character is a nibble)
        let nibbles = |hex: &str| -> Nibbles {
            if hex.is_empty() {
                return Nibbles::new();
            }
            format!("0x{}", hex).parse().expect("valid nibbles hex string")
        };

        // Test cases: (input_targets, expected_output)
        // Expected output format: Vec<(exp_prefix_hex, Vec<key_hex>)>
        let test_cases = vec![
            // Case 1: Empty targets
            (vec![], vec![]),
            // Case 2: Single target without min_len
            (
                vec![ProofV2Target::new(B256::repeat_byte(0x20))],
                vec![(
                    "",
                    vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                )],
            ),
            // Case 3: Multiple targets in same sub-trie (no min_len)
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)),
                    ProofV2Target::new(B256::repeat_byte(0x21)),
                ],
                vec![(
                    "",
                    vec![
                        "2020202020202020202020202020202020202020202020202020202020202020",
                        "2121212121212121212121212121212121212121212121212121212121212121",
                    ],
                )],
            ),
            // Case 4: Multiple targets in different sub-tries
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)).with_min_len(2),
                    ProofV2Target::new(B256::repeat_byte(0x40)).with_min_len(2),
                ],
                vec![
                    ("2", vec!["2020202020202020202020202020202020202020202020202020202020202020"]),
                    ("4", vec!["4040404040404040404040404040404040404040404040404040404040404040"]),
                ],
            ),
            // Case 5: Three targets, two in same sub-trie, one separate
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)).with_min_len(2),
                    ProofV2Target::new(B256::repeat_byte(0x2f)).with_min_len(2),
                    ProofV2Target::new(B256::repeat_byte(0x40)).with_min_len(2),
                ],
                vec![
                    (
                        "2",
                        vec![
                            "2020202020202020202020202020202020202020202020202020202020202020",
                            "2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f",
                        ],
                    ),
                    ("4", vec!["4040404040404040404040404040404040404040404040404040404040404040"]),
                ],
            ),
            // Case 6: Targets with different min_len values in different sub-tries
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)).with_min_len(2),
                    ProofV2Target::new(B256::repeat_byte(0x2f)).with_min_len(3),
                ],
                vec![
                    ("2", vec!["2020202020202020202020202020202020202020202020202020202020202020"]),
                    (
                        "2f",
                        vec!["2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f"],
                    ),
                ],
            ),
            // Case 7: More complex chunking with nested sub-trie prefixes
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)).with_min_len(2),
                    ProofV2Target::new(B256::repeat_byte(0x2f)).with_min_len(4),
                    ProofV2Target::new(B256::repeat_byte(0x4c)).with_min_len(3),
                    ProofV2Target::new(B256::repeat_byte(0x4c)).with_min_len(4),
                    ProofV2Target::new(B256::repeat_byte(0x4e)).with_min_len(3),
                ],
                vec![
                    ("2", vec!["2020202020202020202020202020202020202020202020202020202020202020"]),
                    (
                        "2f2",
                        vec!["2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f"],
                    ),
                    (
                        "4c",
                        vec!["4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c"],
                    ),
                    (
                        "4c4",
                        vec!["4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c"],
                    ),
                    (
                        "4e",
                        vec!["4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e"],
                    ),
                ],
            ),
            // Case 8: Min-len 1 should result in zero-length sub-trie prefix
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)).with_min_len(1),
                    ProofV2Target::new(B256::repeat_byte(0x40)).with_min_len(1),
                ],
                vec![(
                    "",
                    vec![
                        "2020202020202020202020202020202020202020202020202020202020202020",
                        "4040404040404040404040404040404040404040404040404040404040404040",
                    ],
                )],
            ),
            // Case 9: Second target's sub-trie prefix is root
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)).with_min_len(2),
                    ProofV2Target::new(B256::repeat_byte(0x40)).with_min_len(1),
                ],
                vec![
                    ("", vec!["4040404040404040404040404040404040404040404040404040404040404040"]),
                    ("2", vec!["2020202020202020202020202020202020202020202020202020202020202020"]),
                ],
            ),
        ];

        for (i, (mut input_targets, expected)) in test_cases.into_iter().enumerate() {
            let test_case = i + 1;
            let sub_tries: Vec<_> = iter_sub_trie_targets(&mut input_targets).collect();

            assert_eq!(
                sub_tries.len(),
                expected.len(),
                "Test case {} failed: expected {} sub-tries, got {}",
                test_case,
                expected.len(),
                sub_tries.len()
            );

            for (j, (sub_trie, (exp_prefix_hex, exp_keys))) in
                sub_tries.iter().zip(expected.iter()).enumerate()
            {
                let exp_prefix = nibbles(exp_prefix_hex);

                assert_eq!(
                    sub_trie.prefix, exp_prefix,
                    "Test case {} sub-trie {}: prefix mismatch",
                    test_case, j
                );
                assert_eq!(
                    sub_trie.targets.len(),
                    exp_keys.len(),
                    "Test case {} sub-trie {}: expected {} targets, got {}",
                    test_case,
                    j,
                    exp_keys.len(),
                    sub_trie.targets.len()
                );

                for (k, (target, exp_key_hex)) in
                    sub_trie.targets.iter().zip(exp_keys.iter()).enumerate()
                {
                    let exp_key = nibbles(exp_key_hex);
                    assert_eq!(
                        target.key_nibbles, exp_key,
                        "Test case {} sub-trie {} target {}: key mismatch",
                        test_case, j, k
                    );
                }
            }
        }
    }

    #[test]
    fn test_iter_sub_trie_targets_retain_root_after_key_sort() {
        let mut targets = [
            ProofV2Target::new(B256::repeat_byte(0x40)),
            ProofV2Target::new(B256::repeat_byte(0x20)).with_min_len(1),
        ];

        let sub_tries = iter_sub_trie_targets(&mut targets).collect::<Vec<_>>();

        assert_eq!(sub_tries.len(), 1);
        assert_eq!(sub_tries[0].targets[0].key(), B256::repeat_byte(0x20));
        assert!(sub_tries[0].retain_root);
    }
}
