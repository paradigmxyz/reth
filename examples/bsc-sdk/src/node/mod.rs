use crate::{chainspec::BscChainSpec, evm::transaction::BscTransaction};
use alloy_consensus::{transaction::TransactionMeta, BlockHeader, Transaction, TxEnvelope};
use alloy_network::{Ethereum, Network};
use alloy_primitives::{Bytes, PrimitiveSignature, TxKind, B256, U256};
use alloy_rpc_types::{BlockId, TransactionInfo, TransactionRequest};
use alloy_rpc_types_eth::TransactionReceipt;
use evm::BscExecutorBuilder;
use network::BscNetworkBuilder;
use payload::builder::BscPayloadBuilder;
use pool::BscPoolBuilder;
use reth::{
    api::{FullNodeComponents, FullNodeTypes, NodeTypes},
    beacon_consensus::EthBeaconConsensus,
    builder::{
        components::{BasicPayloadServiceBuilder, ComponentsBuilder, ConsensusBuilder},
        rpc::{EngineValidatorBuilder, EthApiBuilder, EthApiCtx, RpcAddOns},
        AddOnsContext, BuilderContext, DebugNode, Node, NodeAdapter, NodeComponentsBuilder,
    },
    consensus::{ConsensusError, FullConsensus},
    primitives::{EthereumHardforks, NodePrimitives, Receipt, Recovered, SealedHeader},
    rpc::{
        api::eth::{
            helpers::{
                AddDevSigners, EthApiSpec, EthSigner, EthState, LoadFee, LoadState, SpawnBlocking,
            },
            RpcNodeCoreExt,
        },
        compat::TransactionCompat,
        eth::{DevSigner, EthApiTypes, FullEthApiServer, RpcNodeCore},
        server_types::eth::{
            revm_utils::CallFees, utils::recover_raw_transaction, EthApiError, EthReceiptBuilder,
            EthStateCache, FeeHistoryCache, GasPriceOracle, PendingBlock,
            RpcInvalidTransactionError,
        },
    },
    tasks::{
        pool::{BlockingTaskGuard, BlockingTaskPool},
        TaskSpawner,
    },
    transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool},
};
use reth_chainspec::EthChainSpec;
use reth_evm::{
    block::BlockExecutorFactory, ConfigureEvm, EvmEnv, EvmFactory, NextBlockEnvAttributes, SpecFor,
};
use reth_network::NetworkInfo;
use reth_node_ethereum::{EthEngineTypes, EthereumEngineValidator};
use reth_optimism_rpc::eth::EthApiNodeBackend;
use reth_primitives::{Block, BlockBody, EthPrimitives, TransactionSigned, TxType};
use reth_primitives_traits::{BlockBody as _, SignedTransaction};
use reth_provider::{
    providers::ProviderFactoryBuilder, BlockNumReader, BlockReader, BlockReaderIdExt,
    CanonStateSubscriptions, ChainSpecProvider, EthStorage, HeaderProvider, ProviderBlock,
    ProviderHeader, ProviderReceipt, ProviderTx, ReceiptProvider, StageCheckpointReader,
    StateProviderFactory, TransactionsProvider,
};
use reth_rpc_eth_api::{
    helpers::{
        estimate::EstimateCall, Call, EthBlocks, EthCall, EthFees, EthTransactions, LoadBlock,
        LoadPendingBlock, LoadReceipt, LoadTransaction, Trace,
    },
    types::RpcTypes,
    FromEthApiError, FromEvmError, FullEthApiTypes, IntoEthApiError, RpcReceipt,
};
use reth_trie_db::MerklePatriciaTrie;
use revm::{
    context::{Block as _, TxEnv},
    database::Database,
};
use std::{fmt, sync::Arc};

pub mod evm;
pub mod network;
pub mod payload;
pub mod pool;

/// Bsc addons configuring RPC types
pub type BscNodeAddOns<N> = RpcAddOns<N, BscEthApiBuilder, BscEngineValidatorBuilder>;

