//! Request-scoped witness capture shared by the Engine API and its validator.

use alloy_primitives::{map::B256Map, Bytes, B256};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};

/// Coordinates witness capture without changing engine messages or execution results.
///
/// Each node owns its registry. Entries live only while RPC requests are outstanding; this is
/// not a block cache. Concurrent requests for the same block share the capture.
#[derive(Clone, Debug, Default)]
pub struct PayloadWitnessRequests {
    inner: Arc<Requests>,
}

impl PayloadWitnessRequests {
    /// Marks this registry as connected to a validator supporting capture.
    pub fn enable(&self) {
        self.inner.enabled.store(true, Ordering::Release);
    }

    /// Returns whether the node's validator supports capture.
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Acquire)
    }

    /// Registers interest before submitting a payload to the engine.
    pub fn register(&self, hash: B256) -> PayloadWitnessRequest {
        let mut requests = self.inner.requests.lock().unwrap();
        let (count, result) = requests.entry(hash).or_default();
        *count += 1;
        let result = result.clone();
        self.inner.active.store(true, Ordering::Release);
        PayloadWitnessRequest { requests: self.clone(), hash, result }
    }

    /// Returns the capture slot for a block with an outstanding witness request.
    ///
    /// Validators publish only after all block validation has succeeded. Proof failures are
    /// reported separately from consensus validation through this slot.
    pub fn pending(&self, hash: &B256) -> Option<PayloadWitnessCapture> {
        if !self.inner.active.load(Ordering::Acquire) {
            return None;
        }
        self.inner.requests.lock().unwrap().get(hash).map(|(_, result)| result.clone())
    }
}

/// Keeps a capture alive until its RPC completes or is cancelled.
#[derive(Debug)]
pub struct PayloadWitnessRequest {
    requests: PayloadWitnessRequests,
    hash: B256,
    result: PayloadWitnessCapture,
}

impl PayloadWitnessRequest {
    /// Returns the captured witness, if execution selected this request for capture.
    /// Requests arriving after execution starts may have no witness, just like already-known
    /// blocks.
    pub fn result(&self) -> Option<&Result<Bytes, String>> {
        self.result.result.get()
    }
}

impl Drop for PayloadWitnessRequest {
    fn drop(&mut self) {
        let mut requests = self.requests.inner.requests.lock().unwrap();
        if let Some((count, _)) = requests.get_mut(&self.hash) {
            *count -= 1;
            if *count == 0 {
                requests.remove(&self.hash);
            }
        }
        self.requests.inner.active.store(!requests.is_empty(), Ordering::Release);
    }
}

/// A capture slot belonging to one generation of outstanding requests.
#[derive(Clone, Debug, Default)]
pub struct PayloadWitnessCapture {
    result: Arc<OnceLock<Result<Bytes, String>>>,
}

impl PayloadWitnessCapture {
    /// Publishes a witness or proof error after successful block validation.
    pub fn publish(&self, result: Result<Bytes, String>) {
        let _ = self.result.set(result);
    }
}

#[derive(Debug, Default)]
struct Requests {
    enabled: AtomicBool,
    active: AtomicBool,
    requests: Mutex<B256Map<(usize, PayloadWitnessCapture)>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_requests_share_capture_and_clean_up() {
        let requests = PayloadWitnessRequests::default();
        let first = requests.register(B256::ZERO);
        let second = requests.register(B256::ZERO);
        let capture = requests.pending(&B256::ZERO).unwrap();
        capture.publish(Ok(Bytes::from_static(b"witness")));
        assert_eq!(first.result(), Some(&Ok(Bytes::from_static(b"witness"))));
        assert_eq!(first.result(), second.result());
        drop(first);
        assert!(requests.pending(&B256::ZERO).is_some());
        drop(second);
        assert!(requests.pending(&B256::ZERO).is_none());
        assert!(requests.register(B256::ZERO).result().is_none());
    }

    #[test]
    fn cancellation_does_not_publish_to_a_later_request() {
        let requests = PayloadWitnessRequests::default();
        let cancelled = requests.register(B256::ZERO);
        let capture = requests.pending(&B256::ZERO).unwrap();
        drop(cancelled);
        let later = requests.register(B256::ZERO);
        capture.publish(Ok(Bytes::new()));
        assert!(later.result().is_none());
    }
}
