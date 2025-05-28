use crate::validation::StatelessValidationError;
use alloc::{format, vec::Vec};
use alloy_primitives::{keccak256, map::B256Map, Address, B256, U256};
use alloy_rlp::{Decodable, Encodable};
use alloy_rpc_types_debug::ExecutionWitness;
use alloy_trie::{TrieAccount, EMPTY_ROOT_HASH};
use itertools::Itertools;
use reth_errors::ProviderError;
use reth_revm::state::Bytecode;
use reth_trie_common::{HashedPostState, Nibbles, TRIE_ACCOUNT_RLP_MAX_SIZE};
use reth_trie_sparse::{
    blinded::DefaultBlindedProviderFactory, errors::SparseStateTrieResult, SparseStateTrie,
    SparseTrie,
};

/// `StatelessTrie` structure for usage during stateless validation
#[derive(Debug)]
pub struct StatelessTrie {
    inner: SparseStateTrie,
}

impl StatelessTrie {
    /// Initialize the stateless trie using the `ExecutionWitness`
    ///
    /// Note: Currently this method does not check that the `ExecutionWitness`
    /// is complete for all of the preimage keys.
    pub fn new(
        witness: &ExecutionWitness,
        pre_state_root: B256,
    ) -> Result<(Self, B256Map<Bytecode>), StatelessValidationError> {
        verify_execution_witness(witness, pre_state_root)
            .map(|(inner, bytecode)| (Self { inner }, bytecode))
    }

    /// Returns the `TrieAccount` that corresponds to the `Address`
    ///
    /// This method will error if the `ExecutionWitness` is not able to guarantee
    /// that the account is missing from the Trie _and_ the witness was complete.
    pub fn account(&self, address: Address) -> Result<Option<TrieAccount>, ProviderError> {
        let hashed_address = keccak256(address);

        if let Some(bytes) = self.inner.get_account_value(&hashed_address) {
            let account = TrieAccount::decode(&mut bytes.as_slice())?;
            return Ok(Some(account))
        }

        if !self.inner.check_valid_account_witness(hashed_address) {
            return Err(ProviderError::TrieWitnessError(format!(
                "incomplete account witness for {hashed_address:?}"
            )));
        }

        Ok(None)
    }

    /// Returns the storage slot value that corresponds to the given (address, slot) tuple.
    ///
    /// This method will error if the `ExecutionWitness` is not able to guarantee
    /// that the storage was missing from the Trie _and_ the witness was complete.
    pub fn storage(&self, address: Address, slot: U256) -> Result<U256, ProviderError> {
        let hashed_address = keccak256(address);
        let hashed_slot = keccak256(B256::from(slot));

        if let Some(raw) = self.inner.get_storage_slot_value(&hashed_address, &hashed_slot) {
            return Ok(U256::decode(&mut raw.as_slice())?)
        }

        // Storage slot value is not present in the trie, validate that the witness is complete.
        // If the account exists in the trie...
        if let Some(bytes) = self.inner.get_account_value(&hashed_address) {
            // ...check that its storage is either empty or the storage trie was sufficiently
            // revealed...
            let account = TrieAccount::decode(&mut bytes.as_slice())?;
            if account.storage_root != EMPTY_ROOT_HASH &&
                !self.inner.check_valid_storage_witness(hashed_address, hashed_slot)
            {
                return Err(ProviderError::TrieWitnessError(format!(
                    "incomplete storage witness: prover must supply exclusion proof for slot {hashed_slot:?} in account {hashed_address:?}"
                )));
            }
        } else if !self.inner.check_valid_account_witness(hashed_address) {
            // ...else if account is missing, validate that the account trie was sufficiently
            // revealed.
            return Err(ProviderError::TrieWitnessError(format!(
                "incomplete account witness for {hashed_address:?}"
            )));
        }

        Ok(U256::ZERO)
    }

    /// Computes the new state root from the `HashedPostState`.
    pub fn calculate_state_root(
        &mut self,
        state: HashedPostState,
    ) -> Result<B256, StatelessValidationError> {
        calculate_state_root(&mut self.inner, state)
            .map_err(|_e| StatelessValidationError::StatelessStateRootCalculationFailed)
    }
}

