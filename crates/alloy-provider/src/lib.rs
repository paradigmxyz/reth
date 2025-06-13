//! # Alloy Provider for Reth
//!
//! This crate provides an implementation of reth's `StateProviderFactory` and related traits
//! that fetches state data via RPC instead of from a local database.
//!
//! Originally created by [cakevm](https://github.com/cakevm/alloy-reth-provider).
//!
//! ## Features
//!
//! - Implements `StateProviderFactory` for remote RPC state access
//! - Supports Ethereum networks
//! - Useful for testing without requiring a full database
//! - Can be used with reth ExEx (Execution Extensions) for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use alloy_primitives::{Address, BlockHash, BlockNumber, StorageKey, TxNumber, B256, U256};
use alloy_provider::Provider;
use alloy_rpc_types::BlockId;
use alloy_rpc_types_engine::ForkchoiceState;
use reth_chainspec::{ChainInfo, ChainSpecProvider};
use reth_db_api::mock::{DatabaseMock, TxMock};
use reth_errors::ProviderError;
use reth_execution_types::ExecutionOutcome;
use reth_node_types::{BlockTy, HeaderTy, NodeTypes, PrimitivesTy, ReceiptTy, TxTy};
use reth_primitives::{
    Account, Bytecode, RecoveredBlock, SealedBlock, SealedHeader, TransactionMeta,
};
use reth_provider::{
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader,
    BlockchainTreePendingStateProvider, CanonChainTracker, CanonStateNotification,
    CanonStateNotifications, CanonStateSubscriptions, ChainStateBlockReader, ChainStateBlockWriter,
    ChangeSetReader, DatabaseProviderFactory, ExecutionDataProvider, FullExecutionDataProvider,
    HeaderProvider, PruneCheckpointReader, ReceiptProvider, StageCheckpointReader, StateProvider,
    StateProviderBox, StateProviderFactory, StateReader, StateRootProvider, StorageReader,
    TransactionVariant, TransactionsProvider,
};
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{BlockBodyIndicesProvider, DBProvider, NodePrimitivesProvider, StatsReader};
use reth_trie::{updates::TrieUpdates, AccountProof, HashedPostState, MultiProof, TrieInput};
use std::{
    collections::BTreeMap,
    future::Future,
    ops::{RangeBounds, RangeInclusive},
    sync::Arc,
};
use tokio::{runtime::Handle, sync::broadcast};
use tracing::trace;

/// Configuration for `AlloyRethProvider`
#[derive(Debug, Clone, Default)]
pub struct AlloyRethProviderConfig {
    /// Whether to compute state root when creating execution outcomes
    pub compute_state_root: bool,
}

impl AlloyRethProviderConfig {
    /// Sets whether to compute state root when creating execution outcomes
    pub const fn with_compute_state_root(mut self, compute: bool) -> Self {
        self.compute_state_root = compute;
        self
    }
}

/// A provider implementation that uses Alloy RPC to fetch state data
///
/// This provider implements reth's `StateProviderFactory` and related traits,
/// allowing it to be used as a drop-in replacement for database-backed providers
/// in scenarios where RPC access is preferred (e.g., testing).
#[derive(Clone)]
pub struct AlloyRethProvider<P, N: NodeTypes> {
    /// The underlying Alloy provider
    provider: P,
    /// Node types marker
    node_types: std::marker::PhantomData<N>,
    /// Broadcast channel for canon state notifications
    canon_state_notification: broadcast::Sender<CanonStateNotification<PrimitivesTy<N>>>,
    /// Configuration for the provider
    config: AlloyRethProviderConfig,
    /// Cached chain spec
    chain_spec: Arc<N::ChainSpec>,
}

impl<P, N: NodeTypes> std::fmt::Debug for AlloyRethProvider<P, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlloyRethProvider").field("config", &self.config).finish()
    }
}

impl<P, N: NodeTypes> AlloyRethProvider<P, N> {
    /// Creates a new `AlloyRethProvider` with default configuration
    pub fn new(provider: P, _node_types: N) -> Self
    where
        N::ChainSpec: Default,
    {
        Self::new_with_config(provider, _node_types, AlloyRethProviderConfig::default())
    }

    /// Creates a new `AlloyRethProvider` with custom configuration
    pub fn new_with_config(provider: P, _node_types: N, config: AlloyRethProviderConfig) -> Self
    where
        N::ChainSpec: Default,
    {
        let (canon_state_notification, _) = broadcast::channel(1);
        Self {
            provider,
            node_types: std::marker::PhantomData,
            canon_state_notification,
            config,
            chain_spec: Arc::new(N::ChainSpec::default()),
        }
    }

