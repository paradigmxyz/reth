//! `reth db debug-report` command
//!
//! This command generates a debug report for state root validation errors,
//! collecting all necessary information for bug reports as described in
//! [`INVALID_STATE_ROOT_ERROR_MESSAGE`](reth_stages::stages::merkle::INVALID_STATE_ROOT_ERROR_MESSAGE).

use crate::common::CliNodeTypes;
use alloy_consensus::BlockHeader;
use clap::Parser;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_db::DatabaseEnv;
use reth_db_api::database::Database;
use reth_db_common::DbTool;
use reth_node_builder::NodeTypesWithDBAdapter;
use reth_node_core::dirs::{ChainPath, DataDirPath};
use reth_provider::{BlockHashReader, BlockNumReader, HeaderProvider};
use serde::Serialize;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader},
    path::PathBuf,
    sync::Arc,
    time::SystemTime,
};
use tracing::info;

/// The arguments for the `reth db debug-report` command
#[derive(Parser, Debug)]
pub struct Command {
    /// The block number to generate a debug report for.
    /// This is typically the block where state root validation failed.
    #[arg(long, required = true)]
    block: u64,

    /// Include checksum of all database tables.
    /// WARNING: This will take a very long time to run!
    #[arg(long, default_value_t = false)]
    checksum: bool,

    /// Number of log lines to include from the most recent log file.
    #[arg(long, default_value_t = 100)]
    log_lines: usize,

    /// Output file path for the debug report (JSON format).
    /// If not specified, outputs to stdout.
    #[arg(long, short)]
    output: Option<PathBuf>,
}

/// Debug report structure containing all collected information
#[derive(Debug, Serialize)]
pub struct DebugReport {
    /// Report metadata
    pub metadata: ReportMetadata,
    /// Block information
    pub block_info: Option<BlockInfo>,
    /// Database statistics
    pub db_stats: Option<DbStats>,
    /// Recent log entries
    pub logs: Option<LogInfo>,
    /// Any errors encountered during report generation
    pub errors: Vec<String>,
}

/// Report metadata
#[derive(Debug, Serialize)]
pub struct ReportMetadata {
    /// Timestamp when the report was generated
    pub generated_at: String,
    /// reth version
    pub reth_version: String,
    /// Chain name
    pub chain: String,
    /// Target block number
    pub target_block: u64,
    /// Data directory path
    pub data_dir: String,
}

/// Block information
#[derive(Debug, Serialize)]
pub struct BlockInfo {
    /// Block number
    pub number: u64,
    /// Block hash
    pub hash: String,
    /// Parent hash
    pub parent_hash: String,
    /// State root from block header
    pub state_root: String,
    /// Transactions root
    pub transactions_root: String,
    /// Receipts root
    pub receipts_root: String,
    /// Timestamp
    pub timestamp: u64,
}

/// Database statistics
#[derive(Debug, Serialize)]
pub struct DbStats {
    /// Latest block number in database
    pub latest_block: Option<u64>,
    /// Table statistics
    pub tables: Vec<TableStats>,
    /// Checksums (if requested)
    pub checksums: Option<Vec<TableChecksum>>,
}

/// Statistics for a single table
#[derive(Debug, Serialize)]
pub struct TableStats {
    /// Table name
    pub name: String,
    /// Number of entries
    pub entries: u64,
    /// Size in bytes
    pub size_bytes: u64,
}

/// Checksum for a single table
#[derive(Debug, Serialize)]
pub struct TableChecksum {
    /// Table name
    pub name: String,
    /// Checksum value
    pub checksum: String,
}

/// Log information
#[derive(Debug, Serialize)]
pub struct LogInfo {
    /// Log file path
    pub log_file: Option<String>,
    /// Log entries
    pub entries: Vec<String>,
    /// Any error reading logs
    pub error: Option<String>,
}

impl Command {
    /// Execute `db debug-report` command
    pub fn execute<N: CliNodeTypes<ChainSpec: EthereumHardforks>>(
        self,
        data_dir: ChainPath<DataDirPath>,
        tool: &DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", block = self.block, "Generating debug report");

        let mut report = DebugReport {
            metadata: self.generate_metadata(&data_dir, tool)?,
            block_info: None,
            db_stats: None,
            logs: None,
            errors: Vec::new(),
        };

        // Collect block information
        match self.collect_block_info(tool) {
            Ok(info) => report.block_info = Some(info),
            Err(e) => report.errors.push(format!("Failed to collect block info: {e}")),
        }

        // Collect database statistics
        match self.collect_db_stats(tool) {
            Ok(stats) => report.db_stats = Some(stats),
            Err(e) => report.errors.push(format!("Failed to collect DB stats: {e}")),
        }

        // Collect log information
        match self.collect_logs(&data_dir) {
            Ok(logs) => report.logs = Some(logs),
            Err(e) => report.errors.push(format!("Failed to collect logs: {e}")),
        }

        // Output the report
        let json = serde_json::to_string_pretty(&report)?;

        if let Some(output_path) = &self.output {
            fs::write(output_path, &json)?;
            info!(target: "reth::cli", path = %output_path.display(), "Debug report saved");
            println!("Debug report saved to: {}", output_path.display());
        } else {
            println!("{json}");
        }

        // Print instructions
        println!("\n{}", "=".repeat(80));
        println!("DEBUG REPORT GENERATED");
        println!("{}", "=".repeat(80));
        println!("\nPlease submit this report along with your bug report at:");
        println!("https://github.com/paradigmxyz/reth/issues/new");
        println!("\nIf you haven't already, please also include:");
        println!("  - The error message you encountered");
        println!("  - Any relevant context about when the error occurred");

        Ok(())
    }

