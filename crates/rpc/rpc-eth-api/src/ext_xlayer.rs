//! `eth_` Extension traits for xlayer

use alloy_eips::BlockId;
use alloy_rpc_types_eth::state::StateOverride;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_rpc_eth_types::pre_exec_xlayer::PreExecResult;
use alloy_json_rpc::RpcObject;

/// Extension trait for `eth_` namespace for xlayer.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait XLayerEthApiExt<Req: RpcObject> {
    /// Sends signed transaction with the given condition.
    #[method(name = "transactionPreExec")]
    async fn transaction_pre_exec(
        &self,
        args: Vec<Req>,
        block_id: Option<BlockId>,
        state_overrides: Option<StateOverride>,
    ) -> RpcResult<Vec<PreExecResult>>;
}
