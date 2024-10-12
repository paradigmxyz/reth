use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_rpc_types::RpcModules;

/// RPC namespace, used to find the versions of all rpc modules
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "rpc"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "rpc"))]
pub trait RpcApi {
    /// Lists enabled APIs and the version of each.
    #[method(name = "modules")]
    fn rpc_modules(&self) -> RpcResult<RpcModules>;
}
