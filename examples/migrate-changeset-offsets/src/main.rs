//! Migration tool to convert static file changeset offsets from inline `Vec<ChangesetOffset>`
//! (old format) to sidecar `.csoff` files (new format from PR #21596).
//!
//! # What changed
//!
//! **Old format**: `SegmentHeader` contains `changeset_offsets: Option<Vec<ChangesetOffset>>`
//! inline in the `.conf` file.
//!
//! **New format**: `SegmentHeader` contains `changeset_offsets_len: u64` and the actual offsets
//! are stored in a separate `.csoff` sidecar file with fixed-width 16-byte records.
//!
//! # Usage
//!
//! ```bash
//! cargo run -p example-migrate-changeset-offsets --release -- /path/to/static_files
//! ```
//!
//! # What it does
//!
//! For each changeset static file (AccountChangeSets, StorageChangeSets):
//! 1. Reads the `.conf` file containing the old `SegmentHeader` with inline offsets
//! 2. Extracts the `Vec<ChangesetOffset>` from the header
//! 3. Writes a new `.csoff` sidecar file with the offsets in fixed-width binary format
//! 4. Rewrites the `.conf` file with `changeset_offsets_len` instead of the inline vector
//! 5. Creates a `.conf.bak` backup of the original config

use clap::Parser;
use eyre::{Context, Result};
use reth_nippy_jar::{compression::Compressors, CONFIG_FILE_EXTENSION};
use reth_static_file_types::{
    ChangesetOffset, ChangesetOffsetWriter, SegmentRangeInclusive, StaticFileSegment,
};
use serde::{
    de::{SeqAccess, Visitor},
    ser::SerializeStruct,
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Extension for changeset offset sidecar files
const CSOFF_EXTENSION: &str = "csoff";

#[derive(Parser, Debug)]
#[command(name = "migrate-changeset-offsets")]
#[command(about = "Migrate static file changeset offsets from inline Vec to sidecar .csoff files")]
struct Args {
    /// Path to the static files directory
    #[arg(required = true)]
    static_files_dir: PathBuf,

    /// Dry run - only show what would be done without making changes
    #[arg(long, short = 'n')]
    dry_run: bool,

    /// Skip backup creation
    #[arg(long)]
    no_backup: bool,
}

// ============================================================================
// Old format types (for reading existing files with inline `Vec<ChangesetOffset>`)
// ============================================================================

/// Old format SegmentHeader with inline `Vec<ChangesetOffset>`
#[derive(Debug, Clone)]
struct OldSegmentHeader {
    expected_block_range: SegmentRangeInclusive,
    block_range: Option<SegmentRangeInclusive>,
    tx_range: Option<SegmentRangeInclusive>,
    segment: StaticFileSegment,
    changeset_offsets: Option<Vec<ChangesetOffset>>,
}

struct OldSegmentHeaderVisitor;

impl<'de> Visitor<'de> for OldSegmentHeaderVisitor {
    type Value = OldSegmentHeader;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a header struct with 4 or 5 fields")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let expected_block_range =
            seq.next_element()?.ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
        let block_range =
            seq.next_element()?.ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
        let tx_range =
            seq.next_element()?.ok_or_else(|| serde::de::Error::invalid_length(2, &self))?;
        let segment: StaticFileSegment =
            seq.next_element()?.ok_or_else(|| serde::de::Error::invalid_length(3, &self))?;

        let changeset_offsets = if segment.is_change_based() {
            // Old format: Option<Vec<ChangesetOffset>>
            match seq.next_element::<Option<Vec<ChangesetOffset>>>()? {
                Some(offsets) => offsets,
                None => {
                    return Err(serde::de::Error::custom(
                        "changeset_offsets should exist for changeset static files",
                    ))
                }
            }
        } else {
            None
        };

        Ok(OldSegmentHeader {
            expected_block_range,
            block_range,
            tx_range,
            segment,
            changeset_offsets,
        })
    }
}

