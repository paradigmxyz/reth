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

/// Lock-free workload predictor using exponential moving average.
///
/// This predictor tracks historical workload patterns and predicts future
/// resource requirements for parallel trie computation. It uses atomic
/// operations for thread-safe, lock-free updates.
///
/// The struct is cache-line aligned (64 bytes) to prevent false sharing
/// when accessed from multiple threads.
#[repr(align(64))]
#[derive(Debug)]
pub struct WorkloadPredictor {
    /// EMA of storage slot count (stores f64 bits as u64).
    storage_ema: AtomicU64,
    /// EMA of account count (stores f64 bits as u64).
    account_ema: AtomicU64,
}

impl Default for WorkloadPredictor {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkloadPredictor {
    /// Creates a new workload predictor with zero initial estimates.
    pub const fn new() -> Self {
        Self { storage_ema: AtomicU64::new(0), account_ema: AtomicU64::new(0) }
    }

    /// Records observed workload after processing a block.
    ///
    /// Updates the internal EMAs with the actual storage and account counts.
    pub fn record(&self, storage_targets: usize, account_targets: usize) {
        self.update_ema(&self.storage_ema, storage_targets as f64);
        self.update_ema(&self.account_ema, account_targets as f64);
    }

    /// Predicts the next block's workload with headroom.
    ///
    /// Returns `(storage_count, account_count)` predictions based on
    /// historical EMA values multiplied by the headroom factor.
    pub fn predict(&self) -> (usize, usize) {
        let storage = f64::from_bits(self.storage_ema.load(Ordering::Relaxed));
        let accounts = f64::from_bits(self.account_ema.load(Ordering::Relaxed));

        let predicted_storage = (storage * HEADROOM).ceil() as usize;
        let predicted_accounts = (accounts * HEADROOM).ceil() as usize;

        (predicted_storage, predicted_accounts)
    }

    /// Updates an EMA atomic using a compare-and-swap loop.
    fn update_ema(&self, atomic: &AtomicU64, value: f64) {
        loop {
            let current_bits = atomic.load(Ordering::Relaxed);
            let current = f64::from_bits(current_bits);

            // EMA formula: new_ema = alpha * value + (1 - alpha) * current_ema
            // For first observation (current == 0), use the value directly
            let new_ema =
                if current == 0.0 { value } else { ALPHA.mul_add(value, (1.0 - ALPHA) * current) };

            let new_bits = new_ema.to_bits();

            if atomic
                .compare_exchange_weak(current_bits, new_bits, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_predictor() {
        let predictor = WorkloadPredictor::new();
        let (storage, accounts) = predictor.predict();
        assert_eq!(storage, 0);
        assert_eq!(accounts, 0);
    }

    #[test]
    fn test_first_record() {
        let predictor = WorkloadPredictor::new();
        predictor.record(100, 50);

        let (storage, accounts) = predictor.predict();
        // First record uses value directly, then headroom applied
        assert_eq!(storage, (100.0 * HEADROOM).ceil() as usize);
        assert_eq!(accounts, (50.0 * HEADROOM).ceil() as usize);
    }

    #[test]
    fn test_ema_convergence() {
        let predictor = WorkloadPredictor::new();

        // Record same value multiple times
        for _ in 0..10 {
            predictor.record(1000, 500);
        }

        let (storage, accounts) = predictor.predict();
        // Should converge close to value * headroom
        assert!((storage as f64 - 1000.0 * HEADROOM).abs() < 10.0);
        assert!((accounts as f64 - 500.0 * HEADROOM).abs() < 10.0);
    }

    #[test]
    fn test_cache_alignment() {
        assert_eq!(std::mem::align_of::<WorkloadPredictor>(), 64);
    }
}
