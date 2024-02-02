use crate::utils::DbTool;
use clap::Parser;
use comfy_table::{Cell, Row, Table as ComfyTable};
use eyre::WrapErr;
use human_bytes::human_bytes;
use itertools::Itertools;
use reth_db::{database::Database, mdbx, snapshot::iter_snapshots, DatabaseEnv, Tables};
use reth_node_core::dirs::{ChainPath, DataDirPath};
use reth_primitives::snapshot::find_fixed_range;
use reth_provider::providers::SnapshotProvider;
use std::fs::File;

#[derive(Parser, Debug)]
/// The arguments for the `reth db stats` command
pub struct Command {
    /// Show only the total size for snapshot files.
    #[arg(long, default_value_t = false)]
    only_total_size: bool,
}

impl Command {
    /// Execute `db stats` command
    pub fn execute(
        self,
        data_dir: ChainPath<DataDirPath>,
        tool: &DbTool<DatabaseEnv>,
    ) -> eyre::Result<()> {
        let snapshots_stats_table = self.snapshots_stats_table(data_dir, self.only_total_size)?;
        println!("{snapshots_stats_table}");

        println!("\n");

        let db_stats_table = self.db_stats_table(tool)?;
        println!("{db_stats_table}");

        Ok(())
    }

    fn db_stats_table(&self, tool: &DbTool<DatabaseEnv>) -> eyre::Result<ComfyTable> {
        let mut table = ComfyTable::new();
        table.load_preset(comfy_table::presets::ASCII_MARKDOWN);
        table.set_header([
            "Table Name",
            "# Entries",
            "Branch Pages",
            "Leaf Pages",
            "Overflow Pages",
            "Total Size",
        ]);

        tool.provider_factory.db_ref().view(|tx| {
            let mut db_tables = Tables::ALL.iter().map(|table| table.name()).collect::<Vec<_>>();
            db_tables.sort();
            let mut total_size = 0;
            for db_table in db_tables {
                let table_db = tx.inner.open_db(Some(db_table)).wrap_err("Could not open db.")?;

                let stats = tx
                    .inner
                    .db_stat(&table_db)
                    .wrap_err(format!("Could not find table: {db_table}"))?;

                // Defaults to 16KB right now but we should
                // re-evaluate depending on the DB we end up using
                // (e.g. REDB does not have these options as configurable intentionally)
                let page_size = stats.page_size() as usize;
                let leaf_pages = stats.leaf_pages();
                let branch_pages = stats.branch_pages();
                let overflow_pages = stats.overflow_pages();
                let num_pages = leaf_pages + branch_pages + overflow_pages;
                let table_size = page_size * num_pages;

                total_size += table_size;
                let mut row = Row::new();
                row.add_cell(Cell::new(db_table))
                    .add_cell(Cell::new(stats.entries()))
                    .add_cell(Cell::new(branch_pages))
                    .add_cell(Cell::new(leaf_pages))
                    .add_cell(Cell::new(overflow_pages))
                    .add_cell(Cell::new(human_bytes(table_size as f64)));
                table.add_row(row);
            }

            let max_widths = table.column_max_content_widths();
            let mut seperator = Row::new();
            for width in max_widths {
                seperator.add_cell(Cell::new("-".repeat(width as usize)));
            }
            table.add_row(seperator);

            let mut row = Row::new();
            row.add_cell(Cell::new("Tables"))
                .add_cell(Cell::new(""))
                .add_cell(Cell::new(""))
                .add_cell(Cell::new(""))
                .add_cell(Cell::new(""))
                .add_cell(Cell::new(human_bytes(total_size as f64)));
            table.add_row(row);

            let freelist = tx.inner.env().freelist()?;
            let freelist_size =
                freelist * tx.inner.db_stat(&mdbx::Database::freelist_db())?.page_size() as usize;

            let mut row = Row::new();
            row.add_cell(Cell::new("Freelist"))
                .add_cell(Cell::new(freelist))
                .add_cell(Cell::new(""))
                .add_cell(Cell::new(""))
                .add_cell(Cell::new(""))
                .add_cell(Cell::new(human_bytes(freelist_size as f64)));
            table.add_row(row);

            Ok::<(), eyre::Report>(())
        })??;

        Ok(table)
    }

