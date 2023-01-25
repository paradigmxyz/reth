/// A Linear downloader implementation.
pub mod linear;

/// Metrics for linear downloader
pub mod metrics;

/// A downloader implementation that spawns a downloader to a task
pub mod task;

#[cfg(test)]
mod test_utils;
