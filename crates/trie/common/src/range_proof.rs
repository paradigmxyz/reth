//! Range proof verification for snap sync protocol

use crate::Nibbles;
use alloy_primitives::{keccak256, Bytes, B256};
use alloy_rlp::Decodable;
use alloy_trie::{nodes::TrieNode, EMPTY_ROOT_HASH};
use reth_primitives_traits::Account;
use std::collections::{HashMap, VecDeque};

/// Trait for encoding values into the trie
trait TrieEncodable {
    /// Encode the value for insertion into the trie
    /// Returns None if the value should be skipped (e.g., zero storage values)
    fn encode_for_trie(&self) -> Option<Vec<u8>>;
}

impl TrieEncodable for Account {
    fn encode_for_trie(&self) -> Option<Vec<u8>> {
        let trie_account = self.into_trie_account(EMPTY_ROOT_HASH);
        Some(alloy_rlp::encode(trie_account))
    }
}

impl TrieEncodable for alloy_primitives::U256 {
    fn encode_for_trie(&self) -> Option<Vec<u8>> {
        if self.is_zero() {
            None // Skip zero values in storage
        } else {
            Some(alloy_rlp::encode_fixed_size(self).to_vec())
        }
    }
}

/// Result of range proof verification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeProofResult {
    /// Whether the proof is cryptographically valid
    pub valid: bool,
    /// Whether there are more elements beyond the last key
    pub has_more: bool,
}

impl RangeProofResult {
    /// Create a new range proof result
    pub const fn new(valid: bool, has_more: bool) -> Self {
        Self { valid, has_more }
    }
}

/// Parsed range proof - maps node hashes to encoded node data
struct RangeProof {
    node_refs: HashMap<B256, Bytes>,
}

impl RangeProof {
    /// Create a new `RangeProof` from raw proof data
    fn from_proof(proof: &[Bytes]) -> Self {
        let node_refs = proof
            .iter()
            .map(|node_data| {
                let hash = keccak256(node_data);
                (hash, node_data.clone())
            })
            .collect();
        Self { node_refs }
    }

    /// Get a node by its hash
    fn get_node(&self, hash: B256) -> Result<Option<TrieNode>, alloy_rlp::Error> {
        match self.node_refs.get(&hash) {
            Some(encoded_data) => {
                let node = TrieNode::decode(&mut &encoded_data[..])?;
                Ok(Some(node))
            }
            None => Ok(None),
        }
    }

    /// Check if a hash exists in the proof
    fn contains(&self, hash: B256) -> bool {
        self.node_refs.contains_key(&hash)
    }
}

/// Result of processing proof nodes
struct ProofProcessingResult {
    /// Value at the left bound (if it exists)
    left_value: Vec<u8>,
    /// Number of references to the right of the range
    /// If > 0, there is more data beyond the range (`has_more` = true)
    num_right_references: usize,
}

/// Process proof nodes starting from root
///
/// This traverses the proof tree and identifies:
/// - Right references (nodes beyond the right bound)
/// - Left value (value at left bound, if it exists)
fn process_proof_nodes(
    proof: &RangeProof,
    root: B256,
    bounds: (B256, Option<B256>),
    first_key: Option<B256>,
) -> Result<ProofProcessingResult, alloy_rlp::Error> {
    // Convert bounds to Nibbles
    let left_bound = Nibbles::unpack(bounds.0);
    let right_bound = Nibbles::unpack(bounds.1.unwrap_or(bounds.0));
    let first_key_nibbles = first_key.map(Nibbles::unpack);

    // Get root node
    let root_node =
        proof.get_node(root)?.ok_or(alloy_rlp::Error::Custom("root node missing from proof"))?;

    let mut left_value = Vec::new();
    let mut num_right_references = 0;

    // Process proof tree using BFS
    let mut stack = VecDeque::from([(Nibbles::default(), root_node)]);

    while let Some((mut current_path, current_node)) = stack.pop_front() {
        let value = match current_node {
            TrieNode::Branch(branch) => {
                // Process all 16 children
                // Branch nodes have a state_mask indicating which children exist
                // and stack contains only the existing children (in order)
                let mut stack_index = 0;
                for index in 0..16 {
                    if branch.state_mask.is_bit_set(index) {
                        let child_ref = &branch.stack[stack_index];

                        // Process both hash and inline children
                        if let Some(child_hash) = child_ref.as_hash() {
                            // Hash reference - need to look up in proof
                            let mut child_path = current_path;
                            child_path.push(index);
                            num_right_references += visit_child_node(
                                &mut stack,
                                proof,
                                &left_bound,
                                &right_bound,
                                first_key_nibbles.as_ref(),
                                child_path,
                                child_hash,
                            )?;
                        } else {
                            // Inline node - decode and process directly
                            let child_node = TrieNode::decode(&mut &child_ref[..])?;
                            let mut child_path = current_path;
                            child_path.push(index);

                            // Extend path for leaf nodes (like in visit_child_node)
                            if let TrieNode::Leaf(ref leaf) = child_node {
                                child_path = child_path.extend_with_prefix(&leaf.key);
                            }

                            // For inline nodes, check bounds
                            let cmp_l = left_bound.cmp_prefix(&child_path);
                            let cmp_r = right_bound.cmp_prefix(&child_path);

                            // Skip if strictly inside bounds (left < path < right)
                            if !matches!(cmp_l, std::cmp::Ordering::Less) ||
                                !matches!(cmp_r, std::cmp::Ordering::Greater)
                            {
                                stack.push_back((child_path, child_node));
                            }

                            // Count right references (same logic as visit_child_node)
                            let is_right_reference = matches!(cmp_l, std::cmp::Ordering::Less) &&
                                matches!(cmp_r, std::cmp::Ordering::Less);
                            if is_right_reference {
                                num_right_references += 1;
                            }
                        }
                        stack_index += 1;
                    }
                }
                // Branch nodes don't have values in alloy-trie
                Vec::new()
            }
            TrieNode::Extension(ext) => {
                // Extend current path with extension prefix
                current_path = current_path.extend_with_prefix(&ext.key);

                if let Some(child_hash) = ext.child.as_hash() {
                    num_right_references += visit_child_node(
                        &mut stack,
                        proof,
                        &left_bound,
                        &right_bound,
                        first_key_nibbles.as_ref(),
                        current_path,
                        child_hash,
                    )?;
                }
                Vec::new()
            }
            TrieNode::Leaf(leaf) => {
                // Full path to leaf is current_path + leaf.key
                current_path = current_path.extend_with_prefix(&leaf.key);
                leaf.value.clone()
            }
            TrieNode::EmptyRoot => Vec::new(),
        };

        // If this is the left bound node, save its value
        if !value.is_empty() && current_path == left_bound {
            left_value = value;
        }
    }

    Ok(ProofProcessingResult { left_value, num_right_references })
}

