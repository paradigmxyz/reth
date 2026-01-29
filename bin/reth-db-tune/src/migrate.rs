use crate::config::{parse_size, TuningConfig};
use eyre::{bail, Context, Result};
use rocksdb::{
    BlockBasedOptions, ColumnFamilyDescriptor, DBCompressionType, Options, WriteBatch, DB,
};
use serde::Serialize;
use std::{fs, path::Path, time::Instant};
use tracing::{info, warn};

const BATCH_SIZE: usize = 32 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub(crate) struct MigrationReport {
    pub source_path: String,
    pub destination_path: String,
    pub column_families: Vec<CfMigrationStats>,
    pub total_duration_secs: f64,
    pub compact_performed: bool,
    pub verify_performed: bool,
    pub verify_passed: Option<bool>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CfMigrationStats {
    pub name: String,
    pub keys_copied: u64,
    pub bytes_copied: u64,
    pub duration_secs: f64,
}

pub(crate) struct MigrateOptions<'a> {
    pub src: &'a Path,
    pub dst: &'a Path,
    pub config: &'a TuningConfig,
    pub jobs_override: Option<i32>,
    pub compact: bool,
    pub verify: bool,
}

pub(crate) fn run_migration(opts: MigrateOptions<'_>) -> Result<MigrationReport> {
    let start = Instant::now();

    let src_opts = Options::default();
    let cf_names = DB::list_cf(&src_opts, opts.src)
        .wrap_err_with(|| format!("failed to list column families from {:?}", opts.src))?;

    info!(cfs = ?cf_names, "discovered column families");

    let src_cf_descriptors: Vec<ColumnFamilyDescriptor> = cf_names
        .iter()
        .map(|name| ColumnFamilyDescriptor::new(name.clone(), Options::default()))
        .collect();

    let src_db = DB::open_cf_descriptors_read_only(&src_opts, opts.src, src_cf_descriptors, false)
        .wrap_err("failed to open source database")?;

    if !opts.dst.exists() {
        fs::create_dir_all(opts.dst)?;
    }

    let dst_db = open_destination_db(opts.dst, &cf_names, opts.config, opts.jobs_override)?;

    let mut cf_stats = Vec::new();

    for cf_name in &cf_names {
        let stats = copy_column_family(&src_db, &dst_db, cf_name)?;
        cf_stats.push(stats);
    }

    drop(src_db);

    if opts.compact {
        info!("running compaction on destination database");
        for cf_name in &cf_names {
            if let Some(cf) = dst_db.cf_handle(cf_name) {
                dst_db.compact_range_cf(cf, None::<&[u8]>, None::<&[u8]>);
            }
        }
    }

    let verify_passed = if opts.verify {
        info!("verifying migration");
        let src_db_verify = DB::open_cf_descriptors_read_only(
            &src_opts,
            opts.src,
            cf_names
                .iter()
                .map(|name| ColumnFamilyDescriptor::new(name.clone(), Options::default()))
                .collect::<Vec<_>>(),
            false,
        )?;
        Some(verify_databases(&src_db_verify, &dst_db, &cf_names)?)
    } else {
        None
    };

    let report = MigrationReport {
        source_path: opts.src.display().to_string(),
        destination_path: opts.dst.display().to_string(),
        column_families: cf_stats,
        total_duration_secs: start.elapsed().as_secs_f64(),
        compact_performed: opts.compact,
        verify_performed: opts.verify,
        verify_passed,
    };

    let report_path = opts.dst.join("migration_report.json");
    let report_json = serde_json::to_string_pretty(&report)?;
    fs::write(&report_path, &report_json)?;
    info!(path = %report_path.display(), "wrote migration report");

    Ok(report)
}

fn open_destination_db(
    path: &Path,
    cf_names: &[String],
    config: &TuningConfig,
    jobs_override: Option<i32>,
) -> Result<DB> {
    let mut db_opts = Options::default();
    db_opts.create_if_missing(true);
    db_opts.create_missing_column_families(true);

    let jobs = jobs_override.or(config.default.max_background_jobs).unwrap_or(6);
    db_opts.set_max_background_jobs(jobs);

    let cf_descriptors: Vec<ColumnFamilyDescriptor> = cf_names
        .iter()
        .map(|name| {
            let cf_config = config.get_cf_options(name);
            let cf_opts = build_cf_options(&cf_config);
            ColumnFamilyDescriptor::new(name.clone(), cf_opts)
        })
        .collect();

    DB::open_cf_descriptors(&db_opts, path, cf_descriptors).wrap_err("failed to open destination")
}

