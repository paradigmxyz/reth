use alloy_json_rpc::RpcError;
use core::error;
use derive_more;

/// Supervisor protocol error codes.
///
/// Specs: <https://specs.optimism.io/interop/supervisor.html#protocol-specific-error-codes>
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::TryFrom)]
#[repr(i64)]
#[try_from(repr)]
#[allow(unfulfilled_lint_expectations)]
#[expect(unreachable_pub)]
pub enum SupervisorErrorCode {
    // -3204XX DEADLINE_EXCEEDED errors
    /// Happens when a chain database is not initialized yet.
    #[allow(non_camel_case_types)]
    UNINITIALIZED_CHAIN_DATABASE = -320400,

    // -3205XX NOT_FOUND errors
    /// Happens when we try to retrieve data that is not available (pruned).
    /// It may also happen if we erroneously skip data, that was not considered a conflict, if the
    /// DB is corrupted.
    #[allow(non_camel_case_types)]
    SKIPPED_DATA = -320500,

    /// Happens when a chain is unknown, not in the dependency set.
    #[allow(non_camel_case_types)]
    UNKNOWN_CHAIN = -320501,

    // -3206XX ALREADY_EXISTS errors
    /// Happens when we know for sure that there is different canonical data.
    #[allow(non_camel_case_types)]
    CONFLICTING_DATA = -320600,

    /// Happens when data is accepted as compatible, but did not change anything.
    /// This happens when a node is deriving an L2 block we already know of being
    /// derived from the given source,
    /// but without path to skip forward to newer source blocks without doing the known
    /// derivation work first.
    #[allow(non_camel_case_types)]
    INEFFECTIVE_DATA = -320601,

    // -3209XX FAILED_PRECONDITION errors
    /// Happens when you try to add data to the DB, but it does not actually fit onto
    /// the latest data.
    /// (by being too old or new).
    #[allow(non_camel_case_types)]
    OUT_OF_ORDER = -320900,

    /// Happens when we know for sure that a replacement block is needed before progress
    /// can be made.
    #[allow(non_camel_case_types)]
    AWAITING_REPLACEMENT_BLOCK = -320901,

    // -3211XX OUT_OF_RANGE errors
    /// Happens when data is accessed, but access is not allowed, because of a limited
    /// scope.
    /// E.g. when limiting scope to L2 blocks derived from a specific subset of the L1
    /// chain.
    #[allow(non_camel_case_types)]
    OUT_OF_SCOPE = -321100,

    // -3212XX UNIMPLEMENTED errors
    /// Happens when you try to get the previous block of the first block.
    /// E.g. when trying to determine the previous source block for the first L1 block
    /// in the database.
    #[allow(non_camel_case_types)]
    CANNOT_GET_PARENT_OF_FIRST_BLOCK_IN_DB = -321200,

    // -3214XX UNAVAILABLE errors
    /// Happens when data is just not yet available.
    #[allow(non_camel_case_types)]
    FUTURE_DATA = -321401,

    // -3215XX DATA_LOSS errors
    /// Happens when we search the DB, know the data may be there, but is not (e.g.
    /// different revision).
    #[allow(non_camel_case_types)]
    MISSED_DATA = -321500,

    /// Happens when the underlying DB has some I/O issue.
    #[allow(non_camel_case_types)]
    DATA_CORRUPTION = -321501,
}

/// Failures occurring during validation of inbox entries.
#[derive(thiserror::Error, Debug)]
pub enum InteropTxValidatorError {
    /// Inbox entry validation against the Supervisor took longer than allowed.
    #[error("inbox entry validation timed out, timeout: {0} secs")]
    Timeout(u64),

