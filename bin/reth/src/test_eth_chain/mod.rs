//! Command for running Ethereum chain tests.

use clap::Parser;
use eyre::eyre;
use futures::{stream::FuturesUnordered, StreamExt};
use std::path::PathBuf;
use tracing::{error, info, warn};
/// Models for parsing JSON blockchain tests
pub mod models;
/// Ethereum blockhain test runner
pub mod runner;

use runner::TestOutcome;

/// Execute Ethereum blockchain tests by specifying path to json files
#[derive(Debug, Parser)]
pub struct Command {
    /// Path to Ethereum JSON test files
    path: Vec<PathBuf>,
}

impl Command {
    /// Execute the command
    pub async fn execute(self) -> eyre::Result<()> {
        let mut futs: FuturesUnordered<_> = self
            .path
            .iter()
            .flat_map(|item| reth_staged_sync::utils::find_all_files_with_postfix(item, ".json"))
            .map(|file| async { (runner::run_test(file.clone()).await, file) })
            .collect();

        let mut failed = 0;
        let mut passed = 0;
        let mut skipped = 0;
        while let Some((result, file)) = futs.next().await {
            match TestOutcome::from(result) {
                TestOutcome::Passed => {
                    info!(target: "reth::cli", "[+] Test {file:?} passed.");
                    passed += 1;
                }
                TestOutcome::Skipped => {
                    warn!(target: "reth::cli", "[=] Test {file:?} skipped.");
                    skipped += 1;
                }
                TestOutcome::Failed(error) => {
                    error!(target: "reth::cli", "Test {file:?} failed:\n{error}");
                    failed += 1;
                }
            }
        }

        info!(target: "reth::cli", "{passed}/{0} tests passed, {skipped}/{0} skipped, {failed}/{0} failed.\n", failed + passed + skipped);

        if failed != 0 {
            Err(eyre!("Failed {failed} tests"))
        } else {
            Ok(())
        }
    }
}
