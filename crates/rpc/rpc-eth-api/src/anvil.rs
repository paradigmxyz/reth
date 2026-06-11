//! Anvil-specific extensions for the `eth_` namespace.

use crate::{
    helpers::{EthTransactions, FullEthApi},
    RpcTxReq,
};
use alloy_json_rpc::RpcObject;
use alloy_primitives::{B256, U256, U64};
use async_trait::async_trait;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use jsonrpsee_types::error::ErrorObject;
use tracing::trace;

/// Anvil-specific `eth_` namespace API.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait AnvilEthApi<TxReq: RpcObject> {
    /// Resends a pending local transaction with an updated gas price or gas limit.
    #[method(name = "resend")]
    async fn resend(
        &self,
        request: TxReq,
        gas_price: Option<U256>,
        gas_limit: Option<U64>,
    ) -> RpcResult<B256>;
}

#[async_trait]
impl<T> AnvilEthApiServer<RpcTxReq<T::NetworkTypes>> for T
where
    T: FullEthApi,
    ErrorObject<'static>: From<T::Error>,
{
    /// Handler for: `eth_resend`
    async fn resend(
        &self,
        request: RpcTxReq<T::NetworkTypes>,
        gas_price: Option<U256>,
        gas_limit: Option<U64>,
    ) -> RpcResult<B256> {
        trace!(target: "rpc::eth", ?request, ?gas_price, ?gas_limit, "Serving eth_resend");
        Ok(EthTransactions::resend_transaction(self, request, gas_price, gas_limit).await?)
    }
}
