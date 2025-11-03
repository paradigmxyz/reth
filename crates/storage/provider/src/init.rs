//! Genesis block number initialization utilities for static file providers.

use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use tracing::debug;

static GENESIS_BLOCK_NUMBER: AtomicU64 = AtomicU64::new(0);
static IS_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Set global genesis block number at the beginning for global usage
pub fn set_genesis_block_number(block_number: u64) {
    debug!("Setting genesis block number to {}", block_number);
    GENESIS_BLOCK_NUMBER.store(block_number, Ordering::SeqCst);
    IS_INITIALIZED.store(true, Ordering::SeqCst);
}

/// Get global genesis block number, NEVER call this before calling `set_genesis_block_number`
pub fn get_genesis_block_number() -> u64 {
    if !IS_INITIALIZED.load(Ordering::SeqCst) {
        eprintln!("ERROR: Genesis block number not initialized! Call set_genesis_block_number() first.");
        eprintln!("Stack trace:");
        let backtrace = std::backtrace::Backtrace::capture();
        eprintln!("{}", backtrace);
        std::process::exit(1);
    }
    GENESIS_BLOCK_NUMBER.load(Ordering::SeqCst)
}
