use crate::proof_v2::increment_and_strip_trailing_zeros;
use alloy_primitives::B256;
use reth_trie_common::Nibbles;

/// Target describes a proof target. For every proof target given, the
/// [`crate::proof_v2::ProofCalculator`] will calculate and return all nodes whose path is a prefix
/// of the target's `key`.
#[derive(Debug, Copy, Clone)]
pub struct Target {
    pub(crate) key: Nibbles,
    /// The shortest trie node path which can be retained by this target. `key.starts_with(prefix)`
    /// is always true.
    pub(crate) prefix: Nibbles,
}

impl Target {
    /// Returns a new [`Target`] which matches all trie nodes whose path is a prefix of this key.
    pub fn new(key: B256) -> Self {
        // SAFETY: key is a B256 and so is exactly 32-bytes.
        let key = unsafe { Nibbles::unpack_unchecked(key.as_slice()) };
        Self { key, prefix: Nibbles::new() }
    }

    /// Only match trie nodes whose path is at least this long.
    ///
    /// # Panics
    ///
    /// This method panics if `min_len` is greater than 64.
    pub fn with_min_len(mut self, min_len: u8) -> Self {
        debug_assert!(min_len <= 64);
        self.prefix = self.key;
        self.prefix.truncate(min_len as usize);
        self
    }
}

impl From<B256> for Target {
    fn from(key: B256) -> Self {
        Self::new(key)
    }
}

// A helper function for getting the largest prefix of the sub-trie which contains a particular
// target, based on its prefix.
//
// In general the target will only match within the sub-trie with the same prefix as the
// target's. However there is an exception:
//
// Given a trie with a node at 0xabc, there must be a branch at 0xab. A target with prefix 0xabc
// needs to match that node, but in order to know the node is at that path the branch at 0xab
// must be constructed. Therefore the sub-trie prefix is the target prefix with a nibble
// truncated.
//
// For a target with an empty prefix we still use an empty sub-trie prefix; this will still
// construct the branch at the root node (if there is one), the only behavioral difference
// between targets with prefix lengths zero and one will be that with prefix length zero the
// root node's proof will be retained.
#[inline]
fn sub_trie_prefix(target_prefix: Nibbles) -> Nibbles {
    let mut sub_trie_prefix = target_prefix;
    sub_trie_prefix.truncate(target_prefix.len().saturating_sub(1));
    sub_trie_prefix
}

// A helper function which returns the first path following a sub-trie in lexicographical order.
#[inline]
fn sub_trie_upper_bound(sub_trie_prefix: &Nibbles) -> Option<Nibbles> {
    increment_and_strip_trailing_zeros(sub_trie_prefix)
}

/// Describes a set of targets which all apply to a single sub-trie, ie a section of the overall
/// trie whose nodes all share a prefix.
pub(crate) struct SubTrieTargets<'a> {
    /// The prefix which all nodes in the sub-trie share. This is also the first node in the trie
    /// in lexicographic order.
    pub(crate) prefix: Nibbles,
    /// The targets belonging to this sub-trie. These will be sorted by their `key` field,
    /// lexicographically.
    pub(crate) targets: &'a [Target],
    /// Will be true if at least one target in the set has an empty `prefix`.
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

