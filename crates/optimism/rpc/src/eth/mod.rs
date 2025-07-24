//! OP-Reth `eth_` endpoint implementation.

pub mod ext;
pub mod receipt;
pub mod transaction;

mod block;
mod call;
mod pending_block;

use crate::{
    eth::{receipt::OpReceiptConverter, transaction::OpTxInfoMapper},
    OpEthApiError, SequencerClient,
};
use alloy_consensus::transaction::Recovered;
use alloy_primitives::U256;
use alloy_rpc_types_eth::TransactionInfo;
use eyre::WrapErr;
use op_alloy_network::Optimism;
use op_alloy_rpc_types::Transaction;
pub use receipt::{OpReceiptBuilder, OpReceiptFieldsBuilder};
use reth_evm::ConfigureEvm;
use reth_node_api::{FullNodeComponents, FullNodeTypes, HeaderTy};
use reth_node_builder::rpc::{EthApiBuilder, EthApiCtx};
use reth_optimism_primitives::OpPrimitives;
use reth_primitives_traits::{SealedHeaderFor, TxTy};
use reth_rpc::eth::{core::EthApiInner, DevSigner};
use reth_rpc_convert::{
    transaction::{ConvertReceiptInput, RpcTypes, TryIntoSimTx, TryIntoTxEnv},
    RpcHeader, RpcReceipt, RpcTransaction, RpcTxReq,
};
use reth_rpc_eth_api::{
    helpers::{
        pending_block::BuildPendingEnv, spec::SignersForApi, AddDevSigners, EthApiSpec, EthFees,
        EthState, LoadFee, LoadState, SpawnBlocking, Trace,
    },
    transaction::ReceiptConverter,
    EthApiTypes, FromEvmError, FullEthApiServer, RpcConvert, RpcNodeCore,
    RpcNodeCoreExt, SignableTxRequest, TxInfoMapper,
};
use reth_chainspec::ChainSpecProvider;
use reth_rpc_eth_types::{EthStateCache, FeeHistoryCache, GasPriceOracle};
use reth_storage_api::{BlockReader, ProviderHeader, ProviderTx, ReceiptProvider};
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner,
};
use revm::context::{BlockEnv, CfgEnv, TxEnv};
use std::{fmt, fmt::Formatter, marker::PhantomData, sync::Arc};

/// Adapter for [`EthApiInner`], which holds all the data required to serve core `eth_` API.
pub type EthApiNodeBackend<N, Rpc> = EthApiInner<N, Rpc>;

/// OP-Reth `Eth` API implementation.
///
/// This type provides the functionality for handling `eth_` related requests.
///
/// This wraps a default `Eth` implementation, and provides additional functionality where the
/// optimism spec deviates from the default (ethereum) spec, e.g. transaction forwarding to the
/// sequencer, receipts, additional RPC fields for transaction receipts.
///
/// This type implements the [`FullEthApi`](reth_rpc_eth_api::helpers::FullEthApi) by implemented
/// all the `Eth` helper traits and prerequisite traits.
pub struct OpEthApi<N: RpcNodeCore, Rpc: RpcConvert> {
    /// Gateway to node's core components.
    inner: Arc<OpEthApiInner<N, Rpc>>,
}

impl<N: RpcNodeCore, Rpc: RpcConvert> Clone for OpEthApi<N, Rpc> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<N: RpcNodeCore, Rpc: RpcConvert> OpEthApi<N, Rpc> {
    /// Creates a new `OpEthApi`.
    pub fn new(
        eth_api: EthApiNodeBackend<N, Rpc>,
        sequencer_client: Option<SequencerClient>,
        min_suggested_priority_fee: U256,
    ) -> Self {
        let inner =
            Arc::new(OpEthApiInner { eth_api, sequencer_client, min_suggested_priority_fee });
        Self { inner }
    }

    /// Returns a reference to the [`EthApiNodeBackend`].
    pub fn eth_api(&self) -> &EthApiNodeBackend<N, Rpc> {
        self.inner.eth_api()
    }
    /// Returns the configured sequencer client, if any.
    pub fn sequencer_client(&self) -> Option<&SequencerClient> {
        self.inner.sequencer_client()
    }

    /// Build a [`OpEthApi`] using [`OpEthApiBuilder`].
    pub const fn builder() -> OpEthApiBuilder<Rpc> {
        OpEthApiBuilder::new()
    }
}

impl<N, Rpc> EthApiTypes for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    type Error = OpEthApiError;
    type NetworkTypes = Rpc::Network;
    type RpcConvert = Rpc;

    fn tx_resp_builder(&self) -> &Self::RpcConvert {
        self.inner.eth_api.tx_resp_builder()
    }
}

impl<N, Rpc> RpcNodeCore for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    type Primitives = N::Primitives;
    type Provider = N::Provider;
    type Pool = N::Pool;
    type Evm = N::Evm;
    type Network = N::Network;

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
    fn provider(&self) -> &Self::Provider {
        self.inner.eth_api.provider()
    }
}

impl<N, Rpc> RpcNodeCoreExt for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    #[inline]
    fn cache(&self) -> &EthStateCache<N::Primitives> {
        self.inner.eth_api.cache()
    }
}

