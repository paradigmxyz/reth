//! Types used by tracing backends

use serde::{Deserialize, Serialize};

/// The result of a single transaction trace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[allow(missing_docs)]
pub enum TraceResult<Ok, Err> {
    /// Untagged success variant
    Success { result: Ok },
    /// Untagged error variant
    Error { error: Err },
}
