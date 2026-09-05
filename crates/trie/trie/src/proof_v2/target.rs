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

/// Targets within one child of an already-revealed parent, or within the full trie.
pub(crate) struct SubTrieTargets<'a> {
    /// The root path of the requested subtree.
    pub(crate) lower_bound: Nibbles,
    /// The first path after the traversal range, or `None` if it extends through the end of the
    /// trie.
    pub(crate) upper_bound: Option<Nibbles>,
    /// The targets belonging to this sub-trie. These will be sorted by their `key` field,
    /// lexicographically.
    pub(crate) targets: &'a [ProofV2Target],
    /// Every target in this range can be read without the stored branch table.
    pub(crate) leaves_only: bool,
}

/// Finds the deepest target prefix shared by each path in an ordered input stream.
/// Only the targets on either side of a path can give its longest shared prefix.
pub(super) struct TargetDepths<'a> {
    remaining: &'a [ProofV2Target],
    previous: Option<&'a Nibbles>,
}

impl<'a> TargetDepths<'a> {
    pub(super) const fn new(targets: &'a [ProofV2Target]) -> Self {
        Self { remaining: targets, previous: None }
    }

    #[inline]
    pub(super) fn next(&mut self, path: &Nibbles) -> usize {
        let mut next_depth = 0;
        while let Some((target, rest)) = self.remaining.split_first() {
            let key = &target.key_nibbles;
            let common = key.common_prefix_length(path);
            if key.get(common) >= path.get(common) {
                next_depth = common;
                break
            }
            self.previous = Some(key);
            self.remaining = rest;
        }
        self.previous.map_or(next_depth, |key| next_depth.max(key.common_prefix_length(path)))
    }
}

/// Given a set of [`ProofV2Target`]s, returns an iterator over those same [`ProofV2Target`]s
/// grouped by the child of their already-revealed parent that needs a proof.
/// Targets without a known parent share one group covering the full trie.
pub(crate) fn iter_sub_trie_targets(
    targets: &mut [ProofV2Target],
) -> impl Iterator<Item = SubTrieTargets<'_>> {
    // The known parent and its untargeted children need no proof. Requests for each child stay
    // together so they share one traversal.
    targets.sort_unstable_by(|a, b| {
        known_parent_prefix(a)
            .cmp(&known_parent_prefix(b))
            .then_with(|| a.key_nibbles.cmp(&b.key_nibbles))
    });

    targets
        .chunk_by_mut(|current, next| target_child_prefix(current) == target_child_prefix(next))
        .map(|targets| {
            let lower_bound = target_child_prefix(&targets[0]);
            let upper_bound = lower_bound.next_without_prefix();
            let leaves_only = targets.iter().all(|target| target.leaves_only);
            SubTrieTargets { lower_bound, upper_bound, targets, leaves_only }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use proptest::prelude::*;
    use reth_trie_common::ProofV2TargetParent;

    proptest! {
        #[test]
        fn target_depths_match_linear_search(
            entries in prop::collection::vec((any::<[u8; 32]>(), 0usize..65), 0..40),
            query in any::<[u8; 32]>(),
        ) {
            let mut targets = entries.into_iter().map(|(key, parent)| {
                ProofV2Target::new(B256::from(key)).with_parent(
                    parent.checked_sub(1).map_or(ProofV2TargetParent::NONE, ProofV2TargetParent::new),
                )
            }).collect::<Vec<_>>();
            let query = Nibbles::unpack(query);
            for group in iter_sub_trie_targets(&mut targets) {
                let paths = group.targets.iter().map(|target| target.key_nibbles)
                    .chain(core::iter::once(query))
                    .flat_map(|key| (0..=key.len()).rev().map(move |len| key.slice(..len)))
                    .collect::<Vec<_>>();
                let mut paths = paths;
                paths.sort_unstable();
                let mut depths = TargetDepths::new(group.targets);
                for path in paths {
                    let expected = group.targets.iter()
                        .map(|target| target.key_nibbles.common_prefix_length(&path))
                        .max().unwrap_or(0);
                    prop_assert_eq!(depths.next(&path), expected);
                }
            }
        }
    }

    #[test]
    fn empty_target_depths() {
        let mut depths = TargetDepths::new(&[]);
        assert_eq!(depths.next(&Nibbles::new()), 0);
        assert_eq!(depths.next(&Nibbles::unpack(B256::repeat_byte(0xff))), 0);
    }

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
            // Children 0 and f of parent 2 have separate ranges, with a gap between them.
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
                        Some("21"),
                        vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                    ),
                    (
                        Some("2"),
                        "2f",
                        Some("3"),
                        vec!["2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f"],
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
            // Child f of a known root has an unbounded end.
            (
                vec![
                    ProofV2Target::new(B256::repeat_byte(0x20))
                        .with_parent(ProofV2TargetParent::new(0)),
                    ProofV2Target::new(B256::repeat_byte(0xf0))
                        .with_parent(ProofV2TargetParent::new(0)),
                ],
                vec![
                    (
                        Some(""),
                        "2",
                        Some("3"),
                        vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                    ),
                    (
                        Some(""),
                        "f",
                        None,
                        vec!["f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0"],
                    ),
                ],
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
                        known_parent_prefix(&sub_trie.targets[0]),
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
