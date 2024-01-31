//! Abstractions for groups of tests.

use crate::{
    case::{Case, Cases},
    result::assert_tests_pass,
};
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

/// A collection of tests.
pub trait Suite {
    /// The type of test cases in this suite.
    type Case: Case;

    /// The name of the test suite used to locate the individual test cases.
    ///
    /// # Example
    ///
    /// - `GeneralStateTests`
    /// - `BlockchainTests/InvalidBlocks`
    /// - `BlockchainTests/TransitionTests`
    fn suite_name(&self) -> String;

    /// Load an run each contained test case.
    ///
    /// # Note
    ///
    /// This recursively finds every test description in the resulting path.
    fn run(&self) {
        // Build the path to the test suite directory
        let suite_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("ethereum-tests")
            .join(self.suite_name());

        // Verify that the path exists
        assert!(suite_path.exists(), "Test suite path does not exist: {:?}", suite_path);

        // Find all files with the ".json" extension in the test suite directory
        let test_cases = find_all_files_with_extension(&suite_path, ".json")
            .into_iter()
            .map(|test_case_path| {
                let case = Self::Case::load(&test_case_path).expect("test case should load");
                (test_case_path, case)
            })
            .collect();

        // Run the test cases and collect the results
        let results = Cases { test_cases }.run();

        // Assert that all tests in the suite pass
        assert_tests_pass(&self.suite_name(), &suite_path, &results);
    }
}

/// Recursively find all files with a given extension.
fn find_all_files_with_extension(path: &Path, extension: &str) -> Vec<PathBuf> {
    WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(extension))
        .map(DirEntry::into_path)
        .collect()
}