/// A basic ethereum consensus builder.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct BscConsensusBuilder;

impl<Node> ConsensusBuilder<Node> for BscConsensusBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = BscChainSpec, Primitives = EthPrimitives>>,
{
    type Consensus = Arc<dyn FullConsensus<EthPrimitives, Error = ConsensusError>>;

    async fn build_consensus(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        Ok(Arc::new(EthBeaconConsensus::new(ctx.chain_spec())))
    }
}

/// Builder for [`EthereumEngineValidator`].
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct BscEngineValidatorBuilder;

impl<Node, Types> EngineValidatorBuilder<Node> for BscEngineValidatorBuilder
where
    Types:
        NodeTypes<ChainSpec = BscChainSpec, Payload = EthEngineTypes, Primitives = EthPrimitives>,
    Node: FullNodeComponents<Types = Types>,
{
    type Validator = EthereumEngineValidator;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(EthereumEngineValidator::new(Arc::new(ctx.config.chain.clone().as_ref().clone().into())))
    }
}

/// A helper trait with requirements for [`RpcNodeCore`] to be used in [`BscEthApi`].
pub trait BscNodeCore: RpcNodeCore<Provider: BlockReader> {}
impl<T> BscNodeCore for T where T: RpcNodeCore<Provider: BlockReader> {}

/// Container type `BscEthApi`
#[allow(missing_debug_implementations)]
struct BscEthApiInner<N: BscNodeCore> {
    /// Gateway to node's core components.
    eth_api: EthApiNodeBackend<N>,
}

#[derive(Clone)]
pub struct BscEthApi<N: BscNodeCore> {
    /// Gateway to node's core components.
    inner: Arc<BscEthApiInner<N>>,
}

impl<N> BscEthApi<N>
where
    N: BscNodeCore<
        Provider: BlockReaderIdExt
                      + ChainSpecProvider
                      + CanonStateSubscriptions<Primitives = EthPrimitives>
                      + Clone
                      + 'static,
    >,
{
    /// Returns a reference to the [`EthApiNodeBackend`].
    pub fn eth_api(&self) -> &EthApiNodeBackend<N> {
        &self.inner.eth_api
    }

    /// Build a [`BscEthApi`] using [`BscEthApiBuilder`].
    pub fn builder() -> BscEthApiBuilder {
        BscEthApiBuilder::default()
    }
}

impl<N> EthApiTypes for BscEthApi<N>
where
    Self: Send + Sync,
    N: BscNodeCore,
{
    type Error = EthApiError;
    type NetworkTypes = Ethereum;
    type TransactionCompat = Self;

    fn tx_resp_builder(&self) -> &Self::TransactionCompat {
        self
    }
}

impl<N> RpcNodeCore for BscEthApi<N>
where
    N: BscNodeCore,
{
    type Primitives = EthPrimitives;
    type Provider = N::Provider;
    type Pool = N::Pool;
    type Evm = <N as RpcNodeCore>::Evm;
    type Network = <N as RpcNodeCore>::Network;
    type PayloadBuilder = ();

    #[inline]
    fn pool(&self) -> &Self::Pool {
        self.inner.eth_api.pool()
    }

    #[inline]
    fn evm_config(&self) -> &Self::Evm {
        self.inner.eth_api.evm_config()
    }

    #[inline]
    fn network(&self) -> &Self::Network {
        self.inner.eth_api.network()
    }

    #[inline]
    fn payload_builder(&self) -> &Self::PayloadBuilder {
        &()
    }

    #[inline]
    fn provider(&self) -> &Self::Provider {
        self.inner.eth_api.provider()
    }
}

impl<N> RpcNodeCoreExt for BscEthApi<N>
where
    N: BscNodeCore,
{
    #[inline]
    fn cache(&self) -> &EthStateCache<ProviderBlock<N::Provider>, ProviderReceipt<N::Provider>> {
        self.inner.eth_api.cache()
    }
}

