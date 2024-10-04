use super::{
    AccountReader, BlockHashReader, BlockIdReader, StateProofProvider, StateRootProvider,
    StorageRootProvider,
};
use alloy_eips::{BlockId, BlockNumHash, BlockNumberOrTag};
use alloy_primitives::{Address, BlockHash, BlockNumber, StorageKey, StorageValue, B256, U256};
use auto_impl::auto_impl;
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{Bytecode, KECCAK_EMPTY};
use reth_storage_errors::provider::{ProviderError, ProviderResult};

/// Type alias of boxed [`StateProvider`].
pub type StateProviderBox = Box<dyn StateProvider>;

/// An abstraction for a type that provides state data.
#[auto_impl(&, Arc, Box)]
pub trait StateProvider:
    BlockHashReader
    + AccountReader
    + StateRootProvider
    + StorageRootProvider
    + StateProofProvider
    + Send
    + Sync
{
    /// Get storage of given account.
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>>;

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>>;

    /// Get account code by its address.
    ///
    /// Returns `None` if the account doesn't exist or account is not a contract
    fn account_code(&self, addr: Address) -> ProviderResult<Option<Bytecode>> {
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
    fn account_balance(&self, addr: Address) -> ProviderResult<Option<U256>> {
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
    fn account_nonce(&self, addr: Address) -> ProviderResult<Option<u64>> {
        // Get basic account information
        // Returns None if acc doesn't exist
        match self.basic_account(addr)? {
            Some(acc) => Ok(Some(acc.nonce)),
            None => Ok(None),
        }
    }
}

/// Trait implemented for database providers that can be converted into a historical state provider.
pub trait TryIntoHistoricalStateProvider {
    /// Returns a historical [`StateProvider`] indexed by the given historic block number.
    fn try_into_history_at_block(
        self,
        block_number: BlockNumber,
    ) -> ProviderResult<StateProviderBox>;
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
#[auto_impl(&, Arc, Box)]
pub trait StateProviderFactory: BlockIdReader + Send + Sync {
    /// Storage provider for latest block.
    fn latest(&self) -> ProviderResult<StateProviderBox>;

    /// Returns a [`StateProvider`] indexed by the given [`BlockId`].
    ///
    /// Note: if a number or hash is provided this will __only__ look at historical(canonical)
    /// state.
    fn state_by_block_id(&self, block_id: BlockId) -> ProviderResult<StateProviderBox> {
        match block_id {
            BlockId::Number(block_number) => self.state_by_block_number_or_tag(block_number),
            BlockId::Hash(block_hash) => self.history_by_block_hash(block_hash.into()),
        }
    }

    /// Returns a [StateProvider] indexed by the given block number or tag.
    ///
    /// Note: if a number is provided this will only look at historical(canonical) state.
    fn state_by_block_number_or_tag(
        &self,
        number_or_tag: BlockNumberOrTag,
    ) -> ProviderResult<StateProviderBox>;

    /// Returns a historical [StateProvider] indexed by the given historic block number.
    ///
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_number(&self, block: BlockNumber) -> ProviderResult<StateProviderBox>;

    /// Returns a historical [StateProvider] indexed by the given block hash.
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox>;

    /// Returns _any_ [StateProvider] with matching block hash.
    ///
    /// This will return a [StateProvider] for either a historical or pending block.
    fn state_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox>;

    /// Storage provider for pending state.
    ///
    /// Represents the state at the block that extends the canonical chain by one.
    /// If there's no `pending` block, then this is equal to [StateProviderFactory::latest]
    fn pending(&self) -> ProviderResult<StateProviderBox>;

    /// Storage provider for pending state for the given block hash.
    ///
    /// Represents the state at the block that extends the canonical chain.
    ///
    /// If the block couldn't be found, returns `None`.
    fn pending_state_by_hash(&self, block_hash: B256) -> ProviderResult<Option<StateProviderBox>>;
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
    ) -> ProviderResult<Box<dyn FullExecutionDataProvider>> {
        self.find_pending_state_provider(block_hash)
            .ok_or(ProviderError::StateForHashNotFound(block_hash))
    }

    /// Returns state provider if a matching block exists.
    fn find_pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Option<Box<dyn FullExecutionDataProvider>>;
}

/// Provides data required for post-block execution.
///
/// This trait offers methods to access essential post-execution data, including the state changes
/// in accounts and storage, as well as block hashes for both the pending and canonical chains.
///
/// The trait includes:
/// * [`ExecutionOutcome`] - Captures all account and storage changes in the pending chain.
/// * Block hashes - Provides access to the block hashes of both the pending chain and canonical
///   blocks.
#[auto_impl(&, Box)]
pub trait ExecutionDataProvider: Send + Sync {
    /// Return the execution outcome.
    fn execution_outcome(&self) -> &ExecutionOutcome;
    /// Return block hash by block number of pending or canonical chain.
    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash>;
}

impl ExecutionDataProvider for ExecutionOutcome {
    fn execution_outcome(&self) -> &ExecutionOutcome {
        self
    }

    /// Always returns [None] because we don't have any information about the block header.
    fn block_hash(&self, _block_number: BlockNumber) -> Option<BlockHash> {
        None
    }
}

/// Fork data needed for execution on it.
///
/// It contains a canonical fork, the block on what pending chain was forked from.
#[auto_impl(&, Box)]
pub trait BlockExecutionForkProvider {
    /// Return canonical fork, the block on what post state was forked from.
    ///
    /// Needed to create state provider.
    fn canonical_fork(&self) -> BlockNumHash;
}

/// Provides comprehensive post-execution state data required for further execution.
///
/// This trait is used to create a state provider over the pending state and is a combination of
/// [`ExecutionDataProvider`] and [`BlockExecutionForkProvider`].
///
/// The pending state includes:
/// * `ExecutionOutcome`: Contains all changes to accounts and storage within the pending chain.
/// * Block hashes: Represents hashes of both the pending chain and canonical blocks.
/// * Canonical fork: Denotes the block from which the pending chain forked.
pub trait FullExecutionDataProvider: ExecutionDataProvider + BlockExecutionForkProvider {}

impl<T> FullExecutionDataProvider for T where T: ExecutionDataProvider + BlockExecutionForkProvider {}
