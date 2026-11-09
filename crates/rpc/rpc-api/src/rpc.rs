use alloy_rpc_macros::rpc;
use alloy_rpc_server::RpcResult;
use alloy_rpc_types::RpcModules;

/// RPC namespace, used to find the versions of all rpc modules
#[rpc(server, namespace = "rpc")]
pub trait RpcApi {
    /// Lists enabled APIs and the version of each.
    #[method(name = "modules")]
    fn rpc_modules(&self) -> RpcResult<RpcModules>;
}
