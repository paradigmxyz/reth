//! Database migration tool
//!
//! Copies data from one MDBX database to another, table by table.
//! Allows customization of database parameters (page size, max size, growth step, etc.).

use clap::Parser;
use reth_db::DatabaseEnv;
use reth_db_api::{database::Database, transaction::DbTx};
use reth_libmdbx::WriteFlags;
use reth_node_core::args::{parse_byte_size, ByteSize};
use std::{path::PathBuf, time::Instant};
use tracing::info;

/// Format byte size to human-readable string
fn format_byte_size(bytes: usize) -> String {
    ByteSize(bytes).to_string()
}

/// Arguments for the `reth db migrate` command
#[derive(Parser, Debug)]
#[command(next_help_heading = "Copy Options")]
pub struct Command {
    /// Destination database directory (must not exist).
    #[arg(long, value_name = "DEST_PATH")]
    to: PathBuf,

    /// Specific tables to copy (comma-separated). Example: --tables Headers,Bodies
    #[arg(long, value_delimiter = ',')]
    tables: Vec<String>,

    /// Page size (e.g., 4KB, 8KB). Default: system default (typically 4KB).
    /// NOTE: Can only be set when creating a new database.
    #[arg(long, value_parser = parse_byte_size, verbatim_doc_comment)]
    page_size: Option<usize>,

    /// Maximum database size (e.g., 4TB, 12TB). Default: source database size.
    #[arg(long, value_parser = parse_byte_size)]
    max_size: Option<usize>,

    /// Database growth step (e.g., 4GB, 8GB).
    #[arg(long, default_value = "4GB", value_parser = parse_byte_size)]
    growth_step: usize,

    /// Commit every N records. Smaller = less memory, slower.
    #[arg(long, default_value = "100000")]
    commit_every: usize,

    /// Suppress progress messages.
    #[arg(long, short)]
    quiet: bool,
}

impl Command {
    /// Execute the database migration
    pub fn execute(
        &self,
        src_env: &DatabaseEnv,
        db_args: &reth_db::mdbx::DatabaseArguments,
    ) -> eyre::Result<()> {
        // Ensure destination doesn't exist
        if self.to.exists() {
            eyre::bail!("Destination {:?} already exists", self.to);
        }

        // Ensure parent directory exists
        if let Some(parent) = self.to.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        if !self.quiet {
            info!(target: "reth::cli", "Starting database migration...");
            info!(target: "reth::cli", "Destination: {:?}", self.to);
        }

        let start = Instant::now();
        self.execute_custom_copy(src_env, db_args)?;

        let elapsed = start.elapsed();

        if !self.quiet {
            info!(target: "reth::cli", "Copy completed in {:.2}s", elapsed.as_secs_f64());
            
            // Display size comparison
            let src_size = src_env.info()?.map_size();
            if let Ok(dst_metadata) = std::fs::metadata(&self.to.join("mdbx.dat")) {
                let dst_size = dst_metadata.len() as usize;
                info!(target: "reth::cli", "Source database map size: {}", format_byte_size(src_size));
                info!(target: "reth::cli", "Destination file size: {}", format_byte_size(dst_size));
                
                if dst_size < src_size {
                    let reduction = ((src_size - dst_size) as f64 / src_size as f64) * 100.0;
                    info!(target: "reth::cli", 
                          "Size reduction: {} ({:.2}% smaller due to defragmentation)",
                          format_byte_size(src_size - dst_size),
                          reduction);
                    info!(target: "reth::cli", 
                          "  Note: This is normal. The copy process eliminates fragmentation,");
                    info!(target: "reth::cli", 
                          "  empty pages, and compacts the data structure.");
                }
            }
        }

        Ok(())
    }

    /// Execute database copy with parameter customization
    fn execute_custom_copy(
        &self,
        src_env: &DatabaseEnv,
        base_db_args: &reth_db::mdbx::DatabaseArguments,
    ) -> eyre::Result<()> {
        use reth_db::tables::Tables;
        
        // Get source database parameters for display
        let src_info = src_env.info()?;
        let src_stat = src_env.stat()?;
        let src_page_size = src_stat.page_size();
        let src_map_size = src_info.map_size();
        
        // Start with system database arguments (includes log_level, exclusive, max_readers, etc.)
        // then override with user-specified parameters
        let mut dst_args = base_db_args.clone();
        
        // Determine target parameters
        // Priority: user specified > source database
        let max_size_bytes = self.max_size.unwrap_or(src_map_size);
        let growth_step_bytes = self.growth_step;
        
        dst_args = dst_args
            .with_geometry_max_size(Some(max_size_bytes))
            .with_growth_step(Some(growth_step_bytes));
        
        // Override page size if user specified it
        if let Some(page_size) = self.page_size {
            dst_args = dst_args.with_page_size(Some(page_size));
        }

        if !self.quiet {
            info!(target: "reth::cli", "Source database parameters:");
            info!(target: "reth::cli", "  Page size: {}", format_byte_size(src_page_size as usize));
            info!(target: "reth::cli", "  Map size: {}", format_byte_size(src_map_size));
            info!(target: "reth::cli", "Target database parameters:");
            if let Some(page_size) = self.page_size {
                info!(target: "reth::cli", "  Page size: {} (custom)", format_byte_size(page_size));
            } else {
                info!(target: "reth::cli", "  Page size: {} (using system default)", 
                      format_byte_size(src_page_size as usize));
            }
            info!(target: "reth::cli", "  Map size: {}", format_byte_size(max_size_bytes));
            info!(target: "reth::cli", "  Growth step: {}", format_byte_size(growth_step_bytes));
            info!(target: "reth::cli", "  (Other settings: log_level, exclusive, max_readers, etc. inherited from system config)");
        }

        // Create destination database and initialize all tables
        // Using init_db() to properly create all tables and record client version
        let dst_env = reth_db::init_db(&self.to, dst_args)?;

        if !self.quiet {
            info!(target: "reth::cli", "Destination database initialized");
        }

        // Determine which tables to copy
        let tables_to_copy: Vec<String> = if self.tables.is_empty() {
            Tables::ALL.iter().map(|t| t.name().to_string()).collect()
        } else {
            // Validate table names
            let valid_tables: std::collections::HashSet<&str> = 
                Tables::ALL.iter().map(|t| t.name()).collect();
            
            for table in &self.tables {
                if !valid_tables.contains(table.as_str()) {
                    eyre::bail!("Unknown table: {}", table);
                }
            }
            
            self.tables.clone()
        };

        if !self.quiet {
            info!(target: "reth::cli", "Copying {} tables", tables_to_copy.len());
            if !self.tables.is_empty() {
                info!(target: "reth::cli", "  Note: Only copying selected tables. Other tables will be empty.");
            }
        }

        // Copy each table using table-specific implementations
        let total_tables = tables_to_copy.len();
        for (idx, table_name) in tables_to_copy.iter().enumerate() {
            if !self.quiet {
                info!(target: "reth::cli", "[{}/{}] Copying table: {}", 
                      idx + 1, total_tables, table_name);
            }
            
            self.copy_table_generic(src_env, &dst_env, table_name)?;
        }

        Ok(())
    }

