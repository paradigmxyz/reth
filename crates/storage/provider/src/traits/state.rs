use super::AccountProvider;
use crate::{post_state::PostState, BlockHashProvider};
use auto_impl::auto_impl;
use reth_interfaces::{provider::ProviderError, Result};
use reth_primitives::{
    Address, BlockHash, BlockId, BlockNumHash, BlockNumber, BlockNumberOrTag, Bytecode, Bytes,
    StorageKey, StorageValue, H256, KECCAK_EMPTY, U256,
};

/// Type alias of boxed [StateProvider].
pub type StateProviderBox<'a> = Box<dyn StateProvider + 'a>;

/// An abstraction for a type that provides state data.
#[auto_impl(&, Arc, Box)]
pub trait StateProvider:
    BlockHashProvider + AccountProvider + StateRootProvider + Send + Sync
{
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
/// `BlockNumber`, the latest state, or the pending state.
///
/// This type differentiates states into `historical`, `latest` and `pending`, where the `latest`
/// block determines what is historical or pending: `[historical..latest..pending]`.
///
/// The `latest` state represents the state after the most recent block has been committed to the
/// database, `historical` states are states that have been committed to the database before the
/// `latest` state, and `pending` states are states that have not yet been committed to the
/// database which may or may not become the `latest` state, depending on consensus.
///
/// Note: the `pending` block is considered the block that extends the canonical chain but one and
/// has the `latest` block as its parent.
///
/// All states are _inclusive_, meaning they include _all_ all changes made (executed transactions)
/// in their respective blocks. For example [StateProviderFactory::history_by_block_number] for
/// block number `n` will return the state after block `n` was executed (transactions, withdrawals).
/// In other words, all states point to the end of the state's respective block, which is equivalent
/// to state at the beginning of the child block.
///
/// This affects tracing, or replaying blocks, which will need to be executed on top of the state of
/// the parent block. For example, in order to trace block `n`, the state after block `n - 1` needs
/// to be used, since block `n` was executed on its parent block's state.
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

    /// Returns a historical [StateProvider] indexed by the given historic block number.
    ///
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_number(&self, block: BlockNumber) -> Result<StateProviderBox<'_>>;

    /// Returns a historical [StateProvider] indexed by the given block hash.
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_hash(&self, block: BlockHash) -> Result<StateProviderBox<'_>>;

    /// Returns _any_[StateProvider] with matching block hash.
    ///
    /// This will return a [StateProvider] for either a historical or pending block.
    fn state_by_block_hash(&self, block: BlockHash) -> Result<StateProviderBox<'_>>;

    /// Storage provider for pending state.
    ///
    /// Represents the state at the block that extends the canonical chain by one.
    /// If there's no `pending` block, then this is equal to [StateProviderFactory::latest]
    fn pending(&self) -> Result<StateProviderBox<'_>>;

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
    ) -> Result<Box<dyn PostStateDataProvider>> {
        Ok(self
            .find_pending_state_provider(block_hash)
            .ok_or(ProviderError::UnknownBlockHash(block_hash))?)
    }

    /// Returns state provider if a matching block exists.
    fn find_pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Option<Box<dyn PostStateDataProvider>>;
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

/// A type that can compute the state root of a given post state.
#[auto_impl[Box,&, Arc]]
pub trait StateRootProvider: Send + Sync {
    /// Returns the state root of the PostState on top of the current state.
    fn state_root(&self, post_state: PostState) -> Result<H256>;
}
