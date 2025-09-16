//! Linked Partial Merkle Patricia Trie optimized for stateless execution in zkvm

use crate::mpt::MptNode;
use alloy_primitives::{keccak256, map::B256Map, Address, Bytes, B256, U256};
use alloy_trie::{TrieAccount, EMPTY_ROOT_HASH};
use itertools::Itertools;
use reth_stateless::{validation::StatelessValidationError, ExecutionWitness, StatelessTrie};
use reth_storage_errors::ProviderError;
use reth_trie_common::HashedPostState;
use revm_bytecode::Bytecode;

mod execution_witness;
mod mpt;

/// A partial trie that can be updated
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LinkedSparseTrie {
    /// The state trie
    pub state_trie: MptNode,
    /// The storage tries, keyed by the keccak256 hash of the account address
    pub storage_tries: B256Map<MptNode>,
}

impl LinkedSparseTrie {
    /// Create a partial state trie from a previous state root and a list of RLP-encoded MPT nodes
    pub fn new<'a, I>(
        prev_state_root: B256,
        states: I,
    ) -> Result<LinkedSparseTrie, StatelessValidationError>
    where
        I: IntoIterator<Item = &'a Bytes>,
    {
        let (state_trie, storage_tries) =
            execution_witness::build_validated_tries(prev_state_root, states)?;

        Ok(LinkedSparseTrie { state_trie, storage_tries })
    }

    /// Get account by address
    pub fn account(&self, address: Address) -> Result<Option<TrieAccount>, ProviderError> {
        let hashed_address = keccak256(address);
        let account = self.state_trie.get_rlp::<TrieAccount>(&*hashed_address)?;

        Ok(account)
    }

    /// Get storage value of an account at a specific slot.
    pub fn storage(&self, address: Address, slot: U256) -> Result<U256, ProviderError> {
        let hashed_address = keccak256(address);

        // Usual case, where given storage slot is present.
        if let Some(storage_trie) = self.storage_tries.get(&hashed_address) {
            let key = keccak256(slot.to_be_bytes::<32>());
            let value = storage_trie.get_rlp::<U256>(&*key)?.unwrap_or_default();
            return Ok(value);
        }

        // Storage slot value is not present in the trie, validate that the witness is complete.
        // FIXME: do we need to validate that the storage trie is empty? seems get_rlp has already
        // proven that.
        let account = self.state_trie.get_rlp::<TrieAccount>(&*hashed_address)?;
        match account {
            Some(account) => {
                if account.storage_root != EMPTY_ROOT_HASH {
                    // if an account has a non-empty storage root, then during the building of the
                    // trie, we already failed if the storage trie was not present.
                    unreachable!("pre-built storage trie shall be present");
                }
            }
            None => {
                todo!("Validate that account witness is valid");
            }
        }

        // Account doesn't exist or has empty storage root.
        Ok(U256::ZERO)
    }

    /// Mutates state based on diffs provided in [`HashedPostState`].
    pub fn calculate_state_root(
        &mut self,
        state: HashedPostState,
    ) -> Result<B256, StatelessValidationError> {
        let err_fn = |_e| StatelessValidationError::StatelessStateRootCalculationFailed;
        // 1. Apply storage‑slot updates and compute each contract’s storage root
        //
        //
        // We walk over every (address, storage) pair in deterministic order
        // and update the corresponding per‑account storage trie in‑place.
        for (address, storage) in
            state.storages.into_iter().sorted_unstable_by_key(|(addr, _)| *addr)
        {
            let storage_trie = self.storage_tries.entry(address).or_default();
            if storage.wiped {
                storage_trie.clear();
            }

            // Apply slot‑level changes
            for (hashed_slot, value) in
                storage.storage.into_iter().sorted_unstable_by_key(|(slot, _)| *slot)
            {
                if value.is_zero() {
                    storage_trie.delete(&*hashed_slot).map_err(err_fn)?;
                } else {
                    storage_trie.insert_rlp(&*hashed_slot, value).map_err(err_fn)?;
                }
            }
        }

        for (hashed_address, account) in
            state.accounts.into_iter().sorted_unstable_by_key(|(addr, _)| *addr)
        {
            let Some(account) = account else {
                self.state_trie.delete(&*hashed_address).map_err(err_fn)?;
                continue;
            };

            // Determine which storage root should be used for this account
            let storage_root = if let Some(storage_trie) = self.storage_tries.get(&hashed_address) {
                storage_trie.hash()
            } else if let Some(value) =
                self.state_trie.get_rlp::<TrieAccount>(&*hashed_address).map_err(err_fn)?
            {
                value.storage_root
            } else {
                EMPTY_ROOT_HASH
            };

            let account = account.into_trie_account(storage_root);
            self.state_trie.insert_rlp(&*hashed_address, account).map_err(err_fn)?;
        }

        Ok(self.state_trie.hash())
    }
}

impl StatelessTrie for LinkedSparseTrie {
    fn new(
        witness: &ExecutionWitness,
        pre_state_root: B256,
    ) -> Result<(Self, B256Map<Bytecode>), StatelessValidationError>
    where
        Self: Sized,
    {
        let mut bytecode = B256Map::default();
        for code in witness.codes.iter() {
            let hash = keccak256(code);
            bytecode.insert(hash, Bytecode::new_raw(code.clone()));
        }

        let trie = Self::new(pre_state_root, &witness.state)?;
        Ok((trie, bytecode))
    }

    fn account(&self, address: Address) -> Result<Option<TrieAccount>, ProviderError> {
        LinkedSparseTrie::account(self, address)
    }

    fn storage(&self, address: Address, slot: U256) -> Result<U256, ProviderError> {
        LinkedSparseTrie::storage(self, address, slot)
    }

    fn calculate_state_root(
        &mut self,
        state: HashedPostState,
    ) -> Result<B256, StatelessValidationError> {
        LinkedSparseTrie::calculate_state_root(self, state)
    }
}
