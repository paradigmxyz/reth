//! Workload predictor for parallel trie computation.
//!
//! Uses exponential moving average (EMA) to predict the next block's workload,
//! enabling better resource allocation for parallel state root computation.

use std::sync::atomic::{AtomicU64, Ordering};

/// Smoothing factor for exponential moving average.
/// Lower values give more weight to historical data.
const ALPHA: f64 = 0.3;

/// Headroom multiplier applied to predictions.
/// Ensures we slightly over-provision to handle workload variance.
const HEADROOM: f64 = 1.25;

/// Lock-free workload predictor using exponential moving average (stores f64 bits as u64).
///
/// This predictor tracks historical workload patterns and predicts future
/// resource requirements for parallel trie computation. It uses atomic
/// operations for thread-safe, lock-free updates.
#[derive(Debug, Default)]
pub struct WorkloadEma(AtomicU64);

impl WorkloadEma {
    /// Creates a new workload EMA with zero initial estimate.
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    /// Records observed workload and updates the EMA.
    pub fn record(&self, value: usize) {
        let value = value as f64;
        loop {
            let current_bits = self.0.load(Ordering::Relaxed);
            let current = f64::from_bits(current_bits);

            // EMA formula: new_ema = alpha * value + (1 - alpha) * current_ema
            // For first observation (current == 0), use the value directly
            let new_ema =
                if current == 0.0 { value } else { ALPHA.mul_add(value, (1.0 - ALPHA) * current) };

            if self
                .0
                .compare_exchange_weak(
                    current_bits,
                    new_ema.to_bits(),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
        }
    }

    /// Predicts the next workload with headroom applied.
    pub fn predict(&self) -> usize {
        let ema = f64::from_bits(self.0.load(Ordering::Relaxed));
        (ema * HEADROOM).ceil() as usize
    }
}

/// Lock-free workload predictor using exponential moving average.
///
/// This predictor tracks historical workload patterns and predicts future
/// resource requirements for parallel trie computation. It uses atomic
/// operations for thread-safe, lock-free updates.
///
/// The struct is cache-line aligned (64 bytes) to prevent false sharing
/// when accessed from multiple threads.
#[repr(align(64))]
#[derive(Debug, Default)]
pub struct WorkloadPredictor {
    /// EMA of storage slot count.
    storage_ema: WorkloadEma,
    /// EMA of account count.
    account_ema: WorkloadEma,
}

impl WorkloadPredictor {
    /// Creates a new workload predictor with zero initial estimates.
    pub const fn new() -> Self {
        Self { storage_ema: WorkloadEma::new(), account_ema: WorkloadEma::new() }
    }

    /// Records observed workload after processing a block.
    pub fn record(&self, storage_targets: usize, account_targets: usize) {
        self.storage_ema.record(storage_targets);
        self.account_ema.record(account_targets);
    }

    /// Predicts the next block's workload with headroom.
    ///
    /// Returns `(storage_count, account_count)` predictions.
    pub fn predict(&self) -> (usize, usize) {
        (self.storage_ema.predict(), self.account_ema.predict())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_ema() {
        let ema = WorkloadEma::new();
        assert_eq!(ema.predict(), 0);
    }

    #[test]
    fn test_first_record() {
        let ema = WorkloadEma::new();
        ema.record(100);
        assert_eq!(ema.predict(), (100.0 * HEADROOM).ceil() as usize);
    }

    #[test]
    fn test_ema_convergence() {
        let ema = WorkloadEma::new();
        for _ in 0..10 {
            ema.record(1000);
        }
        assert!((ema.predict() as f64 - 1000.0 * HEADROOM).abs() < 10.0);
    }

    #[test]
    fn test_predictor() {
        let predictor = WorkloadPredictor::new();
        predictor.record(100, 50);
        let (storage, accounts) = predictor.predict();
        assert_eq!(storage, (100.0 * HEADROOM).ceil() as usize);
        assert_eq!(accounts, (50.0 * HEADROOM).ceil() as usize);
    }

    #[test]
    fn test_cache_alignment() {
        assert_eq!(std::mem::align_of::<WorkloadPredictor>(), 64);
    }
}