    fn snapshots_stats_table(
        &self,
        data_dir: ChainPath<DataDirPath>,
        only_total_size: bool,
    ) -> eyre::Result<ComfyTable> {
        let mut table = ComfyTable::new();
        table.load_preset(comfy_table::presets::ASCII_MARKDOWN);

        if !only_total_size {
            table.set_header([
                "Segment",
                "Block Range",
                "Transaction Range",
                "Shape",
                "Data Size",
                "Index Size",
                "Offsets Size",
                "Config Size",
                "Total Size",
            ]);
        } else {
            table.set_header(["Segment", "Block Range", "Transaction Range", "Shape", "Size"]);
        }

        let snapshots = iter_snapshots(data_dir.snapshots_path())?;
        let snapshot_provider = SnapshotProvider::new(data_dir.snapshots_path())?;

        let mut total_data_size = 0;
        let mut total_index_size = 0;
        let mut total_offsets_size = 0;
        let mut total_config_size = 0;

        for (segment, ranges) in snapshots.into_iter().sorted_by_key(|(segment, _)| *segment) {
            for (block_range, tx_range) in ranges {
                let fixed_block_range = find_fixed_range(*block_range.start());
                let jar_provider = snapshot_provider
                    .get_segment_provider(segment, || Some(fixed_block_range.clone()), None)?
                    .expect("something went wrong");

                let columns = jar_provider.columns();
                let rows = jar_provider.rows();
                let data_size = File::open(jar_provider.data_path())
                    .and_then(|file| file.metadata())
                    .map(|metadata| metadata.len())
                    .unwrap_or_default();
                let index_size = File::open(jar_provider.index_path())
                    .and_then(|file| file.metadata())
                    .map(|metadata| metadata.len())
                    .unwrap_or_default();
                let offsets_size = File::open(jar_provider.offsets_path())
                    .and_then(|file| file.metadata())
                    .map(|metadata| metadata.len())
                    .unwrap_or_default();
                let config_size = File::open(jar_provider.config_path())
                    .and_then(|file| file.metadata())
                    .map(|metadata| metadata.len())
                    .unwrap_or_default();

                let mut row = Row::new();
                row.add_cell(Cell::new(segment))
                    .add_cell(Cell::new(format!("{block_range:?}")))
                    .add_cell(Cell::new(
                        tx_range.map_or("N/A".to_string(), |tx_range| format!("{tx_range:?}")),
                    ))
                    .add_cell(Cell::new(format!("{columns} x {rows}")));
                if !only_total_size {
                    row.add_cell(Cell::new(human_bytes(data_size as f64)))
                        .add_cell(Cell::new(human_bytes(index_size as f64)))
                        .add_cell(Cell::new(human_bytes(offsets_size as f64)))
                        .add_cell(Cell::new(human_bytes(config_size as f64)));
                }
                row.add_cell(Cell::new(human_bytes(
                    (data_size + index_size + offsets_size + config_size) as f64,
                )));
                table.add_row(row);

                total_data_size += data_size;
                total_index_size += index_size;
                total_offsets_size += offsets_size;
                total_config_size += config_size;
            }
        }

        let max_widths = table.column_max_content_widths();
        let mut seperator = Row::new();
        for width in max_widths {
            seperator.add_cell(Cell::new("-".repeat(width as usize)));
        }
        table.add_row(seperator);

        let mut row = Row::new();
        row.add_cell(Cell::new("Total"))
            .add_cell(Cell::new(""))
            .add_cell(Cell::new(""))
            .add_cell(Cell::new(""));
        if !only_total_size {
            row.add_cell(Cell::new(human_bytes(total_data_size as f64)))
                .add_cell(Cell::new(human_bytes(total_index_size as f64)))
                .add_cell(Cell::new(human_bytes(total_offsets_size as f64)))
                .add_cell(Cell::new(human_bytes(total_config_size as f64)));
        }
        row.add_cell(Cell::new(human_bytes(
            (total_data_size + total_index_size + total_offsets_size + total_config_size) as f64,
        )));
        table.add_row(row);

        Ok(table)
    }
}