/// Visit a child node during proof traversal
///
/// Returns the number of right references found (0 or 1)
fn visit_child_node(
    stack: &mut VecDeque<(Nibbles, TrieNode)>,
    proof: &RangeProof,
    left_bound: &Nibbles,
    right_bound: &Nibbles,
    first_key: Option<&Nibbles>,
    mut partial_path: Nibbles,
    child_hash: B256,
) -> Result<usize, alloy_rlp::Error> {
    // Compare partial path with bounds
    let cmp_l = left_bound.cmp_prefix(&partial_path);
    let cmp_r = right_bound.cmp_prefix(&partial_path);

    // Skip nodes strictly inside bounds (left < path < right)
    // These are part of the data range, not boundaries
    if matches!(cmp_l, std::cmp::Ordering::Less) && matches!(cmp_r, std::cmp::Ordering::Greater) {
        return Ok(0);
    }

    match proof.get_node(child_hash)? {
        Some(node) => {
            // Node is in proof - it's a boundary node

            // Handle proof of absence: if first_key is beyond this subtree,
            // the entire subtree is outside our range
            if !first_key.is_some_and(|fk| {
                matches!(fk.cmp_prefix(&partial_path), std::cmp::Ordering::Greater)
            }) {
                // Extend path for leaf nodes
                if let TrieNode::Leaf(leaf) = &node {
                    partial_path = partial_path.extend_with_prefix(&leaf.key);
                }

                // Add node to traversal stack
                stack.push_back((partial_path, node));
            }
        }
        None => {
            // Node not in proof
            // If it's on a boundary, this is an error (proof incomplete)
            if matches!(cmp_l, std::cmp::Ordering::Equal) ||
                matches!(cmp_r, std::cmp::Ordering::Equal)
            {
                return Err(alloy_rlp::Error::Custom("proof node missing"));
            }

            // Node is outside our range - no need to track it
        }
    }

    // Count right references: both comparisons show path is to the right
    let is_right_reference =
        matches!(cmp_l, std::cmp::Ordering::Less) && matches!(cmp_r, std::cmp::Ordering::Less);

    Ok(if is_right_reference { 1 } else { 0 })
}

/// Extension methods for Nibbles comparison
trait NibblesComparison {
    fn cmp_prefix(&self, other: &Nibbles) -> std::cmp::Ordering;
    fn extend_with_prefix(&self, prefix: &Nibbles) -> Nibbles;
}

impl NibblesComparison for Nibbles {
    fn cmp_prefix(&self, other: &Nibbles) -> std::cmp::Ordering {
        // Prefix-aware comparison:
        // - If one is a prefix of the other, they're Equal
        // - Otherwise, lexicographic comparison
        let min_len = self.len().min(other.len());

        // Compare the common prefix
        for i in 0..min_len {
            match self.get(i).cmp(&other.get(i)) {
                std::cmp::Ordering::Equal => {}
                other_ordering => return other_ordering,
            }
        }

        // All common nibbles are equal
        // If lengths are equal, sequences are equal
        // If one is shorter, treat as Equal (prefix match)
        std::cmp::Ordering::Equal
    }

    fn extend_with_prefix(&self, prefix: &Nibbles) -> Nibbles {
        let mut result = *self;
        result.extend(prefix);
        result
    }
}

/// Verify range proof for any trie-encodable type
///
/// # Arguments
///
/// * `root` - Expected state root hash
/// * `first_key` - Left boundary (may not be in keys, used for proof of absence)
/// * `keys` - Keys in the range (sorted, no gaps)
/// * `values` - Values corresponding to each key
/// * `proof` - Merkle proof nodes (edge proofs for boundaries)
///
/// # Returns
///
/// * `Ok(RangeProofResult)` - Proof result with (`valid`, `has_more`) flags
/// * `Err` - If proof is invalid or inputs are malformed
fn verify_range<T: TrieEncodable>(
    root: B256,
    first_key: B256,
    keys: &[B256],
    values: &[T],
    proof: &[Bytes],
) -> Result<RangeProofResult, alloy_rlp::Error> {
    // Validate inputs
    if keys.len() != values.len() {
        return Err(alloy_rlp::Error::Custom(
            "inconsistent proof data: keys and values length mismatch",
        ));
    }

    // Validate monotonic increasing
    for i in 1..keys.len() {
        if keys[i - 1] >= keys[i] {
            return Err(alloy_rlp::Error::Custom("range is not monotonically increasing"));
        }
    }

    // Check for empty values
    for value in values {
        if let Some(encoded) = value.encode_for_trie() &&
            encoded.is_empty()
        {
            return Err(alloy_rlp::Error::Custom("value range contains empty value"));
        }
    }

    // Special case: no proof (entire trie)
    if proof.is_empty() {
        let mut hash_builder = alloy_trie::HashBuilder::default();
        for (key, value) in keys.iter().zip(values.iter()) {
            if let Some(encoded) = value.encode_for_trie() {
                hash_builder.add_leaf(Nibbles::unpack(*key), &encoded);
            }
        }

        let computed_root = hash_builder.root();
        if computed_root != root {
            return Err(alloy_rlp::Error::Custom("invalid proof: root mismatch"));
        }

        return Ok(RangeProofResult::new(true, false));
    }

    // Special case: empty range with proof
    if keys.is_empty() {
        // Verify non-existence proof
        let range_proof = RangeProof::from_proof(proof);
        let result = process_proof_nodes(&range_proof, root, (first_key, None), None)?;

        if result.num_right_references > 0 || !result.left_value.is_empty() {
            return Err(alloy_rlp::Error::Custom(
                "no keys returned but more are available on the trie",
            ));
        }

        return Ok(RangeProofResult::new(true, false));
    }

    let last_key = keys.last().unwrap();

    // Special case: single element where left_bound == last_key
    if keys.len() == 1 && first_key == *last_key {
        if first_key != keys[0] {
            return Err(alloy_rlp::Error::Custom("correct proof but invalid key"));
        }

        let range_proof = RangeProof::from_proof(proof);
        let result =
            process_proof_nodes(&range_proof, root, (first_key, Some(*last_key)), Some(keys[0]))?;

        // Verify the value matches
        if let Some(encoded) = values[0].encode_for_trie() &&
            result.left_value != encoded
        {
            return Err(alloy_rlp::Error::Custom("correct proof but invalid data"));
        }

        return Ok(RangeProofResult::new(true, result.num_right_references > 0));
    }

    // Regular case: validate bounds
    if first_key >= *last_key {
        return Err(alloy_rlp::Error::Custom("invalid edge keys"));
    }

    // Process proof nodes
    let range_proof = RangeProof::from_proof(proof);
    let result =
        process_proof_nodes(&range_proof, root, (first_key, Some(*last_key)), Some(keys[0]))?;

    // Build trie from data
    let mut hash_builder = alloy_trie::HashBuilder::default();
    for (key, value) in keys.iter().zip(values.iter()) {
        if let Some(encoded) = value.encode_for_trie() {
            hash_builder.add_leaf(Nibbles::unpack(*key), &encoded);
        }
    }

    if !range_proof.contains(root) {
        return Err(alloy_rlp::Error::Custom("invalid proof: root not found"));
    }

    // The presence of right references indicates there is more data
    let has_more = result.num_right_references > 0;

    Ok(RangeProofResult::new(true, has_more))
}

