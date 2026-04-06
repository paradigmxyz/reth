//! Command-line interface for running Ethereum execution spec tests.
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use ef_tests::{
    cases::{
        blockchain_test::BlockchainTests,
        engine_test::{EngineTestMode, EngineTests},
        engine_x_test::EngineXTests,
    },
    Suite,
};

/// Reth test runner for Ethereum execution spec test fixtures.
#[derive(Debug, Parser)]
#[command(name = "ef-test-runner", version)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to the test suite (used when no subcommand is given).
    suite_path: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Execute blockchain test fixtures (blockchain_tests format).
    #[command(name = "blocktest")]
    BlockTest {
        /// Path to the test fixtures directory or file.
        path: PathBuf,

        /// Output results as a JSON array.
        #[arg(long)]
        json_array: bool,
    },
    /// Execute engine test fixtures (blockchain_test_engine format)
    /// through the real Engine API tree path.
    #[command(name = "enginetest")]
    EngineTest {
        /// Path to the blockchain_tests_engine fixture directory.
        path: PathBuf,

        /// Fast mode: direct EVM execution with parameter validation.
        /// Skips engine tree. (~5s for 40k tests)
        #[arg(long)]
        fast: bool,

        /// Output results as a JSON array.
        #[arg(long)]
        json_array: bool,
    },
    /// Execute engine-x test fixtures (blockchain_test_engine_x format)
    /// with cached provider factories per (fork, preHash).
    #[command(name = "enginextest")]
    EngineXTest {
        /// Path to the test fixtures directory (e.g. .../blockchain_tests_engine_x/prague/).
        path: PathBuf,

        /// Path to the pre_alloc directory containing shared genesis state files.
        #[arg(long)]
        pre_alloc_dir: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::BlockTest { path, json_array }) => {
            let suite_path = if path.ends_with("blockchain_tests") {
                path
            } else {
                let candidate = path.join("blockchain_tests");
                if candidate.exists() { candidate } else { path }
            };
            if json_array {
                BlockchainTests::new(suite_path).run_json();
            } else {
                BlockchainTests::new(suite_path).run();
            }
        }
        Some(Commands::EngineTest { path, fast, json_array }) => {
            let mode = if fast {
                EngineTestMode::Fast
            } else {
                EngineTestMode::EngineTree
            };
            if json_array {
                EngineTests::new(path).with_mode(mode).run_json();
            } else {
                EngineTests::new(path).with_mode(mode).run();
            }
        }
        Some(Commands::EngineXTest { path, pre_alloc_dir }) => {
            EngineXTests::new(path, pre_alloc_dir).run();
        }
        None => {
            let suite_path = cli
                .suite_path
                .expect("suite_path is required when no subcommand is given");
            BlockchainTests::new(suite_path.join("blockchain_tests")).run();
        }
    }
}
