//! Test case definitions

use crate::result::{CaseResult, Error};
use rayon::prelude::*;
use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};

/// A single test case, capable of loading a JSON description of itself and running it.
///
/// See <https://ethereum-tests.readthedocs.io/> for test specs.
pub trait Case: Debug + Send + Sync + Sized + 'static {
    /// A description of the test.
    fn description(&self) -> String {
        "no description".to_string()
    }

    /// Load the test from the given file path.
    ///
    /// The file can be assumed to be a valid EF test case as described on <https://ethereum-tests.readthedocs.io/>.
    fn load(path: &Path) -> Result<Self, Error>;

    /// Run the test.
    fn run(self) -> Result<(), Error>;
}

/// A container for multiple test cases.
#[derive(Debug)]
pub struct Cases<T> {
    /// The contained test cases and the path to each test.
    pub test_cases: Vec<(PathBuf, T)>,
}

/// Stack size of the threads that run test cases.
///
/// Deeply recursive cases such as `Create2Recursive` need more than rayon's default 2 MiB stack.
/// This matches the worker stack size of evm2's own EEST runner.
const CASE_THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;

impl<T: Case> Cases<T> {
    /// Run the contained test cases.
    pub fn run(self) -> Vec<CaseResult> {
        rayon::ThreadPoolBuilder::new()
            .stack_size(CASE_THREAD_STACK_SIZE)
            .build()
            .expect("failed to build the test case thread pool")
            .install(|| {
                self.test_cases
                    .into_par_iter()
                    .map(|(path, case)| CaseResult::new(&path, case.description(), case.run()))
                    .collect()
            })
    }
}