impl<N, Rpc> EthApiSpec for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    type Transaction = ProviderTx<Self::Provider>;
    type Rpc = Rpc::Network;

    #[inline]
    fn starting_block(&self) -> U256 {
        self.inner.eth_api.starting_block()
    }

    #[inline]
    fn signers(&self) -> &SignersForApi<Self> {
        self.inner.eth_api.signers()
    }
}

impl<N, Rpc> SpawnBlocking for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
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

impl<N, Rpc> LoadFee for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError>,
{
    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<Self::Provider> {
        self.inner.eth_api.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache<ProviderHeader<N::Provider>> {
        self.inner.eth_api.fee_history_cache()
    }

    async fn suggested_priority_fee(&self) -> Result<U256, Self::Error> {
        let min_tip = U256::from(self.inner.min_suggested_priority_fee);
        self.inner.eth_api.gas_oracle().op_suggest_tip_cap(min_tip).await.map_err(Into::into)
    }
}

impl<N, Rpc> LoadState for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
}

impl<N, Rpc> EthState for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    #[inline]
    fn max_proof_window(&self) -> u64 {
        self.inner.eth_api.eth_proof_window()
    }
}

impl<N, Rpc> EthFees for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError>,
{
}

impl<N, Rpc> Trace for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
}

impl<N, Rpc> AddDevSigners for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<
        Network: RpcTypes<TransactionRequest: SignableTxRequest<ProviderTx<N::Provider>>>,
    >,
{
    fn with_dev_accounts(&self) {
        *self.inner.eth_api.signers().write() = DevSigner::random_signers(20)
    }
}

impl<N: RpcNodeCore, Rpc: RpcConvert> fmt::Debug for OpEthApi<N, Rpc> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpEthApi").finish_non_exhaustive()
    }
}

/// Container type `OpEthApi`
pub struct OpEthApiInner<N: RpcNodeCore, Rpc: RpcConvert> {
    /// Gateway to node's core components.
    eth_api: EthApiNodeBackend<N, Rpc>,
    /// Sequencer client, configured to forward submitted transactions to sequencer of given OP
    /// network.
    sequencer_client: Option<SequencerClient>,
    /// Minimum priority fee enforced by OP-specific logic.
    ///
    /// See also <https://github.com/ethereum-optimism/op-geth/blob/d4e0fe9bb0c2075a9bff269fb975464dd8498f75/eth/gasprice/optimism-gasprice.go#L38-L38>
    min_suggested_priority_fee: U256,
}

impl<N: RpcNodeCore, Rpc: RpcConvert> fmt::Debug for OpEthApiInner<N, Rpc> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpEthApiInner").finish()
    }
}

impl<N: RpcNodeCore, Rpc: RpcConvert> OpEthApiInner<N, Rpc> {
    /// Returns a reference to the [`EthApiNodeBackend`].
    const fn eth_api(&self) -> &EthApiNodeBackend<N, Rpc> {
        &self.eth_api
    }

    /// Returns the configured sequencer client, if any.
    const fn sequencer_client(&self) -> Option<&SequencerClient> {
        self.sequencer_client.as_ref()
    }
}

/// An RPC converter for Optimism that implements [`RpcConvert`].
#[derive(Debug, Clone)]
pub struct OpRpcConverter<Provider> {
    tx_mapper: OpTxInfoMapper<Provider>,
    receipt_converter: OpReceiptConverter<Provider>,
}

impl<Provider> OpRpcConverter<Provider>
where
    Provider: Clone,
{
    /// Creates a new [`OpRpcConverter`] with the given transaction mapper and receipt converter.
    pub fn new(
        tx_mapper: OpTxInfoMapper<Provider>,
        receipt_converter: OpReceiptConverter<Provider>,
    ) -> Self {
        Self { tx_mapper, receipt_converter }
    }
}

