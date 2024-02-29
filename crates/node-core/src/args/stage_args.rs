//! Shared arguments related to stages
use derive_more::Display;

/// Represents a specific stage within the data pipeline.
///
/// Different stages within the pipeline have dedicated functionalities and operations.
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, clap::ValueEnum, Display)]
pub enum StageEnum {
    /// The headers stage within the pipeline.
    ///
    /// This stage handles operations related to block headers.
    Headers,
    /// The bodies stage within the pipeline.
    ///
    /// This stage deals with block bodies and their associated data.
    Bodies,
    /// The senders stage within the pipeline.
    ///
    /// Responsible for sender-related processes and data recovery.
    Senders,
    /// The execution stage within the pipeline.
    ///
    /// Handles the execution of transactions and contracts.
    Execution,
    /// The account hashing stage within the pipeline.
    ///
    /// Manages operations related to hashing account data.
    AccountHashing,
    /// The storage hashing stage within the pipeline.
    ///
    /// Manages operations related to hashing storage data.
    StorageHashing,
    /// The hashing stage within the pipeline.
    ///
    /// Covers general data hashing operations.
    Hashing,
    /// The Merkle stage within the pipeline.
    ///
    /// Handles Merkle tree-related computations and data processing.
    Merkle,
    /// The transaction lookup stage within the pipeline.
    ///
    /// Deals with the retrieval and processing of transactions.
    TxLookup,
    /// The account history stage within the pipeline.
    ///
    /// Manages historical data related to accounts.
    AccountHistory,
    /// The storage history stage within the pipeline.
    ///
    /// Manages historical data related to storage.
    StorageHistory,
}
