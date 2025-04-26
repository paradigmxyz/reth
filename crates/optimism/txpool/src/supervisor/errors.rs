use alloy_json_rpc::RpcError;
use core::error;
use std::fmt;

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

            // Try to map to specific error variants
            #[allow(clippy::manual_range_contains)]
            if code == -320501 {
                // UNKNOWN_CHAIN error
                #[allow(clippy::manual_range_contains)]
                let chain_id = extract_chain_id(message).unwrap_or(0);
                return Self::InvalidEntry(InvalidInboxEntry::UnknownChain(chain_id));
            } else if code >= -320999 &&
                code <= -320900 &&
                (message.contains("safety") || message.contains("level"))
            {
                // Try to parse the safety level error
                if let (Some(got), Some(expected)) = (
                    extract_safety_level(message, "got"),
                    extract_safety_level(message, "expected"),
                ) {
                    return Self::InvalidEntry(InvalidInboxEntry::MinimumSafety { got, expected });
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
        let remainder = &message[idx + 8..];
        if let Some(colon_idx) = remainder.find(':') {
            let num_part = &remainder[colon_idx + 1..].trim();
            return num_part.parse::<u64>().ok();
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
    for pattern in &[format!("{}: ", key_param), format!("{} ", key_param)] {
        if let Some(pos) = message.find(pattern) {
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
    None
}
