use crate::util;
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
/// models for parsing json blockchain test.
pub mod models;
/// Runs eth blockhain tests
pub mod runner;

/// Execute ethereum blockchain tests by specifying path to json files
#[derive(Debug, Parser)]
pub struct Command {
    /// Path to json files
    path: Vec<PathBuf>,
}

impl Command {
    /// Execute the command
    pub async fn execute(self) -> Result<()> {
        // note the use of `into_iter()` to consume `items`
        let task_group: Vec<_> = self
            .path
            .iter()
            .map(|item| {
                let mut test = Vec::new();
                let files = util::find_all_json_tests(item);
                for file in files {
                    let tfile = file.clone();
                    let join = tokio::spawn(async move { runner::run_test(&tfile).await });
                    test.push((join, file));
                }
                test
            })
            .collect();
        // await the tasks for resolve's to complete and give back our items
        let mut num_of_failed = 0;
        let mut num_of_passed = 0;
        for tasks in task_group {
            for (join, file) in tasks.into_iter() {
                match join.await.unwrap() {
                    Ok(_) => {
                        num_of_passed += 1;
                    }
                    Err(error) => {
                        num_of_failed += 1;
                        println!("Test {:?} failed:\n {error}\n", file);
                    }
                }
            }
        }

        println!("PASSED {num_of_passed}/{} tests", num_of_passed + num_of_failed);

        Ok(())
    }
}
