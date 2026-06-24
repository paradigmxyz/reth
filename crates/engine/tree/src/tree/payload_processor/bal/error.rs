//! Errors for the parked BAL execution path.

/// Errors surfaced by the BAL execution placeholder.
#[derive(Debug, thiserror::Error)]
pub enum BalExecutionError {
    /// BAL execution is not available in the active pre-Amsterdam execution path.
    #[error("BAL execution is parked until Amsterdam support is ported to the active EVM")]
    Unsupported,
}
