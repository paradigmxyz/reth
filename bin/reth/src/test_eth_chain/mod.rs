use crate::util;
use clap::Parser;
use std::path::PathBuf;
/// Models for parsing JSON blockchain tests
pub mod models;
/// Ethereum blockhain test runner
pub mod runner;

/// Execute Ethereum blockchain tests by specifying path to json files
#[derive(Debug, Parser)]
pub struct Command {
    /// Path to Ethereum JSON test files
    path: Vec<PathBuf>,
}

impl Command {
    /// Execute the command
    pub async fn execute(self) -> eyre::Result<()> {
        // note the use of `into_iter()` to consume `items`
        let task_group: Vec<_> = self
            .path
            .iter()
            .map(|item| {
                util::find_all_json_tests(item).into_iter().map(|file| {
                    let tfile = file.clone();
                    let join = tokio::spawn(async move { runner::run_test(tfile.as_path()).await });
                    (join, file)
                })
            })
            .collect();
        // await the tasks for resolve's to complete and give back our test results
        let mut num_of_failed = 0;
        let mut num_of_passed = 0;
        for tasks in task_group {
            for (join, file) in tasks {
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

        println!("\nPASSED {num_of_passed}/{} tests\n", num_of_passed + num_of_failed);

        Ok(())
    }
}
