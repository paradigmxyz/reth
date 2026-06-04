//! Errors for the parked BAL execution path.

/// Errors surfaced by the BAL execution placeholder.
#[derive(Debug, thiserror::Error)]
pub enum BalExecutionError {
    /// BAL execution is not available in the active evm2 pre-Amsterdam path.
    #[error("BAL execution is parked until Amsterdam support is ported to evm2")]
    Unsupported,
}
