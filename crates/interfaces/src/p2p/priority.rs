/// BlockHeader and BodyHeader DownloadRequest priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    /// Queued from the back for download requests.
    #[default]
    Normal,

    /// Queued from the front for download requests.
    High,
}
