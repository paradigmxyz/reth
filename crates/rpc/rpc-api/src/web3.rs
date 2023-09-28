use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::{Bytes, B256};

/// Web3 rpc interface.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "web3"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "web3"))]
#[async_trait::async_trait]
pub trait Web3Api {
    /// Returns current client version.
    #[method(name = "clientVersion")]
    async fn client_version(&self) -> RpcResult<String>;

    /// Returns sha3 of the given data.
    #[method(name = "sha3")]
    fn sha3(&self, input: Bytes) -> RpcResult<B256>;
}
