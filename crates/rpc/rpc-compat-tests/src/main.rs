//! Command-line interface for the Rust-native Reth RPC compatibility runner.

use clap::{Args, Parser, Subcommand, ValueEnum};
use eyre::Result;
#[cfg(feature = "embedded")]
use reth_rpc_compat_tests::run_embedded;
use reth_rpc_compat_tests::{
    case::discover,
    config::{Config, RunAdditions},
    fixture,
    report::{report_summary, UnexpectedKind},
    schema::SchemaCatalog,
};
use std::{
    collections::BTreeMap,
    io::{self, IsTerminal},
    path::PathBuf,
};

#[derive(Debug, Parser)]
#[command(about = "Run execution-apis RPC compatibility fixtures against embedded Reth")]
struct Cli {
    #[arg(long, default_value = "crates/rpc/rpc-compat-tests/rpc-compat.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Download and verify the configured fixture.
    Fetch(FixtureArgs),
    /// List selected tests without launching Reth.
    List(RunArgs),
    /// Validate fixture parsing, policy, choices, and `OpenRPC` schemas.
    Check(RunArgs),
    /// Launch embedded Reth and run selected tests.
    Run(RunArgs),
    /// Summarize a completed JSON report and list unexpected test IDs.
    ListUnexpected(ListUnexpectedArgs),
}

#[derive(Debug, Clone, Args)]
struct FixtureArgs {
    /// Use a local execution-apis repository or tests directory.
    #[arg(long)]
    fixture: Option<PathBuf>,
    /// Override the configured fixture revision.
    #[arg(long)]
    revision: Option<String>,
    /// Refuse network access.
    #[arg(long)]
    offline: bool,
}

#[derive(Debug, Clone, Args)]
struct RunArgs {
    #[command(flatten)]
    fixture: FixtureArgs,
    /// Add a test inclusion pattern.
    #[arg(long)]
    include: Vec<String>,
    /// Add a test exclusion pattern.
    #[arg(long)]
    exclude: Vec<String>,
    /// Add a skip pattern.
    #[arg(long)]
    skip: Vec<String>,
    /// Add an ignored-test pattern.
    #[arg(long)]
    ignore: Vec<String>,
    /// Add an expected-failure pattern.
    #[arg(long = "xfail")]
    expected_failures: Vec<String>,
    /// Add a failure expected only when JSON-RPC error `data` is compared.
    #[arg(long = "xfail-when-error-data-checked")]
    expected_failures_when_error_data_checked: Vec<String>,
    /// Enable a named profile.
    #[arg(long)]
    profile: Vec<String>,
    /// Select a named choice as NAME=OPTION.
    #[arg(long, value_parser = parse_choice)]
    choice: Vec<(String, String)>,
    /// Stop on the first unexpected result.
    #[arg(long)]
    fail_fast: bool,
    /// Override the per-request timeout.
    #[arg(long)]
    timeout_secs: Option<u64>,
    /// Write a JSON report.
    #[arg(long)]
    report_json: Option<PathBuf>,
    /// Write a `JUnit` XML report.
    #[arg(long)]
    report_junit: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
struct ListUnexpectedArgs {
    /// JSON compatibility report to read.
    #[arg(long, default_value = "target/rpc-compat/report.json")]
    report: PathBuf,
    /// Control colored output.
    #[arg(long, value_enum, default_value = "auto")]
    color: OutputColor,
    /// Emit one collapsible `GitHub` Actions log group per unexpected result.
    #[arg(long)]
    github_groups: bool,
    /// Return a failing exit status when the report contains unexpected results.
    #[arg(long)]
    fail_on_unexpected: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputColor {
    Auto,
    Always,
    Never,
}

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(feature = "embedded")]
    reth_tracing::init_test_tracing();
    let cli = Cli::parse();
    let (config, base) = Config::load(&cli.config)?;
    match cli.command {
        Command::Fetch(args) => {
            let fixture = resolve_fixture(&config, &base, &args).await?;
            println!("{}", fixture.revision);
        }
        Command::List(args) => {
            let fixture = resolve_fixture(&config, &base, &args.fixture).await?;
            let run = resolve_run(&config, &base, &args)?;
            let tests = discover(&fixture.tests)?;
            run.validate_policy(tests.iter().map(|test| test.id.as_str()))?;
            let selected = tests.iter().filter(|test| run.selected(&test.id));
            for test in selected {
                println!(
                    "{}{}{}{}",
                    test.id,
                    if test.spec_only { " [speconly]" } else { "" },
                    if run.skipped(&test.id) { " [skip]" } else { "" },
                    if run.expected_failure(&test.id) { " [xfail]" } else { "" },
                );
            }
        }
        Command::Check(args) => {
            let fixture = resolve_fixture(&config, &base, &args.fixture).await?;
            let run = resolve_run(&config, &base, &args)?;
            let tests = discover(&fixture.tests)?;
            run.validate_policy(tests.iter().map(|test| test.id.as_str()))?;
            let schemas = SchemaCatalog::load(&fixture.root)?;
            println!(
                "validated {} tests and loaded {} OpenRPC result schemas",
                tests.len(),
                schemas.len()
            );
        }
        Command::Run(args) => {
            #[cfg(feature = "embedded")]
            {
                let fixture = resolve_fixture(&config, &base, &args.fixture).await?;
                let run = resolve_run(&config, &base, &args)?;
                run_embedded(fixture, run).await?;
            }
            #[cfg(not(feature = "embedded"))]
            {
                let _ = args;
                return Err(eyre::eyre!("the run command requires the `embedded` feature"));
            }
        }
        Command::ListUnexpected(args) => print_unexpected(args)?,
    }
    Ok(())
}

