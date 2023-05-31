use std::collections::HashMap;

use jsonrpsee::core::RpcResult;
use reth_rpc_api::RPCApiServer;
use reth_rpc_types::RPCModules;

/// `rpc` API implementation.
///
/// This type provides the functionality for handling `rpc` requests
#[derive(Default, Debug)]
pub struct RPCApi;

impl RPCApi {
    /// Return a new RPCApi struct
    pub fn new() -> Self {
        RPCApi {}
    }
}

impl RPCApiServer for RPCApi {
    fn list_apis(&self) -> RpcResult<RPCModules> {
        let module_map = HashMap::from([
            ("txpool".to_owned(), "1.0".to_owned()),
            ("trace".to_owned(), "1.0".to_owned()),
            ("eth".to_owned(), "1.0".to_owned()),
            ("web3".to_owned(), "1.0".to_owned()),
            ("net".to_owned(), "1.0".to_owned()),
        ]);
        Ok(RPCModules::new(module_map))
    }
}
