//! Applies finalized block output using plain address/slot preimages, never MPT hashed keys.

use crate::{AccountData, StateError, StateTree, StateUpdate, TreeHasher};
use alloy_primitives::{keccak256, Bytes, B256, KECCAK256_EMPTY};
use reth_trie_common::bundle_state::BundleState;

/// Applies one block's execution output atomically to a parent PBT state.
///
/// The parent must contain all accounts and code. New code is obtained from the output's
/// contract table; missing or inconsistent code fails the block without changing the parent.
/// The finalized output is authoritative on serial and parallel execution paths alike.
pub fn apply_bundle<H: TreeHasher>(
    parent: &StateTree<H>,
    bundle: &BundleState,
) -> Result<StateTree<H>, ExecutionError> {
    apply_bundle_with(parent, bundle, StateTree::apply_batch)
}

/// Applies block output with parallel hashing of dense PBT update batches.
/// Validation and parent-snapshot guarantees match [`apply_bundle`].
#[cfg(feature = "parallel")]
pub fn apply_bundle_parallel<H: TreeHasher>(
    parent: &StateTree<H>,
    bundle: &BundleState,
) -> Result<StateTree<H>, ExecutionError> {
    apply_bundle_with(parent, bundle, StateTree::apply_batch_parallel)
}

fn apply_bundle_with<H: TreeHasher>(
    parent: &StateTree<H>,
    bundle: &BundleState,
    apply: impl FnOnce(&mut StateTree<H>, Vec<StateUpdate>) -> Result<(), StateError>,
) -> Result<StateTree<H>, ExecutionError> {
    let mut state = parent.clone();
    let mut updates = Vec::new();
    for (address, account) in bundle.state() {
        if account.status.is_not_modified() {
            continue;
        }
        let Some(info) = &account.info else {
            updates.push(StateUpdate::Account { address: *address, account: None });
            continue;
        };
        let code = if info.code_hash == KECCAK256_EMPTY || info.code_hash.is_zero() {
            Bytes::new()
        } else if let Some(code) =
            info.code.as_ref().or_else(|| bundle.contracts.get(&info.code_hash))
        {
            let bytes = code.original_bytes();
            if keccak256(&bytes) != info.code_hash {
                return Err(ExecutionError::MissingCode(info.code_hash));
            }
            bytes
        } else if let Some(account) =
            parent.account(*address).filter(|account| keccak256(&account.code) == info.code_hash)
        {
            account.code.clone()
        } else {
            return Err(ExecutionError::MissingCode(info.code_hash));
        };
        if account.was_destroyed() {
            updates.push(StateUpdate::ClearStorage { address: *address });
        }
        let data = AccountData { nonce: info.nonce, balance: info.balance, code };
        let has_account = !data.is_empty();
        updates.push(StateUpdate::Account { address: *address, account: Some(data) });
        if has_account {
            for (slot, value) in &account.storage {
                updates.push(StateUpdate::Storage {
                    address: *address,
                    slot: *slot,
                    value: B256::from(value.present_value),
                });
            }
        }
    }
    apply(&mut state, updates)?;
    Ok(state)
}

/// Invalid or incomplete execution output.
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    /// The block refers to bytecode unavailable in the parent and output, or with a wrong hash.
    #[error("missing or inconsistent bytecode for {0}")]
    MissingCode(B256),
    /// Invalid Ethereum state encoding.
    #[error(transparent)]
    State(#[from] StateError),
}