fn print_unexpected(args: ListUnexpectedArgs) -> Result<()> {
    let summary = report_summary(&args.report)?;
    let results = &summary.unexpected;
    let color = match args.color {
        OutputColor::Auto => io::stdout().is_terminal() || std::env::var_os("CI").is_some(),
        OutputColor::Always => true,
        OutputColor::Never => false,
    };
    let (bold, reset) = if color { ("\x1b[1m", "\x1b[0m") } else { ("", "") };
    println!("{bold}RPC compatibility summary:{reset}");
    println!("  fixture: {}", summary.fixture_revision);
    println!(
        "  tests: {} selected, {} run, {} skipped",
        summary.selected, summary.executed, summary.skipped
    );
    println!(
        "  outcomes: {} passed, {} failed, {} expected failures, {} unexpected passes, {} ignored",
        summary.passed,
        summary.failed,
        summary.expected_failures,
        summary.unexpected_passes,
        summary.ignored
    );
    println!("  duration: {}.{:03}s", summary.duration_ms / 1000, summary.duration_ms % 1000);
    println!();
    println!("{bold}Unexpected failures:{reset}");
    if results.is_empty() {
        let green = if color { "\x1b[32m" } else { "" };
        println!("  {green}none{reset}");
    } else if !args.github_groups {
        for result in results {
            let color = if color {
                match result.kind {
                    UnexpectedKind::Failure => "\x1b[31m",
                    UnexpectedKind::UnexpectedPass => "\x1b[33m",
                }
            } else {
                ""
            };
            println!("  {color}{}{reset}", result.id);
        }
    }
    if args.github_groups {
        for result in results {
            let label = match result.kind {
                UnexpectedKind::Failure => "Failed",
                UnexpectedKind::UnexpectedPass => "Unexpected pass",
            };
            println!("::group::{label}: {}", result.id);
            if let Some(detail) = &result.detail {
                println!("{detail}");
            } else {
                println!("expected failure passed unexpectedly");
            }
            println!("::endgroup::");
        }
    }
    println!();
    println!("{bold}Passed tests:{reset}");
    if summary.passed_tests.is_empty() {
        println!("  none");
    } else {
        let green = if color { "\x1b[32m" } else { "" };
        for id in &summary.passed_tests {
            println!("  {green}{id}{reset}");
        }
    }
    if args.fail_on_unexpected && !results.is_empty() {
        return Err(eyre::eyre!(
            "compatibility report contains {} unexpected result(s)",
            results.len()
        ))
    }
    Ok(())
}

async fn resolve_fixture(
    config: &Config,
    base: &std::path::Path,
    args: &FixtureArgs,
) -> Result<fixture::Fixture> {
    fixture::resolve(
        &config.fixture,
        base,
        args.fixture.as_deref(),
        args.revision.as_deref(),
        args.offline,
    )
    .await
}

fn resolve_run(
    config: &Config,
    base: &std::path::Path,
    args: &RunArgs,
) -> Result<reth_rpc_compat_tests::config::ResolvedRunConfig> {
    let choices = args.choice.iter().cloned().collect::<BTreeMap<_, _>>();
    let additions = RunAdditions {
        include: args.include.clone(),
        exclude: args.exclude.clone(),
        skip: args.skip.clone(),
        ignore: args.ignore.clone(),
        expected_failures: args.expected_failures.clone(),
        expected_failures_when_error_data_checked: args
            .expected_failures_when_error_data_checked
            .clone(),
        fail_fast: args.fail_fast,
        timeout_secs: args.timeout_secs,
        report_json: args.report_json.clone(),
        report_junit: args.report_junit.clone(),
    };
    config.resolve_run(base, &args.profile, &choices, &additions)
}

fn parse_choice(value: &str) -> Result<(String, String), String> {
    let (name, option) =
        value.split_once('=').ok_or_else(|| "choice must use NAME=OPTION".to_string())?;
    if name.is_empty() || option.is_empty() {
        return Err("choice name and option must not be empty".to_string());
    }
    Ok((name.to_string(), option.to_string()))
}
