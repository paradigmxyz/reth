//! Test case definitions

use crate::result::{CaseResult, Error};
use rayon::{prelude::*, ThreadPool, ThreadPoolBuilder};
use serde::de::DeserializeOwned;
use std::{
    fmt::Debug,
    fs,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

/// Stack size of the worker threads that run test cases.
///
/// The EVM recurses natively per call frame, so a 1024-deep call chain needs more than the 2 MiB
/// default.
const WORKER_STACK_SIZE: usize = 8 * 1024 * 1024;

static POOL: LazyLock<ThreadPool> = LazyLock::new(|| {
    ThreadPoolBuilder::new()
        .stack_size(WORKER_STACK_SIZE)
        .build()
        .expect("failed to build the test case thread pool")
});

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

impl<T: Case> Cases<T> {
    /// Run the contained test cases.
    pub fn run(self) -> Vec<CaseResult> {
        POOL.install(|| {
            self.test_cases
                .into_par_iter()
                .map(|(path, case)| CaseResult::new(&path, case.description(), case.run()))
                .collect()
        })
    }
}

/// Reads the JSON test file at `path`.
pub fn load_json<T: DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let contents =
        fs::read_to_string(path).map_err(|error| Error::Io { path: path.into(), error })?;
    serde_json::from_str(&contents)
        .map_err(|error| Error::CouldNotDeserialize { path: path.into(), error })
}

/// Runs file workers with one shared queue and bounded thread stacks.
pub(crate) fn run_workers(
    files: Vec<PathBuf>,
    workers: usize,
    worker: impl Fn(&mut dyn Iterator<Item = PathBuf>) + Sync,
) {
    let workers = workers.max(1).min(files.len());
    let files = Mutex::new(files.into_iter());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            std::thread::Builder::new()
                .stack_size(WORKER_STACK_SIZE)
                .spawn_scoped(scope, || {
                    let mut files = std::iter::from_fn(|| files.lock().unwrap().next());
                    worker(&mut files);
                })
                .expect("failed to spawn a fixture worker");
        }
    });
}