impl<'de> Deserialize<'de> for OldSegmentHeader {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        const FIELDS: &[&str] =
            &["expected_block_range", "block_range", "tx_range", "segment", "changeset_offsets"];
        deserializer.deserialize_struct("SegmentHeader", FIELDS, OldSegmentHeaderVisitor)
    }
}

/// Old format NippyJar with OldSegmentHeader
/// Uses the real Compressors type from reth_nippy_jar
#[derive(Debug, Deserialize)]
struct OldNippyJar {
    version: usize,
    user_header: OldSegmentHeader,
    columns: usize,
    rows: usize,
    compressor: Option<Compressors>,
    max_row_size: usize,
}

// ============================================================================
// New format types (for writing migrated files with changeset_offsets_len: u64)
// ============================================================================

/// New format SegmentHeader with changeset_offsets_len instead of inline Vec
#[derive(Debug, Clone)]
struct NewSegmentHeader {
    expected_block_range: SegmentRangeInclusive,
    block_range: Option<SegmentRangeInclusive>,
    tx_range: Option<SegmentRangeInclusive>,
    segment: StaticFileSegment,
    changeset_offsets_len: u64,
}

impl Serialize for NewSegmentHeader {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let len = if self.segment.is_change_based() { 5 } else { 4 };
        let mut state = serializer.serialize_struct("SegmentHeader", len)?;
        state.serialize_field("expected_block_range", &self.expected_block_range)?;
        state.serialize_field("block_range", &self.block_range)?;
        state.serialize_field("tx_range", &self.tx_range)?;
        state.serialize_field("segment", &self.segment)?;
        if self.segment.is_change_based() {
            state.serialize_field("changeset_offsets", &self.changeset_offsets_len)?;
        }
        state.end()
    }
}

/// New format NippyJar with NewSegmentHeader
/// Uses the real Compressors type from reth_nippy_jar
#[derive(Debug, Serialize)]
struct NewNippyJar {
    version: usize,
    user_header: NewSegmentHeader,
    columns: usize,
    rows: usize,
    compressor: Option<Compressors>,
    max_row_size: usize,
}

// ============================================================================
// Migration logic
// ============================================================================

fn find_changeset_configs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut configs = Vec::new();

    for entry in fs::read_dir(dir).wrap_err("Failed to read static files directory")? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() &&
            let Some(ext) = path.extension() &&
            ext == CONFIG_FILE_EXTENSION
        {
            let file_name = path.file_stem().unwrap_or_default().to_string_lossy();
            if file_name.contains("account-change-sets") ||
                file_name.contains("storage-change-sets")
            {
                configs.push(path);
            }
        }
    }

    configs.sort();
    Ok(configs)
}

fn write_csoff_file(path: &Path, offsets: &[ChangesetOffset]) -> Result<()> {
    // Create new file with committed_len=0 (fresh file)
    let mut writer = ChangesetOffsetWriter::new(path, 0)
        .wrap_err_with(|| format!("Failed to create csoff file: {}", path.display()))?;

    writer.append_many(offsets).wrap_err("Failed to write offsets")?;
    writer.sync().wrap_err("Failed to sync csoff file")?;
    Ok(())
}

#[derive(Debug)]
enum MigrateResult {
    Migrated,
    AlreadyMigrated,
    Skipped,
    DryRun,
}