/// Verifies execution witness [`ExecutionWitness`] against an expected pre-state root.
///
/// This function takes the RLP-encoded values provided in [`ExecutionWitness`]
/// (which includes state trie nodes, storage trie nodes, and contract bytecode)
/// and uses it to populate a new [`SparseStateTrie`].
///
/// If the computed root hash matches the `pre_state_root`, it signifies that the
/// provided execution witness is consistent with that pre-state root. In this case, the function
/// returns the populated [`SparseStateTrie`] and a [`B256Map`] containing the
/// contract bytecode (mapping code hash to [`Bytecode`]).
///
/// The bytecode has a separate mapping because the [`SparseStateTrie`] does not store the
/// contract bytecode, only the hash of it (code hash).
///
/// If the roots do not match, it returns `None`, indicating the witness is invalid
/// for the given `pre_state_root`.
// Note: This approach might be inefficient for ZKVMs requiring minimal memory operations, which
// would explain why they have for the most part re-implemented this function.
fn verify_execution_witness(
    witness: &ExecutionWitness,
    pre_state_root: B256,
) -> Result<(SparseStateTrie, B256Map<Bytecode>), StatelessValidationError> {
    let mut trie = SparseStateTrie::new(DefaultBlindedProviderFactory);
    let mut state_witness = B256Map::default();
    let mut bytecode = B256Map::default();

    for rlp_encoded in &witness.state {
        let hash = keccak256(rlp_encoded);
        state_witness.insert(hash, rlp_encoded.clone());
    }
    for rlp_encoded in &witness.codes {
        let hash = keccak256(rlp_encoded);
        bytecode.insert(hash, Bytecode::new_raw(rlp_encoded.clone()));
    }

    // Reveal the witness with our state root
    // This method builds a trie using the sparse trie using the state_witness with
    // the root being the pre_state_root.
    // Here are some things to note:
    // - You can pass in more witnesses than is needed for the block execution.
    // - If you try to get an account and it has not been seen. This means that the account
    // was not inserted into the Trie. It does not mean that the account does not exist.
    // In order to determine an account not existing, we must do an exclusion proof.
    trie.reveal_witness(pre_state_root, &state_witness)
        .map_err(|_e| StatelessValidationError::WitnessRevealFailed { pre_state_root })?;

    // Calculate the root
    let computed_root = trie
        .root()
        .map_err(|_e| StatelessValidationError::StatelessPreStateRootCalculationFailed)?;

    if computed_root == pre_state_root {
        Ok((trie, bytecode))
    } else {
        Err(StatelessValidationError::PreStateRootMismatch {
            got: computed_root,
            expected: pre_state_root,
        })
    }
}

// Copied and modified from ress: https://github.com/paradigmxyz/ress/blob/06bf2c4788e45b8fcbd640e38b6243e6f87c4d0e/crates/engine/src/tree/root.rs
/// Calculates the post-execution state root by applying state changes to a sparse trie.
///
/// This function takes a [`SparseStateTrie`] with the pre-state and a [`HashedPostState`]
/// containing account and storage changes resulting from block execution (state diff).
///
/// It modifies the input `trie` in place to reflect these changes and then calculates the
/// final post-execution state root.
fn calculate_state_root(
    trie: &mut SparseStateTrie,
    state: HashedPostState,
) -> SparseStateTrieResult<B256> {
    // 1. Apply storage‑slot updates and compute each contract’s storage root
    //
    //
    // We walk over every (address, storage) pair in deterministic order
    // and update the corresponding per‑account storage trie in‑place.
    // When we’re done we collect (address, updated_storage_trie) in a `Vec`
    // so that we can insert them back into the outer state trie afterwards ― this avoids
    // borrowing issues.
    let mut storage_results = Vec::with_capacity(state.storages.len());

    for (address, storage) in state.storages.into_iter().sorted_unstable_by_key(|(addr, _)| *addr) {
        // Take the existing storage trie (or create an empty, “revealed” one)
        let mut storage_trie =
            trie.take_storage_trie(&address).unwrap_or_else(SparseTrie::revealed_empty);

        if storage.wiped {
            storage_trie.wipe()?;
        }

        // Apply slot‑level changes
        for (hashed_slot, value) in
            storage.storage.into_iter().sorted_unstable_by_key(|(slot, _)| *slot)
        {
            let nibbles = Nibbles::unpack(hashed_slot);
            if value.is_zero() {
                storage_trie.remove_leaf(&nibbles)?;
            } else {
                storage_trie.update_leaf(nibbles, alloy_rlp::encode_fixed_size(&value).to_vec())?;
            }
        }

        // Finalise the storage‑trie root before pushing the result
        storage_trie.root();
        storage_results.push((address, storage_trie));
    }

    // Insert every updated storage trie back into the outer state trie
    for (address, storage_trie) in storage_results {
        trie.insert_storage_trie(address, storage_trie);
    }

    // 2. Apply account‑level updates and (re)encode the account nodes
    // Update accounts with new values
    // TODO: upstream changes into reth so that `SparseStateTrie::update_account` handles this
    let mut account_rlp_buf = Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE);

    for (hashed_address, account) in
        state.accounts.into_iter().sorted_unstable_by_key(|(addr, _)| *addr)
    {
        let nibbles = Nibbles::unpack(hashed_address);
        let account = account.unwrap_or_default();

        // Determine which storage root should be used for this account
        let storage_root = if let Some(storage_trie) = trie.storage_trie_mut(&hashed_address) {
            storage_trie.root()
        } else if let Some(value) = trie.get_account_value(&hashed_address) {
            TrieAccount::decode(&mut &value[..])?.storage_root
        } else {
            EMPTY_ROOT_HASH
        };

        // Decide whether to remove or update the account leaf
        if account.is_empty() && storage_root == EMPTY_ROOT_HASH {
            trie.remove_account_leaf(&nibbles)?;
        } else {
            account_rlp_buf.clear();
            account.into_trie_account(storage_root).encode(&mut account_rlp_buf);
            trie.update_account_leaf(nibbles, account_rlp_buf.clone())?;
        }
    }

    // Return new state root
    trie.root()
}
