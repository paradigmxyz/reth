use crate::eth::ArbEthApi;
use reth_evm::{SpecFor, TxEnvFor};
use reth_rpc_eth_api::{
    helpers::{estimate::EstimateCall, Call, EthCall},
    FromEvmError, RpcConvert, RpcNodeCore,
};
use crate::error::ArbEthApiError;

impl<N, Rpc> EthCall for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<
        Primitives = N::Primitives,
        Error = ArbEthApiError,
        TxEnv = TxEnvFor<N::Evm>,
        Spec = SpecFor<N::Evm>,
    >,
{
}

impl<N, Rpc> EstimateCall for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<
        Primitives = N::Primitives,
        Error = ArbEthApiError,
        TxEnv = TxEnvFor<N::Evm>,
        Spec = SpecFor<N::Evm>,
    >,
{
}

impl<N, Rpc> Call for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<
        Primitives = N::Primitives,
        Error = ArbEthApiError,
        TxEnv = TxEnvFor<N::Evm>,
        Spec = SpecFor<N::Evm>,
    >,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.eth_api().gas_cap()
    }

    #[inline]
    fn max_simulate_blocks(&self) -> u64 {
        self.eth_api().max_simulate_blocks()
    }
}
