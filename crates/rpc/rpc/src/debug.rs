use crate::result::internal_rpc_err;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult as Result;
use reth_primitives::{rpc::BlockId, Bytes, H256};
use reth_rpc_api::DebugApiServer;
use reth_rpc_types::RichBlock;

/// `debug` API implementation.
///
/// This type provides the functionality for handling `debug` related requests.
pub struct DebugApi {
    // TODO: add proper handler when implementing functions
}

#[async_trait]
impl DebugApiServer for DebugApi {
    async fn raw_header(&self, _block_id: BlockId) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn raw_block(&self, _block_id: BlockId) -> Result<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

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

impl std::fmt::Debug for DebugApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugApi").finish_non_exhaustive()
    }
}
