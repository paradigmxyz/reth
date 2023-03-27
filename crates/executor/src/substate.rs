//! Substate for blockchain trees

use reth_interfaces::{provider::ProviderError, Result};
use reth_primitives::{Account, Address, BlockHash, BlockNumber, Bytecode, Bytes, H256, U256};
use reth_provider::{
    post_state::PostState, AccountProvider, BlockHashProvider, PostStateDataProvider, StateProvider,
};
use std::collections::BTreeMap;

use crate::blockchain_tree::chain::ForkBlock;

/// A state provider that either resolves to data in a wrapped [`PostState`], or an underlying state
/// provider.
pub struct PostStateProvider<SP: StateProvider, PSDP: PostStateDataProvider> {
    /// The inner state provider.
    pub state_provider: SP,
    /// Post state data,
    pub post_state_data_provider: PSDP,
}

/// Structure that bundles references of data needs to implement [`PostStateDataProvider`]
#[derive(Clone, Debug)]
pub struct PostStateDataRef<'a> {
    /// The wrapped state after execution of one or more transactions and/or blocks.
    pub state: &'a PostState,
    /// The blocks in the sidechain.
    pub sidechain_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    /// The blocks in the canonical chain.
    pub canonical_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    /// Canonical fork
    pub canonical_fork: ForkBlock,
}

impl<'a> PostStateDataProvider for PostStateDataRef<'a> {
    fn state(&self) -> &PostState {
        self.state
    }

    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash> {
        let block_hash = self.sidechain_block_hashes.get(&block_number).cloned();
        if block_hash.is_some() {
            return block_hash
        }

        self.canonical_block_hashes.get(&block_number).cloned()
    }

    fn canonical_fork(&self) -> ForkBlock {
        self.canonical_fork
    }
}

/// Structure that contains data needs to implement [`PostStateDataProvider`]
#[derive(Clone, Debug)]
pub struct PostStateData {
    /// Post state with changes
    pub state: PostState,
    /// Parent block hashes needs for evm BLOCKHASH opcode.
    /// NOTE: it does not mean that all hashes are there but all until finalized are there.
    /// Other hashes can be obtained from provider
    pub parent_block_hashed: BTreeMap<BlockNumber, BlockHash>,
    /// Canonical block where state forked from.
    pub canonical_fork: ForkBlock,
}

impl PostStateDataProvider for PostStateData {
    fn state(&self) -> &PostState {
        &self.state
    }

    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash> {
        self.parent_block_hashed.get(&block_number).cloned()
    }

    fn canonical_fork(&self) -> ForkBlock {
        self.canonical_fork
    }
}

impl<SP: StateProvider, PSDP: PostStateDataProvider> PostStateProvider<SP, PSDP> {
    /// Create new post-state provider
    pub fn new(state_provider: SP, post_state_data_provider: PSDP) -> Self {
        Self { state_provider, post_state_data_provider }
    }
}

/* Implement StateProvider traits */

impl<SP: StateProvider, PSDP: PostStateDataProvider> BlockHashProvider
    for PostStateProvider<SP, PSDP>
{
    fn block_hash(&self, block_number: BlockNumber) -> Result<Option<H256>> {
        let block_hash = self.post_state_data_provider.block_hash(block_number);
        if block_hash.is_some() {
            return Ok(block_hash)
        }
        self.state_provider.block_hash(block_number)
    }

    fn canonical_hashes_range(&self, _start: BlockNumber, _end: BlockNumber) -> Result<Vec<H256>> {
        unimplemented!()
    }
}

impl<SP: StateProvider, PSDP: PostStateDataProvider> AccountProvider
    for PostStateProvider<SP, PSDP>
{
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        if let Some(account) = self.post_state_data_provider.state().account(&address) {
            Ok(*account)
        } else {
            self.state_provider.basic_account(address)
        }
    }
}

impl<SP: StateProvider, PSDP: PostStateDataProvider> StateProvider for PostStateProvider<SP, PSDP> {
    fn storage(
        &self,
        account: Address,
        storage_key: reth_primitives::StorageKey,
    ) -> Result<Option<reth_primitives::StorageValue>> {
        if let Some(storage) = self.state.account_storage(&account) {
            if let Some(value) =
                storage.storage.get(&U256::from_be_bytes(storage_key.to_fixed_bytes()))
            {
                return Ok(Some(*value))
            } else if storage.wiped {
                return Ok(Some(U256::ZERO))
            }
        }

        self.state_provider.storage(account, storage_key)
    }

    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytecode>> {
        if let Some(bytecode) = self.post_state_data_provider.state().bytecode(&code_hash).cloned()
        {
            return Ok(Some(bytecode))
        }

        self.state_provider.bytecode_by_hash(code_hash)
    }

    fn proof(
        &self,
        _address: Address,
        _keys: &[H256],
    ) -> Result<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)> {
        Err(ProviderError::HistoryStateRoot.into())
    }
}
