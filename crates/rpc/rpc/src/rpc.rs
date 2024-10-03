use alloy_primitives::map::HashMap;
use alloy_rpc_types::RpcModules;
use jsonrpsee::core::RpcResult;
use reth_rpc_api::RpcApiServer;
use std::sync::Arc;

/// `rpc` API implementation.
///
/// This type provides the functionality for handling `rpc` requests
#[derive(Debug, Clone, Default)]
pub struct RPCApi {
    /// The implementation of the Arc api
    rpc_modules: Arc<RpcModules>,
}

impl RPCApi {
    /// Return a new `RPCApi` struct, with given `module_map`
    pub fn new(module_map: HashMap<String, String>) -> Self {
        Self { rpc_modules: Arc::new(RpcModules::new(module_map)) }
    }
}

impl RpcApiServer for RPCApi {
    fn rpc_modules(&self) -> RpcResult<RpcModules> {
        Ok(self.rpc_modules.as_ref().clone())
    }
}