    /// Helper function to execute async operations in a blocking context
    fn block_on_async<F, T>(&self, fut: F) -> T
    where
        F: Future<Output = T>,
    {
        tokio::task::block_in_place(move || Handle::current().block_on(fut))
    }
}

impl<P, N> AlloyRethProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    /// Helper function to create a state provider for a given block ID
    fn create_state_provider(&self, block_id: BlockId) -> AlloyRethStateProvider<P, N> {
        AlloyRethStateProvider::with_chain_spec(
            self.provider.clone(),
            block_id,
            self.chain_spec.clone(),
        )
    }

    /// Helper function to get state provider by block number
    fn state_by_block_number(
        &self,
        block_number: BlockNumber,
    ) -> Result<StateProviderBox, ProviderError> {
        Ok(Box::new(self.create_state_provider(BlockId::number(block_number))))
    }
}

impl<P, N> BlockHashReader for AlloyRethProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn block_hash(&self, number: BlockNumber) -> Result<Option<B256>, ProviderError> {
        let block = self.block_on_async(async {
            self.provider.get_block_by_number(number.into()).await.map_err(ProviderError::other)
        })?;
        Ok(block.map(|b| b.header.hash))
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> Result<Vec<B256>, ProviderError> {
        // Would need to make multiple RPC calls
        Ok(Vec::new())
    }
}

impl<P, N> BlockNumReader for AlloyRethProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn chain_info(&self) -> Result<reth_chainspec::ChainInfo, ProviderError> {
        // For RPC provider, we can't get full chain info
        Err(ProviderError::UnsupportedProvider)
    }

    fn best_block_number(&self) -> Result<BlockNumber, ProviderError> {
        self.block_on_async(async {
            self.provider.get_block_number().await.map_err(ProviderError::other)
        })
    }

    fn last_block_number(&self) -> Result<BlockNumber, ProviderError> {
        self.best_block_number()
    }

    fn block_number(&self, hash: B256) -> Result<Option<BlockNumber>, ProviderError> {
        let block = self.block_on_async(async {
            self.provider.get_block_by_hash(hash).await.map_err(ProviderError::other)
        })?;
        Ok(block.map(|b| b.header.number))
    }
}

impl<P, N> BlockIdReader for AlloyRethProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn block_number_for_id(&self, block_id: BlockId) -> Result<Option<BlockNumber>, ProviderError> {
        match block_id {
            BlockId::Hash(hash) => {
                let block = self.block_on_async(async {
                    self.provider
                        .get_block_by_hash(hash.block_hash)
                        .await
                        .map_err(ProviderError::other)
                })?;
                Ok(block.map(|b| b.header.number))
            }
            BlockId::Number(number_or_tag) => match number_or_tag {
                alloy_rpc_types::BlockNumberOrTag::Number(num) => Ok(Some(num)),
                alloy_rpc_types::BlockNumberOrTag::Latest => self.block_on_async(async {
                    self.provider.get_block_number().await.map(Some).map_err(ProviderError::other)
                }),
                _ => Ok(None),
            },
        }
    }

    fn pending_block_num_hash(&self) -> Result<Option<alloy_eips::BlockNumHash>, ProviderError> {
        // RPC doesn't provide pending block number and hash together
        Ok(None)
    }

    fn safe_block_num_hash(&self) -> Result<Option<alloy_eips::BlockNumHash>, ProviderError> {
        // RPC doesn't provide safe block number and hash
        Ok(None)
    }

    fn finalized_block_num_hash(&self) -> Result<Option<alloy_eips::BlockNumHash>, ProviderError> {
        // RPC doesn't provide finalized block number and hash
        Ok(None)
    }
}

impl<P, N> StateProviderFactory for AlloyRethProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn latest(&self) -> Result<StateProviderBox, ProviderError> {
        trace!(target: "alloy-provider", "Getting latest state provider");

        let block_number = self.block_on_async(async {
            self.provider.get_block_number().await.map_err(ProviderError::other)
        })?;

