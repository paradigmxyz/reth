//! Command for running Ethereum chain tests.

use clap::Parser;
use eyre::eyre;
use std::path::PathBuf;
use tracing::{error, info};
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
        let futs: Vec<_> = self
            .path
            .iter()
            .flat_map(|item| reth_cli_utils::find_all_files_with_postfix(item, ".json"))
            .map(|file| async { (runner::run_test(file.clone()).await, file) })
            .collect();

        let results = futures::future::join_all(futs).await;
        // await the tasks for resolve's to complete and give back our test results
        let mut num_of_failed = 0;
        let mut num_of_passed = 0;
        for (result, file) in results {
            match result {
                Ok(_) => {
                    num_of_passed += 1;
                }
                Err(error) => {
                    num_of_failed += 1;
                    error!("Test {file:?} failed:\n {error}\n");
                }
            }
        }

        info!("\nPASSED {num_of_passed}/{} tests\n", num_of_passed + num_of_failed);

        if num_of_failed != 0 {
            Err(eyre!("Failed {num_of_failed} tests"))
        } else {
            Ok(())
        }
    }
}
