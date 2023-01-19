/// BlockHeader and BodyHeader DownloadRequest priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    #[default]
    Normal,
    High,
}
