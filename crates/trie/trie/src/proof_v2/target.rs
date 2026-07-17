use reth_trie_common::{Nibbles, ProofV2Target};

// Returns the path of the already-revealed parent branch for a target. `None` means the target
// needs the actual trie root, while `Some(Nibbles::new())` means the root branch is already
// revealed.
#[inline]
pub(crate) fn known_parent_prefix(target: &ProofV2Target) -> Option<Nibbles> {
    target.parent_path_len.map(|parent_path_len| {
        let mut prefix = target.key_nibbles;
        prefix.truncate(parent_path_len as usize);
        prefix
    })
}

// A helper function which returns the first path following a sub-trie in lexicographical order.
#[inline]
fn sub_trie_upper_bound(sub_trie_prefix: &Nibbles) -> Option<Nibbles> {
    sub_trie_prefix.next_without_prefix()
}

/// Describes a set of targets which all apply to a single sub-trie, ie a section of the overall
/// trie whose nodes all share a prefix.
pub(crate) struct SubTrieTargets<'a> {
    /// The path of the already-revealed parent branch. `None` means the actual trie root is
    /// requested, while `Some(Nibbles::new())` means the root branch is already revealed.
    pub(crate) parent_prefix: Option<Nibbles>,
    /// The targets belonging to this sub-trie. These will be sorted by their `key` field,
    /// lexicographically.
    pub(crate) targets: &'a [ProofV2Target],
}

impl<'a> SubTrieTargets<'a> {
    /// Returns the concrete prefix used to traverse this sub-trie.
    #[inline]
    pub(crate) fn prefix(&self) -> Nibbles {
        self.parent_prefix.unwrap_or_default()
    }

    // A helper function which returns the first path following a sub-trie in lexicographical order.
    #[inline]
    pub(crate) fn upper_bound(&self) -> Option<Nibbles> {
        sub_trie_upper_bound(&self.prefix())
    }
}

