use crate::{result::internal_rpc_err, EthApiSpec};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult as Result;
use reth_primitives::{rpc::BlockId, Bytes, H256};
use reth_rpc_api::DebugApiServer;
use reth_rpc_types::RichBlock;

/// `debug` API implementation.
///
/// This type provides the functionality for handling `debug` related requests.
#[non_exhaustive]
pub struct DebugApi<Eth> {
    /// The implementation of `eth` API
    eth: Eth,
}

// === impl DebugApi ===

impl<Eth> DebugApi<Eth> {
    /// Create a new instance of the [DebugApi]
    pub fn new(eth: Eth) -> Self {
        Self { eth }
    }
}

#[async_trait]
impl<Eth> DebugApiServer for DebugApi<Eth>
where
    Eth: EthApiSpec + 'static,
{
    async fn raw_header(&self, _block_id: BlockId) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn raw_block(&self, _block_id: BlockId) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Returns the bytes of the transaction for the given hash.
    async fn raw_transaction(&self, _hash: H256) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn raw_receipts(&self, _block_id: BlockId) -> Result<Vec<Bytes>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn bad_blocks(&self) -> Result<Vec<RichBlock>> {
        Err(internal_rpc_err("unimplemented"))
    }
}

impl<Eth> std::fmt::Debug for DebugApi<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugApi").finish_non_exhaustive()
    }
}
