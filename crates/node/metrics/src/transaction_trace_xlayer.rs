//! Transaction tracing module for monitoring transaction lifecycle

use alloy_primitives::B256;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

/// Number of log entries to write before forcing a flush
const FLUSH_INTERVAL_WRITES: u64 = 100;

/// Time interval between flushes (in seconds)
const FLUSH_INTERVAL_SECONDS: u64 = 1;

/// Fixed chain name
const CHAIN_NAME: &str = "X Layer";

/// Fixed business name
const BUSINESS_NAME: &str = "X Layer";

/// Fixed chain ID
const CHAIN_ID: &str = "196";

/// RPC service name
const RPC_SERVICE_NAME: &str = "okx-defi-xlayer-rpcpay-pro";

/// Sequencer service name
const SEQ_SERVICE_NAME: &str = "okx-defi-xlayer-egseqz-pro";

/// Node type for identifying sequencer vs RPC node
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    /// Sequencer node (builds blocks)
    Sequencer,
    /// RPC node (forwards transactions to sequencer)
    Rpc,
    /// Unknown node type (default)
    Unknown,
}

impl NodeType {
    /// Returns the string representation of the node type
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeType::Sequencer => "sequencer",
            NodeType::Rpc => "rpc",
            NodeType::Unknown => "unknown",
        }
    }
}

/// Transaction process ID for tracking different stages in the transaction lifecycle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransactionProcessId {
    /// RPC node: Transaction received and ready to forward
    RpcReceiveTxEnd = 15010,
    
    /// Sequencer node: Transaction received and added to pool
    SeqReceiveTxEnd = 15030,

    /// Sequencer node: Block building started
    SeqBlockBuildStart = 15032,

    /// Sequencer node: Transaction execution completed
    SeqTxExecutionEnd = 15034,

    /// Sequencer node: Block building completed
    SeqBlockBuildEnd = 15036,

    /// Sequencer node: Block sending started
    SeqBlockSendStart = 15042,

    /// RPC node: Block received from sequencer
    RpcBlockReceiveEnd = 15060,
    
    /// RPC node: Block insertion completed
    RpcBlockInsertEnd = 15062,
}

impl TransactionProcessId {
    /// Returns the string representation of the process ID
    pub fn as_str(&self) -> &'static str {
        match self {
            TransactionProcessId::RpcReceiveTxEnd => "xlayer_rpc_receive_tx",
            TransactionProcessId::SeqReceiveTxEnd => "xlayer_seq_receive_tx",
            TransactionProcessId::SeqBlockBuildStart => "xlayer_seq_begin_block",
            TransactionProcessId::SeqTxExecutionEnd => "xlayer_seq_package_tx",
            TransactionProcessId::SeqBlockBuildEnd => "xlayer_seq_end_block",
            TransactionProcessId::SeqBlockSendStart => "xlayer_seq_ds_sent",
            TransactionProcessId::RpcBlockReceiveEnd => "xlayer_rpc_receive_block",
            TransactionProcessId::RpcBlockInsertEnd => "xlayer_rpc_finish_block",
        }
    }

    /// Returns the service name based on the process ID
    pub fn service_name(&self) -> &'static str {
        match self {
            // RPC-related process IDs
            TransactionProcessId::RpcReceiveTxEnd
            | TransactionProcessId::RpcBlockReceiveEnd
            | TransactionProcessId::RpcBlockInsertEnd => RPC_SERVICE_NAME,
            
            // Sequencer-related process IDs
            TransactionProcessId::SeqReceiveTxEnd
            | TransactionProcessId::SeqBlockBuildStart
            | TransactionProcessId::SeqTxExecutionEnd
            | TransactionProcessId::SeqBlockBuildEnd
            | TransactionProcessId::SeqBlockSendStart => SEQ_SERVICE_NAME,
        }
    }
}


/// Transaction tracer for monitoring transaction lifecycle
#[derive(Clone, Debug)]
pub struct TransactionTracer {
    inner: Arc<TransactionTracerInner>,
}

#[derive(Debug)]
struct TransactionTracerInner {
    enabled: bool,
    #[allow(dead_code)]
    node_type: NodeType,
    output_file: Mutex<Option<File>>,
    write_count: AtomicU64,
    last_flush_time: Mutex<Instant>,
}