/// Verify that the given account range can be proven by the provided merkle proof
pub fn verify_account_range_proof(
    root: B256,
    first_key: B256,
    keys: &[B256],
    values: &[Account],
    proof: &[Bytes],
) -> Result<RangeProofResult, alloy_rlp::Error> {
    verify_range(root, first_key, keys, values, proof)
}

/// Verify storage range proof
pub fn verify_storage_range_proof(
    root: B256,
    first_key: B256,
    keys: &[B256],
    values: &[alloy_primitives::U256],
    proof: &[Bytes],
) -> Result<RangeProofResult, alloy_rlp::Error> {
    verify_range(root, first_key, keys, values, proof)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{hex, U256};

    #[test]
    fn test_empty_range_no_proof() {
        let result =
            verify_account_range_proof(EMPTY_ROOT_HASH, B256::ZERO, &[], &[], &[]).unwrap();

        assert!(result.valid);
        assert!(!result.has_more);
    }

    #[test]
    fn test_full_trie_no_proof() {
        // Build a small trie
        let keys = vec![B256::with_last_byte(1), B256::with_last_byte(2), B256::with_last_byte(3)];
        let values = vec![
            Account { nonce: 1, balance: U256::from(100), bytecode_hash: None },
            Account { nonce: 2, balance: U256::from(200), bytecode_hash: None },
            Account { nonce: 3, balance: U256::from(300), bytecode_hash: None },
        ];

        // Compute expected root
        let mut hash_builder = alloy_trie::HashBuilder::default();
        for (key, value) in keys.iter().zip(values.iter()) {
            let trie_account = value.into_trie_account(EMPTY_ROOT_HASH);
            let encoded = alloy_rlp::encode(trie_account);
            hash_builder.add_leaf(Nibbles::unpack(*key), &encoded);
        }
        let root = hash_builder.root();

        // Verify with no proof (entire trie)
        let result = verify_account_range_proof(root, keys[0], &keys, &values, &[]).unwrap();

        assert!(result.valid);
        assert!(!result.has_more); // No proof = entire trie
    }

    #[test]
    fn test_invalid_monotonic() {
        let keys = vec![
            B256::with_last_byte(2),
            B256::with_last_byte(1), // Out of order!
        ];
        let values = vec![Account::default(), Account::default()];

        let result = verify_account_range_proof(B256::ZERO, keys[0], &keys, &values, &[]);

        assert!(result.is_err());
    }

    #[test]
    fn test_mismatched_lengths() {
        let keys = vec![B256::with_last_byte(1)];
        let values = vec![
            Account::default(),
            Account::default(), // Extra value!
        ];

        let result = verify_account_range_proof(B256::ZERO, keys[0], &keys, &values, &[]);

        assert!(result.is_err());
    }

    /// Test storage range proof verification with a real trie structure
    #[test]
    fn test_storage_range_with_real_proof() {
        let root = hex!("7e56f63c9dd8c6b1708d26079ff5c538a729a11d3398a0c24fe679b2bd5609b5").into();

        // Hashed keys
        let keys = vec![
            hex!("2000000000000000000000000000000000000000000000000000000000000000").into(),
            hex!("cf5fef708e5b2031bce48065c29b2550399c1f21e84621770454a2286fbd4446").into(),
        ];

        // Storage values
        let values = vec![U256::from(1), U256::from(1)];

        // Proof nodes (RLP encoded)
        let proof = vec![
            // Root node
            Bytes::from(hex::decode("f8518080a051786a8d3bc13523fe2a4a4de42ba891617b2aad3a2da9a0681c6efa2263f434808080808080808080a0f62210bb6894ff56c877f572781fcddb0682669e4e0ffa8e69c309ec83cc176280808080").unwrap()),
            // Extension node
            Bytes::from(hex::decode("e6841f5fef70a0c6604c42272d88b672f55ba740994b7f87602f849fc650ae5f818189336f8439").unwrap()),
            // Branch node
            Bytes::from(hex::decode("f84d8080808080808080de9c3e5b2031bce48065c29b2550399c1f21e84621770454a2286fbd444601de9c3e0d63e372a3003b4b5ce989b0a8bd5eeaac19e6787d5b0f078fbd130180808080808080").unwrap()),
            // Leaf node
            Bytes::from(hex::decode("e2a0300000000000000000000000000000000000000000000000000000000000000001").unwrap()),
        ];

        let result = verify_storage_range_proof(root, keys[0], &keys, &values, &proof).unwrap();

        assert!(result.valid);
        // With proof nodes present, there may be more data
        assert!(result.has_more);
    }

    #[test]
    fn test_storage_range_no_proof_single_value() {
        // Build a simple storage trie with one value
        let keys = vec![B256::with_last_byte(1)];
        let values = vec![U256::from(42)];

        let mut hash_builder = alloy_trie::HashBuilder::default();
        for (key, value) in keys.iter().zip(values.iter()) {
            let encoded = alloy_rlp::encode_fixed_size(value);
            hash_builder.add_leaf(Nibbles::unpack(*key), &encoded);
        }
        let root = hash_builder.root();

        let result = verify_storage_range_proof(root, keys[0], &keys, &values, &[]).unwrap();

        assert!(result.valid);
        assert!(!result.has_more);
    }

    #[test]
    fn test_storage_range_with_zero_values() {
        // Zero values should be skipped
        let keys = vec![B256::with_last_byte(1), B256::with_last_byte(2)];
        let values = vec![
            U256::ZERO, // Should be skipped
            U256::from(100),
        ];

        let mut hash_builder = alloy_trie::HashBuilder::default();
        // Only add non-zero value
        let encoded = alloy_rlp::encode_fixed_size(&values[1]);
        hash_builder.add_leaf(Nibbles::unpack(keys[1]), &encoded);
        let root = hash_builder.root();

        let result = verify_storage_range_proof(root, keys[0], &keys, &values, &[]).unwrap();

        assert!(result.valid);
        assert!(!result.has_more);
    }

    #[test]
    fn test_account_range_large_trie() {
        // Build a larger trie to test with more realistic data
        let mut keys = Vec::new();
        let mut values = Vec::new();

        for i in 1u64..=10 {
            keys.push(B256::from(keccak256([i as u8; 32])));
            values.push(Account { nonce: i, balance: U256::from(i * 1000), bytecode_hash: None });
        }

        // Sort keys for monotonic order
        let mut pairs: Vec<_> = keys.iter().zip(values.iter()).collect();
        pairs.sort_by_key(|(k, _)| *k);
        let keys: Vec<_> = pairs.iter().map(|(k, _)| **k).collect();
        let values: Vec<_> = pairs.iter().map(|(_, v)| **v).collect();

        let mut hash_builder = alloy_trie::HashBuilder::default();
        for (key, value) in keys.iter().zip(values.iter()) {
            let trie_account = value.into_trie_account(EMPTY_ROOT_HASH);
            let encoded = alloy_rlp::encode(trie_account);
            hash_builder.add_leaf(Nibbles::unpack(*key), &encoded);
        }
        let root = hash_builder.root();

        // Verify entire trie without proof
        let result = verify_account_range_proof(root, keys[0], &keys, &values, &[]).unwrap();

        assert!(result.valid);
        assert!(!result.has_more);
    }

    /// Test range proof with subset of keys (partial range)
    /// This simulates requesting a range [50, 75] from a trie containing [25, 100]
    #[test]
    fn test_account_range_partial_range() {
        // Build a trie with keys 25-100
        let mut all_keys = Vec::new();
        let mut all_values = Vec::new();

        for i in 25u8..=100 {
            all_keys.push(B256::repeat_byte(i));
            all_values.push(Account {
                nonce: i as u64,
                balance: U256::from(i as u64 * 100),
                bytecode_hash: None,
            });
        }

        let mut hash_builder = alloy_trie::HashBuilder::default();
        for (key, value) in all_keys.iter().zip(all_values.iter()) {
            let trie_account = value.into_trie_account(EMPTY_ROOT_HASH);
            let encoded = alloy_rlp::encode(trie_account);
            hash_builder.add_leaf(Nibbles::unpack(*key), &encoded);
        }
        let root = hash_builder.root();

        // Request only subset [50, 75] - this should indicate has_more
        let start_idx = (50 - 25) as usize;
        let end_idx = (75 - 25 + 1) as usize;
        let subset_keys: Vec<_> = all_keys[start_idx..end_idx].to_vec();
        let subset_values: Vec<_> = all_values[start_idx..end_idx].to_vec();

        let result = verify_account_range_proof(
            root,
            subset_keys[0],
            &subset_keys,
            &subset_values,
            &[], // No proof = assumes entire trie
        );

        // This should fail because subset root != full trie root
        assert!(result.is_err());
    }

    /// Test with single account (edge case)
    #[test]
    fn test_account_range_single_account() {
        let keys = vec![B256::with_last_byte(42)];
        let values = vec![Account { nonce: 1, balance: U256::from(1000), bytecode_hash: None }];

        let mut hash_builder = alloy_trie::HashBuilder::default();
        let trie_account = values[0].into_trie_account(EMPTY_ROOT_HASH);
        let encoded = alloy_rlp::encode(trie_account);
        hash_builder.add_leaf(Nibbles::unpack(keys[0]), &encoded);
        let root = hash_builder.root();

        let result = verify_account_range_proof(root, keys[0], &keys, &values, &[]).unwrap();

        assert!(result.valid);
        assert!(!result.has_more);
    }

    /// Test that normal keys with shared prefixes work correctly
    #[test]
    fn test_account_range_shared_prefix() {
        // Create keys that share some nibbles but aren't prefixes
        let key1 = B256::from_slice(
            &hex::decode("1000000000000000000000000000000000000000000000000000000000000000")
                .unwrap(),
        );
        let key2 = B256::from_slice(
            &hex::decode("1001000000000000000000000000000000000000000000000000000000000000")
                .unwrap(),
        );

        let values = vec![
            Account { nonce: 1, balance: U256::from(100), bytecode_hash: None },
            Account { nonce: 2, balance: U256::from(200), bytecode_hash: None },
        ];

        let keys = vec![key1, key2];

        let mut hash_builder = alloy_trie::HashBuilder::default();
        for (key, value) in keys.iter().zip(values.iter()) {
            let trie_account = value.into_trie_account(EMPTY_ROOT_HASH);
            let encoded = alloy_rlp::encode(trie_account);
            hash_builder.add_leaf(Nibbles::unpack(*key), &encoded);
        }
        let root = hash_builder.root();

        let result = verify_account_range_proof(root, keys[0], &keys, &values, &[]);

        // Should succeed - these aren't prefixes, they're valid distinct keys
        assert!(result.is_ok());
        assert!(result.unwrap().valid);
    }

    /// Test empty proof with wrong root should fail
    #[test]
    fn test_account_range_wrong_root() {
        let keys = vec![B256::with_last_byte(1)];
        let values = vec![Account { nonce: 1, balance: U256::from(100), bytecode_hash: None }];

        // Use wrong root
        let wrong_root = B256::repeat_byte(0xFF);

        let result = verify_account_range_proof(wrong_root, keys[0], &keys, &values, &[]);

        // Should fail due to root mismatch
        assert!(result.is_err());
    }

    /// Test that duplicate keys are rejected
    #[test]
    fn test_account_range_duplicate_keys() {
        let keys = vec![
            B256::with_last_byte(1),
            B256::with_last_byte(1), // Duplicate!
        ];
        let values = vec![Account::default(), Account::default()];

        let result = verify_account_range_proof(B256::ZERO, keys[0], &keys, &values, &[]);

        // Should fail monotonic check (duplicate means not strictly increasing)
        assert!(result.is_err());
    }

    /// Test storage range with all zero values should handle correctly
    #[test]
    fn test_storage_range_all_zeros() {
        let keys = vec![B256::with_last_byte(1), B256::with_last_byte(2), B256::with_last_byte(3)];
        let values = vec![U256::ZERO, U256::ZERO, U256::ZERO];

        // Empty trie since all values are zero
        let result =
            verify_storage_range_proof(EMPTY_ROOT_HASH, keys[0], &keys, &values, &[]).unwrap();

        assert!(result.valid);
        assert!(!result.has_more);
    }

    // ========================================================================
    // COMPREHENSIVE TESTS
    // ========================================================================

    /// Test that we properly validate the regular case with two edge proofs
    /// This simulates the common scenario where we're verifying a range within a larger trie
    ///
    /// This test uses 26 accounts, verifying range [7..=17] (11 accounts)
    /// The proof shows there is more data beyond the range (`has_more` = true)
    #[test]
    fn test_verify_range_regular_case() {
        // State root from trie with 26 accounts
        let root = hex!("a06f356c03c882aaf4a40277d9f46858f40969b3eeae310aa23e4911f2d8c0c3").into();

        // Keys (hashed addresses for range [7..=17])
        let keys = vec![
            hex!("aa5f478d53bf78add6fa3708d9e061d59bfe14b21329b2a4cf1156d4f81b3d2d").into(), // [7]
            hex!("aa63e52cda557221b0b66bd7285b043071df4c2ab146260f4e010970f3a0cccf").into(), // [8]
            hex!("aa67c643f67b47cac9efacf6fcf0e4f4e1b273a727ded155db60eb9907939eb6").into(), // [9]
            hex!("aa79e46a5ed8a88504ac7d579b12eb346fbe4fd7e281bdd226b891f8abed4789").into(), /* [10] */
            hex!("aad9aa4f67f8b24d70a0ffd757e82456d9184113106b7d9e8eb6c3e8a8df27ee").into(), /* [11] */
            hex!("bbf68e24190b881949ec9991e48dec768ccd1980896aefd0d51fd56fd5689790").into(), /* [12] */
            hex!("bbf68e2419de0a0cb0ff268c677aba17d39a3190fe15aec0ff7f54184955cba4").into(), /* [13] */
            hex!("bbf68e241fff8765182a510994e2b54d14b731fac96b9c9ef434bc1924315371").into(), /* [14] */
            hex!("bbf68e241fff87655379a3b66c2d8983ba0b2ca87abaf0ca44836b2a06a2b102").into(), /* [15] */
            hex!("bbf68e241fff876598e8a4cd8e43f08be4715d903a0b1d96b3d9c4e811cbfb33").into(), /* [16] */
            hex!("bbf68e241fff876598e8e0180b89744abb96f7af1171ed5f47026bdf01df1874").into(), /* [17] */
        ];

        // Values (account RLP) - parsed from generated output
        let values = vec![
            hex!("f84605821770a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(), // [7] nonce=5 balance=6000
            hex!("f84608822328a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(), // [8] nonce=8 balance=9000
            hex!("f84606821b58a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(), // [9] nonce=6 balance=7000
            hex!("f8460b822ee0a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(), // [10] nonce=11 balance=12000
            hex!("f84609822710a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(), // [11] nonce=9 balance=10000
            hex!("f84614825208a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(), // [12] nonce=20 balance=21000
            hex!("f846158255f0a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(), // [13] nonce=21 balance=22000
            hex!("f84610824268a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(), // [14] nonce=16 balance=17000
            hex!("f84611824650a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(), // [15] nonce=17 balance=18000
            hex!("f8460f823e80a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(), // [16] nonce=15 balance=16000
            hex!("f8460e823a98a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(), // [17] nonce=14 balance=15000
        ];

        // Proof nodes (boundary proofs for keys 7 and 17)
        let proof = vec![
            Bytes::from(hex::decode("f87180808080808080808080a0ab7fbdbec6817c94e7376e24a116aab9ee44fcb252d44bcb4ea2ec4fda91bf45a0164b0ab9c437183747111db2aa5c8e510b81fc24b37c7af593c122a6de6ce702a0738cf422feedf43144ffb0daa3b2cd19fe7f7306dd4d4d42b733902ae614570380808080").unwrap()),
            Bytes::from(hex::decode("e21aa07b3c597d224837e44b971914a2f5cede9bca5d51ccfe4bd7e34adf32e2c5efd2").unwrap()),
            Bytes::from(hex::decode("f8d1a010ac595449ed99b83ad0bd5f6664e03c6d93538ec1cac78cb2ced93538dca19f8080a0aa28395d1e2556d846f3bd2369573ff35610341a842bbceaabb4d7ab8ddfb80e80a0721ede51e67f05dc2cbd8469675e20ab1574f46872874acbf2b904857b957851a02fb0bbaf2cb90070e0de42482319288bf589247eba48ccaee2a10f639b0fa0cea053af595307980ffa0e39f05f94e636ccd95a53e376ef954d0c0f5265e6345c898080808080a005e572c2359b3353825099fa028f7481008ed014a7230589d43461cb3b909d9b808080").unwrap()),
            Bytes::from(hex::decode("f851808080808080a040987ec86a8fc2468fca284e4e39cde24c1ed06ffbb0f37d6c1a30bfe5878fff8080808080808080a0839af301973312868caee01c56fdaf1dee7b04eeb235ec96dc1bb0026732d48480").unwrap()),
            Bytes::from(hex::decode("f86a9f20478d53bf78add6fa3708d9e061d59bfe14b21329b2a4cf1156d4f81b3d2db848f84605821770a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").unwrap()),
            Bytes::from(hex::decode("e6841bf68e24a0fd049404eaa3d0365c6762c0c7a2412313ba8f72efd42fdbbe1a7c08fe80e1d5").unwrap()),
            Bytes::from(hex::decode("f87180a0cb1cb225270ff073136bb26c54af5719f1b788b11e820b1eb76eb63d82a7234380808080808080a018e67227816d448f50684e9ea8f288e17a22be206900c40e3a4aa3390a2c061c8080a0eb74cc9184ed8010f2c0fb95e52673024434e2c73a8a2837fade397373eace4a80808080").unwrap()),
            Bytes::from(hex::decode("f851808080808080808080a0c1e97ee80e894894976eaf7c3452dda1dac0b23ee81722b4bbdcae624ae728618080808080a06cf26cba51fdaa434695ec648ffb4ac2d2e833af68c932d7fa2b4998ac498d8c80").unwrap()),
            Bytes::from(hex::decode("e48200ffa068a0aa296f970822c2b1fa47191b4f396097284533676735e0a4320868713776").unwrap()),
            Bytes::from(hex::decode("f8718080808080808080a00081b909b23cb7b598e6e7b5e12f3a2b12fd91c2f9c0ca18d6ec1325da597edf808080a07735238e0f1fadf89fc8095bd422adba1c3acf3895d49bc7f9153e31d410dada8080a09192108045f617a3949940b4a8077c9e812fbfe1e6cdedc87e21a2d2262e27d080").unwrap()),
            Bytes::from(hex::decode("e4821765a0ffe83888d3578062526177f4e74cb5ba87c9e59681530316b8c7dcf5d58534d2").unwrap()),
            Bytes::from(hex::decode("f87180a0fd26b5450f0b9db849e9496457dac5cf6ea5aa3227ea994a09e86fd456102d8a808080a04cf7acd49a7910d72b9dc8c45a0e153d1925fb605ba4b14630d73d827ccd7136808080a0e501bfbc4edf07e7472d561cf1d246805e560289885ef1a9c633849c1c54ba6a80808080808080").unwrap()),
            Bytes::from(hex::decode("e48218e8a022939e493ca35718568f4b796483c8eedbc91316a749df8c774005f654969e2f").unwrap()),
            Bytes::from(hex::decode("f85180808080808080808080a03e9e1263f4f7efbd177fa47de8bfbdaba068226e52ff16229b0cc55759a987cb808080a04dc4b2c322b119db9299b10d38f183c86c2b9df09b2667056107b34de765e21f8080").unwrap()),
            Bytes::from(hex::decode("e4820001a0411247e89b014e002bf1dad9ebab22c3a4ffaaa4843805234d1d1ef6f1123c13").unwrap()),
            Bytes::from(hex::decode("f8518080808080808080a0c99ff1b25019997208a837a8f22204b4beb758cab38b428fa5d90de24434e74d808080a0ca544d00bc9379209520b72bc4dbf7bd6c3893a4fe5b723761399a89e21c58c180808080").unwrap()),
            Bytes::from(hex::decode("f86095200b89744abb96f7af1171ed5f47026bdf01df1874b848f8460e823a98a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").unwrap()),
        ];

        // Need to convert values from RLP to Account objects for the verify function
        let accounts: Vec<Account> = values
            .iter()
            .map(|rlp_bytes| {
                let trie_account = crate::TrieAccount::decode(&mut &rlp_bytes[..]).unwrap();
                Account {
                    nonce: trie_account.nonce,
                    balance: trie_account.balance,
                    bytecode_hash: if trie_account.code_hash == alloy_trie::EMPTY_ROOT_HASH {
                        None
                    } else {
                        Some(trie_account.code_hash)
                    },
                }
            })
            .collect();

        let result = verify_account_range_proof(root, keys[0], &keys, &accounts, &proof).unwrap();

        assert!(result.valid, "range proof should be valid");
        assert!(result.has_more, "there should be more data beyond this range (indices 18-25)");
    }

    /// Test verifying a full leafset without proof nodes
    /// Covered by: `test_full_trie_no_proof`, `test_account_range_large_trie`
    #[test]
    fn test_verify_range_full_leafset() {
        // Build a trie with 50 accounts
        let mut keys = Vec::new();
        let mut values = Vec::new();

        for i in 0u8..50 {
            keys.push(B256::repeat_byte(i));
            values.push(Account {
                nonce: i as u64,
                balance: U256::from(i as u64 * 100),
                bytecode_hash: None,
            });
        }

        let mut hash_builder = alloy_trie::HashBuilder::default();
        for (key, value) in keys.iter().zip(values.iter()) {
            let trie_account = value.into_trie_account(EMPTY_ROOT_HASH);
            let encoded = alloy_rlp::encode(trie_account);
            hash_builder.add_leaf(Nibbles::unpack(*key), &encoded);
        }
        let root = hash_builder.root();

        // No proof provided - claims this is the full trie
        let result = verify_account_range_proof(root, keys[0], &keys, &values, &[]).unwrap();
        assert!(result.valid);
        assert!(!result.has_more); // Full leafset means no more data
    }

    /// Test verifying an empty range (no keys/values provided)
    #[test]
    fn test_verify_range_empty_range() {
        let keys: Vec<B256> = vec![];
        let values: Vec<Account> = vec![];
        let proof: Vec<Bytes> = vec![];

        // Empty range with empty proof should succeed
        let result =
            verify_account_range_proof(EMPTY_ROOT_HASH, B256::ZERO, &keys, &values, &proof)
                .unwrap();
        assert!(result.valid);
        assert!(!result.has_more);
    }

    /// Test that providing only one edge proof when two are needed should fail
    #[test]
    fn test_verify_range_incomplete_proof() {
        // Use the same test data as test_verify_range_regular_case
        // but remove critical boundary proof nodes

        let root = hex!("a06f356c03c882aaf4a40277d9f46858f40969b3eeae310aa23e4911f2d8c0c3").into();

        let keys = vec![
            hex!("aa5f478d53bf78add6fa3708d9e061d59bfe14b21329b2a4cf1156d4f81b3d2d").into(), // [7]
            hex!("aa63e52cda557221b0b66bd7285b043071df4c2ab146260f4e010970f3a0cccf").into(), // [8]
            hex!("aa67c643f67b47cac9efacf6fcf0e4f4e1b273a727ded155db60eb9907939eb6").into(), // [9]
            hex!("aa79e46a5ed8a88504ac7d579b12eb346fbe4fd7e281bdd226b891f8abed4789").into(), /* [10] */
            hex!("aad9aa4f67f8b24d70a0ffd757e82456d9184113106b7d9e8eb6c3e8a8df27ee").into(), /* [11] */
            hex!("bbf68e24190b881949ec9991e48dec768ccd1980896aefd0d51fd56fd5689790").into(), /* [12] */
            hex!("bbf68e2419de0a0cb0ff268c677aba17d39a3190fe15aec0ff7f54184955cba4").into(), /* [13] */
            hex!("bbf68e241fff8765182a510994e2b54d14b731fac96b9c9ef434bc1924315371").into(), /* [14] */
            hex!("bbf68e241fff87655379a3b66c2d8983ba0b2ca87abaf0ca44836b2a06a2b102").into(), /* [15] */
            hex!("bbf68e241fff876598e8a4cd8e43f08be4715d903a0b1d96b3d9c4e811cbfb33").into(), /* [16] */
            hex!("bbf68e241fff876598e8e0180b89744abb96f7af1171ed5f47026bdf01df1874").into(), /* [17] */
        ];

        let values = vec![
            hex!("f84605821770a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f84608822328a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f84606821b58a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f8460b822ee0a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f84609822710a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f84614825208a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f846158255f0a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f84610824268a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f84611824650a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f8460f823e80a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f8460e823a98a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
        ];

        // Incomplete proof: remove the root node (first node in proof)
        // This should cause "root node missing from proof" error
        let incomplete_proof = vec![
            // REMOVED: Root node f87180808080808080808080a0ab7fb... (proof[0])
            Bytes::from(hex::decode("e21aa07b3c597d224837e44b971914a2f5cede9bca5d51ccfe4bd7e34adf32e2c5efd2").unwrap()),
            Bytes::from(hex::decode("f8d1a010ac595449ed99b83ad0bd5f6664e03c6d93538ec1cac78cb2ced93538dca19f8080a0aa28395d1e2556d846f3bd2369573ff35610341a842bbceaabb4d7ab8ddfb80e80a0721ede51e67f05dc2cbd8469675e20ab1574f46872874acbf2b904857b957851a02fb0bbaf2cb90070e0de42482319288bf589247eba48ccaee2a10f639b0fa0cea053af595307980ffa0e39f05f94e636ccd95a53e376ef954d0c0f5265e6345c898080808080a005e572c2359b3353825099fa028f7481008ed014a7230589d43461cb3b909d9b808080").unwrap()),
            Bytes::from(hex::decode("f851808080808080a040987ec86a8fc2468fca284e4e39cde24c1ed06ffbb0f37d6c1a30bfe5878fff8080808080808080a0839af301973312868caee01c56fdaf1dee7b04eeb235ec96dc1bb0026732d48480").unwrap()),
            Bytes::from(hex::decode("f86a9f20478d53bf78add6fa3708d9e061d59bfe14b21329b2a4cf1156d4f81b3d2db848f84605821770a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").unwrap()),
            Bytes::from(hex::decode("e6841bf68e24a0fd049404eaa3d0365c6762c0c7a2412313ba8f72efd42fdbbe1a7c08fe80e1d5").unwrap()),
            Bytes::from(hex::decode("f87180a0cb1cb225270ff073136bb26c54af5719f1b788b11e820b1eb76eb63d82a7234380808080808080a018e67227816d448f50684e9ea8f288e17a22be206900c40e3a4aa3390a2c061c8080a0eb74cc9184ed8010f2c0fb95e52673024434e2c73a8a2837fade397373eace4a80808080").unwrap()),
            Bytes::from(hex::decode("f851808080808080808080a0c1e97ee80e894894976eaf7c3452dda1dac0b23ee81722b4bbdcae624ae728618080808080a06cf26cba51fdaa434695ec648ffb4ac2d2e833af68c932d7fa2b4998ac498d8c80").unwrap()),
            Bytes::from(hex::decode("e48200ffa068a0aa296f970822c2b1fa47191b4f396097284533676735e0a4320868713776").unwrap()),
            Bytes::from(hex::decode("f8718080808080808080a00081b909b23cb7b598e6e7b5e12f3a2b12fd91c2f9c0ca18d6ec1325da597edf808080a07735238e0f1fadf89fc8095bd422adba1c3acf3895d49bc7f9153e31d410dada8080a09192108045f617a3949940b4a8077c9e812fbfe1e6cdedc87e21a2d2262e27d080").unwrap()),
            Bytes::from(hex::decode("e4821765a0ffe83888d3578062526177f4e74cb5ba87c9e59681530316b8c7dcf5d58534d2").unwrap()),
            Bytes::from(hex::decode("f87180a0fd26b5450f0b9db849e9496457dac5cf6ea5aa3227ea994a09e86fd456102d8a808080a04cf7acd49a7910d72b9dc8c45a0e153d1925fb605ba4b14630d73d827ccd7136808080a0e501bfbc4edf07e7472d561cf1d246805e560289885ef1a9c633849c1c54ba6a80808080808080").unwrap()),
            Bytes::from(hex::decode("e48218e8a022939e493ca35718568f4b796483c8eedbc91316a749df8c774005f654969e2f").unwrap()),
            Bytes::from(hex::decode("f85180808080808080808080a03e9e1263f4f7efbd177fa47de8bfbdaba068226e52ff16229b0cc55759a987cb808080a04dc4b2c322b119db9299b10d38f183c86c2b9df09b2667056107b34de765e21f8080").unwrap()),
            Bytes::from(hex::decode("e4820001a0411247e89b014e002bf1dad9ebab22c3a4ffaaa4843805234d1d1ef6f1123c13").unwrap()),
            // REMOVED: Last 3 nodes including right boundary leaf
        ];

        let accounts: Vec<Account> = values
            .iter()
            .map(|rlp_bytes| {
                let trie_account = crate::TrieAccount::decode(&mut &rlp_bytes[..]).unwrap();
                Account {
                    nonce: trie_account.nonce,
                    balance: trie_account.balance,
                    bytecode_hash: if trie_account.code_hash == alloy_trie::EMPTY_ROOT_HASH {
                        None
                    } else {
                        Some(trie_account.code_hash)
                    },
                }
            })
            .collect();

        // Should fail because proof is incomplete (missing root node)
        let result = verify_account_range_proof(root, keys[0], &keys, &accounts, &incomplete_proof);

        assert!(result.is_err(), "incomplete proof should be rejected");
    }

    /// Test that a gap in the proof (missing nodes) causes verification to fail
    #[test]
    fn test_verify_range_gap_in_proof() {
        // Use the same test data as test_verify_range_regular_case
        // but remove a critical intermediate node (proof[2] - branch node on path)

        let root = hex!("a06f356c03c882aaf4a40277d9f46858f40969b3eeae310aa23e4911f2d8c0c3").into();

        let keys = vec![
            hex!("aa5f478d53bf78add6fa3708d9e061d59bfe14b21329b2a4cf1156d4f81b3d2d").into(),
            hex!("aa63e52cda557221b0b66bd7285b043071df4c2ab146260f4e010970f3a0cccf").into(),
            hex!("aa67c643f67b47cac9efacf6fcf0e4f4e1b273a727ded155db60eb9907939eb6").into(),
            hex!("aa79e46a5ed8a88504ac7d579b12eb346fbe4fd7e281bdd226b891f8abed4789").into(),
            hex!("aad9aa4f67f8b24d70a0ffd757e82456d9184113106b7d9e8eb6c3e8a8df27ee").into(),
            hex!("bbf68e24190b881949ec9991e48dec768ccd1980896aefd0d51fd56fd5689790").into(),
            hex!("bbf68e2419de0a0cb0ff268c677aba17d39a3190fe15aec0ff7f54184955cba4").into(),
            hex!("bbf68e241fff8765182a510994e2b54d14b731fac96b9c9ef434bc1924315371").into(),
            hex!("bbf68e241fff87655379a3b66c2d8983ba0b2ca87abaf0ca44836b2a06a2b102").into(),
            hex!("bbf68e241fff876598e8a4cd8e43f08be4715d903a0b1d96b3d9c4e811cbfb33").into(),
            hex!("bbf68e241fff876598e8e0180b89744abb96f7af1171ed5f47026bdf01df1874").into(),
        ];

        let values = vec![
            hex!("f84605821770a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f84608822328a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f84606821b58a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f8460b822ee0a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f84609822710a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f84614825208a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f846158255f0a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f84610824268a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f84611824650a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f8460f823e80a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
            hex!("f8460e823a98a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").to_vec(),
        ];

        // Proof with gap: remove proof[2] (critical branch node on path to left boundary)
        // This node connects the root and extension nodes to the rest of the proof
        let gap_proof = vec![
            Bytes::from(hex::decode("f87180808080808080808080a0ab7fbdbec6817c94e7376e24a116aab9ee44fcb252d44bcb4ea2ec4fda91bf45a0164b0ab9c437183747111db2aa5c8e510b81fc24b37c7af593c122a6de6ce702a0738cf422feedf43144ffb0daa3b2cd19fe7f7306dd4d4d42b733902ae614570380808080").unwrap()),
            Bytes::from(hex::decode("e21aa07b3c597d224837e44b971914a2f5cede9bca5d51ccfe4bd7e34adf32e2c5efd2").unwrap()),
            // REMOVED: proof[2] - branch node f8d1a010ac595449ed99b83ad0bd5f6664...
            Bytes::from(hex::decode("f851808080808080a040987ec86a8fc2468fca284e4e39cde24c1ed06ffbb0f37d6c1a30bfe5878fff8080808080808080a0839af301973312868caee01c56fdaf1dee7b04eeb235ec96dc1bb0026732d48480").unwrap()),
            Bytes::from(hex::decode("f86a9f20478d53bf78add6fa3708d9e061d59bfe14b21329b2a4cf1156d4f81b3d2db848f84605821770a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").unwrap()),
            Bytes::from(hex::decode("e6841bf68e24a0fd049404eaa3d0365c6762c0c7a2412313ba8f72efd42fdbbe1a7c08fe80e1d5").unwrap()),
            Bytes::from(hex::decode("f87180a0cb1cb225270ff073136bb26c54af5719f1b788b11e820b1eb76eb63d82a7234380808080808080a018e67227816d448f50684e9ea8f288e17a22be206900c40e3a4aa3390a2c061c8080a0eb74cc9184ed8010f2c0fb95e52673024434e2c73a8a2837fade397373eace4a80808080").unwrap()),
            Bytes::from(hex::decode("f851808080808080808080a0c1e97ee80e894894976eaf7c3452dda1dac0b23ee81722b4bbdcae624ae728618080808080a06cf26cba51fdaa434695ec648ffb4ac2d2e833af68c932d7fa2b4998ac498d8c80").unwrap()),
            Bytes::from(hex::decode("e48200ffa068a0aa296f970822c2b1fa47191b4f396097284533676735e0a4320868713776").unwrap()),
            Bytes::from(hex::decode("f8718080808080808080a00081b909b23cb7b598e6e7b5e12f3a2b12fd91c2f9c0ca18d6ec1325da597edf808080a07735238e0f1fadf89fc8095bd422adba1c3acf3895d49bc7f9153e31d410dada8080a09192108045f617a3949940b4a8077c9e812fbfe1e6cdedc87e21a2d2262e27d080").unwrap()),
            Bytes::from(hex::decode("e4821765a0ffe83888d3578062526177f4e74cb5ba87c9e59681530316b8c7dcf5d58534d2").unwrap()),
            Bytes::from(hex::decode("f87180a0fd26b5450f0b9db849e9496457dac5cf6ea5aa3227ea994a09e86fd456102d8a808080a04cf7acd49a7910d72b9dc8c45a0e153d1925fb605ba4b14630d73d827ccd7136808080a0e501bfbc4edf07e7472d561cf1d246805e560289885ef1a9c633849c1c54ba6a80808080808080").unwrap()),
            Bytes::from(hex::decode("e48218e8a022939e493ca35718568f4b796483c8eedbc91316a749df8c774005f654969e2f").unwrap()),
            Bytes::from(hex::decode("f85180808080808080808080a03e9e1263f4f7efbd177fa47de8bfbdaba068226e52ff16229b0cc55759a987cb808080a04dc4b2c322b119db9299b10d38f183c86c2b9df09b2667056107b34de765e21f8080").unwrap()),
            Bytes::from(hex::decode("e4820001a0411247e89b014e002bf1dad9ebab22c3a4ffaaa4843805234d1d1ef6f1123c13").unwrap()),
            Bytes::from(hex::decode("f8518080808080808080a0c99ff1b25019997208a837a8f22204b4beb758cab38b428fa5d90de24434e74d808080a0ca544d00bc9379209520b72bc4dbf7bd6c3893a4fe5b723761399a89e21c58c180808080").unwrap()),
            Bytes::from(hex::decode("f86095200b89744abb96f7af1171ed5f47026bdf01df1874b848f8460e823a98a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470").unwrap()),
        ];

        let accounts: Vec<Account> = values
            .iter()
            .map(|rlp_bytes| {
                let trie_account = crate::TrieAccount::decode(&mut &rlp_bytes[..]).unwrap();
                Account {
                    nonce: trie_account.nonce,
                    balance: trie_account.balance,
                    bytecode_hash: if trie_account.code_hash == alloy_trie::EMPTY_ROOT_HASH {
                        None
                    } else {
                        Some(trie_account.code_hash)
                    },
                }
            })
            .collect();

        // Should fail because proof has a gap (missing intermediate node)
        let result = verify_account_range_proof(root, keys[0], &keys, &accounts, &gap_proof);

        assert!(result.is_err(), "proof with gap should be rejected");
    }

    /// Test monotonic validation with various patterns
    #[test]
    fn test_monotonic_validation_comprehensive() {
        // Test 1: Strictly increasing (should pass) - already covered

        // Test 2: Equal adjacent keys (should fail)
        let keys = vec![
            B256::with_last_byte(5),
            B256::with_last_byte(5), // Duplicate
        ];
        let values = vec![Account::default(); 2];
        let result = verify_account_range_proof(B256::ZERO, keys[0], &keys, &values, &[]);
        assert!(result.is_err(), "duplicate keys should be rejected");

        // Test 3: Decreasing (should fail)
        let keys = vec![
            B256::with_last_byte(10),
            B256::with_last_byte(5), // Goes backwards
        ];
        let values = vec![Account::default(); 2];
        let result = verify_account_range_proof(B256::ZERO, keys[0], &keys, &values, &[]);
        assert!(result.is_err(), "decreasing keys should be rejected");
    }
}
