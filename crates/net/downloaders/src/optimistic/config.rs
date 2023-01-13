/// The downloader configuration.
#[derive(Debug)]
pub struct DownloaderConfig {
    /// Number of downloads to process concurrently.
    pub concurrency: usize,
    /// The number of max queued downloads.
    pub queue_size: usize,
    /// Download buffer size.
    pub buffer_size: usize,
    /// Download batch related configuration.
    pub batch: BatchDownloadConfig,
}

impl Default for DownloaderConfig {
    fn default() -> Self {
        Self {
            concurrency: 20,
            queue_size: 100,
            buffer_size: 1000,
            batch: BatchDownloadConfig::default(),
        }
    }
}

/// The configuration for batch download related things.
#[derive(Debug)]
pub struct BatchDownloadConfig {
    /// Headers batch size.
    pub headers_batch_size: u64,
}

impl Default for BatchDownloadConfig {
    fn default() -> Self {
        Self { headers_batch_size: 100 }
    }
}
