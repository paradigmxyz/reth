/// A type that knows when the last fork choice update was received.
pub trait CanonChainTracker: Send + Sync {
    /// Returns the last time a fork choice update was received from the CL.
    ///
    /// Returns `None` if none was received yet, or if the type does not track them.
    #[cfg(feature = "std")]
    fn last_received_update_timestamp(&self) -> Option<std::time::Instant>;
}
