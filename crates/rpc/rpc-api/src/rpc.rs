use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_rpc_types::RPCModules;
use std::sync::Arc;

/// RPC namespace, used to find the versions of all rpc modules
#[cfg_attr(not(feature = "client"), rpc(server))]
#[cfg_attr(feature = "client", rpc(server, client))]
#[async_trait::async_trait]
pub trait RPCApi {
    /// Lists enabled APIs and the version of each.
    #[method(name = "rpc_modules")]
    fn rpc_modules(&self) -> RpcResult<Arc<RPCModules>>;
}