        self.state_by_block_number(block_number)
    }

    fn state_by_block_id(&self, block_id: BlockId) -> Result<StateProviderBox, ProviderError> {
        Ok(Box::new(self.create_state_provider(block_id)))
    }

    fn state_by_block_number_or_tag(
        &self,
        number_or_tag: alloy_rpc_types::BlockNumberOrTag,
    ) -> Result<StateProviderBox, ProviderError> {
        match number_or_tag {
            alloy_rpc_types::BlockNumberOrTag::Latest => self.latest(),
            alloy_rpc_types::BlockNumberOrTag::Pending => self.pending(),
            alloy_rpc_types::BlockNumberOrTag::Number(num) => self.state_by_block_number(num),
            _ => Err(ProviderError::UnsupportedProvider),
        }
    }

    fn history_by_block_number(
        &self,
        block_number: BlockNumber,
    ) -> Result<StateProviderBox, ProviderError> {
        self.state_by_block_number(block_number)
    }

    fn history_by_block_hash(
        &self,
        block_hash: BlockHash,
    ) -> Result<StateProviderBox, ProviderError> {
        self.state_by_block_hash(block_hash)
    }

    fn state_by_block_hash(
        &self,
        block_hash: BlockHash,
    ) -> Result<StateProviderBox, ProviderError> {
        trace!(target: "alloy-provider", ?block_hash, "Getting state provider by block hash");

        let block = self.block_on_async(async {
            self.provider
                .get_block_by_hash(block_hash)
                .await
                .map_err(ProviderError::other)?
                .ok_or(ProviderError::BlockHashNotFound(block_hash))
        })?;

        let block_number = block.header.number;
        Ok(Box::new(self.create_state_provider(BlockId::number(block_number))))
    }

    fn pending(&self) -> Result<StateProviderBox, ProviderError> {
        trace!(target: "alloy-provider", "Getting pending state provider");
        self.latest()
    }

    fn pending_state_by_hash(
        &self,
        _block_hash: B256,
    ) -> Result<Option<StateProviderBox>, ProviderError> {
        // RPC provider doesn't support pending state by hash
        Ok(None)
    }
}

impl<P, N> DatabaseProviderFactory for AlloyRethProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    type DB = DatabaseMock;
    type ProviderRW = AlloyRethStateProvider<P, N>;
    type Provider = AlloyRethStateProvider<P, N>;

    fn database_provider_ro(&self) -> Result<Self::Provider, ProviderError> {
        // RPC provider returns a new state provider
        let block_number = self.block_on_async(async {
            self.provider.get_block_number().await.map_err(ProviderError::other)
        })?;

        Ok(self.create_state_provider(BlockId::number(block_number)))
    }

    fn database_provider_rw(&self) -> Result<Self::ProviderRW, ProviderError> {
        // RPC provider returns a new state provider
        let block_number = self.block_on_async(async {
            self.provider.get_block_number().await.map_err(ProviderError::other)
        })?;

        Ok(self.create_state_provider(BlockId::number(block_number)))
    }
}

impl<P, N> CanonChainTracker for AlloyRethProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    type Header = alloy_consensus::Header;
    fn on_forkchoice_update_received(&self, _update: &ForkchoiceState) {
        // No-op for RPC provider
    }

    fn last_received_update_timestamp(&self) -> Option<std::time::Instant> {
        None
    }

    fn set_canonical_head(&self, _header: SealedHeader<Self::Header>) {
        // No-op for RPC provider
    }

    fn set_safe(&self, _header: SealedHeader<Self::Header>) {
        // No-op for RPC provider
    }

    fn set_finalized(&self, _header: SealedHeader<Self::Header>) {
        // No-op for RPC provider
    }
}

impl<P, N> NodePrimitivesProvider for AlloyRethProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    type Primitives = PrimitivesTy<N>;
}

impl<P, N> CanonStateSubscriptions for AlloyRethProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications<PrimitivesTy<N>> {
        trace!(target: "alloy-provider", "Subscribing to canonical state notifications");
        self.canon_state_notification.subscribe()
    }

    // canonical_state_stream has a default implementation in the trait
}

impl<P, N> ChainSpecProvider for AlloyRethProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
    N::ChainSpec: Default,
{
    type ChainSpec = N::ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.chain_spec.clone()
    }
}

impl<P, N> BlockchainTreePendingStateProvider for AlloyRethProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn pending_state_provider(
        &self,
        _block_hash: BlockHash,
    ) -> Result<Box<dyn FullExecutionDataProvider>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn find_pending_state_provider(
        &self,
        _block_hash: BlockHash,
    ) -> Option<Box<dyn FullExecutionDataProvider>> {
        None
    }
}

/// State provider implementation that fetches state via RPC
#[derive(Clone)]
pub struct AlloyRethStateProvider<P, N: NodeTypes> {
    /// The underlying Alloy provider
    provider: P,
    /// The block ID to fetch state at
    block_id: BlockId,
    /// Node types marker
    node_types: std::marker::PhantomData<N>,
    /// Cached chain spec (shared with parent provider)
    chain_spec: Option<Arc<N::ChainSpec>>,
}

impl<P: std::fmt::Debug, N: NodeTypes> std::fmt::Debug for AlloyRethStateProvider<P, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlloyRethStateProvider")
            .field("provider", &self.provider)
            .field("block_id", &self.block_id)
            .finish()
    }
}

