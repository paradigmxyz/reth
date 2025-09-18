//! # RPC Blockchain Provider for Reth
//!
//! This crate provides an RPC-based implementation of reth's `StateProviderFactory` and related
//! traits that fetches blockchain data via RPC instead of from a local database.
//!
//! Similar to the [`BlockchainProvider`](../../provider/src/providers/blockchain_provider.rs)
//! which provides access to local blockchain data, this crate offers the same functionality but for
//! remote blockchain access via RPC.
//!
//! Originally created by [cakevm](https://github.com/cakevm/alloy-reth-provider).
//!
//! ## Features
//!
//! - Implements `StateProviderFactory` for remote RPC state access
//! - Supports Ethereum and Optimism network
//! - Useful for testing without requiring a full database
//! - Can be used with reth ExEx (Execution Extensions) for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use alloy_consensus::{constants::KECCAK_EMPTY, BlockHeader};
use alloy_eips::{BlockHashOrNumber, BlockNumberOrTag};
use alloy_network::{primitives::HeaderResponse, BlockResponse};
use alloy_primitives::{
    map::HashMap, Address, BlockHash, BlockNumber, StorageKey, TxHash, TxNumber, B256, U256,
};
use alloy_provider::{ext::DebugApi, network::Network, Provider};
use alloy_rpc_types::{AccountInfo, BlockId};
use alloy_rpc_types_engine::ForkchoiceState;
use parking_lot::RwLock;
use reth_chainspec::{ChainInfo, ChainSpecProvider};
use reth_db_api::{
    mock::{DatabaseMock, TxMock},
    models::StoredBlockBodyIndices,
};
use reth_errors::{ProviderError, ProviderResult};
use reth_node_types::{
    Block, BlockBody, BlockTy, HeaderTy, NodeTypes, PrimitivesTy, ReceiptTy, TxTy,
};
use reth_primitives::{Account, Bytecode, RecoveredBlock, SealedHeader, TransactionMeta};
use reth_provider::{
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BytecodeReader,
    CanonChainTracker, CanonStateNotification, CanonStateNotifications, CanonStateSubscriptions,
    ChainStateBlockReader, ChainStateBlockWriter, ChangeSetReader, DatabaseProviderFactory,
    HeaderProvider, PruneCheckpointReader, ReceiptProvider, StageCheckpointReader, StateProvider,
    StateProviderBox, StateProviderFactory, StateReader, StateRootProvider, StorageReader,
    TransactionVariant, TransactionsProvider,
};
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_rpc_convert::{TryFromBlockResponse, TryFromReceiptResponse, TryFromTransactionResponse};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{
    BlockBodyIndicesProvider, BlockReaderIdExt, BlockSource, DBProvider, NodePrimitivesProvider,
    ReceiptProviderIdExt, StatsReader,
};
use reth_trie::{updates::TrieUpdates, AccountProof, HashedPostState, MultiProof, TrieInput};
use std::{
    collections::BTreeMap,
    future::{Future, IntoFuture},
    ops::{RangeBounds, RangeInclusive},
    sync::Arc,
};
use tokio::{runtime::Handle, sync::broadcast};
use tracing::{trace, warn};

/// Configuration for `RpcBlockchainProvider`
#[derive(Debug, Clone)]
pub struct RpcBlockchainProviderConfig {
    /// Whether to compute state root when creating execution outcomes
    pub compute_state_root: bool,
    /// Whether to use Reth-specific RPC methods for better performance
    ///
    /// If enabled, the node will use Reth's RPC methods (`debug_codeByHash` and
    /// `eth_getAccountInfo`) to speed up account information retrieval. When disabled, it will
    /// use multiple standard RPC calls to get account information.
    pub reth_rpc_support: bool,
}

impl Default for RpcBlockchainProviderConfig {
    fn default() -> Self {
        Self { compute_state_root: false, reth_rpc_support: true }
    }
}

impl RpcBlockchainProviderConfig {
    /// Sets whether to compute state root when creating execution outcomes
    pub const fn with_compute_state_root(mut self, compute: bool) -> Self {
        self.compute_state_root = compute;
        self
    }

    /// Sets whether to use Reth-specific RPC methods for better performance
    pub const fn with_reth_rpc_support(mut self, support: bool) -> Self {
        self.reth_rpc_support = support;
        self
    }
}

