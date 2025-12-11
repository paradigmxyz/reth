use alloy_primitives::B256;
use reth_trie_common::Nibbles;

use crate::proof_v2::increment_and_strip_trailing_zeros;

/// Target describes a proof target. For every proof target given, the
/// [`crate::proof_v2::ProofCalculator`] will calculate and return all nodes whose path is a prefix
/// of the target's `key`.
#[derive(Debug, Copy, Clone)]
pub struct Target {
    pub(crate) key: Nibbles,
    /// The lower bound of the range of trie nodes which can be retained by this target. In other
    /// words, the shortest trie node path which can be retained by this target.
    pub(crate) lower_bound: Nibbles,
}

impl Target {
    /// Returns a new [`Target`] which matches all trie nodes whose path is a prefix of this key.
    pub fn new(key: B256) -> Self {
        // SAFETY: key is a B256 and so is exactly 32-bytes.
        let key = unsafe { Nibbles::unpack_unchecked(key.as_slice()) };
        Self { key, lower_bound: Nibbles::new() }
    }

    /// Only match trie nodes whose path is at least this long.
    ///
    /// # Panics
    ///
    /// This method panics if `min_len` is greater than 64.
    pub fn with_min_len(mut self, min_len: u8) -> Self {
        debug_assert!(min_len <= 64);
        self.lower_bound = self.key;
        self.lower_bound.truncate(min_len as usize);
        self
    }

    /// Returns the exclusive upper bound of the range of possible trie nodes which can be retained
    /// by this target, or None for unbounded.
    fn upper_bound(&self) -> Option<Nibbles> {
        increment_and_strip_trailing_zeros(&self.lower_bound)
    }
}

impl From<B256> for Target {
    fn from(key: B256) -> Self {
        Self::new(key)
    }
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
}

/// Given a set of [`Target`]s, returns an iterator over those same [`Target`]s chunked by the
/// sub-tries they apply to within the overall trie.
pub(crate) fn iter_sub_trie_targets<'a>(
    targets: &'a mut [Target],
) -> impl Iterator<Item = SubTrieTargets<'a>> {
    // TODO this isn't quite right... if the lower_bound of a target is 0xabc, then the lower_bound
    // of the sub-trie is actually 0xab, because we need to calculate the 0xab sub-trie in case 0xab
    // is a branch, when could then hav a leaf/extension at 0xabc.

    // First sort lexicographically by lower bound. We will use this for chunking targets into
    // contiguous sections in the next steps based on their bounds.
    targets.sort_unstable_by_key(|target| target.lower_bound);

    // We now chunk targets, such that each chunk contains all targets belonging to the same
    // sub-trie. We are taking advantage of the following properties:
    //
    // - The first target in the chunk has the lowest lower bound (see previous sorting step).
    //
    // - The first target in the chunk's upper bound will therefore be the highest upper bound, and
    //   the upper bound of the whole chunk.
    //      - For example, given a chunk with lower bounds [0x2, 0x2f, 0x2fa], the upper bounds will
    //        be [0x3, 0x3, 0x2fb]. Note that no target could match a trie node with path equal to
    //        or greater than 0x3.
    //
    // - If a target's lower bound does not lie within the bounds of the current chunk, then that
    //   target must be the first target of the next chunk, covering a separate sub-trie.
    //      - Example: given lower bounds of [0x2, 0x2fa, 0x4c, 0x4ce, 0x4e], we would end up with
    //        the following chunks:
    //          - [0x2, 0x2a]  w/ upper bound 0x3
    //          - [0x4c 0x4ce] w/ upper bound 0x4d
    //          - [0x4e]       w/ upper bound 0x4f
    let mut upper_bound = targets.first().and_then(|t| t.upper_bound());
    let target_chunks = targets.chunk_by_mut(move |_, next| {
        if let Some(some_upper_bound) = upper_bound {
            let same_chunk = next.lower_bound < some_upper_bound;
            if !same_chunk {
                upper_bound = next.upper_bound();
            }
            same_chunk
        } else {
            true
        }
    });

    // Map the chunks to the return type. Within each chunk we want targets to be sorted by their
    // key, as that will be the order they are checked by the `ProofCalculator`.
    target_chunks.map(|target_chunk| {
        let lower_bound = target_chunk[0].lower_bound;
        target_chunk.sort_unstable_by_key(|target| target.key);
        SubTrieTargets { prefix: lower_bound, targets: target_chunk }
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
            // lower_bound is empty
            (
                vec![Target::new(B256::repeat_byte(0x20))],
                vec![(
                    "",
                    vec!["2020202020202020202020202020202020202020202020202020202020202020"],
                )],
            ),
            // Case 3: Multiple targets in same sub-trie (no min_len)
            // Both have empty lower_bound, so they're in the same sub-trie
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
            // with_min_len(1) gives lower_bound with first 1 nibble
            // First has lower_bound=0x2
            // Second has lower_bound=0x4
            (
                vec![
                    Target::new(B256::repeat_byte(0x20)).with_min_len(1),
                    Target::new(B256::repeat_byte(0x40)).with_min_len(1),
                ],
                vec![
                    ("2", vec!["2020202020202020202020202020202020202020202020202020202020202020"]),
                    ("4", vec!["4040404040404040404040404040404040404040404040404040404040404040"]),
                ],
            ),
            // Case 5: Three targets, two in same sub-trie, one separate
            // 0x20 and 0x2f both have lower_bound=0x2
            // 0x40 has lower_bound=0x4
            (
                vec![
                    Target::new(B256::repeat_byte(0x20)).with_min_len(1),
                    Target::new(B256::repeat_byte(0x2f)).with_min_len(1),
                    Target::new(B256::repeat_byte(0x40)).with_min_len(1),
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
            // First has min_len=1 (lower=0x2), second has min_len=2 (lower=0x2f)
            // Second's lower bound (0x2f) < first's upper bound (0x3), so same sub-trie
            (
                vec![
                    Target::new(B256::repeat_byte(0x20)).with_min_len(1),
                    Target::new(B256::repeat_byte(0x2f)).with_min_len(2),
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
            // As described in the function comments: [0x2, 0x2fa, 0x4c, 0x4ce, 0x4e]
            (
                vec![
                    Target::new(B256::repeat_byte(0x20)).with_min_len(1), // lower_bound: 0x2
                    Target::new(B256::repeat_byte(0x2f)).with_min_len(3), // lower_bound: 0x2f2
                    Target::new(B256::repeat_byte(0x4c)).with_min_len(2), // lower_bound: 0x4c
                    Target::new(B256::repeat_byte(0x4c)).with_min_len(3), // lower_bound: 0x4c4
                    Target::new(B256::repeat_byte(0x4e)).with_min_len(2), // lower_bound: 0x4e
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
        ];

        for (i, (mut input_targets, expected)) in test_cases.into_iter().enumerate() {
            let sub_tries: Vec<_> = iter_sub_trie_targets(&mut input_targets).collect();

            assert_eq!(
                sub_tries.len(),
                expected.len(),
                "Test case {} failed: expected {} sub-tries, got {}",
                i,
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
                    i, j
                );
                assert_eq!(
                    sub_trie.targets.len(),
                    exp_keys.len(),
                    "Test case {} sub-trie {}: expected {} targets, got {}",
                    i,
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
                        i, j, k
                    );
                }
            }
        }
    }
}
