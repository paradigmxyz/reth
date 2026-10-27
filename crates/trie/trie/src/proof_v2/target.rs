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
            // Empty targets.
            (vec![], vec![]),
            // A root traversal stays unbounded and sorts its targets.
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x21)),
                    ProofV2Target::new(B256::repeat_byte(0x20)),
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
            // Targets below children 0 and f of parent 2 span [20, 3).
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
            // Nested parent paths remain separate groups.
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
            // A known-root span ending in child f is unbounded.
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20))
                        .with_parent(ProofV2TargetParent::new(0)),
                    ProofV2Target::new(B256::repeat_byte(0xf0))
                        .with_parent(ProofV2TargetParent::new(0)),
                ],
                vec![(
                    Some(""),
                    "2",
                    None,
                    vec![
                        "2020202020202020202020202020202020202020202020202020202020202020",
                        "f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0",
                    ],
                )],
            ),
            // Parent ordering can make traversal ranges move backwards (4 then 20).
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
            // Root and root-parent targets are distinct despite sharing a prefix.
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
            let actual = iter_sub_trie_targets(&mut input_targets)
                .map(|sub_trie| {
                    (
                        sub_trie.parent_prefix,
                        sub_trie.lower_bound,
                        sub_trie.upper_bound,
                        sub_trie
                            .targets
                            .iter()
                            .map(|target| target.key_nibbles)
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            let expected = expected
                .into_iter()
                .map(|(parent_prefix, lower_bound, upper_bound, keys)| {
                    (
                        parent_prefix.map(nibbles),
                        nibbles(lower_bound),
                        upper_bound.map(nibbles),
                        keys.into_iter().map(nibbles).collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();

            assert_eq!(actual, expected, "test case {}", i + 1);
        }
    }
}
