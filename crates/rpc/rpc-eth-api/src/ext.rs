//! `eth_` Extension traits.

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_eth::erc4337::ConditionalOptions;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};

/// Extension trait for `eth_` namespace for L2s.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait L2EthApiExt {
    /// Sends signed transaction with the given condition.
    #[method(name = "sendRawTransactionConditional")]
    async fn send_raw_transaction_conditional(
        &self,
        bytes: Bytes,
        condition: ConditionalOptions,
    ) -> RpcResult<B256>;
}