impl<P: Clone, N: NodeTypes> AlloyRethStateProvider<P, N> {
    /// Creates a new state provider for the given block
    pub const fn new(
        provider: P,
        block_id: BlockId,
        _primitives: std::marker::PhantomData<N>,
    ) -> Self {
        Self { provider, block_id, node_types: std::marker::PhantomData, chain_spec: None }
    }

    /// Creates a new state provider with a cached chain spec
    pub const fn with_chain_spec(
        provider: P,
        block_id: BlockId,
        chain_spec: Arc<N::ChainSpec>,
    ) -> Self {
        Self {
            provider,
            block_id,
            node_types: std::marker::PhantomData,
            chain_spec: Some(chain_spec),
        }
    }

    /// Helper function to execute async operations in a blocking context
    fn block_on_async<F, T>(&self, fut: F) -> T
    where
        F: Future<Output = T>,
    {
        tokio::task::block_in_place(move || Handle::current().block_on(fut))
    }

    /// Helper function to create a new state provider with a different block ID
    fn with_block_id(&self, block_id: BlockId) -> Self {
        Self {
            provider: self.provider.clone(),
            block_id,
            node_types: self.node_types,
            chain_spec: self.chain_spec.clone(),
        }
    }

    /// Get account information from RPC
    fn get_account(&self, address: Address) -> Result<Option<Account>, ProviderError>
    where
        P: Provider<alloy_network::AnyNetwork> + Clone + 'static,
    {
        self.block_on_async(async {
            // Get account info in a single RPC call
            let account_info = self
                .provider
                .get_account_info(address)
                .block_id(self.block_id)
                .await
                .map_err(ProviderError::other)?;

            // Only return account if it exists (has balance, nonce, or code)
            if account_info.balance.is_zero() &&
                account_info.nonce == 0 &&
                account_info.code.is_empty()
            {
                Ok(None)
            } else {
                let bytecode = if account_info.code.is_empty() {
                    None
                } else {
                    Some(Bytecode::new_raw(account_info.code))
                };

                Ok(Some(Account {
                    balance: account_info.balance,
                    nonce: account_info.nonce,
                    bytecode_hash: bytecode.as_ref().map(|b| b.hash_slow()),
                }))
            }
        })
    }
}

impl<P, N> StateProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn storage(
        &self,
        address: Address,
        storage_key: StorageKey,
    ) -> Result<Option<U256>, ProviderError> {
        self.block_on_async(async {
            let value = self
                .provider
                .get_storage_at(address, storage_key.into())
                .block_id(self.block_id)
                .await
                .map_err(ProviderError::other)?;

            if value.is_zero() {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        })
    }

    fn bytecode_by_hash(&self, _code_hash: &B256) -> Result<Option<Bytecode>, ProviderError> {
        // Cannot fetch bytecode by hash via RPC
        Ok(None)
    }

    fn account_code(&self, addr: &Address) -> Result<Option<Bytecode>, ProviderError> {
        self.block_on_async(async {
            let code = self
                .provider
                .get_code_at(*addr)
                .block_id(self.block_id)
                .await
                .map_err(ProviderError::other)?;

            if code.is_empty() {
                Ok(None)
            } else {
                Ok(Some(Bytecode::new_raw(code)))
            }
        })
    }

    fn account_balance(&self, addr: &Address) -> Result<Option<U256>, ProviderError> {
        self.get_account(*addr).map(|acc| acc.map(|a| a.balance))
    }

    fn account_nonce(&self, addr: &Address) -> Result<Option<u64>, ProviderError> {
        self.get_account(*addr).map(|acc| acc.map(|a| a.nonce))
    }
}

impl<P, N> AccountReader for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn basic_account(&self, address: &Address) -> Result<Option<Account>, ProviderError> {
        self.get_account(*address)
    }
}

impl<P, N> StateRootProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn state_root(&self, _state: HashedPostState) -> Result<B256, ProviderError> {
        // Return the state root from the block
        self.block_on_async(async {
            let block = self
                .provider
                .get_block(self.block_id)
                .await
                .map_err(ProviderError::other)?
                .ok_or(ProviderError::HeaderNotFound(0.into()))?;

            Ok(block.header.state_root)
        })
    }

    fn state_root_from_nodes(&self, _input: TrieInput) -> Result<B256, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn state_root_with_updates(
        &self,
        _state: HashedPostState,
    ) -> Result<(B256, TrieUpdates), ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        _input: TrieInput,
    ) -> Result<(B256, TrieUpdates), ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, N> StorageReader for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn plain_state_storages(
        &self,
        addresses_with_keys: impl IntoIterator<Item = (Address, impl IntoIterator<Item = StorageKey>)>,
    ) -> Result<Vec<(Address, Vec<reth_primitives::StorageEntry>)>, ProviderError> {
        let mut results = Vec::new();

        for (address, keys) in addresses_with_keys {
            let mut values = Vec::new();
            for key in keys {
                let value = self.storage(address, key)?.unwrap_or_default();
                values.push(reth_primitives::StorageEntry::new(key, value));
            }
            results.push((address, values));
        }

        Ok(results)
    }

    fn changed_storages_with_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> Result<BTreeMap<Address, std::collections::BTreeSet<StorageKey>>, ProviderError> {
        Ok(BTreeMap::new())
    }

    fn changed_storages_and_blocks_with_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> Result<BTreeMap<(Address, StorageKey), Vec<u64>>, ProviderError> {
        Ok(BTreeMap::new())
    }
}

