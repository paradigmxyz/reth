//! Bounded, block-local speculative results. A miss never waits for a worker.

use alloy_primitives::B256;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

/// Avoid allocating an unbounded artifact table from untrusted transaction counts.
pub(crate) const MAX_ARTIFACTS: usize = 32_768;

#[derive(Debug)]
pub(crate) struct PrewarmArtifacts<A> {
    enabled: AtomicBool,
    slots: Vec<Mutex<Option<(B256, A)>>>,
}

impl<A> PrewarmArtifacts<A> {
    pub(crate) fn new(count: usize) -> Option<Self> {
        (count <= MAX_ARTIFACTS).then(|| Self {
            enabled: AtomicBool::new(true),
            slots: (0..count).map(|_| Mutex::new(None)).collect(),
        })
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub(crate) fn disable(&self) {
        self.enabled.store(false, Ordering::Relaxed);
    }

    pub(crate) fn insert(&self, index: usize, hash: B256, artifact: A) {
        if !self.enabled() {
            return
        }
        if let Some(slot) = self.slots.get(index) {
            if let Ok(mut slot) = slot.try_lock() {
                if self.enabled() && slot.is_none() {
                    *slot = Some((hash, artifact));
                }
            }
        }
    }

    pub(crate) fn take(&self, index: usize, hash: B256) -> Option<A> {
        if !self.enabled() {
            return None
        }
        let mut slot = self.slots.get(index)?.try_lock().ok()?;
        let (expected, artifact) = slot.take()?;
        (expected == hash).then_some(artifact)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifacts_are_bounded_and_consumed_once_for_exact_identity() {
        assert!(PrewarmArtifacts::<u32>::new(MAX_ARTIFACTS + 1).is_none());
        let slots = PrewarmArtifacts::new(2).unwrap();
        let hash = B256::repeat_byte(1);
        assert_eq!(slots.take(0, hash), None);
        slots.insert(0, hash, 7);
        assert_eq!(slots.take(1, hash), None);
        assert_eq!(slots.take(0, hash), Some(7));
        assert_eq!(slots.take(0, hash), None);
        slots.insert(1, hash, 8);
        assert_eq!(slots.take(1, B256::ZERO), None);
        assert_eq!(slots.take(1, hash), None);
    }

    #[test]
    fn cancellation_and_contention_fall_back_without_waiting() {
        let slots = PrewarmArtifacts::new(1).unwrap();
        slots.insert(0, B256::ZERO, 1);
        let guard = slots.slots[0].lock().unwrap();
        assert_eq!(slots.take(0, B256::ZERO), None);
        drop(guard);
        slots.disable();
        assert_eq!(slots.take(0, B256::ZERO), None);
        slots.insert(0, B256::ZERO, 2);
        assert!(!slots.enabled());
    }
}
