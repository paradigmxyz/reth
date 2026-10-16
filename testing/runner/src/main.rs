//! Command-line interface for running tests.
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use clap::{Args, Parser, Subcommand};
use ef_tests::{
    cases::blockchain_test::{remove_shared_rocksdb, BlockTestOptions, BlockchainTests},
    result::FixtureResult,
    suite::find_all_files_with_extension,
    Suite,
};
use reth_node_core::{args::StorageArgs, version::version_metadata};
use reth_provider::StorageSettings;

mod engine_test;
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
    exit_unless_readable(&args.paths);
    if args.bal_report {
        report::init();
    }
    let results = Mutex::new(Vec::new());
    let on_result = |result: FixtureResult| results.lock().unwrap().push(result);
    match command {
        Command::BlockTest(BlockTestArgs { run: args, bal_withhold }) => {
            let options = BlockTestOptions { bal_sequential: args.bal_sequential, bal_withhold };
            let files = fixture_files(&args.paths, "blockchain_tests");
            exit_unless_readable(&files);
            BlockchainTests::run_fixtures(files, options, args.workers, &on_result);
            // Every fixture ran, so nothing uses the shared RocksDB instance anymore.
            remove_shared_rocksdb();
        }
        Command::EngineTest(EngineTestArgs { run: args, storage }) => {
            let files = fixture_files(&args.paths, "blockchain_tests_engine");
            exit_unless_readable(&files);
            engine_test::run_fixtures(
                files,
                args.bal_sequential,
                storage_settings(storage),
                args.workers,
                &on_result,
            );
        }
    }
    // A file that could not be loaded has no fixture results: it goes to stderr only, and means
    // the run did not cover all of its input.
    let (unloaded, results): (Vec<_>, Vec<_>) =
        results.into_inner().unwrap().into_iter().partition(|result| result.load_failed);
    println!("{}", serde_json::to_string(&results).expect("results serialize"));
    for result in &unloaded {
        eprintln!("error: {}", result.error);
    }
    if !unloaded.is_empty() {
        std::process::exit(2);
    }
    if !results.iter().all(|result| result.pass) {
        std::process::exit(1);
    }
}

/// Exits 2, before any fixture runs, if one of `paths` does not exist or cannot be read.
fn exit_unless_readable(paths: &[PathBuf]) {
    for path in paths {
        if let Err(err) = check_readable(path) {
            eprintln!("error: cannot read {}: {err}", path.display());
            std::process::exit(2);
        }
    }
}

/// Returns an error if `path` does not exist or cannot be read.
fn check_readable(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        fs::read_dir(path).map(drop)
    } else {
        fs::File::open(path).map(drop)
    }
}

/// Returns every fixture file under `paths`, in order.
fn fixture_files(paths: &[PathBuf], format: &str) -> Vec<PathBuf> {
    paths
        .iter()
        .flat_map(|path| {
            let mut files = find_all_files_with_extension(&fixtures_path(path, format), ".json");
            files.sort();
            files
        })
        .collect()
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
    use std::{sync::Condvar, time::Duration};

    /// Runs two copies of the fixture file `file` with `run` on two workers, and checks that they
    /// run at the same time: each result waits for the other one, which never comes while the
    /// first worker blocks the file queue.
    fn check_files_run_at_once(
        file: &str,
        run: impl FnOnce(Vec<PathBuf>, &(dyn Fn(FixtureResult) + Sync)),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(file);
        let files = (0..2)
            .map(|i| {
                let path = dir.path().join(format!("{i}.json"));
                fs::copy(&fixture, &path).unwrap();
                path
            })
            .collect();

        let arrived = (Mutex::new(0), Condvar::new());
        let timed_out = Mutex::new(Vec::new());
        run(files, &|_| {
            let (count, all_arrived) = &arrived;
            let mut count = count.lock().unwrap();
            *count += 1;
            all_arrived.notify_all();
            let (_count, wait) = all_arrived
                .wait_timeout_while(count, Duration::from_secs(5), |count| *count < 2)
                .unwrap();
            timed_out.lock().unwrap().push(wait.timed_out());
        });
        assert_eq!(*timed_out.lock().unwrap(), [false, false]);
    }

    #[test]
    fn blocktest_workers_run_files_at_the_same_time() {
        check_files_run_at_once("blocktest_empty_block.json", |files, on_result| {
            BlockchainTests::run_fixtures(files, BlockTestOptions::default(), 2, on_result)
        });
    }

    #[test]
    fn enginetest_workers_run_files_at_the_same_time() {
        check_files_run_at_once("enginetest_empty_block.json", |files, on_result| {
            engine_test::run_fixtures(files, false, StorageSettings::base(), 2, on_result)
        });
    }

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
