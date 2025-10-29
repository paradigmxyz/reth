//! Storage metadata models.

use reth_codecs::{add_arbitrary_tests, Compact};
use serde::{Deserialize, Serialize};

/// Storage configuration settings for this node.
///
/// These should be set during `init_genesis` or `init_db` depending on whether we want dictate
/// behaviour of new or old nodes respectively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct StorageSettings {
    /// Whether this node always writes receipts to static files.
    ///
    /// If this is set to FALSE AND receipt pruning IS ENABLED, all receipts should be written to DB. Otherwise, they should be written to static files. This ensures that older nodes do not need to migrate their current DB tables to static files. For more, read: <https://github.com/paradigmxyz/reth/issues/18890#issuecomment-3457760097>
    pub receipts_static_files: bool,
}

impl StorageSettings {
    /// Creates a new `StorageSettings` with default values.
    pub const fn new() -> Self {
        Self { receipts_static_files: false }
    }

    /// Sets the `receipts_static_files` flag to true.
    pub const fn with_receipts_on_static_files(mut self) -> Self {
        self.receipts_static_files = true;
        self
    }
}
