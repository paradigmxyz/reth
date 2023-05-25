use crate::result::Error;
use rayon::iter::IntoParallelIterator;
use result::CaseResult;
use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};

pub trait Case: Debug + Sync {
    fn description(&self) -> String {
        "no description".to_string()
    }

    fn load(path: &Path) -> Self;

    fn run(&self) -> Result<(), Error>;
}

pub struct Cases<T> {
    pub test_cases: Vec<(PathBuf, T)>,
}

impl<T: Case> Cases<T> {
    pub fn run(&self) -> Vec<CaseResult> {
        self.test_cases
            .into_par_iter()
            .map(|(path, case)| CaseResult::new(path, case, case.run()))
            .collect()
    }
}
