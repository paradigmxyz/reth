use reth_primitives_traits::SignedTransaction;
use alloy_consensus::TxReceipt;
use reth_rpc_eth_api::FromEthApiError;


use alloy_consensus::{EthereumTxEnvelope, TxEip4844};

use core::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use alloy_primitives::U256;
use reth_evm::ConfigureEvm;
use arb_alloy_network::ArbNetwork;
use reth_node_api::{FullNodeComponents, FullNodeTypes, HeaderTy};
use reth_node_builder::rpc::{EthApiBuilder, EthApiCtx};
use reth_rpc::eth::core::EthApiInner;
use reth_rpc_eth_api::{
    helpers::{
        pending_block::{PendingEnvBuilder, BuildPendingEnv, LoadPendingBlock},
        AddDevSigners, EthApiSpec, EthFees, EthState, LoadFee, LoadState, SpawnBlocking, Trace,
    },
    EthApiTypes, FromEvmError, FullEthApiServer, RpcConvert, RpcConverter, RpcNodeCore,
    RpcNodeCoreExt, RpcTypes,
};
use reth_rpc_eth_types::{EthStateCache, FeeHistoryCache, GasPriceOracle};
use crate::error::ArbEthApiError;
use reth_storage_api::{ProviderHeader, ProviderTx};
pub mod txinfo;
pub mod response;
pub mod header;
use reth_tasks::pool::{BlockingTaskGuard, BlockingTaskPool};
use reth_tasks::TaskSpawner;
use reth_arbitrum_primitives::ArbTransactionSigned;

#[derive(Clone, Debug)]
pub struct ArbRpcTypes;

impl reth_rpc_eth_api::RpcTypes for ArbRpcTypes {
    type Header = alloy_serde::WithOtherFields<alloy_rpc_types_eth::Header<alloy_consensus::Header>>;
    type Receipt = alloy_rpc_types_eth::TransactionReceipt;
    type TransactionResponse = alloy_serde::WithOtherFields<alloy_rpc_types_eth::Transaction<ArbTransactionSigned>>;
    type TransactionRequest = crate::eth::transaction::ArbTransactionRequest;
}

pub mod receipt;
pub mod block;
pub mod call;
pub mod pending_block;
pub mod transaction;



pub type EthApiNodeBackend<N, Rpc> = EthApiInner<N, Rpc>;

pub struct ArbEthApi<N: RpcNodeCore, Rpc: RpcConvert> {
    inner: Arc<ArbEthApiInner<N, Rpc>>,
}

impl<N: RpcNodeCore, Rpc: RpcConvert> Clone for ArbEthApi<N, Rpc> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<N: RpcNodeCore, Rpc: RpcConvert> ArbEthApi<N, Rpc> {
    pub fn new(eth_api: EthApiNodeBackend<N, Rpc>) -> Self {
        let inner = Arc::new(ArbEthApiInner { eth_api });
        Self { inner }
    }
    pub fn eth_api(&self) -> &EthApiNodeBackend<N, Rpc> {
        self.inner.eth_api()
    }
    pub const fn builder<NetworkT>() -> ArbEthApiBuilder<NetworkT> {
        ArbEthApiBuilder::new()
    }
}

impl<N, Rpc> EthApiTypes for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    type Error = ArbEthApiError;
    type NetworkTypes = Rpc::Network;
    type RpcConvert = Rpc;

    fn tx_resp_builder(&self) -> &Self::RpcConvert {
        self.inner.eth_api.tx_resp_builder()
    }
}

impl<N, Rpc> RpcNodeCore for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    type Primitives = N::Primitives;
    type Provider = N::Provider;
    type Pool = N::Pool;
    type Evm = N::Evm;
    type Network = N::Network;

    fn pool(&self) -> &Self::Pool { self.inner.eth_api.pool() }
    fn evm_config(&self) -> &Self::Evm { self.inner.eth_api.evm_config() }
    fn network(&self) -> &Self::Network { self.inner.eth_api.network() }
    fn provider(&self) -> &Self::Provider { self.inner.eth_api.provider() }
}

impl<N, Rpc> RpcNodeCoreExt for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    fn cache(&self) -> &EthStateCache<N::Primitives> { self.inner.eth_api.cache() }
}

impl<N, Rpc> EthApiSpec for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    fn starting_block(&self) -> U256 { self.inner.eth_api.starting_block() }
}

impl<N, Rpc> SpawnBlocking for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    fn io_task_spawner(&self) -> impl TaskSpawner { self.inner.eth_api.task_spawner() }
    fn tracing_task_pool(&self) -> &BlockingTaskPool { self.inner.eth_api.blocking_task_pool() }
    fn tracing_task_guard(&self) -> &BlockingTaskGuard { self.inner.eth_api.blocking_task_guard() }
}

impl<N, Rpc> LoadFee for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
    fn gas_oracle(&self) -> &GasPriceOracle<Self::Provider> { self.inner.eth_api.gas_oracle() }
    fn fee_history_cache(&self) -> &FeeHistoryCache<ProviderHeader<N::Provider>> {
        self.inner.eth_api.fee_history_cache()
    }
}

impl<N, Rpc> LoadState for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
    Self: LoadPendingBlock,
{
}