impl<N> EthApiSpec for BscEthApi<N>
where
    N: BscNodeCore<
        Provider: ChainSpecProvider<ChainSpec: EthereumHardforks>
                      + BlockNumReader
                      + StageCheckpointReader,
        Network: NetworkInfo,
    >,
{
    type Transaction = ProviderTx<Self::Provider>;

    #[inline]
    fn starting_block(&self) -> U256 {
        self.inner.eth_api.starting_block()
    }

    #[inline]
    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner<ProviderTx<Self::Provider>>>>> {
        self.inner.eth_api.signers()
    }
}

impl<N> SpawnBlocking for BscEthApi<N>
where
    Self: Send + Sync + Clone + 'static,
    N: BscNodeCore,
{
    #[inline]
    fn io_task_spawner(&self) -> impl TaskSpawner {
        self.inner.eth_api.task_spawner()
    }

    #[inline]
    fn tracing_task_pool(&self) -> &BlockingTaskPool {
        self.inner.eth_api.blocking_task_pool()
    }

    #[inline]
    fn tracing_task_guard(&self) -> &BlockingTaskGuard {
        self.inner.eth_api.blocking_task_guard()
    }
}

impl<N> LoadFee for BscEthApi<N>
where
    Self: LoadBlock<Provider = N::Provider>,
    N: BscNodeCore<
        Provider: BlockReaderIdExt
                      + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                      + StateProviderFactory,
    >,
{
    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<Self::Provider> {
        self.inner.eth_api.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache {
        self.inner.eth_api.fee_history_cache()
    }
}

impl<N> LoadState for BscEthApi<N> where
    N: BscNodeCore<
        Provider: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks>,
        Pool: TransactionPool,
    >
{
}

impl<N> EthState for BscEthApi<N>
where
    Self: LoadState + SpawnBlocking,
    N: BscNodeCore,
{
    #[inline]
    fn max_proof_window(&self) -> u64 {
        self.inner.eth_api.eth_proof_window()
    }
}

impl<N> EthFees for BscEthApi<N>
where
    Self: LoadFee,
    N: BscNodeCore,
{
}

impl<N> Trace for BscEthApi<N>
where
    Self: RpcNodeCore<Provider: BlockReader>
        + LoadState<
            Evm: ConfigureEvm<
                Primitives: NodePrimitives<
                    BlockHeader = ProviderHeader<Self::Provider>,
                    SignedTx = ProviderTx<Self::Provider>,
                >,
            >,
            Error: FromEvmError<Self::Evm>,
        >,
    N: BscNodeCore,
{
}

impl<N> AddDevSigners for BscEthApi<N>
where
    N: BscNodeCore,
{
    fn with_dev_accounts(&self) {
        *self.inner.eth_api.signers().write() = DevSigner::random_signers(20)
    }
}

impl<N> LoadTransaction for BscEthApi<N>
where
    Self: SpawnBlocking + FullEthApiTypes + RpcNodeCoreExt,
    N: BscNodeCore<Provider: TransactionsProvider, Pool: TransactionPool>,
    Self::Pool: TransactionPool,
{
}

impl<N: BscNodeCore> fmt::Debug for BscEthApi<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BscEthApi").finish_non_exhaustive()
    }
}

