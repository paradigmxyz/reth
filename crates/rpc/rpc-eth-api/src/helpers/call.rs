//! Loads a pending block from database. Helper trait for `eth_` transaction, call and trace RPC
//! methods.

use super::{LoadBlock, LoadPendingBlock, LoadState, SpawnBlocking, Trace};
use crate::{helpers::estimate::EstimateCall, FromEvmError, FullEthApiTypes, RpcBlock};
use alloy_eips::eip2930::AccessListResult;
use alloy_primitives::{Bytes, U256};
use alloy_rpc_types_eth::{
    simulate::{SimulatePayload, SimulatedBlock},
    state::{EvmOverrides, StateOverride},
    BlockId, Bundle, EthCallResponse, StateContext,
};
use futures::Future;
use reth_errors::ProviderError;
use reth_rpc_convert::{RpcConvert, RpcTxReq};
use reth_rpc_eth_types::{error::FromEthApiError, EthApiError};

/// Result type for `eth_simulateV1` RPC method.
pub type SimulatedBlocksResult<N, E> = Result<Vec<SimulatedBlock<RpcBlock<N>>>, E>;

/// Execution related functions for the [`EthApiServer`](crate::EthApiServer) trait in
/// the `eth_` namespace.
pub trait EthCall: EstimateCall + Call + LoadPendingBlock + LoadBlock + FullEthApiTypes {
    /// Estimate gas needed for execution of the `request` at the [`BlockId`].
    fn estimate_gas_at(
        &self,
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send {
        EstimateCall::estimate_gas_at(self, request, at, overrides)
    }

    /// `eth_simulateV1` executes an arbitrary number of transactions on top of the requested state.
    /// The transactions are packed into individual blocks. Overrides can be provided.
    ///
    /// See also: <https://github.com/ethereum/go-ethereum/pull/27720>
    fn simulate_v1(
        &self,
        payload: SimulatePayload<RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>>,
        block: Option<BlockId>,
    ) -> impl Future<Output = SimulatedBlocksResult<Self::NetworkTypes, Self::Error>> + Send {
        let _ = (payload, block);
        async move {
            Err(Self::Error::from_eth_err(EthApiError::Unsupported(
                "eth_simulateV1 is unsupported by the active EVM execution path",
            )))
        }
    }

    /// Executes the call request (`eth_call`) and returns the output.
    fn call(
        &self,
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        block_number: Option<BlockId>,
        overrides: EvmOverrides,
    ) -> impl Future<Output = Result<Bytes, Self::Error>> + Send {
        let _ = (request, block_number, overrides);
        async move {
            Err(Self::Error::from_eth_err(EthApiError::Unsupported(
                "eth_call is unsupported by the active EVM execution path",
            )))
        }
    }

    /// Simulate arbitrary number of transactions at an arbitrary blockchain index, with the
    /// optionality of state overrides.
    fn call_many(
        &self,
        bundles: Vec<Bundle<RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>>>,
        state_context: Option<StateContext>,
        state_override: Option<StateOverride>,
    ) -> impl Future<Output = Result<Vec<Vec<EthCallResponse>>, Self::Error>> + Send {
        let _ = (bundles, state_context, state_override);
        async move {
            Err(Self::Error::from_eth_err(EthApiError::Unsupported(
                "eth_callMany is unsupported by the active EVM execution path",
            )))
        }
    }

    /// Creates [`AccessListResult`] for the [`RpcTxReq`] at the given
    /// [`BlockId`], or latest block.
    fn create_access_list_at(
        &self,
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        block_number: Option<BlockId>,
        state_override: Option<StateOverride>,
    ) -> impl Future<Output = Result<AccessListResult, Self::Error>> + Send
    where
        Self: Trace,
    {
        let _ = (request, block_number, state_override);
        async move {
            Err(Self::Error::from_eth_err(EthApiError::Unsupported(
                "eth_createAccessList is unsupported by the active EVM execution path",
            )))
        }
    }
}

/// Executes code on state.
pub trait Call:
    LoadState<
        RpcConvert: RpcConvert<Evm = Self::Evm>,
        Error: FromEvmError<Self::Evm>
                   + From<<Self::RpcConvert as RpcConvert>::Error>
                   + From<ProviderError>,
    > + SpawnBlocking
{
    /// Returns default gas limit to use for `eth_call` and tracing RPC methods.
    ///
    /// Data access in default trait method implementations.
    fn call_gas_limit(&self) -> u64;

    /// Returns the maximum number of blocks accepted for `eth_simulateV1`.
    fn max_simulate_blocks(&self) -> u64;

    /// Returns whether `eth_simulateV1` should compute state roots.
    fn compute_state_root_for_eth_simulate(&self) -> bool;

    /// Returns the maximum memory the EVM can allocate per RPC request.
    fn evm_memory_limit(&self) -> u64;
}
