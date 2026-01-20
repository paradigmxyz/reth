//! `reth report` command
//!
//! Generates a debug report archive for state root validation errors and other issues.
//! This command collects all necessary information for bug reports into a single `.tar.gz` archive.

use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use clap::Parser;
use flate2::{write::GzEncoder, Compression};
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_db_api::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx, Tables};
use reth_node_core::dirs::logs_dir;
use reth_provider::BlockNumReader;
use serde::Serialize;
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};
use tar::Builder;
use tracing::info;

/// Generate a debug report archive for bug reports.
///
/// This command collects diagnostic information including:
/// - Database table statistics
/// - Stage checkpoints (sync progress)
/// - Prune checkpoints (pruning progress)
/// - VersionHistory table dump
/// - Log files
/// - Invalid block hook outputs
/// - Optional: database checksums (--checksum flag)
#[derive(Parser, Debug)]
#[command(name = "report")]
pub struct Command<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    /// Include checksum of all database tables.
    /// WARNING: This will take a very long time to run!
    #[arg(long, default_value_t = false)]
    checksum: bool,

    /// Output file path for the debug report archive.
    /// Defaults to `reth_report_<timestamp>.tar.gz` in the current directory.
    #[arg(long, short)]
    output: Option<PathBuf>,

    /// Skip the confirmation prompt and proceed directly.
    #[arg(long, short = 'y', default_value_t = false)]
    yes: bool,
}

/// Report metadata stored as JSON in the archive
#[derive(Debug, Serialize)]
struct ReportMetadata {
    /// Timestamp when the report was generated
    generated_at: String,
    /// reth version
    reth_version: String,
    /// Chain name
    chain: String,
    /// Data directory path
    data_dir: String,
}

/// Database statistics
#[derive(Debug, Serialize)]
struct DbStats {
    /// Latest block number in database
    latest_block: Option<u64>,
    /// Table statistics
    tables: Vec<TableStats>,
    /// Checksums (if requested)
    checksums: Option<Vec<TableChecksum>>,
}

/// Statistics for a single table
#[derive(Debug, Serialize)]
struct TableStats {
    /// Table name
    name: String,
    /// Number of entries
    entries: u64,
    /// Size in bytes
    size_bytes: u64,
}

/// Checksum for a single table
#[derive(Debug, Serialize)]
struct TableChecksum {
    /// Table name
    name: String,
    /// Checksum value
    checksum: String,
}

/// Version history entry
#[derive(Debug, Serialize)]
struct VersionHistoryEntry {
    /// Timestamp (Unix seconds)
    timestamp: u64,
    /// Client version
    version: String,
    /// Git SHA
    git_sha: String,
    /// Build timestamp
    build_timestamp: String,
}

/// Stage checkpoint entry for serialization
#[derive(Debug, Serialize)]
struct StageCheckpointEntry {
    /// Stage name
    stage: String,
    /// Block number
    block_number: u64,
}

