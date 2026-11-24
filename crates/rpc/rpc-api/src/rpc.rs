use alloy_rpc_types::RpcModules;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};

/// RPC namespace, used to find the versions of all rpc modules
#[rpc(server, namespace = "rpc")]
pub trait RpcApi {
    /// Lists enabled APIs and the version of each.
    #[method(name = "modules")]
    fn rpc_modules(&self) -> RpcResult<RpcModules>;
}
