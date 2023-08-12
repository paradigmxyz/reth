//! Test results and errors

use crate::Case;
use reth_db::DatabaseError;
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
    #[error("Test was skipped")]
    Skipped,
    /// An IO error occurred
    #[error("An error occurred interacting with the file system at {path}: {error}")]
    Io {
        /// The path to the file or directory
        path: PathBuf,
        /// The specific error
        #[source]
        error: std::io::Error,
    },
    /// A deserialization error occurred
    #[error("An error occurred deserializing the test at {path}: {error}")]
    CouldNotDeserialize {
        /// The path to the file we wanted to deserialize
        path: PathBuf,
        /// The specific error
        #[source]
        error: serde_json::Error,
    },
    /// A database error occurred.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// A test assertion failed.
    #[error("Test failed: {0}")]
    Assertion(String),
    /// An error internally in reth occurred.
    #[error("Test failed: {0}")]
    RethError(#[from] reth_interfaces::Error),
    /// An error occurred while decoding RLP.
    #[error("An error occurred deserializing RLP")]
    RlpDecodeError(#[from] reth_rlp::DecodeError),
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
    pub fn new(path: &Path, case: &impl Case, result: Result<(), Error>) -> Self {
        CaseResult { desc: case.description(), path: path.into(), result }
    }
}

/// Assert that all the given tests passed and print the results to stdout.
pub(crate) fn assert_tests_pass(suite_name: &str, path: &Path, results: &[CaseResult]) {
    let (passed, failed, skipped) = categorize_results(results);

    print_results(suite_name, path, &passed, &failed, &skipped);

    if !failed.is_empty() {
        panic!("Some tests failed (see above)");
    }
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
