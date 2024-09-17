use alloy_primitives::{keccak256, Bytes, B256};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_network_api::NetworkInfo;
use reth_rpc_api::Web3ApiServer;
use reth_rpc_server_types::ToRpcResult;

/// `web3` API implementation.
///
/// This type provides the functionality for handling `web3` related requests.
pub struct Web3Api<N> {
    /// An interface to interact with the network
    network: N,
}

impl<N> Web3Api<N> {
    /// Creates a new instance of `Web3Api`.
    pub const fn new(network: N) -> Self {
        Self { network }
    }
}

#[async_trait]
impl<N> Web3ApiServer for Web3Api<N>
where
    N: NetworkInfo + 'static,
{
    /// Handler for `web3_clientVersion`
    async fn client_version(&self) -> RpcResult<String> {
        let status = self.network.network_status().await.to_rpc_result()?;
        Ok(status.client_version)
    }

    /// Handler for `web3_sha3`
    fn sha3(&self, input: Bytes) -> RpcResult<B256> {
        Ok(keccak256(input))
    }
}

impl<N> std::fmt::Debug for Web3Api<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Web3Api").finish_non_exhaustive()
    }
}