impl<N, Rpc> EthState for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
    Self: LoadPendingBlock,
{
    fn max_proof_window(&self) -> u64 { self.inner.eth_api.eth_proof_window() }
}

impl<N, Rpc> EthFees for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
}

impl<N, Rpc> Trace for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
}
impl<N, Rpc> AddDevSigners for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert,
{
    fn with_dev_accounts(&self) {}
}


impl<N: RpcNodeCore, Rpc: RpcConvert> fmt::Debug for ArbEthApi<N, Rpc> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArbEthApi").finish_non_exhaustive()
    }
}

pub struct ArbEthApiInner<N: RpcNodeCore, Rpc: RpcConvert> {
    eth_api: EthApiNodeBackend<N, Rpc>,
}

impl<N: RpcNodeCore, Rpc: RpcConvert> fmt::Debug for ArbEthApiInner<N, Rpc> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArbEthApiInner").finish()
    }
}

impl<N: RpcNodeCore, Rpc: RpcConvert> ArbEthApiInner<N, Rpc> {
    const fn eth_api(&self) -> &EthApiNodeBackend<N, Rpc> { &self.eth_api }
}

pub type ArbRpcConvert<N, NetworkT> = RpcConverter<
    NetworkT,
    <N as FullNodeComponents>::Evm,
    receipt::ArbReceiptConverter<<N as FullNodeTypes>::Provider>,
    header::ArbHeaderConverter,
    txinfo::ArbTxInfoMapper<<N as FullNodeTypes>::Provider>,
    (),
    response::ArbRpcTxConverter,
>;

#[derive(Debug)]
pub struct ArbEthApiBuilder<NetworkT = ArbRpcTypes> {
    _nt: PhantomData<NetworkT>,
}

impl<NetworkT> Default for ArbEthApiBuilder<NetworkT> {
    fn default() -> Self { Self { _nt: PhantomData } }
}

impl<NetworkT> ArbEthApiBuilder<NetworkT> {
    pub const fn new() -> Self { Self { _nt: PhantomData } }
}

impl<N, NetworkT> EthApiBuilder<N> for ArbEthApiBuilder<NetworkT>
where
    N: FullNodeComponents<
        Evm: ConfigureEvm<NextBlockEnvCtx: BuildPendingEnv<HeaderTy<N::Types>>>,
        Types: reth_node_api::NodeTypes<ChainSpec: reth_chainspec::Hardforks + reth_chainspec::EthereumHardforks>,
    >,
    NetworkT: RpcTypes,
    ArbRpcConvert<N, NetworkT>: RpcConvert<Network = NetworkT>,
    ArbEthApi<N, ArbRpcConvert<N, NetworkT>>:
        FullEthApiServer<Provider = N::Provider, Pool = N::Pool> + AddDevSigners,
{
    type EthApi = ArbEthApi<N, ArbRpcConvert<N, NetworkT>>;

    async fn build_eth_api(self, ctx: EthApiCtx<'_, N>) -> eyre::Result<Self::EthApi> {
        let rpc_converter = RpcConverter::new(receipt::ArbReceiptConverter::new(
            ctx.components.provider().clone(),
        ))
        .with_header_converter(header::ArbHeaderConverter)
        .with_mapper(txinfo::ArbTxInfoMapper::new(ctx.components.provider().clone()))
        .with_rpc_tx_converter(response::ArbRpcTxConverter);
        let eth_api = ctx.eth_api_builder().with_rpc_converter(rpc_converter).build_inner();
        Ok(ArbEthApi::new(eth_api))
    }
}





impl<N, Rpc> reth_rpc_eth_api::helpers::LoadReceipt for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = crate::error::ArbEthApiError>,
{
    fn build_transaction_receipt(
        &self,
        tx: reth_storage_api::ProviderTx<Self::Provider>,
        meta: alloy_consensus::transaction::TransactionMeta,
        receipt: reth_storage_api::ProviderReceipt<Self::Provider>,
    ) -> impl core::future::Future<
        Output = core::result::Result<
            <<Self::RpcConvert as reth_rpc_convert::transaction::RpcConvert>::Network as RpcTypes>::Receipt,
            Self::Error
        >
    > + Send {
        let this = self.clone();
        async move {
            use reth_rpc_eth_types::EthApiError;
            use reth_rpc_convert::transaction::ConvertReceiptInput;

            let hash = meta.block_hash;
            let all_receipts = this
                .cache()
                .get_receipts(hash)
                .await
                .map_err(Self::Error::from_eth_err)?
                .ok_or(EthApiError::HeaderNotFound(hash.into()))?;

            let mut gas_used = 0u64;
            let mut next_log_index = 0usize;
            if meta.index > 0 {
                for r in all_receipts.iter().take(meta.index as usize) {
                    gas_used = r.cumulative_gas_used();
                    next_log_index += r.logs().len();
                }
            }

            let recovered_ref = tx.with_signer_ref(alloy_primitives::Address::ZERO);

            let input = ConvertReceiptInput {
                tx: recovered_ref,
                gas_used: receipt.cumulative_gas_used() - gas_used,
                receipt,
                next_log_index,
                meta,
            };

            Ok(this.tx_resp_builder().convert_receipts(vec![input])?.pop().unwrap())
        }
    }
}
