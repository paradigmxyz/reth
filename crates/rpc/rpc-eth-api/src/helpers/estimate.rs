//! Estimate gas needed implementation

use super::{Call, LoadPendingBlock};
use crate::FromEthApiError;
use alloy_primitives::U256;
use alloy_rpc_types_eth::{state::EvmOverrides, BlockId};
use futures::Future;
use reth_evm::EvmEnvFor;
use reth_rpc_convert::{RpcConvert, RpcTxReq};
use reth_rpc_eth_types::EthApiError;

/// Gas execution estimates.
pub trait EstimateCall: Call {
    /// Estimates the gas usage of the `request` with the state.
    fn estimate_gas_with<S>(
        &self,
        evm_env: EvmEnvFor<Self::Evm>,
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        state: S,
        overrides: EvmOverrides,
    ) -> Result<U256, Self::Error> {
        let _ = (evm_env, request, state, overrides);
        Err(Self::Error::from_eth_err(EthApiError::Unsupported(
            "eth_estimateGas is unsupported by the active EVM execution path",
        )))
    }

    /// Estimate gas needed for execution of the `request` at the [`BlockId`].
    fn estimate_gas_at(
        &self,
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send
    where
        Self: LoadPendingBlock,
    {
        let _ = (request, at, overrides);
        async move {
            Err(Self::Error::from_eth_err(EthApiError::Unsupported(
                "eth_estimateGas is unsupported by the active EVM execution path",
            )))
        }
    }
}
