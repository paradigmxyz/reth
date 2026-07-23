use reth_trie_common::{Nibbles, ProofV2Target};

// Returns the path of the already-revealed parent branch for a target. `None` means the target
// needs the actual trie root, while `Some(Nibbles::new())` means the root branch is already
// revealed.
#[inline]
pub(crate) fn known_parent_prefix(target: &ProofV2Target) -> Option<Nibbles> {
    target.parent.path(target.key_nibbles)
}

// Returns the direct child of the known parent which contains the target. If there is no known
// parent then the target requires the full trie.
#[inline]
fn target_child_prefix(target: &ProofV2Target) -> Nibbles {
    target
        .parent
        .path_len()
        .map_or_else(Nibbles::new, |parent_len| target.key_nibbles.slice(0..parent_len + 1))
}

/// Describes targets with the same already-revealed parent and the bounded range traversed to
/// calculate their direct children.
pub(crate) struct SubTrieTargets<'a> {
    /// The first path traversed for these targets.
    pub(crate) lower_bound: Nibbles,
    /// The first path after the traversal range, or `None` if it extends through the end of the
    /// trie.
    pub(crate) upper_bound: Option<Nibbles>,
    /// The path of the already-revealed parent branch. `None` means the actual trie root is
    /// requested, while `Some(Nibbles::new())` means the root branch is already revealed.
    pub(crate) parent_prefix: Option<Nibbles>,
    /// The targets belonging to this sub-trie. These will be sorted by their `key` field,
    /// lexicographically.
    pub(crate) targets: &'a [ProofV2Target],
}

