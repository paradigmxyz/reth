use jsonrpsee::{core::RpcResult as Result, proc_macros::rpc};
use reth_primitives::{rpc::BlockId, Bytes, H256};
use reth_rpc_types::RichBlock;

/// Debug rpc interface.
#[cfg_attr(not(feature = "client"), rpc(server))]
#[cfg_attr(feature = "client", rpc(server, client))]
pub trait DebugApi {
    /// Returns an RLP-encoded header.
    #[method(name = "debug_getRawHeader")]
    async fn raw_header(&self, block_id: BlockId) -> Result<Bytes>;

    /// Returns an RLP-encoded block.
    #[method(name = "debug_getRawBlock")]
    async fn raw_block(&self, block_id: BlockId) -> Result<Bytes>;

    /// Returns a EIP-2718 binary-encoded transaction.
    #[method(name = "debug_getRawTransaction")]
    async fn raw_transaction(&self, hash: H256) -> Result<Bytes>;

    /// Returns an array of EIP-2718 binary-encoded receipts.
    #[method(name = "debug_getRawReceipts")]
    async fn raw_receipts(&self, block_id: BlockId) -> Result<Vec<Bytes>>;

    /// Returns an array of recent bad blocks that the client has seen on the network.
    #[method(name = "debug_getBadBlocks")]
    async fn bad_blocks(&self) -> Result<Vec<RichBlock>>;
}