impl<P, N> reth_storage_api::StorageRootProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn storage_root(
        &self,
        _address: Address,
        _hashed_storage: reth_trie::HashedStorage,
    ) -> Result<B256, ProviderError> {
        // RPC doesn't provide storage root computation
        Err(ProviderError::UnsupportedProvider)
    }

    fn storage_proof(
        &self,
        _address: Address,
        _slot: B256,
        _hashed_storage: reth_trie::HashedStorage,
    ) -> Result<reth_trie::StorageProof, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn storage_multiproof(
        &self,
        _address: Address,
        _slots: &[B256],
        _hashed_storage: reth_trie::HashedStorage,
    ) -> Result<reth_trie::StorageMultiProof, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, N> reth_storage_api::StateProofProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn proof(
        &self,
        _input: TrieInput,
        _address: Address,
        _slots: &[B256],
    ) -> Result<AccountProof, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn multiproof(
        &self,
        _input: TrieInput,
        _targets: reth_trie::MultiProofTargets,
    ) -> Result<MultiProof, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn witness(
        &self,
        _input: TrieInput,
        _target: HashedPostState,
    ) -> Result<Vec<alloy_primitives::Bytes>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, N> reth_storage_api::HashedPostStateProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn hashed_post_state(&self, _bundle_state: &revm::database::BundleState) -> HashedPostState {
        // Return empty hashed post state for RPC provider
        HashedPostState::default()
    }
}

impl<P, N> StateReader for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    type Receipt = ReceiptTy<N>;

    fn get_state(
        &self,
        _block: BlockNumber,
    ) -> Result<Option<reth_execution_types::ExecutionOutcome<Self::Receipt>>, ProviderError> {
        // RPC doesn't provide execution outcomes
        Ok(None)
    }
}

impl<P, N> DBProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    type Tx = TxMock;

    fn tx_ref(&self) -> &Self::Tx {
        // We can't use a static here since TxMock doesn't allow direct construction
        // This is fine since we're just returning a mock transaction
        unimplemented!("tx_ref not supported for RPC provider")
    }

    fn tx_mut(&mut self) -> &mut Self::Tx {
        unimplemented!("tx_mut not supported for RPC provider")
    }

    fn into_tx(self) -> Self::Tx {
        TxMock::default()
    }

    fn prune_modes_ref(&self) -> &reth_prune_types::PruneModes {
        unimplemented!("prune modes not supported for RPC provider")
    }

    fn disable_long_read_transaction_safety(self) -> Self {
        // No-op for RPC provider
        self
    }
}

impl<P, N> BlockNumReader for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn chain_info(&self) -> Result<ChainInfo, ProviderError> {
        self.block_on_async(async {
            let block = self
                .provider
                .get_block(self.block_id)
                .await
                .map_err(ProviderError::other)?
                .ok_or(ProviderError::HeaderNotFound(0.into()))?;

            Ok(ChainInfo { best_hash: block.header.hash, best_number: block.header.number })
        })
    }

    fn best_block_number(&self) -> Result<BlockNumber, ProviderError> {
        self.block_on_async(async {
            self.provider.get_block_number().await.map_err(ProviderError::other)
        })
    }

    fn last_block_number(&self) -> Result<BlockNumber, ProviderError> {
        self.best_block_number()
    }

    fn block_number(&self, hash: B256) -> Result<Option<BlockNumber>, ProviderError> {
        self.block_on_async(async {
            let block =
                self.provider.get_block_by_hash(hash).await.map_err(ProviderError::other)?;

            Ok(block.map(|b| b.header.number))
        })
    }
}

impl<P, N> BlockHashReader for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn block_hash(&self, number: u64) -> Result<Option<B256>, ProviderError> {
        self.block_on_async(async {
            let block = self
                .provider
                .get_block_by_number(number.into())
                .await
                .map_err(ProviderError::other)?;

            Ok(block.map(|b| b.header.hash))
        })
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> Result<Vec<B256>, ProviderError> {
        Ok(Vec::new())
    }
}

