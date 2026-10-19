//! Command-line interface for running tests.
use std::path::PathBuf;

use clap::Parser;
use ef_tests::{cases::blockchain_test::BlockchainTests, Suite};

/// Command-line arguments for the test runner.
#[derive(Debug, Parser)]
pub struct TestRunnerCommand {
    /// Path to the test suite
    suite_path: PathBuf,
}

fn main() {
    let cmd = TestRunnerCommand::parse();
    BlockchainTests::new(cmd.suite_path.join("blockchain_tests")).run();
}