/// Prune checkpoint entry for serialization
#[derive(Debug, Serialize)]
struct PruneCheckpointEntry {
    /// Prune segment name
    segment: String,
    /// Highest pruned block number
    block_number: Option<u64>,
    /// Highest pruned transaction number
    tx_number: Option<u64>,
    /// Prune mode
    prune_mode: String,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    /// Execute `report` command
    pub async fn execute<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(self) -> eyre::Result<()> {
        let Environment { provider_factory, data_dir, .. } =
            self.env.init::<N>(AccessRights::RO)?;

        // Show what will be collected
        println!("\n{}", "=".repeat(70));
        println!("RETH DEBUG REPORT GENERATOR");
        println!("{}", "=".repeat(70));
        // Get log directory path (logs are in cache dir, not data dir)
        let log_dir = logs_dir();

        println!("\nThis tool will collect the following information:");
        println!("  • Database table statistics");
        println!("  • Stage checkpoints (sync progress)");
        println!("  • Prune checkpoints (pruning progress)");
        println!("  • VersionHistory table (client version history)");
        if let Some(ref log_path) = log_dir {
            println!("  • Log files from: {}", log_path.display());
        }
        println!(
            "  • Invalid block hook outputs from: {}",
            data_dir.invalid_block_hooks().display()
        );

        if self.checksum {
            println!("  • Database checksums (WARNING: This takes a long time!)");
        }

        println!("\n⚠️  PRIVACY NOTICE:");
        println!("  The generated archive may contain sensitive information about your node.");
        println!("  Please review the contents before sharing publicly.");

        // User consent
        if !self.yes {
            print!("\nDo you want to proceed? [y/N]: ");
            std::io::stdout().flush()?;

            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;

            if !input.trim().eq_ignore_ascii_case("y") {
                println!("Aborted.");
                return Ok(());
            }
        }

        info!(target: "reth::cli", "Generating debug report archive");

        // Capture current time once for consistent timestamps throughout the report
        let now = SystemTime::now();
        let now_secs = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();

        // Determine output path (keep 'Z' suffix to indicate UTC timezone)
        let output_path = self.output.clone().unwrap_or_else(|| {
            let timestamp = humantime::format_rfc3339_seconds(now)
                .to_string()
                .replace([':', '-'], "")
                .replace('T', "_");
            PathBuf::from(format!("reth_report_{timestamp}.tar.gz"))
        });

        // Create the archive
        let file = File::create(&output_path)?;
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = Builder::new(encoder);

        // Collect and add metadata
        let chain_spec = provider_factory.chain_spec();
        let metadata = ReportMetadata {
            generated_at: humantime::format_rfc3339(now).to_string(),
            reth_version: env!("CARGO_PKG_VERSION").to_string(),
            chain: chain_spec.chain().to_string(),
            data_dir: data_dir.data_dir().display().to_string(),
        };
        add_json_to_archive(&mut archive, "metadata.json", &metadata, now_secs)?;

        // Collect DB stats
        let provider = provider_factory.provider()?;
        let latest_block = provider.last_block_number().ok();
        let mut tables_stats = Vec::new();

        provider_factory.db_ref().view(|tx| {
            for table in Tables::ALL {
                if let Ok(table_db) = tx.inner.open_db(Some(table.name())) &&
                    let Ok(table_stats) = tx.inner.db_stat(table_db.dbi())
                {
                    let page_size = table_stats.page_size() as u64;
                    let num_pages = (table_stats.leaf_pages() +
                        table_stats.branch_pages() +
                        table_stats.overflow_pages()) as u64;

                    tables_stats.push(TableStats {
                        name: table.name().to_string(),
                        entries: table_stats.entries() as u64,
                        size_bytes: page_size * num_pages,
                    });
                }
            }
            Ok::<(), eyre::Report>(())
        })??;

        // Collect checksums if requested
        let checksums = if self.checksum {
            info!(target: "reth::cli", "Computing checksums (this may take a while)...");
            Some(collect_checksums(&provider_factory)?)
        } else {
            None
        };

        let db_stats = DbStats { latest_block, tables: tables_stats, checksums };
        add_json_to_archive(&mut archive, "db_stats.json", &db_stats, now_secs)?;
        info!(target: "reth::cli", "Database stats collected");

        // Collect stage checkpoints
        let mut stage_checkpoints = Vec::new();
        provider_factory.db_ref().view(|tx| {
            let mut cursor = tx.cursor_read::<tables::StageCheckpoints>()?;
            while let Some((stage_id, checkpoint)) = cursor.next()? {
                stage_checkpoints.push(StageCheckpointEntry {
                    stage: stage_id.to_string(),
                    block_number: checkpoint.block_number,
                });
            }
            Ok::<(), eyre::Report>(())
        })??;
        add_json_to_archive(&mut archive, "stage_checkpoints.json", &stage_checkpoints, now_secs)?;
        info!(target: "reth::cli", entries = stage_checkpoints.len(), "Stage checkpoints collected");

        // Collect prune checkpoints
        let mut prune_checkpoints = Vec::new();
        provider_factory.db_ref().view(|tx| {
            let mut cursor = tx.cursor_read::<tables::PruneCheckpoints>()?;
            while let Some((segment, checkpoint)) = cursor.next()? {
                prune_checkpoints.push(PruneCheckpointEntry {
                    segment: segment.to_string(),
                    block_number: checkpoint.block_number,
                    tx_number: checkpoint.tx_number,
                    prune_mode: format!("{:?}", checkpoint.prune_mode),
                });
            }
            Ok::<(), eyre::Report>(())
        })??;
        add_json_to_archive(&mut archive, "prune_checkpoints.json", &prune_checkpoints, now_secs)?;
        info!(target: "reth::cli", entries = prune_checkpoints.len(), "Prune checkpoints collected");

        // Collect VersionHistory
        let mut version_history = Vec::new();
        provider_factory.db_ref().view(|tx| {
            let mut cursor = tx.cursor_read::<tables::VersionHistory>()?;
            while let Some((timestamp, version)) = cursor.next()? {
                version_history.push(VersionHistoryEntry {
                    timestamp,
                    version: version.version,
                    git_sha: version.git_sha,
                    build_timestamp: version.build_timestamp,
                });
            }
            Ok::<(), eyre::Report>(())
        })??;
        add_json_to_archive(&mut archive, "version_history.json", &version_history, now_secs)?;
        info!(target: "reth::cli", entries = version_history.len(), "Version history collected");

        // Collect log files (from cache directory)
        let logs_collected = if let Some(ref log_path) = log_dir {
            collect_directory_recursive(&mut archive, log_path, "logs")?
        } else {
            0
        };
        info!(target: "reth::cli", files = logs_collected, "Log files collected");

        // Collect invalid_block_hooks directory
        let hooks_collected = collect_directory_recursive(
            &mut archive,
            &data_dir.invalid_block_hooks(),
            "invalid_block_hooks",
        )?;
        info!(target: "reth::cli", files = hooks_collected, "Invalid block hook files collected");

        // Finalize the archive
        archive.finish()?;

        println!("\n{}", "=".repeat(70));
        println!("DEBUG REPORT GENERATED SUCCESSFULLY");
        println!("{}", "=".repeat(70));
        println!("\nArchive saved to: {}", output_path.display());

        // Generate GitHub issue link
        let issue_link = generate_github_issue_link();
        println!("\n📋 To create a GitHub issue, visit:");
        println!("{issue_link}");
        println!("\n⚠️  IMPORTANT:");
        println!("   • You must be logged into GitHub in your browser");
        println!("   • The issue will be created under YOUR GitHub account");
        println!("   • Please attach the generated archive to your issue");
        println!("   • Review the archive contents before sharing");

        Ok(())
    }
}