impl<P, N> BlockIdReader for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn block_number_for_id(
        &self,
        _block_id: BlockId,
    ) -> Result<Option<BlockNumber>, ProviderError> {
        Ok(None)
    }

    fn pending_block_num_hash(&self) -> Result<Option<alloy_eips::BlockNumHash>, ProviderError> {
        Ok(None)
    }

    fn safe_block_num_hash(&self) -> Result<Option<alloy_eips::BlockNumHash>, ProviderError> {
        Ok(None)
    }

    fn finalized_block_num_hash(&self) -> Result<Option<alloy_eips::BlockNumHash>, ProviderError> {
        Ok(None)
    }
}

impl<P, N> BlockReader for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    type Block = BlockTy<N>;

    fn find_block_by_hash(
        &self,
        _hash: B256,
        _source: reth_provider::BlockSource,
    ) -> Result<Option<Self::Block>, ProviderError> {
        Ok(None)
    }

    fn block(
        &self,
        _id: alloy_rpc_types::BlockHashOrNumber,
    ) -> Result<Option<Self::Block>, ProviderError> {
        Ok(None)
    }

    fn pending_block(&self) -> Result<Option<RecoveredBlock<Self::Block>>, ProviderError> {
        Ok(None)
    }

    fn pending_block_and_receipts(
        &self,
    ) -> Result<Option<(SealedBlock<Self::Block>, Vec<Self::Receipt>)>, ProviderError> {
        Ok(None)
    }

    fn recovered_block(
        &self,
        _id: alloy_rpc_types::BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> Result<Option<RecoveredBlock<Self::Block>>, ProviderError> {
        Ok(None)
    }

    fn sealed_block_with_senders(
        &self,
        _id: alloy_rpc_types::BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> Result<Option<RecoveredBlock<BlockTy<N>>>, ProviderError> {
        Ok(None)
    }

    fn block_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> Result<Vec<Self::Block>, ProviderError> {
        Ok(Vec::new())
    }

    fn block_with_senders_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> Result<Vec<RecoveredBlock<BlockTy<N>>>, ProviderError> {
        Ok(Vec::new())
    }

    fn recovered_block_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> Result<Vec<RecoveredBlock<Self::Block>>, ProviderError> {
        Ok(Vec::new())
    }
}

impl<P, N> TransactionsProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    type Transaction = TxTy<N>;

    fn transaction_id(&self, _tx_hash: B256) -> Result<Option<TxNumber>, ProviderError> {
        Ok(None)
    }

    fn transaction_by_id(&self, _id: TxNumber) -> Result<Option<Self::Transaction>, ProviderError> {
        Ok(None)
    }

    fn transaction_by_id_unhashed(
        &self,
        _id: TxNumber,
    ) -> Result<Option<Self::Transaction>, ProviderError> {
        Ok(None)
    }

    fn transaction_by_hash(&self, _hash: B256) -> Result<Option<Self::Transaction>, ProviderError> {
        Ok(None)
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: B256,
    ) -> Result<Option<(Self::Transaction, TransactionMeta)>, ProviderError> {
        Ok(None)
    }

    fn transaction_block(&self, _id: TxNumber) -> Result<Option<BlockNumber>, ProviderError> {
        Ok(None)
    }

    fn transactions_by_block(
        &self,
        _block: alloy_rpc_types::BlockHashOrNumber,
    ) -> Result<Option<Vec<Self::Transaction>>, ProviderError> {
        Ok(None)
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<Vec<Self::Transaction>>, ProviderError> {
        Ok(Vec::new())
    }

    fn transactions_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> Result<Vec<Self::Transaction>, ProviderError> {
        Ok(Vec::new())
    }

    fn senders_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> Result<Vec<Address>, ProviderError> {
        Ok(Vec::new())
    }

    fn transaction_sender(&self, _id: TxNumber) -> Result<Option<Address>, ProviderError> {
        Ok(None)
    }
}

impl<P, N> ReceiptProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    type Receipt = ReceiptTy<N>;

    fn receipt(&self, _id: TxNumber) -> Result<Option<Self::Receipt>, ProviderError> {
        Ok(None)
    }

    fn receipt_by_hash(&self, _hash: B256) -> Result<Option<Self::Receipt>, ProviderError> {
        Ok(None)
    }

    fn receipts_by_block(
        &self,
        _block: alloy_rpc_types::BlockHashOrNumber,
    ) -> Result<Option<Vec<Self::Receipt>>, ProviderError> {
        Ok(None)
    }

    fn receipts_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> Result<Vec<Self::Receipt>, ProviderError> {
        Ok(Vec::new())
    }

    fn receipts_by_block_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> Result<Vec<Vec<Self::Receipt>>, ProviderError> {
        Ok(Vec::new())
    }
}

