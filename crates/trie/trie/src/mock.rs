/// The type of key visit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyVisitType<T> {
    /// Seeked exact key.
    SeekExact(T),
    /// Seeked non-exact key, returning the next key if no exact match found.
    SeekNonExact(T),
    /// Next key.
    Next(T),
}
