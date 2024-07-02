use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_primitives::{Address, BlockId, BlockNumberOrTag, Bytes, B256};
use reth_rpc_types::{
    trace::geth::{
        BlockTraceResult, GethDebugTracingCallOptions, GethDebugTracingOptions, GethTrace,
        TraceResult,
    },
    Bundle, RichBlock, StateContext, TransactionRequest,
};
use std::collections::HashMap;

/// Debug rpc interface.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "debug"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "debug"))]
pub trait DebugApi {
    /// Returns an RLP-encoded header.
    #[method(name = "getRawHeader")]
    async fn raw_header(&self, block_id: BlockId) -> RpcResult<Bytes>;

    /// Returns an RLP-encoded block.
    #[method(name = "getRawBlock")]
    async fn raw_block(&self, block_id: BlockId) -> RpcResult<Bytes>;

    /// Returns a EIP-2718 binary-encoded transaction.
    ///
    /// If this is a pooled EIP-4844 transaction, the blob sidecar is included.
    #[method(name = "getRawTransaction")]
    async fn raw_transaction(&self, hash: B256) -> RpcResult<Option<Bytes>>;

    /// Returns an array of EIP-2718 binary-encoded transactions for the given [`BlockId`].
    #[method(name = "getRawTransactions")]
    async fn raw_transactions(&self, block_id: BlockId) -> RpcResult<Vec<Bytes>>;

    /// Returns an array of EIP-2718 binary-encoded receipts.
    #[method(name = "getRawReceipts")]
    async fn raw_receipts(&self, block_id: BlockId) -> RpcResult<Vec<Bytes>>;

    /// Returns an array of recent bad blocks that the client has seen on the network.
    #[method(name = "getBadBlocks")]
    async fn bad_blocks(&self) -> RpcResult<Vec<RichBlock>>;