    /// Copy a table using generic byte-level copying
    /// This works for all tables but doesn't validate table-specific types
    fn copy_table_generic(
        &self,
        src_env: &DatabaseEnv,
        dst_env: &DatabaseEnv,
        table_name: &str,
    ) -> eyre::Result<usize> {
        let mut src_tx = src_env.tx()?;
        
        // Disable timeout for long-running read transaction during copy
        // This is necessary because copying large tables can take a very long time
        src_tx.disable_long_read_transaction_safety();
        
        let mut dst_tx = dst_env.tx_mut()?;
        
        // Open the databases (tables) by name
        // Source: read-only, use open_db() - table must exist
        let src_db = src_tx.inner.open_db(Some(table_name))?;
        // Destination: tables are already created by init_db(), just open them
        let dst_db = dst_tx.inner.open_db(Some(table_name))?;
        
        // Clear destination table before copying
        // This is necessary because:
        // 1. init_db() may have pre-populated some tables (e.g., VersionHistory)
        // 2. APPEND flag requires an empty table or strictly ordered keys
        dst_tx.inner.clear_db(dst_db.dbi())?;
        
        // Get total number of entries for progress calculation
        let total_entries = src_tx.inner.db_stat(&src_db)?.entries();
        
        if !self.quiet {
            info!(
                target: "reth::cli",
                "  Starting copy of table '{}' ({} records)",
                table_name,
                total_entries
            );
        }
        
        // Get cursor for source and destination
        let src_cursor = src_tx.inner.cursor(&src_db)?;
        let mut dst_cursor = dst_tx.inner.cursor(&dst_db)?;
        
        let mut copied = 0usize;
        let mut batch_count = 0usize;
        let mut last_progress = Instant::now();
        let start_time = Instant::now();
        
        // Iterate through all records as byte slices
        for item in src_cursor.iter_slices() {
            let (key, value) = item?;
            
            // Insert into destination (convert Cow to slice)
            // Use APPEND flag for better performance (assumes ordered insert)
            dst_tx.inner.put(dst_db.dbi(), &key, &value, WriteFlags::APPEND)?;
            copied += 1;
            batch_count += 1;
            
            // Periodic commit
            if batch_count >= self.commit_every {
                drop(dst_cursor);
                dst_tx.commit()?;
                
                // Start new transaction
                dst_tx = dst_env.tx_mut()?;
                // Re-open destination table (already created, but need handle in new transaction)
                let dst_db = dst_tx.inner.open_db(Some(table_name))?;
                dst_cursor = dst_tx.inner.cursor(&dst_db)?;
                batch_count = 0;
                
                // Progress logging
                if !self.quiet && last_progress.elapsed().as_secs() >= 5 {
                    let percentage = if total_entries > 0 {
                        (copied as f64 / total_entries as f64 * 100.0).min(100.0)
                    } else {
                        0.0
                    };
                    info!(
                        target: "reth::cli", 
                        "    Progress: {}/{} records ({:.2}%)", 
                        copied, 
                        total_entries,
                        percentage
                    );
                    last_progress = Instant::now();
                }
            }
        }
        
        // Final commit
        if batch_count > 0 {
            drop(dst_cursor);
            dst_tx.commit()?;
        }
        
        // Log completion
        if !self.quiet {
            let elapsed = start_time.elapsed();
            let rate = if elapsed.as_secs() > 0 {
                copied as f64 / elapsed.as_secs() as f64
            } else {
                copied as f64
            };
            
            if copied == 0 {
                info!(
                    target: "reth::cli",
                    "  Completed table '{}': empty table",
                    table_name
                );
            } else {
                info!(
                    target: "reth::cli",
                    "  Completed table '{}': {} records in {:.2}s ({:.0} records/sec)",
                    table_name,
                    copied,
                    elapsed.as_secs_f64(),
                    rate
                );
            }
        }
        
        Ok(copied)
    }
}
