use alloy_primitives::{Bytes, B256};
use alloy_rpc_macros::rpc;
use alloy_rpc_server::RpcResult;

/// Web3 rpc interface.
#[rpc(server, namespace = "web3")]
pub trait Web3Api {
    /// Returns current client version.
    #[method(name = "clientVersion")]
    async fn client_version(&self) -> RpcResult<String>;

    /// Returns sha3 of the given data.
    #[method(name = "sha3")]
    fn sha3(&self, input: Bytes) -> RpcResult<B256>;
}