impl<P, N> HeaderProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    type Header = HeaderTy<N>;

    fn header(&self, _block_hash: &BlockHash) -> Result<Option<Self::Header>, ProviderError> {
        Ok(None)
    }

    fn header_by_number(&self, _num: BlockNumber) -> Result<Option<Self::Header>, ProviderError> {
        Ok(None)
    }

    fn header_td(&self, _hash: &BlockHash) -> Result<Option<U256>, ProviderError> {
        Ok(None)
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> Result<Option<U256>, ProviderError> {
        Ok(None)
    }

    fn headers_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<Self::Header>, ProviderError> {
        Ok(Vec::new())
    }

    fn sealed_header(
        &self,
        _number: BlockNumber,
    ) -> Result<Option<SealedHeader<HeaderTy<N>>>, ProviderError> {
        Ok(None)
    }

    fn sealed_headers_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<SealedHeader<HeaderTy<N>>>, ProviderError> {
        Ok(Vec::new())
    }

    fn sealed_headers_while(
        &self,
        _range: impl RangeBounds<BlockNumber>,
        _predicate: impl FnMut(&SealedHeader<HeaderTy<N>>) -> bool,
    ) -> Result<Vec<SealedHeader<HeaderTy<N>>>, ProviderError> {
        Ok(Vec::new())
    }
}

impl<P, N> PruneCheckpointReader for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn get_prune_checkpoint(
        &self,
        _segment: PruneSegment,
    ) -> Result<Option<PruneCheckpoint>, ProviderError> {
        Ok(None)
    }

    fn get_prune_checkpoints(&self) -> Result<Vec<(PruneSegment, PruneCheckpoint)>, ProviderError> {
        Ok(Vec::new())
    }
}

impl<P, N> StageCheckpointReader for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn get_stage_checkpoint(&self, _id: StageId) -> Result<Option<StageCheckpoint>, ProviderError> {
        Ok(None)
    }

    fn get_stage_checkpoint_progress(
        &self,
        _id: StageId,
    ) -> Result<Option<Vec<u8>>, ProviderError> {
        Ok(None)
    }

    fn get_all_checkpoints(&self) -> Result<Vec<(String, StageCheckpoint)>, ProviderError> {
        Ok(Vec::new())
    }
}

impl<P, N> ChangeSetReader for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn account_block_changeset(
        &self,
        _block_number: BlockNumber,
    ) -> Result<Vec<reth_db_api::models::AccountBeforeTx>, ProviderError> {
        Ok(Vec::new())
    }
}

impl<P, N> StateProviderFactory for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug + Send + Sync,
    N: NodeTypes + 'static,
    N::ChainSpec: Send + Sync,
    Self: Clone + 'static,
{
    fn latest(&self) -> Result<StateProviderBox, ProviderError> {
        Ok(Box::new(self.clone()) as StateProviderBox)
    }

    fn state_by_block_id(&self, block_id: BlockId) -> Result<StateProviderBox, ProviderError> {
        Ok(Box::new(self.with_block_id(block_id)))
    }

    fn state_by_block_number_or_tag(
        &self,
        number_or_tag: alloy_rpc_types::BlockNumberOrTag,
    ) -> Result<StateProviderBox, ProviderError> {
        match number_or_tag {
            alloy_rpc_types::BlockNumberOrTag::Latest => self.latest(),
            alloy_rpc_types::BlockNumberOrTag::Pending => self.pending(),
            alloy_rpc_types::BlockNumberOrTag::Number(num) => self.history_by_block_number(num),
            _ => Err(ProviderError::UnsupportedProvider),
        }
    }

    fn history_by_block_number(
        &self,
        block_number: BlockNumber,
    ) -> Result<StateProviderBox, ProviderError> {
        Ok(Box::new(Self::new(
            self.provider.clone(),
            BlockId::number(block_number),
            self.node_types,
        )))
    }

    fn history_by_block_hash(
        &self,
        block_hash: BlockHash,
    ) -> Result<StateProviderBox, ProviderError> {
        Ok(Box::new(self.with_block_id(BlockId::hash(block_hash))))
    }

    fn state_by_block_hash(
        &self,
        block_hash: BlockHash,
    ) -> Result<StateProviderBox, ProviderError> {
        self.history_by_block_hash(block_hash)
    }

    fn pending(&self) -> Result<StateProviderBox, ProviderError> {
        Ok(Box::new(self.clone()))
    }

    fn pending_state_by_hash(
        &self,
        _block_hash: B256,
    ) -> Result<Option<StateProviderBox>, ProviderError> {
        // RPC provider doesn't support pending state by hash
        Ok(None)
    }
}

