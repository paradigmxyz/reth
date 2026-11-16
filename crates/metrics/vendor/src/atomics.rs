//! Atomic types used for metrics.
//!
//! As the most commonly used types for metrics storage are atomic integers, implementations of
//! `CounterFn` and `GaugeFn` must be provided in this crate due to Rust's "orphan rules", which
//! disallow a crate from implementing a foreign trait on a foreign type.
//!
//! Further, we always require an atomic integer of a certain size regardless of whether the
//! standard library exposes an atomic integer of that size for the target architecture.
//!
//! As such, the atomic types that we provide handle implementations for are publicly re-exporter
//! here for downstream crates to utilize.

use std::sync::atomic::Ordering;

#[cfg(target_pointer_width = "32")]
pub use portable_atomic::AtomicU64;
#[cfg(not(target_pointer_width = "32"))]
pub use std::sync::atomic::AtomicU64;

use super::{CounterFn, GaugeFn};

impl CounterFn for AtomicU64 {
    fn increment(&self, value: u64) {
        let _ = self.fetch_add(value, Ordering::Release);
    }

    fn absolute(&self, value: u64) {
        let _ = self.fetch_max(value, Ordering::AcqRel);
    }
}

impl GaugeFn for AtomicU64 {
    fn increment(&self, value: f64) {
        loop {
            let result = self.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |curr| {
                let input = f64::from_bits(curr);
                let output = input + value;
                Some(output.to_bits())
            });

            if result.is_ok() {
                break;
            }
        }
    }

    fn decrement(&self, value: f64) {
        loop {
            let result = self.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |curr| {
                let input = f64::from_bits(curr);
                let output = input - value;
                Some(output.to_bits())
            });

            if result.is_ok() {
                break;
            }
        }
    }

    fn set(&self, value: f64) {
        let _ = self.swap(value.to_bits(), Ordering::AcqRel);
    }
}