fn migrate_config(config_path: &Path, dry_run: bool, no_backup: bool) -> Result<MigrateResult> {
    println!("Processing: {}", config_path.display());

    let config_data = fs::read(config_path)
        .wrap_err_with(|| format!("Failed to read config: {}", config_path.display()))?;

    // Try to deserialize as old format (with inline Vec<ChangesetOffset>)
    let old_jar: OldNippyJar = match bincode::deserialize(&config_data) {
        Ok(jar) => jar,
        Err(e) => {
            // Check if it's already in new format by trying to load as SegmentHeader
            // which now expects changeset_offsets_len: u64
            println!("  Could not parse as old format: {e}");
            println!("  This file may already be migrated or is corrupted.");
            return Ok(MigrateResult::AlreadyMigrated);
        }
    };

    let old_header = &old_jar.user_header;

    if !old_header.segment.is_change_based() {
        println!("  Not a changeset segment: {:?}", old_header.segment);
        return Ok(MigrateResult::Skipped);
    }

    // Check if there are offsets to migrate
    let offsets = match &old_header.changeset_offsets {
        Some(offsets) if !offsets.is_empty() => offsets,
        _ => {
            println!("  No offsets to migrate (empty or already migrated)");
            return Ok(MigrateResult::AlreadyMigrated);
        }
    };

    println!("  Found {} changeset offsets to migrate", offsets.len());

    if dry_run {
        println!("  [DRY RUN] Would create .csoff file with {} records", offsets.len());
        println!("  [DRY RUN] Would update config with changeset_offsets_len = {}", offsets.len());
        return Ok(MigrateResult::DryRun);
    }

    // Determine paths - the .conf extension is on the full filename, not a separate extension
    // e.g., static_file_account-change-sets_0_499999.conf ->
    // static_file_account-change-sets_0_499999.csoff
    let csoff_path = config_path.with_extension(CSOFF_EXTENSION);

    // Write the .csoff sidecar file using the real ChangesetOffsetWriter
    write_csoff_file(&csoff_path, offsets)?;
    println!("  Written {} records to {}", offsets.len(), csoff_path.display());

    // Create new header with changeset_offsets_len
    let new_header = NewSegmentHeader {
        expected_block_range: old_header.expected_block_range,
        block_range: old_header.block_range,
        tx_range: old_header.tx_range,
        segment: old_header.segment,
        changeset_offsets_len: offsets.len() as u64,
    };

    let new_jar = NewNippyJar {
        version: old_jar.version,
        user_header: new_header,
        columns: old_jar.columns,
        rows: old_jar.rows,
        compressor: old_jar.compressor,
        max_row_size: old_jar.max_row_size,
    };

    // Backup old config
    if !no_backup {
        let backup_path = config_path.with_extension("conf.bak");
        fs::copy(config_path, &backup_path)
            .wrap_err_with(|| format!("Failed to create backup: {}", backup_path.display()))?;
        println!("  Backed up config to {}", backup_path.display());
    }

    // Write new config
    let new_config_data =
        bincode::serialize(&new_jar).wrap_err("Failed to serialize new config")?;
    fs::write(config_path, new_config_data)
        .wrap_err_with(|| format!("Failed to write new config: {}", config_path.display()))?;
    println!("  Updated config file");

    Ok(MigrateResult::Migrated)
}

fn main() -> Result<()> {
    let args = Args::parse();

    if !args.static_files_dir.exists() {
        eyre::bail!("Directory does not exist: {}", args.static_files_dir.display());
    }

    println!("Scanning for changeset static files in: {}", args.static_files_dir.display());
    if args.dry_run {
        println!("[DRY RUN MODE] No changes will be made");
    }
    println!();

    let configs = find_changeset_configs(&args.static_files_dir)?;

    if configs.is_empty() {
        println!("No changeset static file configs found.");
        return Ok(());
    }

    println!("Found {} changeset static file configs to process\n", configs.len());

    let mut migrated = 0;
    let mut already_migrated = 0;
    let mut skipped = 0;
    let mut errors = 0;

    for config_path in configs {
        match migrate_config(&config_path, args.dry_run, args.no_backup) {
            Ok(MigrateResult::Migrated | MigrateResult::DryRun) => migrated += 1,
            Ok(MigrateResult::AlreadyMigrated) => already_migrated += 1,
            Ok(MigrateResult::Skipped) => skipped += 1,
            Err(e) => {
                eprintln!("  Error: {e:#}");
                errors += 1;
            }
        }
        println!();
    }

    println!("Migration complete:");
    println!("  Migrated: {migrated}");
    println!("  Already migrated: {already_migrated}");
    println!("  Skipped: {skipped}");
    println!("  Errors: {errors}");

    if errors > 0 {
        eyre::bail!("{errors} files failed to migrate");
    }

    Ok(())
}
