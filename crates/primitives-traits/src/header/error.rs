/// Errors that can occur during header sanity checks.
#[derive(Debug, PartialEq, Eq)]
pub enum HeaderError {
    /// Represents an error when the block difficulty is too large.
    LargeDifficulty,
    /// Represents an error when the block extradata is too large.
    LargeExtraData,
}
