//! This is copied and modified from https://github.com/succinctlabs/rsp
//! crates/mpt/src/execution_witness.rs rev@2a99f35a9b81452eb53af3848e50addfd481363c
//! Under MIT license

use crate::mpt::{resolve_nodes, MptNode, MptNodeData, MptNodeReference};
use alloy_primitives::{
    keccak256,
    map::{B256Map, HashMap},
    Bytes, B256,
};
use alloy_rlp::Decodable;
use alloy_trie::TrieAccount;
use reth_stateless::validation::StatelessValidationError;

// Builds tries from the witness state.
//
// NOTE: This method should be called outside zkVM! In general, you construct tries, then
// validate them inside zkVM.
pub(crate) fn build_validated_tries<'a, I>(
    pre_state_root: B256,
    states: I,
) -> Result<(MptNode, B256Map<MptNode>), StatelessValidationError>
where
    I: IntoIterator<Item = &'a Bytes>,
{
    let err_fn = |_e| StatelessValidationError::WitnessRevealFailed { pre_state_root };

    // Step 1: Decode all RLP-encoded trie nodes and index by hash
    // IMPORTANT: Witness state contains both *state trie* nodes and *storage tries* nodes!
    let mut node_map = HashMap::<MptNodeReference, MptNode>::default();
    let mut node_by_hash = B256Map::<MptNode>::default();
    let mut root_node: Option<MptNode> = None;

    for encoded in states.into_iter() {
        let node = MptNode::decode(&mut encoded.as_ref()).map_err(err_fn)?;
        let hash = keccak256(encoded);
        if hash == pre_state_root {
            root_node = Some(node.clone());
        }
        node_by_hash.insert(hash, node.clone());
        node_map.insert(node.reference(), node);
    }

    // Step 2: Use root_node or fallback to Digest
    let root = root_node.unwrap_or_else(|| MptNodeData::Digest(pre_state_root).into());

    // Build state trie.
    let mut raw_storage_tries = Vec::with_capacity(node_by_hash.len());
    let state_trie = resolve_nodes(&root, &node_map);

    state_trie.try_for_each_leaves(|key, mut value| {
        let account = TrieAccount::decode(&mut value).map_err(err_fn)?;
        let hashed_address = B256::from_slice(key);
        raw_storage_tries.push((hashed_address, account.storage_root));
        Ok::<_, StatelessValidationError>(())
    })?;

    // Step 3: Build storage tries per account efficiently
    let mut storage_tries =
        B256Map::<MptNode>::with_capacity_and_hasher(raw_storage_tries.len(), Default::default());

    for (hashed_address, storage_root) in raw_storage_tries {
        let root_node = match node_by_hash.get(&storage_root).cloned() {
            Some(node) => node,
            None => {
                // An execution witness can include an account leaf (with non-empty storageRoot),
                // but omit its entire storage trie when that account's storage was
                // NOT touched during the block.
                continue;
            }
        };
        let storage_trie = resolve_nodes(&root_node, &node_map);

        if storage_trie.is_digest() {
            return Err(StatelessValidationError::WitnessRevealFailed { pre_state_root });
        }

        // Insert resolved storage trie.
        storage_tries.insert(hashed_address, storage_trie);
    }

    // Step 3a: Verify that state_trie was built correctly - confirm tree hash with pre-state root.
    validate_state_trie(&state_trie, pre_state_root)?;

    // Step 3b: Verify that each storage trie matches the declared storage_root in the state trie.
    validate_storage_tries(pre_state_root, &state_trie, &storage_tries)?;

    Ok((state_trie, storage_tries))
}

// Validate that state_trie was built correctly - confirm tree hash with prev state root.
fn validate_state_trie(
    state_trie: &MptNode,
    pre_state_root: B256,
) -> Result<(), StatelessValidationError> {
    if state_trie.hash() != pre_state_root {
        return Err(StatelessValidationError::PreStateRootMismatch {
            got: state_trie.hash(),
            expected: pre_state_root,
        });
    }
    Ok(())
}

// Validates that each storage trie matches the declared storage_root in the state trie.
fn validate_storage_tries(
    pre_state_root: B256,
    state_trie: &MptNode,
    storage_tries: &B256Map<MptNode>,
) -> Result<(), StatelessValidationError> {
    for (address_hash, storage_trie) in storage_tries.iter() {
        let account = state_trie
            .get_rlp::<TrieAccount>(address_hash.as_slice())
            .map_err(|_e| StatelessValidationError::WitnessRevealFailed { pre_state_root })?
            .ok_or(StatelessValidationError::WitnessRevealFailed { pre_state_root })?;

        let expected = account.storage_root;
        let got = storage_trie.hash();

        if expected != got {
            return Err(StatelessValidationError::PreStorageRootMismatch {
                address_hash: *address_hash,
                expected,
                got,
            });
        }
    }

    Ok(())
}