impl<N> TransactionCompat<TransactionSigned> for BscEthApi<N>
where
    N: FullNodeComponents<Provider: ReceiptProvider<Receipt = Receipt>>,
{
    type Transaction = <Ethereum as Network>::TransactionResponse;

    type Error = EthApiError;

    fn fill(
        &self,
        tx: Recovered<TransactionSigned>,
        tx_info: TransactionInfo,
    ) -> Result<Self::Transaction, Self::Error> {
        let tx = tx.convert::<TxEnvelope>();

        let TransactionInfo {
            block_hash, block_number, index: transaction_index, base_fee, ..
        } = tx_info;

        let effective_gas_price = base_fee
            .map(|base_fee| {
                tx.effective_tip_per_gas(base_fee).unwrap_or_default() + base_fee as u128
            })
            .unwrap_or_else(|| tx.max_fee_per_gas());

        Ok(alloy_rpc_types::Transaction {
            inner: tx,
            block_hash,
            block_number,
            transaction_index,
            effective_gas_price: Some(effective_gas_price),
        })
    }

    fn build_simulate_v1_transaction(
        &self,
        request: TransactionRequest,
    ) -> Result<TransactionSigned, Self::Error> {
        let Ok(tx) = request.build_typed_tx() else {
            return Err(EthApiError::TransactionConversionError)
        };

        // Create an empty signature for the transaction.
        let signature = PrimitiveSignature::new(Default::default(), Default::default(), false);
        Ok(TransactionSigned::new_unhashed(tx.into(), signature))
    }

    fn otterscan_api_truncate_input(tx: &mut Self::Transaction) {
        let input = tx.inner.inner_mut().input_mut();
        *input = input.slice(..4);
    }
}

impl<N> EthTransactions for BscEthApi<N>
where
    Self: LoadTransaction<Provider: BlockReaderIdExt>,
    N: BscNodeCore<Provider: BlockReader<Transaction = ProviderTx<Self::Provider>>>,
{
    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner<ProviderTx<Self::Provider>>>>> {
        self.inner.eth_api.signers()
    }

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    async fn send_raw_transaction(&self, tx: Bytes) -> Result<B256, Self::Error> {
        let recovered = recover_raw_transaction(&tx)?;

        // broadcast raw transaction to subscribers if there is any.
        self.inner.eth_api.broadcast_raw_transaction(tx);

        let pool_transaction = <Self::Pool as TransactionPool>::Transaction::from_pooled(recovered);

        // submit the transaction to the pool with a `Local` origin
        let hash = self
            .pool()
            .add_transaction(TransactionOrigin::Local, pool_transaction)
            .await
            .map_err(Self::Error::from_eth_err)?;

        Ok(hash)
    }
}

impl<N> EthBlocks for BscEthApi<N>
where
    Self: LoadBlock<
        Error = EthApiError,
        NetworkTypes: RpcTypes<Receipt = TransactionReceipt>,
        Provider: BlockReader<Transaction = TransactionSigned, Receipt = Receipt>,
    >,
    N: BscNodeCore<Provider: ChainSpecProvider<ChainSpec = BscChainSpec> + HeaderProvider>,
{
    async fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> Result<Option<Vec<RpcReceipt<Self::NetworkTypes>>>, Self::Error>
    where
        Self: LoadReceipt,
    {
        if let Some((block, receipts)) = self.load_block_and_receipts(block_id).await? {
            let block_number = block.number();
            let base_fee = block.base_fee_per_gas();
            let block_hash = block.hash();
            let excess_blob_gas = block.excess_blob_gas();
            let timestamp = block.timestamp();
            let blob_params = self.provider().chain_spec().blob_params_at_timestamp(timestamp);

            return block
                .body()
                .transactions()
                .iter()
                .zip(receipts.iter())
                .enumerate()
                .map(|(idx, (tx, receipt))| {
                    let meta = TransactionMeta {
                        tx_hash: *tx.tx_hash(),
                        index: idx as u64,
                        block_hash,
                        block_number,
                        base_fee,
                        excess_blob_gas,
                        timestamp,
                    };
                    EthReceiptBuilder::new(tx, meta, receipt, &receipts, blob_params)
                        .map(|builder| builder.build())
                })
                .collect::<Result<Vec<_>, Self::Error>>()
                .map(Some)
        }

        Ok(None)
    }
}

impl<N> EthCall for BscEthApi<N>
where
    Self: EstimateCall + LoadBlock + FullEthApiTypes,
    N: BscNodeCore,
{
}

