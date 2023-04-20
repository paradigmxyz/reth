use jsonrpsee::core::RpcResult as Result;
use reth_primitives::{filter::Filter, Address, BlockId, BlockNumberOrTag, Bytes, H256, U256, U64};
use reth_rpc_api::{EngineEthApiServer, EthApiServer, EthFilterApiServer};
/// Re-export for convenience
pub use reth_rpc_engine_api::EngineApi;
use reth_rpc_types::{state::StateOverride, CallRequest, Log, RichBlock, SyncStatus};
use tracing::trace;

/// A wrapper type for the `EthApi` and `EthFilter` implementations that only expose the required
/// subset for the `eth_` namespace used in auth server alongside the `engine_` namespace.
#[derive(Debug, Clone)]
pub struct EngineEthApi<Eth, EthFilter> {
    eth: Eth,
    eth_filter: EthFilter,
}

impl<Eth, EthFilter> EngineEthApi<Eth, EthFilter> {
    /// Create a new `EngineEthApi` instance.
    pub fn new(eth: Eth, eth_filter: EthFilter) -> Self {
        Self { eth, eth_filter }
    }
}

#[async_trait::async_trait]
impl<Eth, EthFilter> EngineEthApiServer for EngineEthApi<Eth, EthFilter>
where
    Eth: EthApiServer,
    EthFilter: EthFilterApiServer,
{
    /// Handler for: `eth_syncing`
    fn syncing(&self) -> Result<SyncStatus> {
        trace!(target: "rpc::eth", "Serving eth_syncing [engine]");
        self.eth.syncing()
    }

    /// Handler for: `eth_chainId`
    async fn chain_id(&self) -> Result<Option<U64>> {
        trace!(target: "rpc::eth", "Serving eth_chainId [engine]");
        self.eth.chain_id().await
    }

    /// Handler for: `eth_blockNumber`
    fn block_number(&self) -> Result<U256> {
        trace!(target: "rpc::eth", "Serving eth_blockNumber [engine]");
        self.eth.block_number()
    }

    /// Handler for: `eth_call`
    async fn call(
        &self,
        request: CallRequest,
        block_number: Option<BlockId>,
        state_overrides: Option<StateOverride>,
    ) -> Result<Bytes> {
        trace!(target: "rpc::eth", "Serving eth_call [engine]");
        self.eth.call(request, block_number, state_overrides).await
    }

    /// Handler for: `eth_getCode`
    async fn get_code(&self, address: Address, block_number: Option<BlockId>) -> Result<Bytes> {
        trace!(target: "rpc::eth", "Serving eth_getCode [engine]");
        self.eth.get_code(address, block_number).await
    }

    /// Handler for: `eth_getBlockByHash`
    async fn block_by_hash(&self, hash: H256, full: bool) -> Result<Option<RichBlock>> {
        trace!(target: "rpc::eth", "Serving eth_getBlockByHash [engine]");
        self.eth.block_by_hash(hash, full).await
    }

    /// Handler for: `eth_getBlockByNumber`
    async fn block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> Result<Option<RichBlock>> {
        trace!(target: "rpc::eth", "Serving eth_getBlockByNumber [engine]");
        self.eth.block_by_number(number, full).await
    }

    /// Handler for: `eth_sendRawTransaction`
    async fn send_raw_transaction(&self, bytes: Bytes) -> Result<H256> {
        trace!(target: "rpc::eth", "Serving eth_sendRawTransaction [engine]");
        self.eth.send_raw_transaction(bytes).await
    }

    /// Handler for `eth_getLogs`
    async fn logs(&self, filter: Filter) -> Result<Vec<Log>> {
        trace!(target: "rpc::eth", "Serving eth_getLogs [engine]");
        self.eth_filter.logs(filter).await
    }
}
