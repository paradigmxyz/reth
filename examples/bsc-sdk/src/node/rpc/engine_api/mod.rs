use jsonrpsee_core::server::RpcModule;
use reth::rpc::api::IntoEngineApiRpcModule;

pub mod builder;
pub mod payload;
pub mod validator;

impl IntoEngineApiRpcModule for BscEngineApi {
    fn into_rpc_module(self) -> RpcModule<()> {
        RpcModule::new(())
    }
}

#[derive(Debug, Default)]
#[non_exhaustive]
pub struct BscEngineApi;