fn collect_checksums<DB: Database>(
    provider_factory: &reth_provider::ProviderFactory<
        impl reth_provider::providers::ProviderNodeTypes<DB = DB>,
    >,
) -> eyre::Result<Vec<TableChecksum>> {
    use crate::db::checksum::ChecksumViewer;
    use reth_db_api::TableViewer;
    use reth_db_common::DbTool;

    let tool = DbTool::new(provider_factory.clone())?;
    let mut checksums = Vec::new();

    for &table in Tables::ALL {
        match ChecksumViewer::new(&tool).view_rt(table) {
            Ok((checksum, _elapsed)) => {
                checksums.push(TableChecksum {
                    name: table.name().to_string(),
                    checksum: format!("{checksum:x}"),
                });
            }
            Err(e) => {
                checksums.push(TableChecksum {
                    name: table.name().to_string(),
                    checksum: format!("ERROR: {e}"),
                });
            }
        }
    }

    Ok(checksums)
}

fn collect_directory_recursive<W: Write>(
    archive: &mut Builder<W>,
    dir: &Path,
    archive_prefix: &str,
) -> eyre::Result<usize> {
    let mut count = 0;

    if !dir.exists() {
        // This is normal if no invalid blocks have been detected
        return Ok(0);
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let archive_path = format!("{archive_prefix}/{name}");

            if path.is_dir() {
                count += collect_directory_recursive(archive, &path, &archive_path)?;
            } else {
                add_file_to_archive(archive, &path, &archive_path)?;
                count += 1;
            }
        }
    }

    Ok(count)
}

fn add_json_to_archive<W: Write, T: Serialize>(
    archive: &mut Builder<W>,
    name: &str,
    data: &T,
    mtime: u64,
) -> eyre::Result<()> {
    let json = serde_json::to_string_pretty(data)?;
    let bytes = json.as_bytes();

    let mut header = tar::Header::new_gnu();
    header.set_path(name)?;
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(mtime);
    header.set_cksum();

    archive.append(&header, bytes)?;
    Ok(())
}

fn add_file_to_archive<W: Write>(
    archive: &mut Builder<W>,
    file_path: &Path,
    archive_path: &str,
) -> eyre::Result<()> {
    let mut file = File::open(file_path)?;
    let metadata = file.metadata()?;

    let mut header = tar::Header::new_gnu();
    header.set_path(archive_path)?;
    header.set_size(metadata.len());
    header.set_mode(0o644);
    header.set_mtime(
        metadata
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH)
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    );
    header.set_cksum();

    archive.append(&header, &mut file)?;
    Ok(())
}

fn generate_github_issue_link() -> String {
    format!(
        "https://github.com/paradigmxyz/reth/issues/new?title={}&labels=C-bug",
        urlencoding::encode("Debug Report")
    )
}
