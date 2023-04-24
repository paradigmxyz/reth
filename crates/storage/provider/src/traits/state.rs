use super::AccountProvider;
use crate::{post_state::PostState, BlockHashProvider};
use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{
    Address, BlockHash, BlockId, BlockNumHash, BlockNumber, BlockNumberOrTag, Bytecode, Bytes,
    StorageKey, StorageValue, H256, KECCAK_EMPTY, U256,
};

/// Type alias of boxed [StateProvider].
pub type StateProviderBox<'a> = Box<dyn StateProvider + 'a>;

/// An abstraction for a type that provides state data.
#[auto_impl(&, Arc, Box)]
pub trait StateProvider: BlockHashProvider + AccountProvider + Send + Sync {
    /// Get storage of given account.
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>>;

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytecode>>;

    /// Get account and storage proofs.
    fn proof(&self, address: Address, keys: &[H256])
        -> Result<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)>;

    /// Get account code by its address.
    ///
    /// Returns `None` if the account doesn't exist or account is not a contract
    fn account_code(&self, addr: Address) -> Result<Option<Bytecode>> {
        // Get basic account information
        // Returns None if acc doesn't exist
        let acc = match self.basic_account(addr)? {
            Some(acc) => acc,
            None => return Ok(None),
        };

        if let Some(code_hash) = acc.bytecode_hash {
            if code_hash == KECCAK_EMPTY {
                return Ok(None)
            }
            // Get the code from the code hash
            return self.bytecode_by_hash(code_hash)
        }

        // Return `None` if no code hash is set
        Ok(None)
    }

    /// Get account balance by its address.
    ///
    /// Returns `None` if the account doesn't exist
    fn account_balance(&self, addr: Address) -> Result<Option<U256>> {
        // Get basic account information
        // Returns None if acc doesn't exist
        match self.basic_account(addr)? {
            Some(acc) => Ok(Some(acc.balance)),
            None => Ok(None),
        }
    }

    /// Get account nonce by its address.
    ///
    /// Returns `None` if the account doesn't exist
    fn account_nonce(&self, addr: Address) -> Result<Option<u64>> {
        // Get basic account information
        // Returns None if acc doesn't exist
        match self.basic_account(addr)? {
            Some(acc) => Ok(Some(acc.nonce)),
            None => Ok(None),
        }
    }
}

/// Light wrapper that returns `StateProvider` implementations that correspond to the given
/// `BlockNumber` or the latest state.
pub trait StateProviderFactory: Send + Sync {
    /// Storage provider for latest block.
    fn latest(&self) -> Result<StateProviderBox<'_>>;

    /// Returns a [StateProvider] indexed by the given [BlockId].
    fn state_by_block_id(&self, block_id: BlockId) -> Result<StateProviderBox<'_>> {
        match block_id {
            BlockId::Number(block_number) => self.history_by_block_number_or_tag(block_number),
            BlockId::Hash(block_hash) => self.history_by_block_hash(block_hash.into()),
        }
    }

    /// Returns a [StateProvider] indexed by the given block number or tag
    fn history_by_block_number_or_tag(
        &self,
        number_or_tag: BlockNumberOrTag,
    ) -> Result<StateProviderBox<'_>> {
        match number_or_tag {
            BlockNumberOrTag::Latest => self.latest(),
            BlockNumberOrTag::Finalized => {
                todo!()
            }
            BlockNumberOrTag::Safe => {
                todo!()
            }
            BlockNumberOrTag::Earliest => self.history_by_block_number(0),
            BlockNumberOrTag::Pending => self.pending(),
            BlockNumberOrTag::Number(num) => self.history_by_block_number(num),
        }
    }

    /// Returns a [StateProvider] indexed by the given block number.
    fn history_by_block_number(&self, block: BlockNumber) -> Result<StateProviderBox<'_>>;

    /// Returns a [StateProvider] indexed by the given block hash.
    fn history_by_block_hash(&self, block: BlockHash) -> Result<StateProviderBox<'_>>;

    /// Storage provider for pending state.
    fn pending(&self) -> Result<StateProviderBox<'_>> {
        todo!()
    }

    /// Return a [StateProvider] that contains post state data provider.
    /// Used to inspect or execute transaction on the pending state.
    fn pending_with_provider(
        &self,
        post_state_data: Box<dyn PostStateDataProvider>,
    ) -> Result<StateProviderBox<'_>>;
}

/// Blockchain trait provider that gives access to the blockchain state that is not yet committed
/// (pending).
pub trait BlockchainTreePendingStateProvider: Send + Sync {
    /// Returns a state provider that includes all state changes of the given (pending) block hash.
    ///
    /// In other words, the state provider will return the state after all transactions of the given
    /// hash have been executed.
    fn pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Result<Box<dyn PostStateDataProvider>>;
}

/// Post state data needs for execution on it.
/// This trait is used to create a state provider over pending state.
///
/// Pending state contains:
/// * [`PostState`] contains all changed of accounts and storage of pending chain
/// * block hashes of pending chain and canonical blocks.
/// * canonical fork, the block on what pending chain was forked from.
#[auto_impl[Box,&]]
pub trait PostStateDataProvider: Send + Sync {
    /// Return post state
    fn state(&self) -> &PostState;
    /// Return block hash by block number of pending or canonical chain.
    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash>;
    /// return canonical fork, the block on what post state was forked from.
    ///
    /// Needed to create state provider.
    fn canonical_fork(&self) -> BlockNumHash;
}