    /// Returns the structured logs created during the execution of EVM between two blocks
    /// (excluding start) as a JSON object.
    #[method(name = "traceChain")]
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
    #[method(name = "traceBlock")]
    async fn debug_trace_block(
        &self,
        rlp_block: Bytes,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<Vec<TraceResult>>;

    /// Similar to `debug_traceBlock`, `debug_traceBlockByHash` accepts a block hash and will replay
    /// the block that is already present in the database. For the second parameter see
    /// [GethDebugTracingOptions].
    #[method(name = "traceBlockByHash")]
    async fn debug_trace_block_by_hash(
        &self,
        block: B256,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<Vec<TraceResult>>;

    /// Similar to `debug_traceBlockByHash`, `debug_traceBlockByNumber` accepts a block number
    /// [BlockNumberOrTag] and will replay the block that is already present in the database.
    /// For the second parameter see [GethDebugTracingOptions].
    #[method(name = "traceBlockByNumber")]
    async fn debug_trace_block_by_number(
        &self,
        block: BlockNumberOrTag,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<Vec<TraceResult>>;

    /// The `debug_traceTransaction` debugging method will attempt to run the transaction in the
    /// exact same manner as it was executed on the network. It will replay any transaction that
    /// may have been executed prior to this one before it will finally attempt to execute the
    /// transaction that corresponds to the given hash.
    #[method(name = "traceTransaction")]
    async fn debug_trace_transaction(
        &self,
        tx_hash: B256,
        opts: Option<GethDebugTracingOptions>,
    ) -> RpcResult<GethTrace>;

    /// The `debug_traceCall` method lets you run an `eth_call` within the context of the given
    /// block execution using the final state of parent block as the base.
    ///
    /// The first argument (just as in `eth_call`) is a transaction request.
    /// The block can optionally be specified either by hash or by number as
    /// the second argument.
    /// The trace can be configured similar to `debug_traceTransaction`,
    /// see [GethDebugTracingOptions]. The method returns the same output as
    /// `debug_traceTransaction`.
    #[method(name = "traceCall")]
    async fn debug_trace_call(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<GethTrace>;

    /// The `debug_traceCallMany` method lets you run an `eth_callMany` within the context of the
    /// given block execution using the final state of parent block as the base followed by n
    /// transactions.
    ///
    /// The first argument is a list of bundles. Each bundle can overwrite the block headers. This
    /// will affect all transaction in that bundle.
    /// BlockNumber and transaction_index are optional. Transaction_index
    /// specifies the number of tx in the block to replay and -1 means all transactions should be
    /// replayed.
    /// The trace can be configured similar to `debug_traceTransaction`.
    /// State override apply to all bundles.
    ///
    /// This methods is similar to many `eth_callMany`, hence this returns nested lists of traces.
    /// Where the length of the outer list is the number of bundles and the length of the inner list
    /// (`Vec<GethTrace>`) is the number of transactions in the bundle.
    #[method(name = "traceCallMany")]
    async fn debug_trace_call_many(
        &self,
        bundles: Vec<Bundle>,
        state_context: Option<StateContext>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<Vec<Vec<GethTrace>>>;

    /// The `debug_executionWitness` method allows for re-execution of a block with the purpose of
    /// generating an execution witness. The witness comprises of a map of all hashed trie nodes
    /// to their preimages that were required during the execution of the block, including during
    /// state root recomputation.
    ///
    /// The first and only argument is the block number or block hash.
    #[method(name = "executionWitness")]
    async fn debug_execution_witness(
        &self,
        block: BlockNumberOrTag,
    ) -> RpcResult<HashMap<B256, Bytes>>;

    /// Sets the logging backtrace location. When a backtrace location is set and a log message is
    /// emitted at that location, the stack of the goroutine executing the log statement will
    /// be printed to stderr.
    #[method(name = "backtraceAt")]
    async fn debug_backtrace_at(&self, location: &str) -> RpcResult<()>;

    /// Enumerates all accounts at a given block with paging capability. `maxResults` are returned
    /// in the page and the items have keys that come after the `start` key (hashed address).
    ///
    /// If incompletes is false, then accounts for which the key preimage (i.e: the address) doesn't
    /// exist in db are skipped. NB: geth by default does not store preimages.
    #[method(name = "accountRange")]
    async fn debug_account_range(
        &self,
        block_number: BlockNumberOrTag,
        start: Bytes,
        max_results: u64,
        nocode: bool,
        nostorage: bool,
        incompletes: bool,
    ) -> RpcResult<()>;

    /// Turns on block profiling for the given duration and writes profile data to disk. It uses a
    /// profile rate of 1 for most accurate information. If a different rate is desired, set the
    /// rate and write the profile manually using `debug_writeBlockProfile`.
    #[method(name = "blockProfile")]
    async fn debug_block_profile(&self, file: String, seconds: u64) -> RpcResult<()>;

    /// Flattens the entire key-value database into a single level, removing all unused slots and
    /// merging all keys.
    #[method(name = "chaindbCompact")]
    async fn debug_chaindb_compact(&self) -> RpcResult<()>;

    /// Returns leveldb properties of the key-value database.
    #[method(name = "chaindbProperty")]
    async fn debug_chaindb_property(&self, property: String) -> RpcResult<()>;

    /// Turns on CPU profiling for the given duration and writes profile data to disk.
    #[method(name = "cpuProfile")]
    async fn debug_cpu_profile(&self, file: String, seconds: u64) -> RpcResult<()>;

    /// Retrieves an ancient binary blob from the freezer. The freezer is a collection of
    /// append-only immutable files. The first argument `kind` specifies which table to look up data
    /// from. The list of all table kinds are as follows:
    #[method(name = "dbAncient")]
    async fn debug_db_ancient(&self, kind: String, number: u64) -> RpcResult<()>;

    /// Returns the number of ancient items in the ancient store.
    #[method(name = "dbAncients")]
    async fn debug_db_ancients(&self) -> RpcResult<()>;

    /// Returns the raw value of a key stored in the database.
    #[method(name = "dbGet")]
    async fn debug_db_get(&self, key: String) -> RpcResult<()>;

    /// Retrieves the state that corresponds to the block number and returns a list of accounts
    /// (including storage and code).
    #[method(name = "dumpBlock")]
    async fn debug_dump_block(&self, number: BlockId) -> RpcResult<()>;

    /// Forces garbage collection.
    #[method(name = "freeOSMemory")]
    async fn debug_free_os_memory(&self) -> RpcResult<()>;

    /// Forces a temporary client freeze, normally when the server is overloaded.
    #[method(name = "freezeClient")]
    async fn debug_freeze_client(&self, node: String) -> RpcResult<()>;

    /// Returns garbage collection statistics.
    #[method(name = "gcStats")]
    async fn debug_gc_stats(&self) -> RpcResult<()>;

    /// Returns the first number where the node has accessible state on disk. This is the
    /// post-state of that block and the pre-state of the next block. The (from, to) parameters
    /// are the sequence of blocks to search, which can go either forwards or backwards.
    ///
    /// Note: to get the last state pass in the range of blocks in reverse, i.e. (last, first).
    #[method(name = "getAccessibleState")]
    async fn debug_get_accessible_state(
        &self,
        from: BlockNumberOrTag,
        to: BlockNumberOrTag,
    ) -> RpcResult<()>;

    /// Returns all accounts that have changed between the two blocks specified. A change is defined
    /// as a difference in nonce, balance, code hash, or storage hash. With one parameter, returns
    /// the list of accounts modified in the specified block.
    #[method(name = "getModifiedAccountsByHash")]
    async fn debug_get_modified_accounts_by_hash(
        &self,
        start_hash: B256,
        end_hash: B256,
    ) -> RpcResult<()>;

    /// Returns all accounts that have changed between the two blocks specified. A change is defined
    /// as a difference in nonce, balance, code hash or storage hash.
    #[method(name = "getModifiedAccountsByNumber")]
    async fn debug_get_modified_accounts_by_number(
        &self,
        start_number: u64,
        end_number: u64,
    ) -> RpcResult<()>;

    /// Turns on Go runtime tracing for the given duration and writes trace data to disk.
    #[method(name = "goTrace")]
    async fn debug_go_trace(&self, file: String, seconds: u64) -> RpcResult<()>;

    /// Executes a block (bad- or canon- or side-), and returns a list of intermediate roots: the
    /// stateroot after each transaction.
    #[method(name = "intermediateRoots")]
    async fn debug_intermediate_roots(
        &self,
        block_hash: B256,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<()>;

    /// Returns detailed runtime memory statistics.
    #[method(name = "memStats")]
    async fn debug_mem_stats(&self) -> RpcResult<()>;

    /// Turns on mutex profiling for `nsec` seconds and writes profile data to file. It uses a
    /// profile rate of 1 for most accurate information. If a different rate is desired, set the
    /// rate and write the profile manually.
    #[method(name = "mutexProfile")]
    async fn debug_mutex_profile(&self, file: String, nsec: u64) -> RpcResult<()>;

    /// Returns the preimage for a sha3 hash, if known.
    #[method(name = "preimage")]
    async fn debug_preimage(&self, hash: B256) -> RpcResult<()>;

    /// Retrieves a block and returns its pretty printed form.
    #[method(name = "printBlock")]
    async fn debug_print_block(&self, number: u64) -> RpcResult<()>;

    /// Fetches and retrieves the seed hash of the block by number.
    #[method(name = "seedHash")]
    async fn debug_seed_hash(&self, number: u64) -> RpcResult<B256>;

    /// Sets the rate (in samples/sec) of goroutine block profile data collection. A non-zero rate
    /// enables block profiling, setting it to zero stops the profile. Collected profile data can be
    /// written using `debug_writeBlockProfile`.
    #[method(name = "setBlockProfileRate")]
    async fn debug_set_block_profile_rate(&self, rate: u64) -> RpcResult<()>;

    /// Sets the garbage collection target percentage. A negative value disables garbage collection.
    #[method(name = "setGCPercent")]
    async fn debug_set_gc_percent(&self, v: i32) -> RpcResult<()>;

    /// Sets the current head of the local chain by block number. Note, this is a destructive action
    /// and may severely damage your chain. Use with extreme caution.
    #[method(name = "setHead")]
    async fn debug_set_head(&self, number: u64) -> RpcResult<()>;

    /// Sets the rate of mutex profiling.
    #[method(name = "setMutexProfileFraction")]
    async fn debug_set_mutex_profile_fraction(&self, rate: i32) -> RpcResult<()>;

    /// Configures how often in-memory state tries are persisted to disk. The interval needs to be
    /// in a format parsable by a time.Duration. Note that the interval is not wall-clock time.
    /// Rather it is accumulated block processing time after which the state should be flushed.
    #[method(name = "setTrieFlushInterval")]
    async fn debug_set_trie_flush_interval(&self, interval: String) -> RpcResult<()>;

    /// Returns a printed representation of the stacks of all goroutines.
    #[method(name = "stacks")]
    async fn debug_stacks(&self) -> RpcResult<()>;

    /// Used to obtain info about a block.
    #[method(name = "standardTraceBadBlockToFile")]
    async fn debug_standard_trace_bad_block_to_file(
        &self,
        block: BlockNumberOrTag,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<()>;

    /// This method is similar to `debug_standardTraceBlockToFile`, but can be used to obtain info
    /// about a block which has been rejected as invalid (for some reason).
    #[method(name = "standardTraceBlockToFile")]
    async fn debug_standard_trace_block_to_file(
        &self,
        block: BlockNumberOrTag,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<()>;

    /// Turns on CPU profiling indefinitely, writing to the given file.
    #[method(name = "startCPUProfile")]
    async fn debug_start_cpu_profile(&self, file: String) -> RpcResult<()>;

    /// Starts writing a Go runtime trace to the given file.
    #[method(name = "startGoTrace")]
    async fn debug_start_go_trace(&self, file: String) -> RpcResult<()>;

    /// Stops an ongoing CPU profile.
    #[method(name = "stopCPUProfile")]
    async fn debug_stop_cpu_profile(&self) -> RpcResult<()>;

    /// Stops writing the Go runtime trace.
    #[method(name = "stopGoTrace")]
    async fn debug_stop_go_trace(&self) -> RpcResult<()>;

    /// Returns the storage at the given block height and transaction index. The result can be
    /// paged by providing a `maxResult` to cap the number of storage slots returned as well as
    /// specifying the offset via `keyStart` (hash of storage key).
    #[method(name = "storageRangeAt")]
    async fn debug_storage_range_at(
        &self,
        block_hash: B256,
        tx_idx: usize,
        contract_address: Address,
        key_start: B256,
        max_result: u64,
    ) -> RpcResult<()>;

    /// Returns the structured logs created during the execution of EVM against a block pulled
    /// from the pool of bad ones and returns them as a JSON object. For the second parameter see
    /// TraceConfig reference.
    #[method(name = "traceBadBlock")]
    async fn debug_trace_bad_block(
        &self,
        block_hash: B256,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<()>;

    /// Sets the logging verbosity ceiling. Log messages with level up to and including the given
    /// level will be printed.
    #[method(name = "verbosity")]
    async fn debug_verbosity(&self, level: usize) -> RpcResult<()>;

    /// Sets the logging verbosity pattern.
    #[method(name = "vmodule")]
    async fn debug_vmodule(&self, pattern: String) -> RpcResult<()>;

    /// Writes a goroutine blocking profile to the given file.
    #[method(name = "writeBlockProfile")]
    async fn debug_write_block_profile(&self, file: String) -> RpcResult<()>;

    /// Writes an allocation profile to the given file.
    #[method(name = "writeMemProfile")]
    async fn debug_write_mem_profile(&self, file: String) -> RpcResult<()>;

    /// Writes a goroutine blocking profile to the given file.
    #[method(name = "writeMutexProfile")]
    async fn debug_write_mutex_profile(&self, file: String) -> RpcResult<()>;
}
