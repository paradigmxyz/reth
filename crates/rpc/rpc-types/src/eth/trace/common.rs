//! Types used by tracing backends

use alloy_primitives::TxHash;
use serde::{Deserialize, Serialize};

/// The result of a single transaction trace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[allow(missing_docs)]
pub enum TraceResult<Ok, Err> {
    /// Untagged success variant
    Success { result: Ok, tx_hash: TxHash },
    /// Untagged error variant
    Error { error: Err },
}
