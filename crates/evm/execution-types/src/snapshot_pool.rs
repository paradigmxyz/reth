use alloy_primitives::BlockNumber;
use once_cell::sync::Lazy;
use std::sync::Mutex;

/// Global accumulator for Parlia checkpoint snapshots that are produced during
/// block execution (every 1 024 blocks) by the BSC‐specific executor and
/// consumed when constructing an `ExecutionOutcome`.
///
/// Each entry stores `(block_number, cbor_compressed_snapshot_bytes)`.
static SNAPSHOT_POOL: Lazy<Mutex<Vec<(BlockNumber, Vec<u8>)>>> = Lazy::new(|| Mutex::new(Vec::new()));

/// Push a newly created snapshot blob into the global pool.
pub fn push(snapshot: (BlockNumber, Vec<u8>)) {
    SNAPSHOT_POOL.lock().expect("snapshot mutex poisoned").push(snapshot);
}

/// Drain **all** queued snapshots, returning them in FIFO order.
pub fn drain() -> Vec<(BlockNumber, Vec<u8>)> {
    SNAPSHOT_POOL.lock().expect("snapshot mutex poisoned").drain(..).collect()
} 