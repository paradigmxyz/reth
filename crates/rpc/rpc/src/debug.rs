use crate::{result::internal_rpc_err, EthApiSpec};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives::{BlockId, BlockNumberOrTag, Bytes, H256};
use reth_rpc_api::DebugApiServer;
use reth_rpc_types::{
    trace::geth::{BlockTraceResult, GethDebugTracingOptions, GethTraceFrame, TraceResult},
    CallRequest, RichBlock,
};

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
    async fn raw_header(&self, _block_id: BlockId) -> RpcResult<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn raw_block(&self, _block_id: BlockId) -> RpcResult<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Returns the bytes of the transaction for the given hash.
    async fn raw_transaction(&self, _hash: H256) -> RpcResult<Bytes> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn raw_receipts(&self, _block_id: BlockId) -> RpcResult<Vec<Bytes>> {
        Err(internal_rpc_err("unimplemented"))
    }

    async fn bad_blocks(&self) -> RpcResult<Vec<RichBlock>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `debug_traceChain`
    async fn debug_trace_chain(
        &self,
        _start_exclusive: BlockNumberOrTag,
        _end_inclusive: BlockNumberOrTag,
    ) -> RpcResult<Vec<BlockTraceResult>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `debug_traceBlock`
    async fn debug_trace_block(
        &self,
        _rlp_block: Bytes,
        _opts: GethDebugTracingOptions,
    ) -> RpcResult<Vec<TraceResult>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `debug_traceBlockByHash`
    async fn debug_trace_block_by_hash(
        &self,
        _block: H256,
        _opts: GethDebugTracingOptions,
    ) -> RpcResult<Vec<TraceResult>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `debug_traceBlockByNumber`
    async fn debug_trace_block_by_number(
        &self,
        _block: BlockNumberOrTag,
        _opts: GethDebugTracingOptions,
    ) -> RpcResult<Vec<TraceResult>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `debug_traceTransaction`
    async fn debug_trace_transaction(
        &self,
        _tx_hash: H256,
        _opts: GethDebugTracingOptions,
    ) -> RpcResult<GethTraceFrame> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `debug_traceCall`
    async fn debug_trace_call(
        &self,
        _request: CallRequest,
        _block_number: Option<BlockId>,
        _opts: GethDebugTracingOptions,
    ) -> RpcResult<GethTraceFrame> {
        Err(internal_rpc_err("unimplemented"))
    }
}

impl<Eth> std::fmt::Debug for DebugApi<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugApi").finish_non_exhaustive()
    }
}
