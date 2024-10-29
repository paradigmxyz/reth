/// A naive concurrent downloader.
#[allow(clippy::module_inception)]
pub mod bodies;

/// A body downloader that does nothing. Useful to build unwind-only pipelines.
pub mod noop;

/// A downloader implementation that spawns a downloader to a task
pub mod task;

mod queue;
mod request;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
