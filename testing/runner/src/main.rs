//! Command-line interface for running tests.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

use clap::{Args, Parser, Subcommand};
use ef_tests::{
    cases::{
        blockchain_test::{remove_shared_rocksdb, BlockTestOptions, BlockchainTests},
        engine_test,
    },
    result::FixtureResult,
    suite::find_all_files_with_extension,
    Error, Suite,
};
use reth_node_core::{
    args::StorageArgs,
    version::{try_init_version_metadata, version_metadata},
};
use reth_provider::StorageSettings;

mod report;

/// Command-line arguments for the test runner.
#[derive(Debug, Parser)]
#[command(
    name = "ef-test-runner",
    version = version_metadata().short_version.as_ref(),
    propagate_version = true,
    args_conflicts_with_subcommands = true,
    arg_required_else_help = true
)]
pub struct TestRunnerCommand {
    #[command(subcommand)]
    command: Option<Command>,
    /// Path to a test suite that contains a `blockchain_tests` directory, when no command is
    /// given.
    suite_path: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run blockchain tests by importing their blocks. Exits 1 if any fixture fails, and 2 if a
    /// path cannot be read or a fixture file cannot be loaded.
    #[command(name = "blocktest", display_name = "ef-test-runner")]
    BlockTest(BlockTestArgs),
    /// Run blockchain tests in the engine format through reth's Engine API handler, each on a
    /// datadir in the system temporary directory (`TMPDIR`; a tmpfs avoids disk syncs). Exits 1
    /// if any fixture fails, and 2 if a path cannot be read or a fixture file cannot be loaded.
    #[command(name = "enginetest", display_name = "ef-test-runner")]
    EngineTest(EngineTestArgs),
}

#[derive(Debug, Args)]
struct RunArgs {
    /// Fixture files, or directories searched for fixture files, all run in one process.
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Run blocks on the sequential executor instead of the BAL-driven parallel one, as the
    /// node's `--engine.disable-bal-parallel-execution` does. Block import has only the
    /// sequential executor, so for `blocktest` this only changes the reported reason.
    #[arg(long)]
    bal_sequential: bool,
    /// Print a `balExecution` JSON line on stderr for each executed block, saying which executor
    /// ran it and, for the sequential one, why.
    #[arg(long)]
    bal_report: bool,
    /// Print each fixture's verdict and full error to stderr as soon as it finishes.
    #[arg(long)]
    progress: bool,
    /// How many fixture files run at once.
    #[arg(long, default_value_t = default_workers())]
    workers: usize,
}

#[derive(Debug, Args)]
struct BlockTestArgs {
    #[command(flatten)]
    run: RunArgs,
    /// Never deliver a block's access list, so every block executes without one, as a block
    /// received without its list does.
    #[arg(long)]
    bal_withhold: bool,
}

#[derive(Debug, Args)]
struct EngineTestArgs {
    #[command(flatten)]
    run: RunArgs,
    #[command(flatten)]
    storage: StorageArgs,
}

/// Returns the storage settings a node writes with the genesis of a new database.
const fn storage_settings(storage: StorageArgs) -> StorageSettings {
    if storage.v2 {
        StorageSettings::v2()
    } else {
        StorageSettings::v1()
    }
}

fn default_workers() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

fn main() {
    try_init_version_metadata(reth_node_core::reth_version_metadata!())
        .expect("runner version metadata is initialized once before parsing arguments");
    let cmd = TestRunnerCommand::parse();
    let Some(command) = cmd.command else {
        let suite_path = cmd.suite_path.expect("a suite path or a command is required");
        BlockchainTests::new(suite_path.join("blockchain_tests")).run();
        return
    };

    let args = match &command {
        Command::BlockTest(BlockTestArgs { run, .. }) |
        Command::EngineTest(EngineTestArgs { run, .. }) => run,
    };
    if args.bal_report {
        report::init();
    }
    let progress = args.progress;
    let results = Mutex::new(Vec::new());
    let load_failed = AtomicBool::new(false);
    let on_result = |result: Result<FixtureResult, Error>| {
        let result = match result {
            Ok(result) => result,
            Err(err) => {
                eprintln!("error: {err}");
                load_failed.store(true, Ordering::Relaxed);
                return
            }
        };
        if progress {
            eprintln!(
                "{} {}{}",
                if result.pass { "PASS" } else { "FAIL" },
                result.name,
                if result.error.is_empty() { String::new() } else { format!(": {}", result.error) }
            );
        }
        results.lock().unwrap().push(result);
    };
    let run_result = match command {
        Command::BlockTest(BlockTestArgs { run: args, bal_withhold }) => {
            let options = BlockTestOptions { bal_sequential: args.bal_sequential, bal_withhold };
            let files = checked_fixture_files(&args.paths, "blockchain_tests");
            BlockchainTests::run_fixtures(files, options, args.workers, &on_result);
            // Every fixture ran, so nothing uses the shared RocksDB instance anymore.
            remove_shared_rocksdb();
            Ok(())
        }
        Command::EngineTest(EngineTestArgs { run: args, storage }) => {
            let files = checked_fixture_files(&args.paths, "blockchain_tests_engine");
            engine_test::run_fixtures(
                files,
                args.bal_sequential,
                storage_settings(storage),
                args.workers,
                &on_result,
            )
        }
    };
    let results = results.into_inner().unwrap();
    println!("{}", serde_json::to_string(&results).expect("results serialize"));
    if let Err(err) = run_result {
        eprintln!("error: {err}");
        std::process::exit(2);
    }
    if load_failed.load(Ordering::Relaxed) {
        std::process::exit(2);
    }
    if !results.iter().all(|result| result.pass) {
        std::process::exit(1);
    }
}

/// Resolves and opens every fixture path before execution, exiting 2 on any discovery error.
fn checked_fixture_files(paths: &[PathBuf], format: &str) -> Vec<PathBuf> {
    fixture_files(paths, format).unwrap_or_else(|err| {
        eprintln!("error: cannot read fixtures: {err}");
        std::process::exit(2)
    })
}

/// Collects every fixture without discarding directory traversal errors.
fn fixture_files(paths: &[PathBuf], format: &str) -> Result<Vec<PathBuf>, Error> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut found = find_all_files_with_extension(&fixtures_path(path, format), ".json")?;
            found.sort();
            files.extend(found);
        } else {
            fs::File::open(path).map_err(|error| Error::Io { path: path.clone(), error })?;
            files.push(path.clone());
        }
    }
    Ok(files)
}

/// Returns the `format` directory of a fixtures release if `path` is one, otherwise `path`.
fn fixtures_path(path: &Path, format: &str) -> PathBuf {
    let candidate = path.join(format);
    if candidate.is_dir() {
        candidate
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enginetest_takes_the_node_storage_layout_flag() {
        let settings = |args: &[&str]| {
            let cmd = TestRunnerCommand::parse_from(
                ["ef-test-runner", "enginetest", "fixtures"].iter().chain(args),
            );
            let Some(Command::EngineTest(args)) = cmd.command else { panic!("{cmd:?}") };
            storage_settings(args.storage)
        };
        assert_eq!(settings(&[]), StorageSettings::v2());
        assert_eq!(settings(&["--storage.v2"]), StorageSettings::v2());
        assert_eq!(settings(&["--storage.v2=false"]), StorageSettings::v1());
    }
}
