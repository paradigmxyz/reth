use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_rpc_types::RpcModules;

/// RPC namespace, used to find the versions of all rpc modules
#[cfg_attr(not(feature = "client"), rpc(server))]
#[cfg_attr(feature = "client", rpc(server, client))]
pub trait RpcApi {
    /// Lists enabled APIs and the version of each.
    #[method(name = "rpc_modules")]
    fn rpc_modules(&self) -> RpcResult<RpcModules>;
}
