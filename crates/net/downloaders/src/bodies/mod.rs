/// A naive concurrent downloader.
#[expect(clippy::module_inception)]
pub mod bodies;

/// Best-effort historical block access-list fetching.
pub mod bal_prefetch;

/// A body downloader that does nothing. Useful to build unwind-only pipelines.
pub mod noop;

/// A downloader implementation that spawns a downloader to a task
pub mod task;

mod queue;
mod request;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
