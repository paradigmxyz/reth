//! Test case definitions

use crate::result::{CaseAsyncResult, CaseResult, Error};
use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};

/// A single test case, capable of loading a JSON description of itself and running it.
///
/// See <https://ethereum-tests.readthedocs.io/> for test specs.
pub trait Case: Debug + Sync + Sized {
    /// A description of the test.
    fn description(&self) -> String {
        "no description".to_string()
    }

    /// Load the test from the given file path.
    ///
    /// The file can be assumed to be a valid EF test case as described on <https://ethereum-tests.readthedocs.io/>.
    fn load(path: &Path) -> Result<Self, Error>;

    /// Run the test.
    fn run(&self) -> Result<(), Error>;
}

/// A single test case, capable of loading a JSON description of itself and running it.
///
/// See <https://ethereum-tests.readthedocs.io/> for test specs.
pub trait CaseAsync: Debug + Sync + Sized {
    /// A description of the test.
    fn description(&self) -> String {
        "no description".to_string()
    }

    /// Load the test from the given file path.
    ///
    /// The file can be assumed to be a valid EF test case as described on <https://ethereum-tests.readthedocs.io/>.
    fn load(path: &Path) -> Result<Self, Error>;

    /// Run the test.
    async fn run(&self) -> Result<(), Error>;
}

/// A container for multiple test cases.
#[derive(Debug)]
pub struct Cases<T> {
    /// The contained test cases and the path to each test.
    pub test_cases: Vec<(PathBuf, T)>,
}

impl<T: Case> Cases<T> {
    /// Run the contained test cases.
    pub fn run(&self) -> Vec<CaseResult> {
        self.test_cases.iter().map(|(path, case)| CaseResult::new(path, case, case.run())).collect()
    }
}

/// A container for multiple test cases.
#[derive(Debug)]
pub struct CasesAsync<T> {
    /// The contained test cases and the path to each test.
    pub test_cases: Vec<(PathBuf, T)>,
}

impl<T: CaseAsync> CasesAsync<T> {
    /// Run the contained test cases.
    pub async fn run(&self) -> Vec<CaseAsyncResult> {
        let mut results = Vec::new();
        for (path, case) in self.test_cases.iter() {
            let res = case.run().await;
            results.push(CaseAsyncResult::new(path, case, res));
        }
        results
        //self.test_cases.iter().map(|(path, case)| CaseAsyncResult::new(path, case,
        // case.run().await)).collect()
    }
}