impl<N> EstimateCall for BscEthApi<N>
where
    Self: Call,
    Self::Error: From<EthApiError>,
    N: BscNodeCore,
{
}

impl<N> Call for BscEthApi<N>
where
    Self: LoadState<
            Evm: ConfigureEvm<
                Primitives: NodePrimitives<
                    BlockHeader = ProviderHeader<Self::Provider>,
                    SignedTx = ProviderTx<Self::Provider>,
                >,
                BlockExecutorFactory: BlockExecutorFactory<
                    EvmFactory: EvmFactory<Tx = BscTransaction<TxEnv>>,
                >,
            >,
            Error: FromEvmError<Self::Evm>,
        > + SpawnBlocking,
    Self::Error: From<EthApiError>,
    N: BscNodeCore,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.eth_api.gas_cap()
    }

    #[inline]
    fn max_simulate_blocks(&self) -> u64 {
        self.inner.eth_api.max_simulate_blocks()
    }

    fn create_txn_env(
        &self,
        evm_env: &EvmEnv<SpecFor<Self::Evm>>,
        request: TransactionRequest,
        mut db: impl Database<Error: Into<EthApiError>>,
    ) -> Result<BscTransaction<TxEnv>, Self::Error> {
        // Ensure that if versioned hashes are set, they're not empty
        if request.blob_versioned_hashes.as_ref().is_some_and(|hashes| hashes.is_empty()) {
            return Err(RpcInvalidTransactionError::BlobTransactionMissingBlobHashes.into_eth_err())
        }

        let tx_type = if request.authorization_list.is_some() {
            TxType::Eip7702
        } else if request.sidecar.is_some() || request.max_fee_per_blob_gas.is_some() {
            TxType::Eip4844
        } else if request.max_fee_per_gas.is_some() || request.max_priority_fee_per_gas.is_some() {
            TxType::Eip1559
        } else if request.access_list.is_some() {
            TxType::Eip2930
        } else {
            TxType::Legacy
        } as u8;

        let TransactionRequest {
            from,
            to,
            gas_price,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            gas,
            value,
            input,
            nonce,
            access_list,
            chain_id,
            blob_versioned_hashes,
            max_fee_per_blob_gas,
            authorization_list,
            transaction_type: _,
            sidecar: _,
        } = request;

        let CallFees { max_priority_fee_per_gas, gas_price, max_fee_per_blob_gas } =
            CallFees::ensure_fees(
                gas_price.map(U256::from),
                max_fee_per_gas.map(U256::from),
                max_priority_fee_per_gas.map(U256::from),
                U256::from(evm_env.block_env.basefee),
                blob_versioned_hashes.as_deref(),
                max_fee_per_blob_gas.map(U256::from),
                evm_env.block_env.blob_gasprice().map(U256::from),
            )?;

        let gas_limit = gas.unwrap_or(
            // Use maximum allowed gas limit. The reason for this
            // is that both Erigon and Geth use pre-configured gas cap even if
            // it's possible to derive the gas limit from the block:
            // <https://github.com/ledgerwatch/erigon/blob/eae2d9a79cb70dbe30b3a6b79c436872e4605458/cmd/rpcdaemon/commands/trace_adhoc.go#L956
            // https://github.com/ledgerwatch/erigon/blob/eae2d9a79cb70dbe30b3a6b79c436872e4605458/eth/ethconfig/config.go#L94>
            evm_env.block_env.gas_limit,
        );

        let chain_id = chain_id.unwrap_or(evm_env.cfg_env.chain_id);

        let caller = from.unwrap_or_default();

        let nonce = if let Some(nonce) = nonce {
            nonce
        } else {
            db.basic(caller).map_err(Into::into)?.map(|acc| acc.nonce).unwrap_or_default()
        };

        let env = TxEnv {
            tx_type,
            gas_limit,
            nonce,
            caller,
            gas_price: gas_price.saturating_to(),
            gas_priority_fee: max_priority_fee_per_gas.map(|v| v.saturating_to()),
            kind: to.unwrap_or(TxKind::Create),
            value: value.unwrap_or_default(),
            data: input
                .try_into_unique_input()
                .map_err(Self::Error::from_eth_err)?
                .unwrap_or_default(),
            chain_id: Some(chain_id),
            access_list: access_list.unwrap_or_default(),
            // EIP-4844 fields
            blob_hashes: blob_versioned_hashes.unwrap_or_default(),
            max_fee_per_blob_gas: max_fee_per_blob_gas
                .map(|v| v.saturating_to())
                .unwrap_or_default(),
            // EIP-7702 fields
            authorization_list: authorization_list.unwrap_or_default(),
        };

        Ok(BscTransaction::new(env))
    }
}

