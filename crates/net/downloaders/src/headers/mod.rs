/// A Linear downloader implementation.
pub mod reverse_headers;

/// A downloader implementation that spawns a downloader to a task
pub mod task;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
