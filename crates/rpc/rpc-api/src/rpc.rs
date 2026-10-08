use alloy_rpc_types::RpcModules;
use reth_json_rpc::{rpc, RpcResult};

/// RPC namespace, used to find the versions of all rpc modules
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "rpc"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "rpc"))]
pub trait RpcApi {
    /// Lists enabled APIs and the version of each.
    #[method(name = "modules")]
    fn rpc_modules(&self) -> RpcResult<RpcModules>;
}
