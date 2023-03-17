//! Substate for blockchain trees

use reth_interfaces::{provider::ProviderError, Result};
use reth_primitives::{Account, Address, BlockHash, BlockNumber, Bytecode, Bytes, H256, U256};
use reth_provider::{post_state::PostState, AccountProvider, BlockHashProvider, StateProvider};
use std::collections::BTreeMap;

/// A state provider that either resolves to data in a wrapped [`PostState`], or an underlying state
/// provider.
pub struct PostStateProvider<'a, SP: StateProvider> {
    /// The wrapped state after execution of one or more transactions and/or blocks.
    state: &'a PostState,
    /// The inner state provider.
    provider: SP,
    /// The blocks in the sidechain.
    sidechain_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    /// The blocks in the canonical chain.
    canonical_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
}

impl<'a, SP: StateProvider> PostStateProvider<'a, SP> {
    /// Create new post-state provider
    pub fn new(
        state: &'a PostState,
        provider: SP,
        sidechain_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    ) -> Self {
        Self { state, provider, sidechain_block_hashes, canonical_block_hashes }
    }
}

/* Implement StateProvider traits */

impl<'a, SP: StateProvider> BlockHashProvider for PostStateProvider<'a, SP> {
    fn block_hash(&self, block_number: BlockNumber) -> Result<Option<H256>> {
        if let Some(sidechain_block_hash) = self.sidechain_block_hashes.get(&block_number).cloned()
        {
            return Ok(Some(sidechain_block_hash))
        }

        Ok(Some(
            self.canonical_block_hashes
                .get(&block_number)
                .cloned()
                .ok_or(ProviderError::BlockchainTreeBlockHash { block_number })?,
        ))
    }

    fn canonical_hashes_range(&self, _start: BlockNumber, _end: BlockNumber) -> Result<Vec<H256>> {
        unimplemented!()
    }
}

impl<'a, SP: StateProvider> AccountProvider for PostStateProvider<'a, SP> {
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        if let Some(account) = self.state.account(&address) {
            Ok(*account)
        } else {
            self.provider.basic_account(address)
        }
    }
}

impl<'a, SP: StateProvider> StateProvider for PostStateProvider<'a, SP> {
    fn storage(
        &self,
        account: Address,
        storage_key: reth_primitives::StorageKey,
    ) -> Result<Option<reth_primitives::StorageValue>> {
        if let Some(storage) = self.state.account_storage(&account) {
            if storage.wiped {
                return Ok(Some(U256::ZERO))
            }

            if let Some(value) =
                storage.storage.get(&U256::from_be_bytes(storage_key.to_fixed_bytes()))
            {
                return Ok(Some(*value))
            }
        }

        self.provider.storage(account, storage_key)
    }

    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytecode>> {
        if let Some(bytecode) = self.state.bytecode(&code_hash).cloned() {
            return Ok(Some(bytecode))
        }

        self.provider.bytecode_by_hash(code_hash)
    }

    fn proof(
        &self,
        _address: Address,
        _keys: &[H256],
    ) -> Result<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)> {
        Err(ProviderError::HistoryStateRoot.into())
    }
}
