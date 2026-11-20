/// A type that knows when the last fork choice update was received.
pub trait CanonChainTracker: Send + Sync {
    /// Returns the last time a fork choice update was received from the CL.
    #[cfg(feature = "std")]
    fn last_received_update_timestamp(&self) -> Option<std::time::Instant>;
}
