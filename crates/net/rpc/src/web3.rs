use crate::result::ToRpcResult;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_network::NetworkHandle;
use reth_network_api::NetworkInfo;
use reth_primitives::{keccak256, Bytes, H256};
use reth_rpc_api::Web3ApiServer;

/// `web3` API implementation.
///
/// This type provides the functionality for handling `web3` related requests.
pub struct Web3Api {
    /// An interface to interact with the network
    network: NetworkHandle,
}

impl Web3Api {
    /// Creates a new instance of `Web3Api`.
    pub fn new(network: NetworkHandle) -> Web3Api {
        Web3Api { network }
    }
}

#[async_trait]
impl Web3ApiServer for Web3Api {
    async fn client_version(&self) -> RpcResult<String> {
        let status = self.network.network_status().await.to_rpc_result()?;
        Ok(status.client_version)
    }

    fn sha3(&self, input: Bytes) -> RpcResult<H256> {
        Ok(keccak256(input))
    }
}

impl std::fmt::Debug for Web3Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Web3Api").finish_non_exhaustive()
    }
}
