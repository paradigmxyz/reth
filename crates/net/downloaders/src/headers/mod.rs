/// A Linear downloader implementation.
pub mod reverse_headers;

/// A header downloader that does nothing. Useful to build unwind-only pipelines.
pub mod noop;

/// A downloader implementation that spawns a downloader to a task
pub mod task;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