/// Given a set of [`ProofV2Target`]s, returns an iterator over those same [`ProofV2Target`]s
/// grouped by their already-revealed parent. Each group traverses the bounded span from its first
/// targeted direct child through its last targeted direct child.
pub(crate) fn iter_sub_trie_targets(
    targets: &mut [ProofV2Target],
) -> impl Iterator<Item = SubTrieTargets<'_>> {
    // Sort by parent context first so equal parents are contiguous, then by target key so the
    // first and last targets determine the traversal bounds for the group. `None` and a known root
    // parent remain distinct.
    targets.sort_unstable_by(|a, b| {
        known_parent_prefix(a)
            .cmp(&known_parent_prefix(b))
            .then_with(|| a.key_nibbles.cmp(&b.key_nibbles))
    });

    targets
        .chunk_by_mut(|current, next| known_parent_prefix(current) == known_parent_prefix(next))
        .map(|targets| {
            let parent_prefix = known_parent_prefix(&targets[0]);
            let lower_bound = target_child_prefix(&targets[0]);
            let upper_bound = target_child_prefix(targets.last().expect("chunk is non-empty"))
                .next_without_prefix();
            SubTrieTargets { lower_bound, upper_bound, parent_prefix, targets }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_trie_common::ProofV2TargetParent;

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
        // Expected output format:
        // Vec<(known_parent_prefix_hex, lower_bound_hex, upper_bound_hex, Vec<key_hex>)>
        let test_cases = vec![
            // Case 1: Empty targets
            (vec![], vec![]),
            // Case 2: Single target without a known parent
            (
                vec![ProofV2Target::new(B256::repeat_byte(0x20))],
                vec![(
                    None,
                    "",
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
                    "",
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
                    ProofV2Target::new(B256::repeat_byte(0x20))
                        .with_parent(ProofV2TargetParent::new(1)),
                    ProofV2Target::new(B256::repeat_byte(0x40))
                        .with_parent(ProofV2TargetParent::new(1)),
                ],
                vec![
                    (
                        Some("2"),
                        "20",
                        Some("21"),
                        vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                    ),
                    (
                        Some("4"),
                        "40",
                        Some("41"),
                        vec!["4040404040404040404040404040404040404040404040404040404040404040"],
                    ),
                ],
            ),
            // Case 5: Targets below children 0 and f of parent 2 span [20, 3)
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20))
                        .with_parent(ProofV2TargetParent::new(1)),
                    ProofV2Target::new(B256::repeat_byte(0x2f))
                        .with_parent(ProofV2TargetParent::new(1)),
                    ProofV2Target::new(B256::repeat_byte(0x40))
                        .with_parent(ProofV2TargetParent::new(1)),
                ],
                vec![
                    (
                        Some("2"),
                        "20",
                        Some("3"),
                        vec![
                            "2020202020202020202020202020202020202020202020202020202020202020",
                            "2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f",
                        ],
                    ),
                    (
                        Some("4"),
                        "40",
                        Some("41"),
                        vec!["4040404040404040404040404040404040404040404040404040404040404040"],
                    ),
                ],
            ),
            // Case 6: Targets with different parent paths in different sub-tries
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20))
                        .with_parent(ProofV2TargetParent::new(1)),
                    ProofV2Target::new(B256::repeat_byte(0x2f))
                        .with_parent(ProofV2TargetParent::new(2)),
                ],
                vec![
                    (
                        Some("2"),
                        "20",
                        Some("21"),
                        vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                    ),
                    (
                        Some("2f"),
                        "2f2",
                        Some("2f3"),
                        vec!["2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f"],
                    ),
                ],
            ),
            // Case 7: More complex chunking with nested sub-trie prefixes
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20))
                        .with_parent(ProofV2TargetParent::new(1)),
                    ProofV2Target::new(B256::repeat_byte(0x2f))
                        .with_parent(ProofV2TargetParent::new(3)),
                    ProofV2Target::new(B256::repeat_byte(0x4c))
                        .with_parent(ProofV2TargetParent::new(2)),
                    ProofV2Target::new(B256::repeat_byte(0x4c))
                        .with_parent(ProofV2TargetParent::new(3)),
                    ProofV2Target::new(B256::repeat_byte(0x4e))
                        .with_parent(ProofV2TargetParent::new(2)),
                ],
                vec![
                    (
                        Some("2"),
                        "20",
                        Some("21"),
                        vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                    ),
                    (
                        Some("2f2"),
                        "2f2f",
                        Some("2f3"),
                        vec!["2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f"],
                    ),
                    (
                        Some("4c"),
                        "4c4",
                        Some("4c5"),
                        vec!["4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c"],
                    ),
                    (
                        Some("4c4"),
                        "4c4c",
                        Some("4c4d"),
                        vec!["4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c"],
                    ),
                    (
                        Some("4e"),
                        "4e4",
                        Some("4e5"),
                        vec!["4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e"],
                    ),
                ],
            ),
            // Case 8: Known-root children 2 and 4 span [2, 5)
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20))
                        .with_parent(ProofV2TargetParent::new(0)),
                    ProofV2Target::new(B256::repeat_byte(0x40))
                        .with_parent(ProofV2TargetParent::new(0)),
                ],
                vec![(
                    Some(""),
                    "2",
                    Some("5"),
                    vec![
                        "2020202020202020202020202020202020202020202020202020202020202020",
                        "4040404040404040404040404040404040404040404040404040404040404040",
                    ],
                )],
            ),
            // Case 9: Parent ordering can make traversal ranges move backwards (4 then 20)
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20))
                        .with_parent(ProofV2TargetParent::new(1)),
                    ProofV2Target::new(B256::repeat_byte(0x40))
                        .with_parent(ProofV2TargetParent::new(0)),
                ],
                vec![
                    (
                        Some(""),
                        "4",
                        Some("5"),
                        vec!["4040404040404040404040404040404040404040404040404040404040404040"],
                    ),
                    (
                        Some("2"),
                        "20",
                        Some("21"),
                        vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                    ),
                ],
            ),
            // Case 10: Root and root-parent targets are distinct despite sharing a prefix
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20)),
                    ProofV2Target::new(B256::repeat_byte(0x40))
                        .with_parent(ProofV2TargetParent::new(0)),
                ],
                vec![
                    (
                        None,
                        "",
                        None,
                        vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                    ),
                    (
                        Some(""),
                        "4",
                        Some("5"),
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

            for (
                j,
                (
                    sub_trie,
                    (exp_parent_prefix_hex, exp_lower_bound_hex, exp_upper_bound_hex, exp_keys),
                ),
            ) in sub_tries.iter().zip(expected.iter()).enumerate()
            {
                let exp_parent_prefix = exp_parent_prefix_hex.map(nibbles);
                let exp_lower_bound = nibbles(exp_lower_bound_hex);
                let exp_upper_bound = exp_upper_bound_hex.map(nibbles);

                assert_eq!(
                    sub_trie.parent_prefix, exp_parent_prefix,
                    "Test case {} sub-trie {}: parent prefix mismatch",
                    test_case, j
                );
                assert_eq!(
                    sub_trie.lower_bound, exp_lower_bound,
                    "Test case {} sub-trie {}: lower bound mismatch",
                    test_case, j
                );
                assert_eq!(
                    sub_trie.upper_bound, exp_upper_bound,
                    "Test case {} sub-trie {}: upper bound mismatch",
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
    fn test_iter_sub_trie_targets_key_order_after_sort() {
        let mut targets = [
            ProofV2Target::new(B256::repeat_byte(0x40)),
            ProofV2Target::new(B256::repeat_byte(0x20)),
        ];

        let sub_tries = iter_sub_trie_targets(&mut targets).collect::<Vec<_>>();

        assert_eq!(sub_tries.len(), 1);
        assert_eq!(sub_tries[0].targets[0].key(), B256::repeat_byte(0x20));
        assert_eq!(sub_tries[0].parent_prefix, None);
    }

    #[test]
    fn test_iter_sub_trie_targets_groups_same_parent_child() {
        let key_a = B256::right_padding_from(&[0x20, 0x10]);
        let key_b = B256::right_padding_from(&[0x20, 0x00]);
        let mut targets = [key_a, key_b]
            .map(|key| ProofV2Target::new(key).with_parent(ProofV2TargetParent::new(1)));

        let sub_tries = iter_sub_trie_targets(&mut targets).collect::<Vec<_>>();

        assert_eq!(sub_tries.len(), 1);
        assert_eq!(sub_tries[0].parent_prefix, Some(Nibbles::from_nibbles([0x2])));
        assert_eq!(
            (sub_tries[0].lower_bound, sub_tries[0].upper_bound),
            (Nibbles::from_nibbles([0x2, 0x0]), Some(Nibbles::from_nibbles([0x2, 0x1])),)
        );
        assert_eq!(
            sub_tries[0].targets.iter().map(ProofV2Target::key).collect::<Vec<_>>(),
            [key_b, key_a]
        );
    }

    #[test]
    fn test_child_f_upper_bounds() {
        let root_child_f = B256::repeat_byte(0xf0);
        let non_root_child_f = B256::repeat_byte(0x2f);
        let mut targets = [
            ProofV2Target::new(non_root_child_f).with_parent(ProofV2TargetParent::new(1)),
            ProofV2Target::new(root_child_f).with_parent(ProofV2TargetParent::new(0)),
        ];

        let sub_tries = iter_sub_trie_targets(&mut targets).collect::<Vec<_>>();

        assert_eq!(sub_tries.len(), 2);
        assert_eq!(sub_tries[0].parent_prefix, Some(Nibbles::new()));
        assert_eq!(
            (sub_tries[0].lower_bound, sub_tries[0].upper_bound),
            (Nibbles::from_nibbles([0xf]), None)
        );
        assert_eq!(sub_tries[1].parent_prefix, Some(Nibbles::from_nibbles([0x2])));
        assert_eq!(
            (sub_tries[1].lower_bound, sub_tries[1].upper_bound),
            (Nibbles::from_nibbles([0x2, 0xf]), Some(Nibbles::from_nibbles([0x3])),)
        );
    }
}