/// An RPC-based blockchain provider that fetches blockchain data via remote RPC calls.
///
/// This is the RPC equivalent of
/// [`BlockchainProvider`](../../provider/src/providers/blockchain_provider.rs), implementing
/// the same `StateProviderFactory` and related traits but fetching data from a remote node instead
/// of local storage.
///
/// This provider is useful for:
/// - Testing without requiring a full local database
/// - Accessing blockchain state from remote nodes
/// - Building light clients or tools that don't need full node storage
///
/// The provider type is generic over the network type N (defaulting to `AnyNetwork`),
/// but the current implementation is specialized for `alloy_network::AnyNetwork`
/// as it needs to access block header fields directly.
#[derive(Clone)]
pub struct RpcBlockchainProvider<P, Node, N = alloy_network::AnyNetwork>
where
    Node: NodeTypes,
{
    /// The underlying Alloy provider
    provider: P,
    /// Node types marker
    node_types: std::marker::PhantomData<Node>,
    /// Network marker
    network: std::marker::PhantomData<N>,
    /// Broadcast channel for canon state notifications
    canon_state_notification: broadcast::Sender<CanonStateNotification<PrimitivesTy<Node>>>,
    /// Configuration for the provider
    config: RpcBlockchainProviderConfig,
    /// Cached chain spec
    chain_spec: Arc<Node::ChainSpec>,
}

impl<P, Node: NodeTypes, N> std::fmt::Debug for RpcBlockchainProvider<P, Node, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcBlockchainProvider").field("config", &self.config).finish()
    }
}

impl<P, Node: NodeTypes, N> RpcBlockchainProvider<P, Node, N> {
    /// Creates a new `RpcBlockchainProvider` with default configuration
    pub fn new(provider: P) -> Self
    where
        Node::ChainSpec: Default,
    {
        Self::new_with_config(provider, RpcBlockchainProviderConfig::default())
    }

    /// Creates a new `RpcBlockchainProvider` with custom configuration
    pub fn new_with_config(provider: P, config: RpcBlockchainProviderConfig) -> Self
    where
        Node::ChainSpec: Default,
    {
        let (canon_state_notification, _) = broadcast::channel(1);
        Self {
            provider,
            node_types: std::marker::PhantomData,
            network: std::marker::PhantomData,
            canon_state_notification,
            config,
            chain_spec: Arc::new(Node::ChainSpec::default()),
        }
    }

    /// Use a custom chain spec for the provider
    pub fn with_chain_spec(self, chain_spec: Arc<Node::ChainSpec>) -> Self {
        Self {
            provider: self.provider,
            node_types: std::marker::PhantomData,
            network: std::marker::PhantomData,
            canon_state_notification: self.canon_state_notification,
            config: self.config,
            chain_spec,
        }
    }

    /// Helper function to execute async operations in a blocking context
    fn block_on_async<F, T>(&self, fut: F) -> T
    where
        F: Future<Output = T>,
    {
        tokio::task::block_in_place(move || Handle::current().block_on(fut))
    }

    /// Get a reference to the canon state notification sender
    pub const fn canon_state_notification(
        &self,
    ) -> &broadcast::Sender<CanonStateNotification<PrimitivesTy<Node>>> {
        &self.canon_state_notification
    }
}

impl<P, Node, N> RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    /// Helper function to create a state provider for a given block ID
    fn create_state_provider(&self, block_id: BlockId) -> RpcBlockchainStateProvider<P, Node, N> {
        RpcBlockchainStateProvider::with_chain_spec(
            self.provider.clone(),
            block_id,
            self.chain_spec.clone(),
        )
        .with_compute_state_root(self.config.compute_state_root)
        .with_reth_rpc_support(self.config.reth_rpc_support)
    }

    /// Helper function to get state provider by block number
    fn state_by_block_number(
        &self,
        block_number: BlockNumber,
    ) -> Result<StateProviderBox, ProviderError> {
        Ok(Box::new(self.create_state_provider(BlockId::number(block_number))))
    }
}

