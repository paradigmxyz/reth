//! Backend-independent errors from ordered trie access.

use crate::db::DatabaseError;
use alloc::sync::Arc;
use core::{error::Error, fmt};

/// A cursor operation failed. The original backend error is available through its source.
#[derive(Debug, Clone)]
pub struct TrieCursorError(Arc<dyn Error + Send + Sync>);

impl TrieCursorError {
    /// Wraps the cause of a failed cursor operation.
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Arc::new(error))
    }
}

impl From<DatabaseError> for TrieCursorError {
    fn from(error: DatabaseError) -> Self {
        Self::new(error)
    }
}

impl fmt::Display for TrieCursorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "trie cursor operation failed: {}", self.0)
    }
}

impl Error for TrieCursorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_backend_error_source() {
        let error = TrieCursorError::from(DatabaseError::Other("backend failed".into()));
        let cause = error.source().unwrap();
        assert!(cause.downcast_ref::<DatabaseError>().is_some());
        assert_eq!(cause.to_string(), "backend failed");
    }
}