fn build_cf_options(cf_config: &crate::config::CfOptions) -> Options {
    let mut opts = Options::default();

    if let Some(ref size_str) = cf_config.write_buffer_size &&
        let Ok(size) = parse_size(size_str)
    {
        opts.set_write_buffer_size(size);
    }

    if let Some(n) = cf_config.max_write_buffer_number {
        opts.set_max_write_buffer_number(n);
    }

    if let Some(ct) = cf_config.compression {
        opts.set_compression_type(ct.into());
    } else {
        opts.set_compression_type(DBCompressionType::Lz4);
    }

    let mut block_opts = BlockBasedOptions::default();

    if let Some(ref size_str) = cf_config.block_size &&
        let Ok(size) = parse_size(size_str)
    {
        block_opts.set_block_size(size);
    }

    if let Some(bits) = cf_config.bloom_filter_bits {
        block_opts.set_bloom_filter(bits as f64, false);
    }

    opts.set_block_based_table_factory(&block_opts);
    opts
}

fn copy_column_family(src_db: &DB, dst_db: &DB, cf_name: &str) -> Result<CfMigrationStats> {
    let start = Instant::now();
    let mut keys_copied = 0u64;
    let mut bytes_copied = 0u64;

    let src_cf =
        src_db.cf_handle(cf_name).ok_or_else(|| eyre::eyre!("source CF {} not found", cf_name))?;
    let dst_cf = dst_db
        .cf_handle(cf_name)
        .ok_or_else(|| eyre::eyre!("destination CF {} not found", cf_name))?;

    let mut batch = WriteBatch::default();
    let mut batch_bytes = 0usize;

    let iter = src_db.iterator_cf(src_cf, rocksdb::IteratorMode::Start);

    for item in iter {
        let (key, value) = item?;
        let entry_size = key.len() + value.len();

        batch.put_cf(dst_cf, &key, &value);
        batch_bytes += entry_size;
        keys_copied += 1;
        bytes_copied += entry_size as u64;

        if batch_bytes >= BATCH_SIZE {
            dst_db.write(batch)?;
            batch = WriteBatch::default();
            batch_bytes = 0;

            if keys_copied.is_multiple_of(1_000_000) {
                info!(cf = cf_name, keys = keys_copied, "migration progress");
            }
        }
    }

    if !batch.is_empty() {
        dst_db.write(batch)?;
    }

    let duration = start.elapsed();
    info!(
        cf = cf_name,
        keys = keys_copied,
        bytes = bytes_copied,
        duration_secs = duration.as_secs_f64(),
        "completed CF migration"
    );

    Ok(CfMigrationStats {
        name: cf_name.to_string(),
        keys_copied,
        bytes_copied,
        duration_secs: duration.as_secs_f64(),
    })
}

fn verify_databases(src_db: &DB, dst_db: &DB, cf_names: &[String]) -> Result<bool> {
    for cf_name in cf_names {
        let src_cf = match src_db.cf_handle(cf_name) {
            Some(cf) => cf,
            None => continue,
        };
        let dst_cf = match dst_db.cf_handle(cf_name) {
            Some(cf) => cf,
            None => {
                warn!(cf = cf_name, "CF missing in destination");
                return Ok(false);
            }
        };

        let mut src_iter = src_db.iterator_cf(src_cf, rocksdb::IteratorMode::Start);
        let mut dst_iter = dst_db.iterator_cf(dst_cf, rocksdb::IteratorMode::Start);

        loop {
            match (src_iter.next(), dst_iter.next()) {
                (Some(Ok((sk, sv))), Some(Ok((dk, dv)))) => {
                    if sk != dk || sv != dv {
                        warn!(cf = cf_name, "key/value mismatch");
                        return Ok(false);
                    }
                }
                (None, None) => break,
                (Some(_), None) => {
                    warn!(cf = cf_name, "destination has fewer keys");
                    return Ok(false);
                }
                (None, Some(_)) => {
                    warn!(cf = cf_name, "destination has more keys");
                    return Ok(false);
                }
                (Some(Err(e)), _) | (_, Some(Err(e))) => {
                    bail!("iterator error during verification: {}", e);
                }
            }
        }

        info!(cf = cf_name, "verification passed");
    }

    Ok(true)
}
