use super::AccountReader;
use crate::{
    BlockHashReader, BlockIdReader, BundleStateWithReceipts, HashedAccountReader, StateRootProvider,
};
use auto_impl::auto_impl;
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_primitives::{
    trie::AccountProof, Address, BlockHash, BlockId, BlockNumHash, BlockNumber, BlockNumberOrTag,
    Bytecode, StorageKey, StorageValue, B256, KECCAK_EMPTY, U256,
};

/// Type alias of boxed [StateProvider].
pub type StateProviderBox = Box<dyn StateProvider>;

/// An abstraction for a type that provides state data.
#[auto_impl(&, Arc, Box)]
pub trait StateProvider:
    BlockHashReader + AccountReader + BytecodeProvider + StateRootProvider + Send + Sync
{
    /// Get storage of given account.
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>>;

    /// Get account code by its address.
    ///
    /// Returns `None` if the account doesn't exist or account is not a contract
    fn account_code(&self, address: Address) -> ProviderResult<Option<Bytecode>> {
        // Get basic account information.
        // Returns `None` if account doesn't exist.
        let Some(account) = self.basic_account(address)? else { return Ok(None) };

        if let Some(code_hash) = account.bytecode_hash.filter(|hash| hash != &KECCAK_EMPTY) {
            self.bytecode_by_hash(code_hash)
        } else {
            Ok(None)
        }
    }

    /// Get account balance by its address.
    ///
    /// Returns `None` if the account doesn't exist.
    fn account_balance(&self, address: Address) -> ProviderResult<Option<U256>> {
        Ok(self.basic_account(address)?.map(|a| a.balance))
    }

    /// Get account nonce by its address.
    ///
    /// Returns `None` if the account doesn't exist.
    fn account_nonce(&self, address: Address) -> ProviderResult<Option<u64>> {
        Ok(self.basic_account(address)?.map(|a| a.nonce))
    }

    /// Get account and storage proofs.
    fn proof(&self, address: Address, keys: &[B256]) -> ProviderResult<AccountProof>;
}

/// An abstraction for a type that provides state data for accounts by their hashed address.
#[auto_impl(&, Arc, Box)]
pub trait HashedStateProvider:
    BlockHashReader + HashedAccountReader + BytecodeProvider + Send + Sync
{
    /// Get storage of given account.
    fn hashed_storage(
        &self,
        hashed_address: B256,
        hashed_storage_key: B256,
    ) -> ProviderResult<Option<StorageValue>>;

    /// Get account code by its hashed address.
    ///
    /// Returns `None` if the account doesn't exist or account is not a contract
    fn hashed_account_code(&self, hashed_address: B256) -> ProviderResult<Option<Bytecode>> {
        // Get basic account information
        // Returns `None` if account doesn't exist
        let Some(account) = self.basic_hashed_account(hashed_address)? else { return Ok(None) };

        if let Some(code_hash) = account.bytecode_hash.filter(|hash| hash != &KECCAK_EMPTY) {
            self.bytecode_by_hash(code_hash)
        } else {
            Ok(None)
        }
    }

    /// Get account balance by its hashed address.
    ///
    /// Returns `None` if the account doesn't exist
    fn hashed_account_balance(&self, hashed_address: B256) -> ProviderResult<Option<U256>> {
        Ok(self.basic_hashed_account(hashed_address)?.map(|a| a.balance))
    }

    /// Get account nonce by its hashed address.
    ///
    /// Returns `None` if the account doesn't exist
    fn hashed_account_nonce(&self, hashed_address: B256) -> ProviderResult<Option<u64>> {
        Ok(self.basic_hashed_account(hashed_address)?.map(|a| a.nonce))
    }
}

/// An abstraction for a type that provides bytecode by its hash.
#[auto_impl(&, Arc, Box)]
pub trait BytecodeProvider {
    /// Get bytecode by its hash.
    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>>;
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
pub trait StateProviderFactory: BlockIdReader + Send + Sync {
    /// Storage provider for latest block.
    fn latest(&self) -> ProviderResult<StateProviderBox>;

    /// Returns a [StateProvider] indexed by the given [BlockId].
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
    ) -> ProviderResult<StateProviderBox> {
        match number_or_tag {
            BlockNumberOrTag::Latest => self.latest(),
            BlockNumberOrTag::Finalized => {
                // we can only get the finalized state by hash, not by num
                let hash = match self.finalized_block_hash()? {
                    Some(hash) => hash,
                    None => return Err(ProviderError::FinalizedBlockNotFound),
                };
                // only look at historical state
                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Safe => {
                // we can only get the safe state by hash, not by num
                let hash = match self.safe_block_hash()? {
                    Some(hash) => hash,
                    None => return Err(ProviderError::SafeBlockNotFound),
                };

                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Earliest => self.history_by_block_number(0),
            BlockNumberOrTag::Pending => self.pending(),
            BlockNumberOrTag::Number(num) => {
                // Note: The `BlockchainProvider` could also lookup the tree for the given block number, if for example the block number is `latest + 1`, however this should only support canonical state: <https://github.com/paradigmxyz/reth/issues/4515>
                self.history_by_block_number(num)
            }
        }
    }

    /// Returns a historical [StateProvider] indexed by the given historic block number.
    ///
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_number(&self, block: BlockNumber) -> ProviderResult<StateProviderBox>;

    /// Returns a historical [StateProvider] indexed by the given block hash.
    ///
    /// Note: this only looks at historical blocks, not pending blocks.
    fn history_by_block_hash(&self, block: BlockHash) -> ProviderResult<StateProviderBox>;

    /// Returns _any_[StateProvider] with matching block hash.
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

    /// Return a [StateProvider] that contains bundle state data provider.
    /// Used to inspect or execute transaction on the pending state.
    fn pending_with_provider(
        &self,
        bundle_state_data: Box<dyn BundleStateDataProvider>,
    ) -> ProviderResult<StateProviderBox>;
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
    ) -> ProviderResult<Box<dyn BundleStateDataProvider>> {
        self.find_pending_state_provider(block_hash)
            .ok_or(ProviderError::StateForHashNotFound(block_hash))
    }

    /// Returns state provider if a matching block exists.
    fn find_pending_state_provider(
        &self,
        block_hash: BlockHash,
    ) -> Option<Box<dyn BundleStateDataProvider>>;
}

/// Post state data needs for execution on it.
/// This trait is used to create a state provider over pending state.
///
/// Pending state contains:
/// * [`BundleStateWithReceipts`] contains all changed of accounts and storage of pending chain
/// * block hashes of pending chain and canonical blocks.
/// * canonical fork, the block on what pending chain was forked from.
#[auto_impl(&, Box)]
pub trait BundleStateDataProvider: Send + Sync {
    /// Return post state
    fn state(&self) -> &BundleStateWithReceipts;
    /// Return block hash by block number of pending or canonical chain.
    fn block_hash(&self, block_number: BlockNumber) -> Option<BlockHash>;
    /// return canonical fork, the block on what post state was forked from.
    ///
    /// Needed to create state provider.
    fn canonical_fork(&self) -> BlockNumHash;
}