impl<N> LoadBlock for BscEthApi<N>
where
    Self: LoadPendingBlock + SpawnBlocking + RpcNodeCoreExt,
    N: BscNodeCore,
{
}

impl<N> LoadPendingBlock for BscEthApi<N>
where
    Self: SpawnBlocking
        + EthApiTypes<
            NetworkTypes: RpcTypes<
                Header = alloy_rpc_types_eth::Header<ProviderHeader<Self::Provider>>,
            >,
            Error: FromEvmError<Self::Evm>,
        >,
    N: RpcNodeCore<
        Provider: BlockReaderIdExt<
            Transaction = TransactionSigned,
            Block = Block,
            Receipt = Receipt,
            Header = alloy_consensus::Header,
        > + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                      + StateProviderFactory,
        Pool: TransactionPool<Transaction: PoolTransaction<Consensus = ProviderTx<N::Provider>>>,
        Evm: ConfigureEvm<
            Primitives: NodePrimitives<
                SignedTx = ProviderTx<Self::Provider>,
                BlockHeader = ProviderHeader<Self::Provider>,
                Receipt = ProviderReceipt<Self::Provider>,
                Block = ProviderBlock<Self::Provider>,
            >,
            NextBlockEnvCtx = NextBlockEnvAttributes,
        >,
    >,
{
    #[inline]
    fn pending_block(
        &self,
    ) -> &tokio::sync::Mutex<
        Option<PendingBlock<ProviderBlock<Self::Provider>, ProviderReceipt<Self::Provider>>>,
    > {
        self.inner.eth_api.pending_block()
    }

    fn next_env_attributes(
        &self,
        parent: &SealedHeader<ProviderHeader<Self::Provider>>,
    ) -> Result<<Self::Evm as reth_evm::ConfigureEvm>::NextBlockEnvCtx, Self::Error> {
        Ok(NextBlockEnvAttributes {
            timestamp: parent.timestamp().saturating_add(12),
            suggested_fee_recipient: parent.beneficiary(),
            prev_randao: B256::random(),
            gas_limit: parent.gas_limit(),
            parent_beacon_block_root: parent.parent_beacon_block_root(),
            withdrawals: None,
        })
    }
}

impl<N> LoadReceipt for BscEthApi<N>
where
    Self: Send + Sync,
    N: FullNodeComponents<Types: NodeTypes<ChainSpec = BscChainSpec>>,
    Self::Provider:
        TransactionsProvider<Transaction = TransactionSigned> + ReceiptProvider<Receipt = Receipt>,
{
    async fn build_transaction_receipt(
        &self,
        tx: TransactionSigned,
        meta: TransactionMeta,
        receipt: Receipt,
    ) -> Result<RpcReceipt<Self::NetworkTypes>, Self::Error> {
        let hash = meta.block_hash;
        // get all receipts for the block
        let all_receipts = self
            .cache()
            .get_receipts(hash)
            .await
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(hash.into()))?;
        let blob_params = self.provider().chain_spec().blob_params_at_timestamp(meta.timestamp);

        Ok(EthReceiptBuilder::new(&tx, meta, &receipt, &all_receipts, blob_params)?.build())
    }
}