// Implementation note: While the types are generic over Network N, the trait implementations
// are specialized for AnyNetwork because they need to access block header fields.
// This allows the types to be instantiated with any network while the actual functionality
// requires AnyNetwork. Future improvements could add trait bounds for networks with
// compatible block structures.
impl<P, Node, N> BlockHashReader for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn block_hash(&self, number: BlockNumber) -> Result<Option<B256>, ProviderError> {
        let block = self.block_on_async(async {
            self.provider.get_block_by_number(number.into()).await.map_err(ProviderError::other)
        })?;
        Ok(block.map(|b| b.header().hash()))
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> Result<Vec<B256>, ProviderError> {
        // Would need to make multiple RPC calls
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> BlockNumReader for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn chain_info(&self) -> Result<reth_chainspec::ChainInfo, ProviderError> {
        self.block_on_async(async {
            let block = self
                .provider
                .get_block(BlockId::Number(BlockNumberOrTag::Latest))
                .await
                .map_err(ProviderError::other)?
                .ok_or(ProviderError::HeaderNotFound(0.into()))?;

            Ok(ChainInfo { best_hash: block.header().hash(), best_number: block.header().number() })
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
        let block = self.block_on_async(async {
            self.provider.get_block_by_hash(hash).await.map_err(ProviderError::other)
        })?;
        Ok(block.map(|b| b.header().number()))
    }
}

impl<P, Node, N> BlockIdReader for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
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
                Ok(block.map(|b| b.header().number()))
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
        Err(ProviderError::UnsupportedProvider)
    }

    fn safe_block_num_hash(&self) -> Result<Option<alloy_eips::BlockNumHash>, ProviderError> {
        // RPC doesn't provide safe block number and hash
        Err(ProviderError::UnsupportedProvider)
    }

    fn finalized_block_num_hash(&self) -> Result<Option<alloy_eips::BlockNumHash>, ProviderError> {
        // RPC doesn't provide finalized block number and hash
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> HeaderProvider for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
    BlockTy<Node>: TryFromBlockResponse<N>,
{
    type Header = HeaderTy<Node>;

    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Self::Header>> {
        let block_response = self.block_on_async(async {
            self.provider.get_block_by_hash(*block_hash).await.map_err(ProviderError::other)
        })?;

        let Some(block_response) = block_response else {
            // If the block was not found, return None
            return Ok(None);
        };

        // Convert the network block response to primitive block
        let block = <BlockTy<Node> as TryFromBlockResponse<N>>::from_block_response(block_response)
            .map_err(ProviderError::other)?;

        Ok(Some(block.into_header()))
    }

    fn header_by_number(&self, num: u64) -> ProviderResult<Option<Self::Header>> {
        let Some(sealed_header) = self.sealed_header(num)? else {
            // If the block was not found, return None
            return Ok(None);
        };

        Ok(Some(sealed_header.into_header()))
    }

    fn header_td(&self, hash: &BlockHash) -> ProviderResult<Option<U256>> {
        let header = self.header(hash).map_err(ProviderError::other)?;

        Ok(header.map(|b| b.difficulty()))
    }

    fn header_td_by_number(&self, number: BlockNumber) -> ProviderResult<Option<U256>> {
        let header = self.header_by_number(number).map_err(ProviderError::other)?;

        Ok(header.map(|b| b.difficulty()))
    }

    fn headers_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn sealed_header(
        &self,
        number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        let block_response = self.block_on_async(async {
            self.provider.get_block_by_number(number.into()).await.map_err(ProviderError::other)
        })?;

        let Some(block_response) = block_response else {
            // If the block was not found, return None
            return Ok(None);
        };
        let block_hash = block_response.header().hash();

        // Convert the network block response to primitive block
        let block = <BlockTy<Node> as TryFromBlockResponse<N>>::from_block_response(block_response)
            .map_err(ProviderError::other)?;

        Ok(Some(SealedHeader::new(block.into_header(), block_hash)))
    }

    fn sealed_headers_while(
        &self,
        _range: impl RangeBounds<BlockNumber>,
        _predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> BlockBodyIndicesProvider for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn block_body_indices(&self, _num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_body_indices_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<StoredBlockBodyIndices>> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> BlockReader for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
    BlockTy<Node>: TryFromBlockResponse<N>,
    TxTy<Node>: TryFromTransactionResponse<N>,
    ReceiptTy<Node>: TryFromReceiptResponse<N>,
{
    type Block = BlockTy<Node>;

    fn find_block_by_hash(
        &self,
        _hash: B256,
        _source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        let block_response = self.block_on_async(async {
            self.provider.get_block(id.into()).full().await.map_err(ProviderError::other)
        })?;

        let Some(block_response) = block_response else {
            // If the block was not found, return None
            return Ok(None);
        };

        // Convert the network block response to primitive block
        let block = <BlockTy<Node> as TryFromBlockResponse<N>>::from_block_response(block_response)
            .map_err(ProviderError::other)?;

        Ok(Some(block))
    }

    fn pending_block(&self) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<(RecoveredBlock<Self::Block>, Vec<Self::Receipt>)>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn recovered_block(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn sealed_block_with_senders(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<RecoveredBlock<Self::Block>>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_range(&self, _range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_with_senders_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn recovered_block_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<RecoveredBlock<Self::Block>>> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> BlockReaderIdExt for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
    BlockTy<Node>: TryFromBlockResponse<N>,
    TxTy<Node>: TryFromTransactionResponse<N>,
    ReceiptTy<Node>: TryFromReceiptResponse<N>,
{
    fn block_by_id(&self, id: BlockId) -> ProviderResult<Option<Self::Block>> {
        match id {
            BlockId::Hash(hash) => self.block_by_hash(hash.block_hash),
            BlockId::Number(number_or_tag) => self.block_by_number_or_tag(number_or_tag),
        }
    }

    fn sealed_header_by_id(
        &self,
        id: BlockId,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        match id {
            BlockId::Hash(hash) => self.sealed_header_by_hash(hash.block_hash),
            BlockId::Number(number_or_tag) => self.sealed_header_by_number_or_tag(number_or_tag),
        }
    }

    fn header_by_id(&self, id: BlockId) -> ProviderResult<Option<Self::Header>> {
        match id {
            BlockId::Hash(hash) => self.header_by_hash_or_number(hash.block_hash.into()),
            BlockId::Number(number_or_tag) => self.header_by_number_or_tag(number_or_tag),
        }
    }
}

impl<P, Node, N> ReceiptProvider for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
    ReceiptTy<Node>: TryFromReceiptResponse<N>,
{
    type Receipt = ReceiptTy<Node>;

    fn receipt(&self, _id: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
        let receipt_response = self.block_on_async(async {
            self.provider.get_transaction_receipt(hash).await.map_err(ProviderError::other)
        })?;

        let Some(receipt_response) = receipt_response else {
            // If the receipt was not found, return None
            return Ok(None);
        };

        // Convert the network receipt response to primitive receipt
        let receipt =
            <ReceiptTy<Node> as TryFromReceiptResponse<N>>::from_receipt_response(receipt_response)
                .map_err(ProviderError::other)?;

        Ok(Some(receipt))
    }

    fn receipts_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        self.block_on_async(async {
            let receipts_response = self
                .provider
                .get_block_receipts(block.into())
                .await
                .map_err(ProviderError::other)?;

            let Some(receipts) = receipts_response else {
                // If the receipts were not found, return None
                return Ok(None);
            };

            // Convert the network receipts response to primitive receipts
            let receipts = receipts
                .into_iter()
                .map(|receipt_response| {
                    <ReceiptTy<Node> as TryFromReceiptResponse<N>>::from_receipt_response(
                        receipt_response,
                    )
                    .map_err(ProviderError::other)
                })
                .collect::<Result<Vec<_>, _>>()?;

            Ok(Some(receipts))
        })
    }

    fn receipts_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn receipts_by_block_range(
        &self,
        _block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Receipt>>> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> ReceiptProviderIdExt for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
    ReceiptTy<Node>: TryFromReceiptResponse<N>,
{
}

impl<P, Node, N> TransactionsProvider for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
    BlockTy<Node>: TryFromBlockResponse<N>,
    TxTy<Node>: TryFromTransactionResponse<N>,
{
    type Transaction = TxTy<Node>;

    fn transaction_id(&self, _tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_by_id(&self, _id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_by_id_unhashed(
        &self,
        _id: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
        let transaction_response = self.block_on_async(async {
            self.provider.get_transaction_by_hash(hash).await.map_err(ProviderError::other)
        })?;

        let Some(transaction_response) = transaction_response else {
            // If the transaction was not found, return None
            return Ok(None);
        };

        // Convert the network transaction response to primitive transaction
        let transaction = <TxTy<Node> as TryFromTransactionResponse<N>>::from_transaction_response(
            transaction_response,
        )
        .map_err(ProviderError::other)?;

        Ok(Some(transaction))
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_block(&self, _id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        let block_response = self.block_on_async(async {
            self.provider.get_block(block.into()).full().await.map_err(ProviderError::other)
        })?;

        let Some(block_response) = block_response else {
            // If the block was not found, return None
            return Ok(None);
        };

        // Convert the network block response to primitive block
        let block = <BlockTy<Node> as TryFromBlockResponse<N>>::from_block_response(block_response)
            .map_err(ProviderError::other)?;

        Ok(Some(block.into_body().into_transactions()))
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn senders_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_sender(&self, _id: TxNumber) -> ProviderResult<Option<Address>> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> StateProviderFactory for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn latest(&self) -> Result<StateProviderBox, ProviderError> {
        Ok(Box::new(self.create_state_provider(self.best_block_number()?.into())))
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

        let block_number = block.header().number();
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
        Err(ProviderError::UnsupportedProvider)
    }

    fn maybe_pending(&self) -> Result<Option<StateProviderBox>, ProviderError> {
        Ok(None)
    }
}

impl<P, Node, N> DatabaseProviderFactory for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    type DB = DatabaseMock;
    type Provider = RpcBlockchainStateProvider<P, Node, N>;
    type ProviderRW = RpcBlockchainStateProvider<P, Node, N>;

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

impl<P, Node, N> CanonChainTracker for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
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

impl<P, Node, N> NodePrimitivesProvider for RpcBlockchainProvider<P, Node, N>
where
    P: Send + Sync,
    N: Send + Sync,
    Node: NodeTypes,
{
    type Primitives = PrimitivesTy<Node>;
}

impl<P, Node, N> CanonStateSubscriptions for RpcBlockchainProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications<PrimitivesTy<Node>> {
        trace!(target: "alloy-provider", "Subscribing to canonical state notifications");
        self.canon_state_notification.subscribe()
    }
}

impl<P, Node, N> ChainSpecProvider for RpcBlockchainProvider<P, Node, N>
where
    P: Send + Sync,
    N: Send + Sync,
    Node: NodeTypes,
    Node::ChainSpec: Default,
{
    type ChainSpec = Node::ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.chain_spec.clone()
    }
}

/// RPC-based state provider implementation that fetches blockchain state via remote RPC calls.
///
/// This is the state provider counterpart to `RpcBlockchainProvider`, handling state queries
/// at specific block heights via RPC instead of local database access.
pub struct RpcBlockchainStateProvider<P, Node, N = alloy_network::AnyNetwork>
where
    Node: NodeTypes,
{
    /// The underlying Alloy provider
    provider: P,
    /// The block ID to fetch state at
    block_id: BlockId,
    /// Node types marker
    node_types: std::marker::PhantomData<Node>,
    /// Network marker
    network: std::marker::PhantomData<N>,
    /// Cached chain spec (shared with parent provider)
    chain_spec: Option<Arc<Node::ChainSpec>>,
    /// Whether to enable state root calculation
    compute_state_root: bool,
    /// Cached bytecode for accounts
    ///
    /// Since the state provider is short-lived, we don't worry about memory leaks.
    code_store: RwLock<HashMap<B256, Bytecode>>,
    /// Whether to use Reth-specific RPC methods for better performance
    reth_rpc_support: bool,
}

impl<P: std::fmt::Debug, Node: NodeTypes, N> std::fmt::Debug
    for RpcBlockchainStateProvider<P, Node, N>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcBlockchainStateProvider")
            .field("provider", &self.provider)
            .field("block_id", &self.block_id)
            .finish()
    }
}

impl<P: Clone, Node: NodeTypes, N> RpcBlockchainStateProvider<P, Node, N> {
    /// Creates a new state provider for the given block
    pub fn new(
        provider: P,
        block_id: BlockId,
        _primitives: std::marker::PhantomData<Node>,
    ) -> Self {
        Self {
            provider,
            block_id,
            node_types: std::marker::PhantomData,
            network: std::marker::PhantomData,
            chain_spec: None,
            compute_state_root: false,
            code_store: RwLock::new(HashMap::default()),
            reth_rpc_support: true,
        }
    }

    /// Creates a new state provider with a cached chain spec
    pub fn with_chain_spec(
        provider: P,
        block_id: BlockId,
        chain_spec: Arc<Node::ChainSpec>,
    ) -> Self {
        Self {
            provider,
            block_id,
            node_types: std::marker::PhantomData,
            network: std::marker::PhantomData,
            chain_spec: Some(chain_spec),
            compute_state_root: false,
            code_store: RwLock::new(HashMap::default()),
            reth_rpc_support: true,
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
            network: self.network,
            chain_spec: self.chain_spec.clone(),
            compute_state_root: self.compute_state_root,
            code_store: RwLock::new(HashMap::default()),
            reth_rpc_support: self.reth_rpc_support,
        }
    }

    /// Helper function to enable state root calculation
    ///
    /// If enabled, the node will compute the state root and updates.
    /// When disabled, it will return zero for state root and no updates.
    pub const fn with_compute_state_root(mut self, is_enable: bool) -> Self {
        self.compute_state_root = is_enable;
        self
    }

    /// Sets whether to use Reth-specific RPC methods for better performance
    ///
    /// If enabled, the node will use Reth's RPC methods (`debug_codeByHash` and
    /// `eth_getAccountInfo`) to speed up account information retrieval. When disabled, it will
    /// use multiple standard RPC calls to get account information.
    pub const fn with_reth_rpc_support(mut self, is_enable: bool) -> Self {
        self.reth_rpc_support = is_enable;
        self
    }

    /// Get account information from RPC
    fn get_account(&self, address: Address) -> Result<Option<Account>, ProviderError>
    where
        P: Provider<N> + Clone + 'static,
        N: Network,
    {
        let account_info = self.block_on_async(async {
            // Get account info in a single RPC call using `eth_getAccountInfo`
            if self.reth_rpc_support {
                return self
                    .provider
                    .get_account_info(address)
                    .block_id(self.block_id)
                    .await
                    .map_err(ProviderError::other);
            }
            // Get account info in multiple RPC calls
            let nonce = self.provider.get_transaction_count(address).block_id(self.block_id);
            let balance = self.provider.get_balance(address).block_id(self.block_id);
            let code = self.provider.get_code_at(address).block_id(self.block_id);

            let (nonce, balance, code) = tokio::join!(nonce, balance, code,);

            let account_info = AccountInfo {
                balance: balance.map_err(ProviderError::other)?,
                nonce: nonce.map_err(ProviderError::other)?,
                code: code.map_err(ProviderError::other)?,
            };

            let code_hash = account_info.code_hash();
            if code_hash != KECCAK_EMPTY {
                // Insert code into the cache
                self.code_store
                    .write()
                    .insert(code_hash, Bytecode::new_raw(account_info.code.clone()));
            }

            Ok(account_info)
        })?;

        // Only return account if it exists (has balance, nonce, or code)
        if account_info.balance.is_zero() && account_info.nonce == 0 && account_info.code.is_empty()
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
    }
}

impl<P, Node, N> StateProvider for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn storage(
        &self,
        address: Address,
        storage_key: StorageKey,
    ) -> Result<Option<U256>, ProviderError> {
        self.block_on_async(async {
            Ok(Some(
                self.provider
                    .get_storage_at(address, storage_key.into())
                    .block_id(self.block_id)
                    .await
                    .map_err(ProviderError::other)?,
            ))
        })
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

impl<P, Node, N> BytecodeReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn bytecode_by_hash(&self, code_hash: &B256) -> Result<Option<Bytecode>, ProviderError> {
        if !self.reth_rpc_support {
            return Ok(self.code_store.read().get(code_hash).cloned());
        }

        self.block_on_async(async {
            // The method `debug_codeByHash` is currently only available on a Reth node
            let code = self
                .provider
                .debug_code_by_hash(*code_hash, None)
                .await
                .map_err(ProviderError::other)?;

            let Some(code) = code else {
                // If the code was not found, return None
                return Ok(None);
            };

            Ok(Some(Bytecode::new_raw(code)))
        })
    }
}

impl<P, Node, N> AccountReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn basic_account(&self, address: &Address) -> Result<Option<Account>, ProviderError> {
        self.get_account(*address)
    }
}

impl<P, Node, N> StateRootProvider for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn state_root(&self, hashed_state: HashedPostState) -> Result<B256, ProviderError> {
        self.state_root_from_nodes(TrieInput::from_state(hashed_state))
    }

    fn state_root_from_nodes(&self, _input: TrieInput) -> Result<B256, ProviderError> {
        warn!("state_root_from_nodes is not implemented and will return zero");
        Ok(B256::ZERO)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> Result<(B256, TrieUpdates), ProviderError> {
        if !self.compute_state_root {
            return Ok((B256::ZERO, TrieUpdates::default()));
        }

        self.block_on_async(async {
            self.provider
                .raw_request::<(HashedPostState, BlockId), (B256, TrieUpdates)>(
                    "debug_stateRootWithUpdates".into(),
                    (hashed_state, self.block_id),
                )
                .into_future()
                .await
                .map_err(ProviderError::other)
        })
    }

    fn state_root_from_nodes_with_updates(
        &self,
        _input: TrieInput,
    ) -> Result<(B256, TrieUpdates), ProviderError> {
        warn!("state_root_from_nodes_with_updates is not implemented and will return zero");
        Ok((B256::ZERO, TrieUpdates::default()))
    }
}

impl<P, Node, N> StorageReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
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

impl<P, Node, N> reth_storage_api::StorageRootProvider for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
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

impl<P, Node, N> reth_storage_api::StateProofProvider for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
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

impl<P, Node, N> reth_storage_api::HashedPostStateProvider
    for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn hashed_post_state(&self, _bundle_state: &revm::database::BundleState) -> HashedPostState {
        // Return empty hashed post state for RPC provider
        HashedPostState::default()
    }
}

impl<P, Node, N> StateReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    type Receipt = ReceiptTy<Node>;

    fn get_state(
        &self,
        _block: BlockNumber,
    ) -> Result<Option<reth_execution_types::ExecutionOutcome<Self::Receipt>>, ProviderError> {
        // RPC doesn't provide execution outcomes
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> DBProvider for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
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

    fn disable_long_read_transaction_safety(self) -> Self {
        // No-op for RPC provider
        self
    }

    fn prune_modes_ref(&self) -> &reth_prune_types::PruneModes {
        unimplemented!("prune modes not supported for RPC provider")
    }
}

impl<P, Node, N> BlockNumReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn chain_info(&self) -> Result<ChainInfo, ProviderError> {
        self.block_on_async(async {
            let block = self
                .provider
                .get_block(self.block_id)
                .await
                .map_err(ProviderError::other)?
                .ok_or(ProviderError::HeaderNotFound(0.into()))?;

            Ok(ChainInfo { best_hash: block.header().hash(), best_number: block.header().number() })
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

            Ok(block.map(|b| b.header().number()))
        })
    }
}

impl<P, Node, N> BlockHashReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn block_hash(&self, number: u64) -> Result<Option<B256>, ProviderError> {
        self.block_on_async(async {
            let block = self
                .provider
                .get_block_by_number(number.into())
                .await
                .map_err(ProviderError::other)?;

            Ok(block.map(|b| b.header().hash()))
        })
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> Result<Vec<B256>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> BlockIdReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn block_number_for_id(
        &self,
        _block_id: BlockId,
    ) -> Result<Option<BlockNumber>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn pending_block_num_hash(&self) -> Result<Option<alloy_eips::BlockNumHash>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn safe_block_num_hash(&self) -> Result<Option<alloy_eips::BlockNumHash>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn finalized_block_num_hash(&self) -> Result<Option<alloy_eips::BlockNumHash>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> BlockReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    type Block = BlockTy<Node>;

    fn find_block_by_hash(
        &self,
        _hash: B256,
        _source: reth_provider::BlockSource,
    ) -> Result<Option<Self::Block>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn block(
        &self,
        _id: alloy_rpc_types::BlockHashOrNumber,
    ) -> Result<Option<Self::Block>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn pending_block(&self) -> Result<Option<RecoveredBlock<Self::Block>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn pending_block_and_receipts(
        &self,
    ) -> Result<Option<(RecoveredBlock<Self::Block>, Vec<Self::Receipt>)>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn recovered_block(
        &self,
        _id: alloy_rpc_types::BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> Result<Option<RecoveredBlock<Self::Block>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn sealed_block_with_senders(
        &self,
        _id: alloy_rpc_types::BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> Result<Option<RecoveredBlock<BlockTy<Node>>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> Result<Vec<Self::Block>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_with_senders_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> Result<Vec<RecoveredBlock<BlockTy<Node>>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn recovered_block_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> Result<Vec<RecoveredBlock<Self::Block>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> TransactionsProvider for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    type Transaction = TxTy<Node>;

    fn transaction_id(&self, _tx_hash: B256) -> Result<Option<TxNumber>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_by_id(&self, _id: TxNumber) -> Result<Option<Self::Transaction>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_by_id_unhashed(
        &self,
        _id: TxNumber,
    ) -> Result<Option<Self::Transaction>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_by_hash(&self, _hash: B256) -> Result<Option<Self::Transaction>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: B256,
    ) -> Result<Option<(Self::Transaction, TransactionMeta)>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_block(&self, _id: TxNumber) -> Result<Option<BlockNumber>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_block(
        &self,
        _block: alloy_rpc_types::BlockHashOrNumber,
    ) -> Result<Option<Vec<Self::Transaction>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<Vec<Self::Transaction>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transactions_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> Result<Vec<Self::Transaction>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn senders_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> Result<Vec<Address>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn transaction_sender(&self, _id: TxNumber) -> Result<Option<Address>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> ReceiptProvider for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    type Receipt = ReceiptTy<Node>;

    fn receipt(&self, _id: TxNumber) -> Result<Option<Self::Receipt>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn receipt_by_hash(&self, _hash: B256) -> Result<Option<Self::Receipt>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn receipts_by_block(
        &self,
        _block: alloy_rpc_types::BlockHashOrNumber,
    ) -> Result<Option<Vec<Self::Receipt>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn receipts_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> Result<Vec<Self::Receipt>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn receipts_by_block_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> Result<Vec<Vec<Self::Receipt>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> HeaderProvider for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    type Header = HeaderTy<Node>;

    fn header(&self, _block_hash: &BlockHash) -> Result<Option<Self::Header>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn header_by_number(&self, _num: BlockNumber) -> Result<Option<Self::Header>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn header_td(&self, _hash: &BlockHash) -> Result<Option<U256>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> Result<Option<U256>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn headers_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<Self::Header>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn sealed_header(
        &self,
        _number: BlockNumber,
    ) -> Result<Option<SealedHeader<HeaderTy<Node>>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn sealed_headers_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<SealedHeader<HeaderTy<Node>>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn sealed_headers_while(
        &self,
        _range: impl RangeBounds<BlockNumber>,
        _predicate: impl FnMut(&SealedHeader<HeaderTy<Node>>) -> bool,
    ) -> Result<Vec<SealedHeader<HeaderTy<Node>>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> PruneCheckpointReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn get_prune_checkpoint(
        &self,
        _segment: PruneSegment,
    ) -> Result<Option<PruneCheckpoint>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn get_prune_checkpoints(&self) -> Result<Vec<(PruneSegment, PruneCheckpoint)>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> StageCheckpointReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn get_stage_checkpoint(&self, _id: StageId) -> Result<Option<StageCheckpoint>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn get_stage_checkpoint_progress(
        &self,
        _id: StageId,
    ) -> Result<Option<Vec<u8>>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn get_all_checkpoints(&self) -> Result<Vec<(String, StageCheckpoint)>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> ChangeSetReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn account_block_changeset(
        &self,
        _block_number: BlockNumber,
    ) -> Result<Vec<reth_db_api::models::AccountBeforeTx>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> StateProviderFactory for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static + Send + Sync,
    Node: NodeTypes + 'static,
    Node::ChainSpec: Send + Sync,
    N: Network,
    Self: Clone + 'static,
{
    fn latest(&self) -> Result<StateProviderBox, ProviderError> {
        Ok(Box::new(self.with_block_id(self.best_block_number()?.into())))
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
        Err(ProviderError::UnsupportedProvider)
    }

    fn maybe_pending(&self) -> ProviderResult<Option<StateProviderBox>> {
        Ok(None)
    }
}

impl<P, Node, N> ChainSpecProvider for RpcBlockchainStateProvider<P, Node, N>
where
    P: Send + Sync + std::fmt::Debug,
    N: Send + Sync,
    Node: NodeTypes,
    Node::ChainSpec: Default,
{
    type ChainSpec = Node::ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        if let Some(chain_spec) = &self.chain_spec {
            chain_spec.clone()
        } else {
            // Fallback for when chain_spec is not provided
            Arc::new(Node::ChainSpec::default())
        }
    }
}

// Note: FullExecutionDataProvider is already implemented via the blanket implementation
// for types that implement both ExecutionDataProvider and BlockExecutionForkProvider

impl<P, Node, N> StatsReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn count_entries<T: reth_db_api::table::Table>(&self) -> Result<usize, ProviderError> {
        Ok(0)
    }
}

impl<P, Node, N> BlockBodyIndicesProvider for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn block_body_indices(
        &self,
        _num: u64,
    ) -> Result<Option<reth_db_api::models::StoredBlockBodyIndices>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn block_body_indices_range(
        &self,
        _range: RangeInclusive<u64>,
    ) -> Result<Vec<reth_db_api::models::StoredBlockBodyIndices>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> NodePrimitivesProvider for RpcBlockchainStateProvider<P, Node, N>
where
    P: Send + Sync + std::fmt::Debug,
    N: Send + Sync,
    Node: NodeTypes,
{
    type Primitives = PrimitivesTy<Node>;
}

impl<P, Node, N> ChainStateBlockReader for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn last_finalized_block_number(&self) -> Result<Option<BlockNumber>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn last_safe_block_number(&self) -> Result<Option<BlockNumber>, ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}

impl<P, Node, N> ChainStateBlockWriter for RpcBlockchainStateProvider<P, Node, N>
where
    P: Provider<N> + Clone + 'static,
    N: Network,
    Node: NodeTypes,
{
    fn save_finalized_block_number(&self, _block_number: BlockNumber) -> Result<(), ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }

    fn save_safe_block_number(&self, _block_number: BlockNumber) -> Result<(), ProviderError> {
        Err(ProviderError::UnsupportedProvider)
    }
}
