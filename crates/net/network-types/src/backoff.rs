/// Describes the type of backoff should be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackoffKind {
    /// Use the lowest configured backoff duration.
    ///
    /// This applies to connection problems where there is a chance that they will be resolved
    /// after the short duration.
    Low,
    /// Use a slightly higher duration to put a peer in timeout
    ///
    /// This applies to more severe connection problems where there is a lower chance that they
    /// will be resolved.
    Medium,
    /// Use the max configured backoff duration.
    ///
    /// This is intended for spammers, or bad peers in general.
    High,
}

// === impl BackoffKind ===

impl BackoffKind {
    /// Returns true if the backoff is considered severe.
    pub const fn is_severe(&self) -> bool {
        matches!(self, Self::Medium | Self::High)
    }
}
