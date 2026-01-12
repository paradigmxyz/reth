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

    /// The path to the test suite directory.
    fn suite_path(&self) -> &Path;

    /// Run all test cases in the suite.
    fn run(&self) {
        let suite_path = self.suite_path();
        for entry in WalkDir::new(suite_path).min_depth(1).max_depth(1) {
            let entry = entry.expect("Failed to read directory");
            if entry.file_type().is_dir() {
                self.run_only(entry.file_name().to_string_lossy().as_ref());
            }
        }
    }

    /// Load and run each contained test case for the provided sub-folder.
    ///
    /// # Note
    ///
    /// This recursively finds every test description in the resulting path.
    fn run_only(&self, name: &str) {
        // Build the path to the test suite directory
        let suite_path = self.suite_path().join(name);

        // Verify that the path exists
        assert!(suite_path.exists(), "Test suite path does not exist: {suite_path:?}");

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
        assert_tests_pass(name, &suite_path, &results);
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
