//! Test results and errors

use alloy_primitives::B256;
use reth_db::DatabaseError;
use reth_evm::{BlockExecutionError, InternalBlockExecutionError};
use reth_provider::ProviderError;
use serde::Serialize;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Test errors
///
/// # Note
///
/// `Error::Skipped` should not be treated as a test failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The test was skipped
    #[error("test was skipped")]
    Skipped,
    /// Block processing failed
    /// Note: This includes but is not limited to execution.
    /// For example, the header number could be incorrect.
    #[error("block {block_number} failed to process: {err}")]
    BlockProcessingFailed {
        /// The block number for the block that failed
        block_number: u64,
        /// The specific error
        #[source]
        err: Box<dyn std::error::Error + Send + Sync>,
    },
    /// An IO error occurred
    #[error("an error occurred interacting with the file system at {path}: {error}")]
    Io {
        /// The path to the file or directory
        path: PathBuf,
        /// The specific error
        #[source]
        error: std::io::Error,
    },
    /// A deserialization error occurred
    #[error("an error occurred deserializing the test at {path}: {error}")]
    CouldNotDeserialize {
        /// The path to the file we wanted to deserialize
        path: PathBuf,
        /// The specific error
        #[source]
        error: serde_json::Error,
    },
    /// A fixture directory could not be traversed.
    #[error(transparent)]
    WalkDir(#[from] walkdir::Error),
    /// An engine operation failed.
    #[error("{context}: {error}")]
    Engine {
        /// The operation that failed.
        context: String,
        /// The original error.
        #[source]
        error: Box<dyn std::error::Error + Send + Sync>,
    },
    /// A database error occurred.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// An internal execution failure, which must never count as an invalid block.
    #[error(transparent)]
    Execution(#[from] InternalBlockExecutionError),
    /// A test assertion failed.
    #[error("test failed: {0}")]
    Assertion(String),
    /// An error internally in reth occurred.
    #[error("test failed: {0}")]
    Provider(#[from] ProviderError),
    /// An error occurred while decoding RLP.
    #[error("an error occurred deserializing RLP: {0}")]
    RlpDecodeError(#[from] alloy_rlp::Error),
    /// A consensus error occurred.
    #[error("an error occurred during consensus checks: {0}")]
    ConsensusError(#[from] reth_consensus::ConsensusError),
}

impl Error {
    /// Preserves the source of an engine setup, request, or shutdown failure.
    pub(crate) fn engine(
        context: impl Into<String>,
        error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::Engine { context: context.into(), error: error.into() }
    }

    /// Separates a consensus-invalid execution from a failure of the executor or its provider.
    pub(crate) fn block_execution_failed(block_number: u64, error: BlockExecutionError) -> Self {
        match error {
            BlockExecutionError::Validation(error) => Self::block_failed(block_number, error),
            BlockExecutionError::Internal(error) => Self::Execution(error),
        }
    }

    /// Create a new [`Error::BlockProcessingFailed`] error.
    pub fn block_failed(
        block_number: u64,
        err: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::BlockProcessingFailed { block_number, err: Box::new(err) }
    }
}

/// The result of running a test.
#[derive(Debug)]
pub struct CaseResult {
    /// A description of the test.
    pub desc: String,
    /// The full path to the test.
    pub path: PathBuf,
    /// The result of the test.
    pub result: Result<(), Error>,
}

impl CaseResult {
    /// Create a new test result.
    pub fn new(path: &Path, desc: String, result: Result<(), Error>) -> Self {
        Self { desc, path: path.into(), result }
    }
}

/// The result of one fixture, as printed by the runner's JSON output.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureResult {
    /// The fixture name.
    pub name: String,
    /// Whether the fixture passed.
    pub pass: bool,
    /// The fixture's network.
    pub fork: String,
    /// The hash of the client's head block once the fixture ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_block_hash: Option<B256>,
    /// The status of the last `engine_newPayload` call, for engine tests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_payload_status: Option<String>,
    /// Why the fixture failed, empty if it passed.
    pub error: String,
    /// Every block or payload the client rejected, in fixture order, whether or not the fixture
    /// expected it.
    pub rejections: Vec<Rejection>,
}

/// A block or payload the client rejected, with the client's own error, so that a consumer can
/// check the reason against the fixture's expected exception.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rejection {
    /// The block's position in the fixture's `blocks`, or the payload's in `engineNewPayloads`.
    pub index: usize,
    /// The rejected block's hash, when the client computed one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<B256>,
    /// The client's error, verbatim.
    pub error: String,
}

impl FixtureResult {
    /// Creates the result of a fixture that ran.
    pub fn new(name: String, fork: String, result: Result<(), Error>) -> Self {
        let pass = result.is_ok();
        let error = result.err().map(|err| err.to_string()).unwrap_or_default();
        Self {
            name,
            pass,
            fork,
            last_block_hash: None,
            last_payload_status: None,
            error,
            rejections: Vec::new(),
        }
    }

    /// Sets the hash of the client's head block.
    pub const fn with_last_block_hash(mut self, hash: B256) -> Self {
        self.last_block_hash = Some(hash);
        self
    }

    /// Sets the blocks or payloads the client rejected.
    pub fn with_rejections(mut self, rejections: Vec<Rejection>) -> Self {
        self.rejections = rejections;
        self
    }
}

/// Assert that all the given tests passed and print the results to stdout.
pub(crate) fn assert_tests_pass(suite_name: &str, path: &Path, results: &[CaseResult]) {
    let (passed, failed, skipped) = categorize_results(results);

    print_results(suite_name, path, &passed, &failed, &skipped);

    assert!(failed.is_empty(), "Some tests failed (see above)");
}

/// Categorize test results into `(passed, failed, skipped)`.
pub(crate) fn categorize_results(
    results: &[CaseResult],
) -> (Vec<&CaseResult>, Vec<&CaseResult>, Vec<&CaseResult>) {
    let mut passed = Vec::new();
    let mut failed = Vec::new();
    let mut skipped = Vec::new();

    for case in results {
        match case.result.as_ref().err() {
            Some(Error::Skipped) => skipped.push(case),
            Some(_) => failed.push(case),
            None => passed.push(case),
        }
    }

    (passed, failed, skipped)
}

/// Display the given test results to stdout.
pub(crate) fn print_results(
    suite_name: &str,
    path: &Path,
    passed: &[&CaseResult],
    failed: &[&CaseResult],
    skipped: &[&CaseResult],
) {
    println!("Suite: {suite_name} (at {})", path.display());
    println!(
        "Ran {} tests ({} passed, {} failed, {} skipped)",
        passed.len() + failed.len() + skipped.len(),
        passed.len(),
        failed.len(),
        skipped.len()
    );

    for case in skipped {
        println!("[S] Case {} skipped", case.path.display());
    }

    for case in failed {
        let error = case.result.as_ref().unwrap_err();
        println!("[!] Case {} failed (description: {}): {}", case.path.display(), case.desc, error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_evm::BlockValidationError;

    #[test]
    fn only_validation_errors_can_count_as_expected_invalid_blocks() {
        let invalid =
            Error::block_execution_failed(1, BlockValidationError::msg("invalid block").into());
        assert!(matches!(invalid, Error::BlockProcessingFailed { block_number: 1, .. }));
        assert_eq!(invalid.to_string(), "block 1 failed to process: invalid block");

        let source = ProviderError::UnsupportedProvider;
        let expected = source.to_string();
        let failure = Error::block_execution_failed(1, BlockExecutionError::other(source));
        assert!(matches!(failure, Error::Execution(_)));
        assert_eq!(failure.to_string(), expected);
        let provider_error = Error::from(ProviderError::UnsupportedProvider);
        assert!(matches!(provider_error, Error::Provider(_)));
        assert_eq!(provider_error.to_string(), format!("test failed: {expected}"));
    }
}
