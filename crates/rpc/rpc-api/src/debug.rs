use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::{BlockId, BlockNumberOrTag, Bytes, H256};
use reth_rpc_types::{
    trace::geth::{
        BlockTraceResult, GethDebugTracingCallOptions, GethDebugTracingOptions, GethTraceFrame,
        TraceResult,
    },
    CallRequest, RichBlock,
};

/// Debug rpc interface.
#[cfg_attr(not(feature = "client"), rpc(server))]
#[cfg_attr(feature = "client", rpc(server, client))]
pub trait DebugApi {
    /// Returns an RLP-encoded header.
    #[method(name = "debug_getRawHeader")]
    async fn raw_header(&self, block_id: BlockId) -> RpcResult<Bytes>;

    /// Returns an RLP-encoded block.
    #[method(name = "debug_getRawBlock")]
    async fn raw_block(&self, block_id: BlockId) -> RpcResult<Bytes>;

    /// Returns a EIP-2718 binary-encoded transaction.
    #[method(name = "debug_getRawTransaction")]
    async fn raw_transaction(&self, hash: H256) -> RpcResult<Bytes>;

    /// Returns an array of EIP-2718 binary-encoded receipts.
    #[method(name = "debug_getRawReceipts")]
    async fn raw_receipts(&self, block_id: BlockId) -> RpcResult<Vec<Bytes>>;

    /// Returns an array of recent bad blocks that the client has seen on the network.
    #[method(name = "debug_getBadBlocks")]
    async fn bad_blocks(&self) -> RpcResult<Vec<RichBlock>>;

    /// Returns the structured logs created during the execution of EVM between two blocks
    /// (excluding start) as a JSON object.
    #[method(name = "debug_traceChain")]
    async fn debug_trace_chain(
        &self,
        start_exclusive: BlockNumberOrTag,
        end_inclusive: BlockNumberOrTag,
    ) -> RpcResult<Vec<BlockTraceResult>>;

    /// The `debug_traceBlock` method will return a full stack trace of all invoked opcodes of all
    /// transaction that were included in this block.
    ///
    /// This expects an rlp encoded block
    ///
    /// Note, the parent of this block must be present, or it will fail. For the second parameter
    /// see [GethDebugTracingOptions] reference.
    #[method(name = "debug_traceBlock")]
    async fn debug_trace_block(
        &self,
        rlp_block: Bytes,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<Vec<TraceResult>>;

    /// Similar to `debug_traceBlock`, `debug_traceBlockByHash` accepts a block hash and will replay
    /// the block that is already present in the database. For the second parameter see
    /// [GethDebugTracingOptions].
    #[method(name = "debug_traceBlockByHash")]
    async fn debug_trace_block_by_hash(
        &self,
        block: H256,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<Vec<TraceResult>>;

    /// Similar to `debug_traceBlockByNumber`, `debug_traceBlockByHash` accepts a block number
    /// [BlockNumberOrTag] and will replay the block that is already present in the database.
    /// For the second parameter see [GethDebugTracingOptions].
    #[method(name = "debug_traceBlockByNumber")]
    async fn debug_trace_block_by_number(
        &self,
        block: BlockNumberOrTag,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<Vec<TraceResult>>;

    /// The `debug_traceTransaction` debugging method will attempt to run the transaction in the
    /// exact same manner as it was executed on the network. It will replay any transaction that
    /// may have been executed prior to this one before it will finally attempt to execute the
    /// transaction that corresponds to the given hash.
    #[method(name = "debug_traceTransaction")]
    async fn debug_trace_transaction(
        &self,
        tx_hash: H256,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<GethTraceFrame>;

    /// The debug_traceCall method lets you run an `eth_call` within the context of the given block
    /// execution using the final state of parent block as the base.
    ///
    /// The first argument (just as in`eth_call`) is a transaction request.
    /// The block can be specified either by hash or by number as
    /// the second argument.
    /// The trace can be configured similar to `debug_traceTransaction`,
    /// see [GethDebugTracingOptions]. The method returns the same output as
    /// `debug_traceTransaction`.
    #[method(name = "debug_traceCall")]
    async fn debug_trace_call(
        &self,
        request: CallRequest,
        block_number: Option<BlockId>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<GethTraceFrame>;
}
