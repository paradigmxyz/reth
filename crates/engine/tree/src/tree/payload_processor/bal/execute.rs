//! Parked BAL execution entrypoint.

use super::BalExecutionError;

/// Placeholder BAL execution entrypoint.
///
/// Amsterdam BAL execution must be rebuilt on the active EVM before this module is compiled again.
pub fn execute_block() -> Result<(), BalExecutionError> {
    Err(BalExecutionError::Unsupported)
}