impl<P, N> ChainSpecProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
    N::ChainSpec: Default,
{
    type ChainSpec = N::ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        if let Some(chain_spec) = &self.chain_spec {
            chain_spec.clone()
        } else {
            // Fallback for when chain_spec is not provided
            Arc::new(N::ChainSpec::default())
        }
    }
}

impl<P, N> ExecutionDataProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn execution_outcome(&self) -> &ExecutionOutcome {
        unimplemented!("execution outcome not available for RPC provider")
    }

    fn block_hash(&self, _block_number: BlockNumber) -> Option<BlockHash> {
        None
    }
}

impl<P, N> reth_provider::BlockExecutionForkProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn canonical_fork(&self) -> alloy_eips::BlockNumHash {
        // For RPC provider, we don't have fork information
        alloy_eips::BlockNumHash::default()
    }
}

// Note: FullExecutionDataProvider is already implemented via the blanket implementation
// for types that implement both ExecutionDataProvider and BlockExecutionForkProvider

impl<P, N> StatsReader for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn count_entries<T: reth_db_api::table::Table>(&self) -> Result<usize, ProviderError> {
        Ok(0)
    }
}

impl<P, N> BlockBodyIndicesProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn block_body_indices(
        &self,
        _num: u64,
    ) -> Result<Option<reth_db_api::models::StoredBlockBodyIndices>, ProviderError> {
        Ok(None)
    }

    fn block_body_indices_range(
        &self,
        _range: RangeInclusive<u64>,
    ) -> Result<Vec<reth_db_api::models::StoredBlockBodyIndices>, ProviderError> {
        Ok(Vec::new())
    }
}

impl<P, N> NodePrimitivesProvider for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    type Primitives = PrimitivesTy<N>;
}

impl<P, N> ChainStateBlockReader for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn last_finalized_block_number(&self) -> Result<Option<BlockNumber>, ProviderError> {
        Ok(None)
    }

    fn last_safe_block_number(&self) -> Result<Option<BlockNumber>, ProviderError> {
        Ok(None)
    }
}

impl<P, N> ChainStateBlockWriter for AlloyRethStateProvider<P, N>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
    N: NodeTypes,
{
    fn save_finalized_block_number(&self, _block_number: BlockNumber) -> Result<(), ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn save_safe_block_number(&self, _block_number: BlockNumber) -> Result<(), ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

// Async database wrapper for revm compatibility
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct AsyncDbWrapper<P> {
    provider: P,
    block_id: BlockId,
}

#[allow(dead_code)]
impl<P> AsyncDbWrapper<P> {
    const fn new(provider: P, block_id: BlockId) -> Self {
        Self { provider, block_id }
    }

    /// Helper function to execute async operations in a blocking context
    fn block_on_async<F, T>(&self, fut: F) -> T
    where
        F: Future<Output = T>,
    {
        tokio::task::block_in_place(move || Handle::current().block_on(fut))
    }
}

impl<P> revm::Database for AsyncDbWrapper<P>
where
    P: Provider<alloy_network::AnyNetwork> + Clone + 'static + std::fmt::Debug,
{
    type Error = ProviderError;

    fn basic(&mut self, address: Address) -> Result<Option<revm::state::AccountInfo>, Self::Error> {
        self.block_on_async(async {
            let account_info = self
                .provider
                .get_account_info(address)
                .block_id(self.block_id)
                .await
                .map_err(ProviderError::other)?;

            // Only return account if it exists
            if account_info.balance.is_zero() &&
                account_info.nonce == 0 &&
                account_info.code.is_empty()
            {
                Ok(None)
            } else {
                let code_hash = if account_info.code.is_empty() {
                    revm_primitives::KECCAK_EMPTY
                } else {
                    revm_primitives::keccak256(&account_info.code)
                };

                Ok(Some(revm::state::AccountInfo {
                    balance: account_info.balance,
                    nonce: account_info.nonce,
                    code_hash,
                    code: if account_info.code.is_empty() {
                        None
                    } else {
                        Some(revm::bytecode::Bytecode::new_raw(account_info.code))
                    },
                }))
            }
        })
    }

    fn code_by_hash(&mut self, _code_hash: B256) -> Result<revm::bytecode::Bytecode, Self::Error> {
        // Cannot fetch bytecode by hash via RPC
        Ok(revm::bytecode::Bytecode::default())
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let index = B256::from(index);

        self.block_on_async(async {
            self.provider
                .get_storage_at(address, index.into())
                .block_id(self.block_id)
                .await
                .map_err(ProviderError::other)
        })
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.block_on_async(async {
            let block = self
                .provider
                .get_block_by_number(number.into())
                .await
                .map_err(ProviderError::other)?
                .ok_or(ProviderError::HeaderNotFound(number.into()))?;

            Ok(block.header.hash)
        })
    }
}
