/// A naive concurrent downloader.
#[allow(clippy::module_inception)]
pub mod bodies;

/// A downloader implementation that spawns a downloader to a task
pub mod task;

mod queue;
mod request;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
