//! `eth_` Extension traits.

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_eth::erc4337::ConditionalOptions;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use tracing::trace;

use crate::helpers::{EthTransactions, FullEthApi};

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

#[async_trait::async_trait]
impl<T> L2EthApiExtServer for T
where
    T: FullEthApi,
    jsonrpsee_types::error::ErrorObject<'static>: From<T::Error>,
{
    /// Handler for: `eth_sendRawTransactionConditional`
    async fn send_raw_transaction_conditional(
        &self,
        bytes: Bytes,
        condition: ConditionalOptions,
    ) -> RpcResult<B256> {
        trace!(target: "rpc::eth", ?bytes, "Serving eth_sendRawTransaction");
        Ok(EthTransactions::send_raw_transaction(self, bytes).await?)
    }
}