impl<Provider> RpcConvert for OpRpcConverter<Provider>
where
    Provider: Send + Sync + Clone + std::fmt::Debug + Unpin + 'static
        + BlockReader
        + ChainSpecProvider
        + ReceiptProvider,
    Provider::ChainSpec: reth_optimism_forks::OpHardforks,
    Provider::Receipt: reth_optimism_primitives::DepositReceipt,
    OpTxInfoMapper<Provider>: Send + Sync + Clone + std::fmt::Debug + Unpin + 'static,
    OpReceiptConverter<Provider>: Send + Sync + Clone + std::fmt::Debug + Unpin + 'static,
    OpTxInfoMapper<Provider>: for<'a> TxInfoMapper<&'a TxTy<OpPrimitives>, Out = op_alloy_consensus::transaction::OpTransactionInfo, Err = reth_storage_api::errors::ProviderError>,
{
    type Primitives = OpPrimitives;
    type Network = Optimism;
    type TxEnv = TxEnv;
    type Error = OpEthApiError;

    fn fill(
        &self,
        tx: Recovered<TxTy<Self::Primitives>>,
        tx_info: TransactionInfo,
    ) -> Result<RpcTransaction<Self::Network>, Self::Error> {
        let (tx, signer) = tx.into_parts();
        let tx_info = self
            .tx_mapper
            .try_map(&tx, tx_info)
            .map_err(|e| OpEthApiError::ConversionError(format!("{:?}", e)))?;
        Ok(Transaction::from_transaction(Recovered::new_unchecked(tx.into(), signer), tx_info))
    }

    fn build_simulate_v1_transaction(
        &self,
        request: RpcTxReq<Self::Network>,
    ) -> Result<TxTy<Self::Primitives>, Self::Error> {
        request.try_into_sim_tx().map_err(|e| OpEthApiError::ConversionError(e.to_string()))
    }

    fn tx_env<Spec>(
        &self,
        request: RpcTxReq<Self::Network>,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<Self::TxEnv, Self::Error> {
        request
            .try_into_tx_env(cfg_env, block_env)
            .map(|op_tx| op_tx.base)
            .map_err(|e| OpEthApiError::ConversionError(format!("{:?}", e)))
    }

    fn convert_receipts(
        &self,
        receipts: Vec<ConvertReceiptInput<'_, Self::Primitives>>,
    ) -> Result<Vec<RpcReceipt<Self::Network>>, Self::Error> {
        ReceiptConverter::<Self::Primitives>::convert_receipts(&self.receipt_converter, receipts)
    }

    fn convert_header(
        &self,
        header: SealedHeaderFor<Self::Primitives>,
        block_size: usize,
    ) -> Result<RpcHeader<Self::Network>, Self::Error> {
        use alloy_rpc_types_eth::Header;
        Ok(Header::from_consensus(header.into(), None, Some(U256::from(block_size))))
    }
}

/// Converter for OP RPC types.
pub type OpRpcConvert<N> = OpRpcConverter<<N as FullNodeTypes>::Provider>;

/// Builds [`OpEthApi`] for Optimism.
#[derive(Debug)]
pub struct OpEthApiBuilder<NetworkT = Optimism> {
    /// Sequencer client, configured to forward submitted transactions to sequencer of given OP
    /// network.
    sequencer_url: Option<String>,
    /// Headers to use for the sequencer client requests.
    sequencer_headers: Vec<String>,
    /// Minimum suggested priority fee (tip)
    min_suggested_priority_fee: u64,
    /// Marker for network types.
    _nt: PhantomData<NetworkT>,
}

impl<NetworkT> Default for OpEthApiBuilder<NetworkT> {
    fn default() -> Self {
        Self {
            sequencer_url: None,
            sequencer_headers: Vec::new(),
            min_suggested_priority_fee: 1_000_000,
            _nt: PhantomData,
        }
    }
}

impl<NetworkT> OpEthApiBuilder<NetworkT> {
    /// Creates a [`OpEthApiBuilder`] instance from core components.
    pub const fn new() -> Self {
        Self {
            sequencer_url: None,
            sequencer_headers: Vec::new(),
            min_suggested_priority_fee: 1_000_000,
            _nt: PhantomData,
        }
    }

    /// With a [`SequencerClient`].
    pub fn with_sequencer(mut self, sequencer_url: Option<String>) -> Self {
        self.sequencer_url = sequencer_url;
        self
    }

    /// With headers to use for the sequencer client requests.
    pub fn with_sequencer_headers(mut self, sequencer_headers: Vec<String>) -> Self {
        self.sequencer_headers = sequencer_headers;
        self
    }

    /// With minimum suggested priority fee (tip)
    pub const fn with_min_suggested_priority_fee(mut self, min: u64) -> Self {
        self.min_suggested_priority_fee = min;
        self
    }
}

impl<N, NetworkT> EthApiBuilder<N> for OpEthApiBuilder<NetworkT>
where
    N: FullNodeComponents<Evm: ConfigureEvm<NextBlockEnvCtx: BuildPendingEnv<HeaderTy<N::Types>>>>,
    NetworkT: RpcTypes,
    OpRpcConvert<N>: RpcConvert<Network = NetworkT>,
    OpEthApi<N, OpRpcConvert<N>>:
        FullEthApiServer<Provider = N::Provider, Pool = N::Pool> + AddDevSigners,
{
    type EthApi = OpEthApi<N, OpRpcConvert<N>>;

    async fn build_eth_api(self, ctx: EthApiCtx<'_, N>) -> eyre::Result<Self::EthApi> {
        let Self { sequencer_url, sequencer_headers, min_suggested_priority_fee, .. } = self;
        let rpc_converter = OpRpcConverter::new(
            OpTxInfoMapper::new(ctx.components.provider().clone()),
            OpReceiptConverter::new(ctx.components.provider().clone()),
        );

        let eth_api = ctx.eth_api_builder().with_rpc_converter(rpc_converter).build_inner();

        let sequencer_client = if let Some(url) = sequencer_url {
            Some(
                SequencerClient::new_with_headers(&url, sequencer_headers)
                    .await
                    .wrap_err_with(|| "Failed to init sequencer client with: {url}")?,
            )
        } else {
            None
        };

        Ok(OpEthApi::new(eth_api, sequencer_client, U256::from(min_suggested_priority_fee)))
    }
}
