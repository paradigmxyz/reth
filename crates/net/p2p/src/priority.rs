/// `BlockHeader` and `BodyHeader` `DownloadRequest` priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    /// Queued from the back for download requests.
    #[default]
    Normal,

    /// Queued from the front for download requests.
    High,
}

impl Priority {
    /// Returns `true` if this is [`Priority::High`]
    pub const fn is_high(&self) -> bool {
        matches!(self, Self::High)
    }

    /// Returns `true` if this is [`Priority::Normal`]
    pub const fn is_normal(&self) -> bool {
        matches!(self, Self::Normal)
    }
}