impl TransactionTracer {
    /// Create a new transaction tracer
    pub fn new(enabled: bool, output_path: Option<PathBuf>, node_type: NodeType) -> Self {
        let output_file = if let Some(ref path) = output_path {
            let file_path = if path.to_string_lossy().ends_with('/') || path.to_string_lossy().ends_with('\\') {
                path.join("trace.log")
            } else if path.extension().is_none() && !path.exists() {
                path.join("trace.log")
            } else {
                path.clone()
            };
            if let Some(parent) = file_path.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    tracing::warn!(
                        target: "tx_trace",
                        ?parent,
                        error = %e,
                        "Failed to create transaction trace output directory"
                    );
                }
            }

            match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file_path)
            {
                Ok(file) => {
                    tracing::info!(
                        target: "tx_trace",
                        ?file_path,
                        "Transaction trace file opened for appending"
                    );
                    Some(file)
                }
                Err(e) => {
                    tracing::warn!(
                        target: "tx_trace",
                        ?file_path,
                        error = %e,
                        "Failed to open transaction trace file"
                    );
                    None
                }
            }
        } else {
            None
        };

        Self {
            inner: Arc::new(TransactionTracerInner {
                enabled,
                node_type,
                output_file: Mutex::new(output_file),
                write_count: AtomicU64::new(0),
                last_flush_time: Mutex::new(Instant::now()),
            }),
        }
    }

    /// Check if tracing is enabled
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled
    }

    /// Write CSV line to trace file with periodic flush
    fn write_to_file(&self, csv_line: &str) {
        match self.inner.output_file.lock() {
            Ok(mut file_guard) => {
                if let Some(ref mut file) = *file_guard {
                    if let Err(e) = writeln!(file, "{}", csv_line) {
                        tracing::warn!(
                            target: "tx_trace",
                            error = %e,
                            "Failed to write to transaction trace file"
                        );
                    } else {
                        let count = self.inner.write_count.fetch_add(1, Ordering::Relaxed) + 1;
                        
                        let should_flush = {
                            let mut last_flush = self.inner.last_flush_time.lock().unwrap();
                            let now = Instant::now();
                            let time_since_flush = now.duration_since(*last_flush);
                            
                            if count % FLUSH_INTERVAL_WRITES == 0 || time_since_flush.as_secs() >= FLUSH_INTERVAL_SECONDS {
                                *last_flush = now;
                                true
                            } else {
                                false
                            }
                        };
                        
                        if should_flush {
                            if let Err(e) = file.flush() {
                                tracing::warn!(
                                    target: "tx_trace",
                                    error = %e,
                                    "Failed to flush transaction trace file"
                                );
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "tx_trace",
                    error = %e,
                    "Failed to acquire lock for transaction trace file"
                );
            }
        }
    }
    
    /// Force flush the trace file
    pub fn flush(&self) {
        match self.inner.output_file.lock() {
            Ok(mut file_guard) => {
                if let Some(ref mut file) = *file_guard {
                    if let Err(e) = file.flush() {
                        tracing::warn!(
                            target: "tx_trace",
                            error = %e,
                            "Failed to flush transaction trace file on shutdown"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "tx_trace",
                    error = %e,
                    "Failed to acquire lock for flushing transaction trace file"
                );
            }
        }
    }

    /// Format CSV line with 23 fields
    fn format_csv_line(
        &self,
        trace: &str,
        process_id: TransactionProcessId,
        current_time: u128,
        block_hash: Option<B256>,
        block_number: Option<u64>,
    ) -> String {
        let escape_csv = |s: &str| -> String {
            if s.contains(',') || s.contains('"') || s.contains('\n') {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_string()
            }
        };

        let chain = CHAIN_NAME;
        let trace_hash = trace.to_lowercase();
        let status_str = "";
        let service_name = process_id.service_name();
        let business = BUSINESS_NAME;
        let client = "";
        let chainld = CHAIN_ID;
        let process_str = (process_id as u32).to_string();
        let process_word_str = process_id.as_str();
        let index = "";
        let inner_index = "";
        let current_time_str = current_time.to_string();
        let referld = "";
        let contract_address = "";
        let block_height = block_number.map(|n| n.to_string()).unwrap_or_default();
        let block_hash_str = block_hash.map(|h| format!("{:#x}", h).to_lowercase()).unwrap_or_default();
        let block_time = "";
        let deposit_confirm_height = "";
        let token_id = "";
        let mev_supplier = "";
        let business_hash = "";
        let transaction_type = "";
        let ext_json = "";
        
        format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            escape_csv(chain), escape_csv(&trace_hash), escape_csv(status_str), escape_csv(service_name),
            escape_csv(business), escape_csv(client), escape_csv(chainld), escape_csv(&process_str),
            escape_csv(process_word_str), escape_csv(index), escape_csv(inner_index), escape_csv(&current_time_str),
            escape_csv(referld), escape_csv(contract_address), escape_csv(&block_height), escape_csv(&block_hash_str),
            escape_csv(block_time), escape_csv(deposit_confirm_height), escape_csv(token_id), escape_csv(mev_supplier),
            escape_csv(business_hash), escape_csv(transaction_type), escape_csv(ext_json)
        )
    }

    /// Log transaction event at current time point
    pub fn log_transaction(
        &self,
        tx_hash: B256,
        process_id: TransactionProcessId,
        block_number: Option<u64>,
    ) {
        if !self.inner.enabled {
            return;
        }

        let timestamp_duration = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let timestamp_ms = timestamp_duration.as_millis();
        let trace_hash = format!("{:#x}", tx_hash);
        
        let csv_line = self.format_csv_line(
            &trace_hash,
            process_id,
            timestamp_ms,
            None,
            block_number,
        );
        
        self.write_to_file(&csv_line);
    }

}

impl Default for TransactionTracer {
    fn default() -> Self {
        Self::new(false, None, NodeType::Unknown)
    }
}

/// Global transaction tracer instance
static GLOBAL_TRACER: std::sync::OnceLock<Arc<TransactionTracer>> = std::sync::OnceLock::new();

/// Initialize the global transaction tracer
pub fn init_global_tracer(enabled: bool, output_path: Option<PathBuf>, node_type: NodeType) {
    let tracer = TransactionTracer::new(enabled, output_path, node_type);
    GLOBAL_TRACER.set(Arc::new(tracer)).ok();
}

/// Get the global transaction tracer
pub fn get_global_tracer() -> Option<Arc<TransactionTracer>> {
    GLOBAL_TRACER.get().cloned()
}

/// Flush the global transaction tracer
pub fn flush_global_tracer() {
    if let Some(tracer) = get_global_tracer() {
        tracer.flush();
    }
}

/// Block tracing functions for monitoring block lifecycle
impl TransactionTracer {
    /// Log block event at current time point
    pub fn log_block(
        &self,
        block_hash: B256,
        block_number: u64,
        process_id: TransactionProcessId,
    ) {
        if !self.inner.enabled {
            return;
        }

        let timestamp_duration = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let timestamp_ms = timestamp_duration.as_millis();
        let trace_hash = format!("{:#x}", block_hash);
        
        let csv_line = self.format_csv_line(
            &trace_hash,
            process_id,
            timestamp_ms,
            Some(block_hash),
            Some(block_number),
        );
        
        self.write_to_file(&csv_line);
    }

    /// Log block event with a specific timestamp
    /// 
    /// This method is used when we need to log a block event with a timestamp
    /// that was saved earlier (e.g., when block building started but block hash
    /// was not yet available).
    /// 
    /// # Arguments
    /// 
    /// * `block_hash` - The hash of the block
    /// * `block_number` - The number of the block
    /// * `process_id` - The process ID for this event
    /// * `timestamp_ms` - The timestamp in milliseconds to use for this log entry
    pub fn log_block_with_timestamp(
        &self,
        block_hash: B256,
        block_number: u64,
        process_id: TransactionProcessId,
        timestamp_ms: u128,
    ) {
        if !self.inner.enabled {
            return;
        }

        let trace_hash = format!("{:#x}", block_hash);
        
        let csv_line = self.format_csv_line(
            &trace_hash,
            process_id,
            timestamp_ms,
            Some(block_hash),
            Some(block_number),
        );
        
        self.write_to_file(&csv_line);
    }

}


#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use std::{
        fs,
        io::Read,
        sync::Arc,
        thread,
    };
    use tempfile::TempDir;

    fn create_test_tracer() -> (TransactionTracer, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let trace_file = temp_dir.path().join("trace.log");
        let tracer = TransactionTracer::new(true, Some(trace_file), NodeType::Unknown);
        (tracer, temp_dir)
    }

    #[test]
    fn test_tracer_creation() {
        let tracer = TransactionTracer::new(false, None, NodeType::Unknown);
        assert!(!tracer.is_enabled());

        let (tracer, _temp_dir) = create_test_tracer();
        assert!(tracer.is_enabled());
    }

    #[test]
    fn test_log_transaction() {
        let (tracer, temp_dir) = create_test_tracer();
        let tx_hash = B256::from([1u8; 32]);

        tracer.log_transaction(tx_hash, TransactionProcessId::RpcReceiveTxEnd, None);

        let trace_file = temp_dir.path().join("trace.log");
        assert!(trace_file.exists());

        let mut contents = String::new();
        fs::File::open(&trace_file)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();

        // CSV format: chain,trace,status,serviceName,business,client,chainld,process,processWord,index,innerIndex,currentTime,referld,contractAddress,blockHeight,blockHash,blockTime,depositConfirmHeight,tokenID,mevSupplier,businessHash,transactionType,extJson
        assert!(contents.contains(","));
        let tx_hash_lower = format!("{:#x}", tx_hash).to_lowercase();
        assert!(contents.contains(&tx_hash_lower));
    }

    #[test]
    fn test_log_block() {
        let (tracer, temp_dir) = create_test_tracer();
        let block_hash = B256::from([2u8; 32]);
        let block_number = 12345u64;

        tracer.log_block(block_hash, block_number, TransactionProcessId::RpcBlockInsertEnd);

        let trace_file = temp_dir.path().join("trace.log");
        let mut contents = String::new();
        fs::File::open(&trace_file)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();

        // CSV format: chain,trace,status,serviceName,business,client,chainld,process,processWord,index,innerIndex,currentTime,referld,contractAddress,blockHeight,blockHash,blockTime,depositConfirmHeight,tokenID,mevSupplier,businessHash,transactionType,extJson
        assert!(contents.contains(","));
        let block_hash_lower = format!("{:#x}", block_hash).to_lowercase();
        assert!(contents.contains(&block_hash_lower));
        assert!(contents.contains(&block_number.to_string()));
    }

    #[test]
    fn test_concurrent_write_to_file() {
        let (tracer, temp_dir) = create_test_tracer();
        let tracer = Arc::new(tracer);
        let trace_file = temp_dir.path().join("trace.log");

        let num_threads = 10;
        let writes_per_thread = 100;
        let mut handles: Vec<std::thread::JoinHandle<()>> = vec![];

        for thread_id in 0..num_threads {
            let tracer_clone = Arc::clone(&tracer);
            let handle = thread::spawn(move || {
                for _i in 0..writes_per_thread {
                    let tx_hash = B256::from([thread_id as u8; 32]);
                    let process_id = TransactionProcessId::RpcReceiveTxEnd;
                    tracer_clone.log_transaction(tx_hash, process_id, None);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let mut contents = String::new();
        fs::File::open(&trace_file)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();

        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), num_threads * writes_per_thread);

        // Verify all lines are valid CSV (23 fields separated by commas)
        for line in lines {
            let fields: Vec<&str> = line.split(',').collect();
            assert!(fields.len() >= 23, "CSV line should have at least 23 fields");
        }
    }


    #[test]
    fn test_concurrent_write_with_different_hashes() {
        let (tracer, temp_dir) = create_test_tracer();
        let tracer = Arc::new(tracer);
        let trace_file = temp_dir.path().join("trace.log");

        let num_threads = 5;
        let mut handles = vec![];

        for thread_id in 0..num_threads {
            let tracer_clone = Arc::clone(&tracer);
            let handle = thread::spawn(move || {
                let mut hash_bytes = [0u8; 32];
                hash_bytes[0] = thread_id as u8;
                let tx_hash = B256::from(hash_bytes);

                for _ in 0..50 {
                    tracer_clone.log_transaction(tx_hash, TransactionProcessId::SeqTxExecutionEnd, None);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let mut contents = String::new();
        fs::File::open(&trace_file)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();

        let log_lines: Vec<&str> = contents
            .lines()
            .filter(|line| !line.is_empty())
            .collect();

        // Each thread writes 50 transactions, so total should be num_threads * 50
        assert_eq!(log_lines.len(), num_threads * 50);

        for line in log_lines {
            assert!(
                line.split(',').count() >= 23, // CSV format with 23 fields
                "Invalid CSV in concurrent write test"
            );
        }
    }
}