/// Given a set of [`Target`]s, returns an iterator over those same [`Target`]s chunked by the
/// sub-tries they apply to within the overall trie.
pub(crate) fn iter_sub_trie_targets<'a>(
    targets: &'a mut [Target],
) -> impl Iterator<Item = SubTrieTargets<'a>> {
    // First sort by the sub-trie prefix of each target, falling back to the actual prefix in cases
    // where the sub-trie prefixes are equal (to differentiate an empty target prefix from an empty
    // sub-trie prefix).
    targets.sort_unstable_by(|a, b| {
        let sub_trie_prefix_a = sub_trie_prefix(a.prefix);
        let sub_trie_prefix_b = sub_trie_prefix(b.prefix);
        sub_trie_prefix_a.cmp(&sub_trie_prefix_b).then_with(|| a.prefix.cmp(&b.prefix))
    });

    // We now chunk targets, such that each chunk contains all targets belonging to the same
    // sub-trie. We are taking advantage of the following properties:
    //
    // - The first target in the chunk has the shortest prefix (see previous sorting step).
    //
    // - The first target in the chunk's upper bound will therefore belong to the sub-trie with the
    //   highest upper bound, and the upper bound of the whole chunk.
    //      - For example, given a chunk with sub-trie prefixes [0x2, 0x2f, 0x2fa], the upper bounds
    //        will be [0x3, 0x3, 0x2fb]. Note that no target could match a trie node with path equal
    //        to or greater than 0x3.
    //
    // - If a target's sub-trie's prefix does not lie within the bounds of the current chunk, then
    //   that target must be the first target of the next chunk, lying in a separate sub-trie.
    //      - Example: given sub-trie prefixes of [0x2, 0x2fa, 0x4c, 0x4ce, 0x4e], we would end up
    //        with the following chunks:
    //          - [0x2, 0x2a]  w/ upper bound 0x3
    //          - [0x4c 0x4ce] w/ upper bound 0x4d
    //          - [0x4e]       w/ upper bound 0x4f
    let mut upper_bound =
        targets.first().and_then(|t| sub_trie_upper_bound(&sub_trie_prefix(t.prefix)));
    let target_chunks = targets.chunk_by_mut(move |_, next| {
        if let Some(some_upper_bound) = upper_bound {
            let sub_trie_prefix = sub_trie_prefix(next.prefix);
            let same_chunk = sub_trie_prefix < some_upper_bound;
            if !same_chunk {
                upper_bound = sub_trie_upper_bound(&sub_trie_prefix);
            }
            same_chunk
        } else {
            true
        }
    });

    // Map the chunks to the return type. Within each chunk we want targets to be sorted by their
    // key, as that will be the order they are checked by the `ProofCalculator`.
    target_chunks.map(move |targets| {
        let prefix = sub_trie_prefix(targets[0].prefix);
        let retain_root = targets[0].prefix.is_empty();
        targets.sort_unstable_by_key(|target| target.key);
        SubTrieTargets { prefix, targets, retain_root }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Expected output format: Vec<(lower_bound_hex, upper_bound_hex_opt, Vec<key_hex>)>
        let test_cases = vec![
            // Case 1: Empty targets
            (vec![], vec![]),
            // Case 2: Single target without min_len
            (
                vec![Target::new(B256::repeat_byte(0x20))],
                vec![(
                    "",
                    vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                )],
            ),
            // Case 3: Multiple targets in same sub-trie (no min_len)
            (
                vec![Target::new(B256::repeat_byte(0x20)), Target::new(B256::repeat_byte(0x21))],
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
                    Target::new(B256::repeat_byte(0x20)).with_min_len(2),
                    Target::new(B256::repeat_byte(0x40)).with_min_len(2),
                ],
                vec![
                    ("2", vec!["2020202020202020202020202020202020202020202020202020202020202020"]),
                    ("4", vec!["4040404040404040404040404040404040404040404040404040404040404040"]),
                ],
            ),
            // Case 5: Three targets, two in same sub-trie, one separate
            (
                vec![
                    Target::new(B256::repeat_byte(0x20)).with_min_len(2),
                    Target::new(B256::repeat_byte(0x2f)).with_min_len(2),
                    Target::new(B256::repeat_byte(0x40)).with_min_len(2),
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
            // Case 6: Targets with different min_len values in same sub-trie
            (
                vec![
                    Target::new(B256::repeat_byte(0x20)).with_min_len(2),
                    Target::new(B256::repeat_byte(0x2f)).with_min_len(3),
                ],
                vec![(
                    "2",
                    vec![
                        "2020202020202020202020202020202020202020202020202020202020202020",
                        "2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f",
                    ],
                )],
            ),
            // Case 7: More complex chunking with multiple sub-tries
            (
                vec![
                    Target::new(B256::repeat_byte(0x20)).with_min_len(2),
                    Target::new(B256::repeat_byte(0x2f)).with_min_len(4),
                    Target::new(B256::repeat_byte(0x4c)).with_min_len(3),
                    Target::new(B256::repeat_byte(0x4c)).with_min_len(4),
                    Target::new(B256::repeat_byte(0x4e)).with_min_len(3),
                ],
                vec![
                    (
                        "2",
                        vec![
                            "2020202020202020202020202020202020202020202020202020202020202020",
                            "2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f",
                        ],
                    ),
                    (
                        "4c",
                        vec![
                            "4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c",
                            "4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c",
                        ],
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
                    Target::new(B256::repeat_byte(0x20)).with_min_len(1),
                    Target::new(B256::repeat_byte(0x40)).with_min_len(1),
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
                    Target::new(B256::repeat_byte(0x20)).with_min_len(2),
                    Target::new(B256::repeat_byte(0x40)).with_min_len(1),
                ],
                vec![(
                    "",
                    vec![
                        "2020202020202020202020202020202020202020202020202020202020202020",
                        "4040404040404040404040404040404040404040404040404040404040404040",
                    ],
                )],
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
                        target.key, exp_key,
                        "Test case {} sub-trie {} target {}: key mismatch",
                        test_case, j, k
                    );
                }
            }
        }
    }
}
