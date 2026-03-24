//! Task utility functions.

pub use thread_priority::{self, *};

/// Increases the current thread's priority.
///
/// Tries [`ThreadPriority::Max`] first. If that fails (e.g. missing `CAP_SYS_NICE`),
/// falls back to a moderate bump via [`ThreadPriority::Crossplatform`] (~5 nice points
/// on unix). Failures are logged at `debug` level.
pub fn increase_thread_priority() {
    if let Err(err) = ThreadPriority::Max.set_for_current() {
        tracing::debug!(?err, "failed to set max thread priority, trying moderate bump");
        // Crossplatform value 62/99 ≈ nice -5 on unix.
        let fallback = ThreadPriority::Crossplatform(
            ThreadPriorityValue::try_from(62u8).expect("62 is within the valid 0..100 range"),
        );
        if let Err(err) = fallback.set_for_current() {
            tracing::debug!(?err, "failed to set moderate thread priority");
        }
    }
}
