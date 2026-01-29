//! `RocksDB` tuning experimentation tool for reth.

use clap::{Parser, Subcommand};
use eyre::{bail, Context, Result};
use reth_tracing::{LayerInfo, LogFormat, RethTracer, Tracer};
use std::{fs, path::PathBuf};
use tracing::info;

mod config;
mod migrate;

use config::TuningConfig;
use migrate::{run_migration, MigrateOptions};

#[derive(Parser)]
#[command(name = "reth-db-tune")]
#[command(about = "RocksDB tuning experimentation tool for reth")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Migrate {
        #[arg(long)]
        src: PathBuf,

        #[arg(long)]
        dst: PathBuf,

        #[arg(long)]
        config: PathBuf,

        #[arg(long)]
        jobs: Option<i32>,

        #[arg(long, default_value = "true")]
        compact: bool,

        #[arg(long, default_value = "false")]
        verify: bool,

        #[arg(long, default_value = "false")]
        overwrite: bool,
    },
}

fn main() -> Result<()> {
    let tracer = RethTracer::new().with_stdout(LayerInfo::new(
        LogFormat::Terminal,
        "info".into(),
        String::new(),
        None,
    ));
    tracer.init()?;

    let cli = Cli::parse();

    match cli.command {
        Commands::Migrate { src, dst, config, jobs, compact, verify, overwrite } => {
            if !src.exists() {
                bail!("source directory does not exist: {:?}", src);
            }

            if dst.exists() && !overwrite {
                bail!("destination directory already exists: {:?}. Use --overwrite to allow.", dst);
            }

            if dst.exists() && overwrite {
                info!(path = %dst.display(), "removing existing destination directory");
                fs::remove_dir_all(&dst)?;
            }

            let config_content =
                fs::read_to_string(&config).wrap_err("failed to read config file")?;
            let tuning_config: TuningConfig =
                toml::from_str(&config_content).wrap_err("failed to parse config file")?;

            info!(src = %src.display(), dst = %dst.display(), "starting migration");

            let report = run_migration(MigrateOptions {
                src: &src,
                dst: &dst,
                config: &tuning_config,
                jobs_override: jobs,
                compact,
                verify,
            })?;

            info!(
                duration_secs = report.total_duration_secs,
                cfs = report.column_families.len(),
                "migration complete"
            );

            if let Some(passed) = report.verify_passed {
                if passed {
                    info!("verification: PASSED");
                } else {
                    bail!("verification: FAILED");
                }
            }

            Ok(())
        }
    }
}
