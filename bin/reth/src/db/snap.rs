use super::tui::DbListTUI;
use crate::utils::{DbTool, ListFilter};
use clap::{clap_derive::ValueEnum, Parser};
use eyre::WrapErr;
use reth_db::{
    database::Database,
    snapshot::{create_snapshot_T1_T2_T3_T4_T5},
    table::Table,
    tables,
    transaction::DbTx,
    DatabaseEnvRO, TableType, TableViewer, Tables,
};
use reth_nippy_jar::NippyJar;
use reth_primitives::BlockNumber;
use std::{cell::RefCell, path::PathBuf};
use tracing::error;

#[derive(Debug, Clone, ValueEnum)]
pub enum Snapshots {
    Blocks,
    Transactions,
    Receipts,
}

#[derive(Parser, Debug)]
/// The arguments for the `reth db list` command
pub struct Command {
    modes: Snapshots,
    #[arg(long, short, default_value = "0")]
    from: usize,
    #[arg(long, short, default_value = "500000")]
    block_interval: usize,
}

impl Command {
    /// Execute `db list` command
    pub fn execute(self, tool: &DbTool<'_, DatabaseEnvRO>) -> eyre::Result<()> {
        let snap_file: PathBuf = "snapshot".into();

        let table_rows = match &self.modes {
            Snapshots::Blocks => tables::Headers::NAME,
            Snapshots::Transactions | Snapshots::Receipts => tables::Transactions::NAME,
        };

        let row_count = tool.db.view(|tx| {
            let table_db = tx.inner.open_db(Some(table_rows)).wrap_err("Could not open db.")?;
            let stats = tx
                .inner
                .db_stat(&table_db)
                .wrap_err(format!("Could not find table: {}", table_rows))?;

            Ok::<usize, eyre::Error>(stats.entries())
        })??;

        assert!(row_count > 0, "No rows on database.");

        let with_compression = true;
        let with_filter = true;

        let mut nippy_jar = NippyJar::new_without_header(5, snap_file.as_path());

        if with_compression {
            nippy_jar = nippy_jar.with_zstd(false, 0);
        }

        if with_filter {
            nippy_jar = nippy_jar.with_cuckoo_filter(row_count as usize + 10).with_mphf();
        }

        tool.db.view(|tx| {
            use reth_db::cursor::DbCursorRO;
            use tables::*;
            let row_count = 500_000;

            // Generate list of hashes for filters & PHF
            let mut cursor = tx.cursor_read::<RawTable<CanonicalHeaders>>().unwrap();
            let hashes = cursor
                .walk(None)
                .unwrap()
                .map(|row| row.map(|(_key, value)| value.into_value()).map_err(|e| e.into()));

            // Hacky type inference. TODO fix
            let mut none_vec = Some(vec![vec![vec![0u8]].into_iter()]);
            let _ = none_vec.take();

            match &self.modes {
                Snapshots::Blocks => {
                    create_snapshot_T1_T2_T3_T4_T5::<
                        HeaderTD,
                        Headers,
                        BlockBodyIndices,
                        BlockOmmers,
                        BlockWithdrawals,
                        BlockNumber,
                    >(
                        tx, 0..=(500_000 - 1), none_vec, Some(hashes), row_count, &mut nippy_jar
                    )
                }
                Snapshots::Transactions => todo!(),
                Snapshots::Receipts => todo!(),
            }
        })??;

        // // Hacky type inference. TODO fix
        // let mut none_vec = Some(vec![vec![vec![0u8]].into_iter()]);
        // let _ = none_vec.take();

        // // Generate list of hashes for filters & PHF
        // let mut cursor = tx.cursor_read::<RawTable<CanonicalHeaders>>().unwrap();
        // let hashes = cursor
        //     .walk(None)
        //     .unwrap()
        //     .map(|row| row.map(|(_key, value)| value.into_value()).map_err(|e| e.into()));

        // create_snapshot_T1_T2::<Headers, HeaderTD, BlockNumber>(
        //     &tx,
        //     range,
        //     none_vec,
        //     Some(hashes),
        //     row_count as usize,
        //     &mut nippy_jar,
        // )
        // .unwrap();

        Ok(())
    }
}
