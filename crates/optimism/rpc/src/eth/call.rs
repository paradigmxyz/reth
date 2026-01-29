use crate::{eth::RpcNodeCore, OpEthApi, OpEthApiError};
use reth_optimism_flashblocks::FlashblockPayload;
use reth_rpc_eth_api::{
    helpers::{estimate::EstimateCall, Call, EthCall},
    FromEvmError, RpcConvert,
};

impl<N, Rpc, F> EthCall for OpEthApi<N, Rpc, F>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError, Evm = N::Evm>,
    F: FlashblockPayload,
{
}

impl<N, Rpc, F> EstimateCall for OpEthApi<N, Rpc, F>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError, Evm = N::Evm>,
    F: FlashblockPayload,
{
}

impl<N, Rpc, F> Call for OpEthApi<N, Rpc, F>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError, Evm = N::Evm>,
    F: FlashblockPayload,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.eth_api.gas_cap()
    }

    #[inline]
    fn max_simulate_blocks(&self) -> u64 {
        self.inner.eth_api.max_simulate_blocks()
    }

    #[inline]
    fn evm_memory_limit(&self) -> u64 {
        self.inner.eth_api.evm_memory_limit()
    }
}