    fn generate_metadata<N: CliNodeTypes>(
        &self,
        data_dir: &ChainPath<DataDirPath>,
        tool: &DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    ) -> eyre::Result<ReportMetadata> {
        let chain_spec = tool.provider_factory.chain_spec();

        Ok(ReportMetadata {
            generated_at: humantime::format_rfc3339(SystemTime::now()).to_string(),
            reth_version: env!("CARGO_PKG_VERSION").to_string(),
            chain: chain_spec.chain().to_string(),
            target_block: self.block,
            data_dir: data_dir.data_dir().display().to_string(),
        })
    }

    fn collect_block_info<N: CliNodeTypes>(
        &self,
        tool: &DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    ) -> eyre::Result<BlockInfo> {
        let provider = tool.provider_factory.provider()?;

        let header = provider
            .header_by_number(self.block)?
            .ok_or_else(|| eyre::eyre!("Block {} not found", self.block))?;

        let hash = provider
            .block_hash(self.block)?
            .ok_or_else(|| eyre::eyre!("Block hash for {} not found", self.block))?;

        Ok(BlockInfo {
            number: self.block,
            hash: format!("{hash:?}"),
            parent_hash: format!("{:?}", header.parent_hash()),
            state_root: format!("{:?}", header.state_root()),
            transactions_root: format!("{:?}", header.transactions_root()),
            receipts_root: format!("{:?}", header.receipts_root()),
            timestamp: header.timestamp(),
        })
    }

    fn collect_db_stats<N: CliNodeTypes>(
        &self,
        tool: &DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    ) -> eyre::Result<DbStats> {
        let provider = tool.provider_factory.provider()?;
        let latest_block = provider.last_block_number().ok();

        // Collect basic table statistics
        let tables = self.collect_table_stats(tool)?;

        // Collect checksums if requested
        let checksums = if self.checksum {
            info!(target: "reth::cli", "Computing checksums (this may take a while)...");
            Some(self.collect_checksums(tool)?)
        } else {
            None
        };

        Ok(DbStats { latest_block, tables, checksums })
    }

    fn collect_table_stats<N: CliNodeTypes>(
        &self,
        tool: &DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    ) -> eyre::Result<Vec<TableStats>> {
        let mut stats = Vec::new();

        tool.provider_factory.db_ref().view(|tx| {
            use reth_db_api::Tables;

            for table in Tables::ALL {
                if let Ok(table_db) = tx.inner.open_db(Some(table.name()))
                    && let Ok(table_stats) = tx.inner.db_stat(&table_db)
                {
                    let page_size = table_stats.page_size() as u64;
                    let num_pages = (table_stats.leaf_pages()
                        + table_stats.branch_pages()
                        + table_stats.overflow_pages()) as u64;

                    stats.push(TableStats {
                        name: table.name().to_string(),
                        entries: table_stats.entries() as u64,
                        size_bytes: page_size * num_pages,
                    });
                }
            }
            Ok::<(), eyre::Report>(())
        })??;

        Ok(stats)
    }

    fn collect_checksums<N: CliNodeTypes>(
        &self,
        tool: &DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    ) -> eyre::Result<Vec<TableChecksum>> {
        use crate::db::checksum::ChecksumViewer;
        use reth_db_api::{TableViewer, Tables};

        let mut checksums = Vec::new();

        for &table in Tables::ALL {
            match ChecksumViewer::new(tool).view_rt(table) {
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

    fn collect_logs(&self, data_dir: &ChainPath<DataDirPath>) -> eyre::Result<LogInfo> {
        // Try to find log files in common locations
        let log_dir = data_dir.data_dir().join("logs");

        let mut log_info = LogInfo { log_file: None, entries: Vec::new(), error: None };

        if !log_dir.exists() {
            log_info.error = Some(format!(
                "Log directory not found at {}. \
                 Please manually include logs from your configured log directory.",
                log_dir.display()
            ));
            return Ok(log_info);
        }

        // Find the most recent log file
        let mut log_files: Vec<_> = fs::read_dir(&log_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
            .collect();

        log_files.sort_by_key(|e| {
            e.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH)
        });

        if let Some(latest_log) = log_files.last() {
            let log_path = latest_log.path();
            log_info.log_file = Some(log_path.display().to_string());

            // Read the last N lines
            match self.read_last_lines(&log_path, self.log_lines) {
                Ok(lines) => log_info.entries = lines,
                Err(e) => log_info.error = Some(format!("Failed to read log file: {e}")),
            }
        } else {
            log_info.error = Some("No log files found in log directory".to_string());
        }

        Ok(log_info)
    }

    fn read_last_lines(&self, path: &PathBuf, n: usize) -> eyre::Result<Vec<String>> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);

        // Read all lines and keep the last N
        let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();

        let start = lines.len().saturating_sub(n);
        Ok(lines[start..].to_vec())
    }
}
