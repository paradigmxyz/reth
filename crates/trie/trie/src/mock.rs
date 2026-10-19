/// The key visit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyVisit<T> {
    /// The type of key visit.
    pub visit_type: KeyVisitType<T>,
    /// The key that was visited, if found.
    pub visited_key: Option<T>,
}

/// The type of key visit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyVisitType<T> {
    /// Seeked exact key.
    SeekExact(T),
    /// Seeked non-exact key, returning the next key if no exact match found.
    SeekNonExact(T),
    /// Next key.
    Next,
}
