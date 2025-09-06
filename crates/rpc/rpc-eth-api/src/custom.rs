//! Custom RPC methods for the `eth_` namespace.
use crate::{
    helpers::{EthApiSpec,FullEthApi},
    RpcBlock, RpcHeader, RpcReceipt, RpcTransaction,
};
use alloy_primitives::{U256};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_rpc_convert::RpcTxReq;

use reth_rpc_server_types::{ToRpcResult};

use tracing::trace;

/// Custom trait for additional RPC methods
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait CustomEthApi {
    /// Test function that returns a custom string.
    #[method(name = "getSven")]
    fn get_sven(&self) -> RpcResult<String>;

    /// Returns the number of most recent block.
    #[method(name = "blockNumberSven")]
    fn block_number_sven(&self) -> RpcResult<U256>;
}

/// Helper trait that combines custom methods with full ETH API functionality
pub trait FullCustomEthApiServer:
    CustomEthApiServer +
    crate::core::EthApiServer<
        RpcTxReq<Self::NetworkTypes>,
        RpcTransaction<Self::NetworkTypes>,
        RpcBlock<Self::NetworkTypes>,
        RpcReceipt<Self::NetworkTypes>,
        RpcHeader<Self::NetworkTypes>,
    > +
    FullEthApi +
    Clone
{
}

// Auto-implement the helper trait for compatible types
impl<T> FullCustomEthApiServer for T where
    T: CustomEthApiServer +
        crate::core::EthApiServer<
            RpcTxReq<T::NetworkTypes>,
            RpcTransaction<T::NetworkTypes>,
            RpcBlock<T::NetworkTypes>,
            RpcReceipt<T::NetworkTypes>,
            RpcHeader<T::NetworkTypes>,
        > +
        FullEthApi +
        Clone
{
}

#[async_trait::async_trait]
impl<T> CustomEthApiServer for T
where
    T: FullEthApi,
    jsonrpsee_types::error::ErrorObject<'static>: From<T::Error>,
{
    /// 
    fn get_sven(&self) -> RpcResult<String> {
        Ok("Sv3n".to_string())
    }

     /// Handler for: `eth_blockNumber`
    fn block_number_sven(&self) -> RpcResult<U256> {
        trace!(target: "rpc::eth", "Serving eth_blockNumber");
        Ok(U256::from(
            EthApiSpec::chain_info(self).with_message("failed to read chain info")?.best_number,
        ))
    }
}