/// Builds [`BscEthApi`] for BSC.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct BscEthApiBuilder;

impl<N> EthApiBuilder<N> for BscEthApiBuilder
where
    N: FullNodeComponents,
    BscEthApi<N>: FullEthApiServer<Provider = N::Provider, Pool = N::Pool>,
{
    type EthApi = BscEthApi<N>;

    fn build_eth_api(self, ctx: EthApiCtx<'_, N>) -> Self::EthApi {
        let eth_api = reth::rpc::eth::EthApiBuilder::new(
            ctx.components.provider().clone(),
            ctx.components.pool().clone(),
            ctx.components.network().clone(),
            ctx.components.evm_config().clone(),
        )
        .eth_cache(ctx.cache)
        .task_spawner(ctx.components.task_executor().clone())
        .gas_cap(ctx.config.rpc_gas_cap.into())
        .max_simulate_blocks(ctx.config.rpc_max_simulate_blocks)
        .eth_proof_window(ctx.config.eth_proof_window)
        .fee_history_cache_config(ctx.config.fee_history_cache)
        .proof_permits(ctx.config.proof_permits)
        .build_inner();

        BscEthApi { inner: Arc::new(BscEthApiInner { eth_api }) }
    }
}

/// Type configuration for a regular BSC node.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct BscNode;

impl BscNode {
    pub fn components<Node>(
        &self,
    ) -> ComponentsBuilder<
        Node,
        BscPoolBuilder,
        BasicPayloadServiceBuilder<BscPayloadBuilder>,
        BscNetworkBuilder,
        BscExecutorBuilder,
        BscConsensusBuilder,
    >
    where
        Node: FullNodeTypes<
            Types: NodeTypes<
                Payload = EthEngineTypes,
                ChainSpec = BscChainSpec,
                Primitives = EthPrimitives,
            >,
        >,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(BscPoolBuilder::default())
            .payload(BasicPayloadServiceBuilder::default())
            .network(BscNetworkBuilder::default())
            .executor(BscExecutorBuilder::default())
            .consensus(BscConsensusBuilder::default())
    }

    pub fn provider_factory_builder() -> ProviderFactoryBuilder<Self> {
        ProviderFactoryBuilder::default()
    }
}

impl<N> Node<N> for BscNode
where
    N: FullNodeTypes<
        Types: NodeTypes<
            Payload = EthEngineTypes,
            ChainSpec = BscChainSpec,
            Primitives = EthPrimitives,
            Storage = EthStorage,
        >,
    >,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        BscPoolBuilder,
        BasicPayloadServiceBuilder<BscPayloadBuilder>,
        BscNetworkBuilder,
        BscExecutorBuilder,
        BscConsensusBuilder,
    >;

    type AddOns = BscNodeAddOns<
        NodeAdapter<N, <Self::ComponentsBuilder as NodeComponentsBuilder<N>>::Components>,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        Self::components(self)
    }

    fn add_ons(&self) -> Self::AddOns {
        BscNodeAddOns::default()
    }
}

impl<N> DebugNode<N> for BscNode
where
    N: FullNodeComponents<Types = Self>,
{
    type RpcBlock = alloy_rpc_types::Block;

    fn rpc_to_primitive_block(rpc_block: Self::RpcBlock) -> Block {
        let alloy_rpc_types::Block { header, transactions, withdrawals, .. } = rpc_block;
        Block {
            header: header.inner,
            body: BlockBody {
                transactions: transactions
                    .into_transactions()
                    .map(|tx| tx.inner.into_inner().into())
                    .collect(),
                ommers: Default::default(),
                withdrawals,
            },
        }
    }
}

impl NodeTypes for BscNode {
    type Primitives = EthPrimitives;
    type ChainSpec = BscChainSpec;
    type StateCommitment = MerklePatriciaTrie;
    type Storage = EthStorage;
    type Payload = EthEngineTypes;
}
