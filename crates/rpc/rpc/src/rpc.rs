use std::{collections::HashMap, sync::Arc};

use jsonrpsee::core::RpcResult;
use reth_rpc_api::RPCApiServer;
use reth_rpc_types::RPCModules;

/// `rpc` API implementation.
///
/// This type provides the functionality for handling `rpc` requests
#[derive(Debug, Default)]
pub struct RPCApi {
    /// The implementation of the Arc api
    rpc_modules: Arc<RPCModules>,
}

impl RPCApi {
    /// Return a new RPCApi struct
    pub fn new() -> Self {
        RPCApi { rpc_modules: Arc::new(RPCModules::new(HashMap::new())) }
    }
}

impl RPCApiServer for RPCApi {
    fn rpc_modules(&self) -> RpcResult<Arc<RPCModules>> {
        Ok(self.rpc_modules.clone())
    }
}