    /// Message does not satisfy validation requirements
    #[error(transparent)]
    InvalidEntry(#[from] InvalidInboxEntry),

    /// Catch-all variant.
    #[error("supervisor server error: {0}")]
    Other(Box<dyn error::Error + Send + Sync>),
}

/// Invalid inbox entry
#[derive(thiserror::Error, Debug)]
pub enum InvalidInboxEntry {
    /// Invalid chain
    #[error("unsupported chain id")]
    UnknownChain,
    /// Data was skipped and is not available
    #[error("data was skipped or pruned and is not available")]
    SkippedData,
    /// Database for the chain is not initialized
    #[error("chain database is not initialized")]
    UninitializedChainDatabase,
    /// Conflicting data exists
    #[error("conflicting data exists in the database")]
    ConflictingData,
    /// Data is ineffective (already known)
    #[error("data is already known and didn't change anything")]
    IneffectiveData,
    /// Data is out of order
    #[error("data is out of order (too old or new)")]
    OutOfOrder,
    /// Waiting for replacement block
    #[error("waiting for replacement block before progress can be made")]
    AwaitingReplacement,
    /// Data is outside allowed scope
    #[error("data access not allowed due to limited scope")]
    OutOfScope,
    /// Cannot get parent of first block
    #[error("cannot get parent of first block in database")]
    NoParentForFirstBlock,
    /// Data is from the future
    #[error("data is not yet available (from the future)")]
    FutureData,
    /// Data was missed
    #[error("data may exist but was not found (possibly different revision)")]
    MissedData,
    /// Data corruption
    #[error("underlying database has I/O issues or is corrupted")]
    DataCorruption,
}

impl InvalidInboxEntry {
    /// Returns the corresponding Supervisor error code for this invalid entry type.
    pub const fn error_code(&self) -> SupervisorErrorCode {
        match self {
            Self::UnknownChain => SupervisorErrorCode::UNKNOWN_CHAIN,
            Self::SkippedData => SupervisorErrorCode::SKIPPED_DATA,
            Self::UninitializedChainDatabase => SupervisorErrorCode::UNINITIALIZED_CHAIN_DATABASE,
            Self::ConflictingData => SupervisorErrorCode::CONFLICTING_DATA,
            Self::IneffectiveData => SupervisorErrorCode::INEFFECTIVE_DATA,
            Self::OutOfOrder => SupervisorErrorCode::OUT_OF_ORDER,
            Self::AwaitingReplacement => SupervisorErrorCode::AWAITING_REPLACEMENT_BLOCK,
            Self::OutOfScope => SupervisorErrorCode::OUT_OF_SCOPE,
            Self::NoParentForFirstBlock => {
                SupervisorErrorCode::CANNOT_GET_PARENT_OF_FIRST_BLOCK_IN_DB
            }
            Self::FutureData => SupervisorErrorCode::FUTURE_DATA,
            Self::MissedData => SupervisorErrorCode::MISSED_DATA,
            Self::DataCorruption => SupervisorErrorCode::DATA_CORRUPTION,
        }
    }
}

impl InteropTxValidatorError {
    /// Returns a new instance of [`Other`](Self::Other) error variant.
    pub fn other<E>(err: E) -> Self
    where
        E: error::Error + Send + Sync + 'static,
    {
        Self::Other(Box::new(err))
    }

    /// This function will parse the error code and message to determine if it matches
    /// one of the known Supervisor error patterns, and return the corresponding specific
    /// error variant. Otherwise, it returns a generic error.
    pub fn from_json_rpc<E>(err: RpcError<E>) -> Self
    where
        E: error::Error + Send + Sync + 'static,
    {
        // Try to extract error details from the RPC error
        if let Some(error_payload) = err.as_error_resp() {
            let code = error_payload.code;

            // Map to specific error variants based on error code
            if let Ok(supervisor_code) = SupervisorErrorCode::try_from(code) {
                match supervisor_code {
                    // DEADLINE_EXCEEDED errors
                    SupervisorErrorCode::UNINITIALIZED_CHAIN_DATABASE => {
                        return Self::InvalidEntry(InvalidInboxEntry::UninitializedChainDatabase);
                    }

                    // NOT_FOUND errors
                    SupervisorErrorCode::SKIPPED_DATA => {
                        return Self::InvalidEntry(InvalidInboxEntry::SkippedData);
                    }
                    SupervisorErrorCode::UNKNOWN_CHAIN => {
                        return Self::InvalidEntry(InvalidInboxEntry::UnknownChain);
                    }

                    // ALREADY_EXISTS errors
                    SupervisorErrorCode::CONFLICTING_DATA => {
                        return Self::InvalidEntry(InvalidInboxEntry::ConflictingData);
                    }
                    SupervisorErrorCode::INEFFECTIVE_DATA => {
                        return Self::InvalidEntry(InvalidInboxEntry::IneffectiveData);
                    }

                    // FAILED_PRECONDITION errors
                    SupervisorErrorCode::OUT_OF_ORDER => {
                        return Self::InvalidEntry(InvalidInboxEntry::OutOfOrder);
                    }
                    SupervisorErrorCode::AWAITING_REPLACEMENT_BLOCK => {
                        return Self::InvalidEntry(InvalidInboxEntry::AwaitingReplacement);
                    }

                    // OUT_OF_RANGE errors
                    SupervisorErrorCode::OUT_OF_SCOPE => {
                        return Self::InvalidEntry(InvalidInboxEntry::OutOfScope);
                    }

                    // UNIMPLEMENTED errors
                    SupervisorErrorCode::CANNOT_GET_PARENT_OF_FIRST_BLOCK_IN_DB => {
                        return Self::InvalidEntry(InvalidInboxEntry::NoParentForFirstBlock);
                    }

                    // UNAVAILABLE errors
                    SupervisorErrorCode::FUTURE_DATA => {
                        return Self::InvalidEntry(InvalidInboxEntry::FutureData);
                    }

                    // DATA_LOSS errors
                    SupervisorErrorCode::MISSED_DATA => {
                        return Self::InvalidEntry(InvalidInboxEntry::MissedData);
                    }
                    SupervisorErrorCode::DATA_CORRUPTION => {
                        return Self::InvalidEntry(InvalidInboxEntry::DataCorruption);
                    }
                }
            }
        }

        // Default to generic error
        Self::Other(Box::new(err))
    }
}