/// Given a set of [`ProofV2Target`]s, returns an iterator over those same [`ProofV2Target`]s
/// chunked by the sub-tries they apply to within the overall trie.
pub(crate) fn iter_sub_trie_targets(
    targets: &mut [ProofV2Target],
) -> impl Iterator<Item = SubTrieTargets<'_>> {
    // Sort globally by parent context, then target key. `None` and `Some(Nibbles::new())` remain
    // distinct despite both traversing from the root. This also leaves each resulting chunk
    // ordered for the `ProofCalculator`.
    targets.sort_unstable_by(|a, b| {
        known_parent_prefix(a)
            .cmp(&known_parent_prefix(b))
            .then_with(|| a.key_nibbles.cmp(&b.key_nibbles))
    });

    // Chunk targets by exact known-parent prefix. Prefixes nested below a previous prefix are still
    // processed as their own sub-trie.
    let target_chunks = targets
        .chunk_by_mut(|current, next| known_parent_prefix(current) == known_parent_prefix(next));

    // Map the chunks to the return type.
    target_chunks.map(move |targets| {
        let parent_prefix = known_parent_prefix(&targets[0]);
        SubTrieTargets { parent_prefix, targets }
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
        // Expected output format: Vec<(known_parent_prefix_hex, Vec<key_hex>)>
        let test_cases = vec![
            // Case 1: Empty targets
            (vec![], vec![]),
            // Case 2: Single target without a known parent
            (
                vec![ProofV2Target::new(B256::repeat_byte(0x20))],
                vec![(
                    None,
                    vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                )],
            ),
            // Case 3: Multiple targets in same sub-trie without known parents
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)),
                    ProofV2Target::new(B256::repeat_byte(0x21)),
                ],
                vec![(
                    None,
                    vec![
                        "2020202020202020202020202020202020202020202020202020202020202020",
                        "2121212121212121212121212121212121212121212121212121212121212121",
                    ],
                )],
            ),
            // Case 4: Multiple targets in different sub-tries
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)).with_parent_path_len(Some(1)),
                    ProofV2Target::new(B256::repeat_byte(0x40)).with_parent_path_len(Some(1)),
                ],
                vec![
                    (
                        Some("2"),
                        vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                    ),
                    (
                        Some("4"),
                        vec!["4040404040404040404040404040404040404040404040404040404040404040"],
                    ),
                ],
            ),
            // Case 5: Three targets, two in same sub-trie, one separate
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)).with_parent_path_len(Some(1)),
                    ProofV2Target::new(B256::repeat_byte(0x2f)).with_parent_path_len(Some(1)),
                    ProofV2Target::new(B256::repeat_byte(0x40)).with_parent_path_len(Some(1)),
                ],
                vec![
                    (
                        Some("2"),
                        vec![
                            "2020202020202020202020202020202020202020202020202020202020202020",
                            "2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f",
                        ],
                    ),
                    (
                        Some("4"),
                        vec!["4040404040404040404040404040404040404040404040404040404040404040"],
                    ),
                ],
            ),
            // Case 6: Targets with different parent paths in different sub-tries
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)).with_parent_path_len(Some(1)),
                    ProofV2Target::new(B256::repeat_byte(0x2f)).with_parent_path_len(Some(2)),
                ],
                vec![
                    (
                        Some("2"),
                        vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                    ),
                    (
                        Some("2f"),
                        vec!["2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f"],
                    ),
                ],
            ),
            // Case 7: More complex chunking with nested sub-trie prefixes
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)).with_parent_path_len(Some(1)),
                    ProofV2Target::new(B256::repeat_byte(0x2f)).with_parent_path_len(Some(3)),
                    ProofV2Target::new(B256::repeat_byte(0x4c)).with_parent_path_len(Some(2)),
                    ProofV2Target::new(B256::repeat_byte(0x4c)).with_parent_path_len(Some(3)),
                    ProofV2Target::new(B256::repeat_byte(0x4e)).with_parent_path_len(Some(2)),
                ],
                vec![
                    (
                        Some("2"),
                        vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                    ),
                    (
                        Some("2f2"),
                        vec!["2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f"],
                    ),
                    (
                        Some("4c"),
                        vec!["4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c"],
                    ),
                    (
                        Some("4c4"),
                        vec!["4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c"],
                    ),
                    (
                        Some("4e"),
                        vec!["4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e"],
                    ),
                ],
            ),
            // Case 8: A root parent should result in a zero-length sub-trie prefix
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)).with_parent_path_len(Some(0)),
                    ProofV2Target::new(B256::repeat_byte(0x40)).with_parent_path_len(Some(0)),
                ],
                vec![(
                    Some(""),
                    vec![
                        "2020202020202020202020202020202020202020202020202020202020202020",
                        "4040404040404040404040404040404040404040404040404040404040404040",
                    ],
                )],
            ),
            // Case 9: Second target's sub-trie prefix is root
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)).with_parent_path_len(Some(1)),
                    ProofV2Target::new(B256::repeat_byte(0x40)).with_parent_path_len(Some(0)),
                ],
                vec![
                    (
                        Some(""),
                        vec!["4040404040404040404040404040404040404040404040404040404040404040"],
                    ),
                    (
                        Some("2"),
                        vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                    ),
                ],
            ),
            // Case 10: Root and root-parent targets are distinct despite sharing a prefix
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)),
                    ProofV2Target::new(B256::repeat_byte(0x40)).with_parent_path_len(Some(0)),
                ],
                vec![
                    (
                        None,
                        vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                    ),
                    (
                        Some(""),
                        vec!["4040404040404040404040404040404040404040404040404040404040404040"],
                    ),
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

            for (j, (sub_trie, (exp_parent_prefix_hex, exp_keys))) in
                sub_tries.iter().zip(expected.iter()).enumerate()
            {
                let exp_parent_prefix = exp_parent_prefix_hex.map(nibbles);
                let exp_prefix = exp_parent_prefix.unwrap_or_default();

                assert_eq!(
                    sub_trie.parent_prefix, exp_parent_prefix,
                    "Test case {} sub-trie {}: parent prefix mismatch",
                    test_case, j
                );
                assert_eq!(
                    sub_trie.prefix(),
                    exp_prefix,
                    "Test case {} sub-trie {}: traversal prefix mismatch",
                    test_case,
                    j
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
    fn test_iter_sub_trie_targets_key_order_after_global_sort() {
        let mut targets = [
            ProofV2Target::new(B256::repeat_byte(0x40)),
            ProofV2Target::new(B256::repeat_byte(0x20)),
        ];

        let sub_tries = iter_sub_trie_targets(&mut targets).collect::<Vec<_>>();

        assert_eq!(sub_tries.len(), 1);
        assert_eq!(sub_tries[0].targets[0].key(), B256::repeat_byte(0x20));
        assert_eq!(sub_tries[0].parent_prefix, None);
    }
}
