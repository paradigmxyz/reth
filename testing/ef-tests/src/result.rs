use crate::case::Case;
use rayon::iter::ParallelIterator;
use std::{
    fmt::{Debug, Display, Formatter},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Error {
    Skipped,
    Foo,
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(self, f)
    }
}

impl std::error::Error for Error {}

pub struct CaseResult {
    pub desc: String,
    pub path: PathBuf,
    pub result: Result<(), Error>,
}

impl CaseResult {
    pub fn new(path: &Path, case: &impl Case, result: Result<(), Error>) -> Self {
        CaseResult { desc: case.description(), path: path.into(), result }
    }
}

pub(crate) fn assert_tests_pass(suite_name: &str, path: &Path, results: &[CaseResult]) {
    let (passed, failed, skipped) = categorize_results(results);

    print_results(suite_name, &passed, &failed, &skipped);

    if !failed.is_empty() {
        panic!("Some tests failed (see above)");
    }
}

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

pub(crate) fn print_results(
    suite_name: &str,
    passed: &[&CaseResult],
    failed: &[&CaseResult],
    skipped: &[&CaseResult],
) {
    println!("Suite: {suite_name}");
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
        let error = case.result.clone().unwrap_err();

        println!(
            "[!] Case {} failed (description: {}): {}",
            case.path.display(),
            case.desc,
            error.message()
        );
    }
}
