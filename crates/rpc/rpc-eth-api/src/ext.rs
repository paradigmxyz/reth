//! `eth_` Extension traits.

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_eth::erc4337::TransactionConditional;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};

/// Extension trait for `eth_` namespace for L2s.
#[rpc(server, namespace = "eth")]
pub trait L2EthApiExt {
    /// Sends signed transaction with the given condition.
    #[method(name = "sendRawTransactionConditional")]
    async fn send_raw_transaction_conditional(
        &self,
        bytes: Bytes,
        condition: TransactionConditional,
    ) -> RpcResult<B256>;
}
