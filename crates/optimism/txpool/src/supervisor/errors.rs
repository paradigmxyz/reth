use alloy_json_rpc::RpcError;
use core::error;
use derive_more;
use std::fmt;

/// Supervisor protocol error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::TryFrom)]
#[repr(i64)]
#[try_from(repr)]
#[allow(unreachable_pub)]
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

    // -3210XX ABORTED errors
    /// Happens in iterator to indicate iteration has to stop.
    /// This error might only be used internally and not sent over the network.
    #[allow(non_camel_case_types)]
    ITER_STOP = -321000,

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
    /// Message does not meet minimum safety level
    #[error("message does not meet min safety level, got: {got}, expected: {expected}")]
    MinimumSafety {
        /// Actual level of the message
        got: SafetyLevel,
        /// Minimum acceptable level that was passed to supervisor
        expected: SafetyLevel,
    },
    /// Invalid chain
    #[error("unsupported chain id: {0}")]
    UnknownChain(u64),
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

/// Safety level of an inbox entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SafetyLevel {
    /// Unsafe level, no safety guarantees
    Unsafe = 0,
    /// Safe level, provides safety guarantees
    Safe = 1,
    /// Finalized level, provides finality guarantees
    Finalized = 2,
}

impl fmt::Display for SafetyLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[allow(clippy::use_self)]
            SafetyLevel::Unsafe => write!(f, "unsafe"),
            #[allow(clippy::use_self)]
            SafetyLevel::Safe => write!(f, "safe"),
            #[allow(clippy::use_self)]
            SafetyLevel::Finalized => write!(f, "finalized"),
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
            // Extract code and message from the error payload
            let code = error_payload.code;
            let message = error_payload.message.as_ref();

            // condition to map to specific error variants based on error code
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
                        let chain_id = extract_chain_id(message).unwrap_or(0);
                        return Self::InvalidEntry(InvalidInboxEntry::UnknownChain(chain_id));
                    }

                    // ALREADY_EXISTS errors
                    SupervisorErrorCode::CONFLICTING_DATA => {
                        return Self::InvalidEntry(InvalidInboxEntry::ConflictingData);
                    }
                    SupervisorErrorCode::INEFFECTIVE_DATA => {
                        return Self::InvalidEntry(InvalidInboxEntry::IneffectiveData);
                    }

                    // FAILED_PRECONDITION errors - handle safety level errors
                    SupervisorErrorCode::OUT_OF_ORDER => {
                        if message.contains("safety") || message.contains("level") {
                            if let (Some(got), Some(expected)) = (
                                extract_safety_level(message, "got"),
                                extract_safety_level(message, "expected"),
                            ) {
                                return Self::InvalidEntry(InvalidInboxEntry::MinimumSafety {
                                    got,
                                    expected,
                                });
                            }
                        }

                        // Default to generic out of order error
                        return Self::InvalidEntry(InvalidInboxEntry::OutOfOrder);
                    }
                    SupervisorErrorCode::AWAITING_REPLACEMENT_BLOCK => {
                        return Self::InvalidEntry(InvalidInboxEntry::AwaitingReplacement);
                    }

                    // ABORTED errors
                    SupervisorErrorCode::ITER_STOP => {
                        return Self::other(err);
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

/// Extracts a chain ID from error messages like "unsupported chain id: 1234"
fn extract_chain_id(message: &str) -> Option<u64> {
    // Common pattern for chain ID errors
    if let Some(idx) = message.find("chain id") {
        if idx + 8 < message.len() {
            // Ensure there are characters after "chain id"
            let remainder = &message[idx + 8..];
            if let Some(colon_idx) = remainder.find(':') {
                #[allow(clippy::int_plus_one)]
                if colon_idx + 1 <= remainder.len() {
                    // Ensure there are characters after the colon
                    let num_part = &remainder[colon_idx + 1..].trim();
                    return num_part.parse::<u64>().ok();
                }
            }
        }
    }

    // Fallback: look for any number in the message
    #[allow(clippy::is_digit_ascii_radix)]
    message
        .split_whitespace()
        .filter_map(|word| word.trim_matches(|c: char| !c.is_digit(10)).parse::<u64>().ok())
        .next()
}

/// Extracts a safety level value from error message patterns
fn extract_safety_level(message: &str, key_param: &str) -> Option<SafetyLevel> {
    // Look for patterns like "got: 0" or "expected: 1" in the message
    let patterns = [format!("{key_param}: "), format!("{key_param} ")];
    for pattern in &patterns {
        if let Some(pos) = message.find(pattern.as_str()) {
            if pos + pattern.len() < message.len() {
                // Ensure there are characters after the pattern
                let value_part = &message[pos + pattern.len()..];
                let value = value_part.chars().next().and_then(|c| c.to_digit(10));

                return match value {
                    Some(0) => Some(SafetyLevel::Unsafe),
                    Some(1) => Some(SafetyLevel::Safe),
                    Some(2) => Some(SafetyLevel::Finalized),
                    _ => None,
                };
            }
        }
    }
    None
}
