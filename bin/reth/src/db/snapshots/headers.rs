use super::{
    bench::{bench, BenchKind},
    Command, Compression, PerfectHashingFunction, Snapshots,
};
use crate::utils::DbTool;
use rand::{seq::SliceRandom, Rng};
use reth_db::{
    cursor::DbCursorRO, database::Database, open_db_read_only, snapshot::create_snapshot_T1_T2,
    table::Decompress, tables, transaction::DbTx, DatabaseEnvRO,
};
use reth_interfaces::db::LogLevel;
use reth_nippy_jar::NippyJar;
use reth_primitives::{BlockNumber, ChainSpec, Header};
use reth_provider::{HeaderProvider, ProviderError};
use std::{path::Path, sync::Arc};
use tables::*;

impl Command {
    pub(crate) fn generate_headers_snapshot(
        &self,
        tool: &DbTool<'_, DatabaseEnvRO>,
        compression: &Compression,
        phf: &PerfectHashingFunction,
    ) -> eyre::Result<()> {
        let mut jar = self.prepare_jar(2, tool, Snapshots::Headers, compression, phf, || {
            let dataset = tool.db.view(|tx| {
                let mut cursor = tx.cursor_read::<reth_db::RawTable<reth_db::Headers>>().unwrap();
                let v1 = cursor
                    .walk_back(None)
                    .unwrap()
                    .take(self.block_interval.min(1000))
                    .map(|row| row.map(|(_key, value)| value.into_value()).expect(""))
                    .collect::<Vec<_>>();
                let mut cursor = tx.cursor_read::<reth_db::RawTable<reth_db::HeaderTD>>().unwrap();
                let v2 = cursor
                    .walk_back(None)
                    .unwrap()
                    .take(self.block_interval.min(1000))
                    .map(|row| row.map(|(_key, value)| value.into_value()).expect(""))
                    .collect::<Vec<_>>();
                vec![v1, v2]
            })?;
            Ok(Some(dataset))
        })?;

        tool.db.view(|tx| {
            // Hacky type inference. TODO fix
            let mut none_vec = Some(vec![vec![vec![0u8]].into_iter()]);
            let _ = none_vec.take();

            // Generate list of hashes for filters & PHF
            let mut cursor = tx.cursor_read::<RawTable<CanonicalHeaders>>().unwrap();
            let hashes = cursor
                .walk(Some(RawKey::from(self.from as u64)))
                .unwrap()
                .take(self.block_interval)
                .map(|row| row.map(|(_key, value)| value.into_value()).map_err(|e| e.into()));

            create_snapshot_T1_T2::<Headers, HeaderTD, BlockNumber>(
                tx,
                self.from as u64..=(self.from as u64 + self.block_interval as u64),
                vec![],
                // We already prepared the dictionary beforehand
                none_vec,
                Some(hashes),
                self.block_interval,
                &mut jar,
            )
        })??;

        Ok(())
    }

    pub(crate) fn bench_headers_snapshot(
        &self,
        db_path: &Path,
        log_level: Option<LogLevel>,
        chain: Arc<ChainSpec>,
        compression: &Compression,
        phf: &PerfectHashingFunction,
    ) -> eyre::Result<()> {
        let mut jar = NippyJar::load_without_header(&self.get_file_path(
            Snapshots::Headers,
            compression,
            phf,
        ))?;
        let mut row_indexes = (self.from..(self.from + self.block_interval)).collect::<Vec<_>>();
        let mut rng = rand::thread_rng();
        let mut dictionaries = None;
        let (provider, decompressors) = self.prepare_jar_provider(&mut jar, &mut dictionaries)?;
        let mut cursor = if !decompressors.is_empty() {
            provider.cursor_with_decompressors(decompressors)
        } else {
            provider.cursor()
        };
        let mode = Snapshots::Headers;

        for bench_kind in [BenchKind::Walk, BenchKind::RandomAll] {
            let db = open_db_read_only(db_path, log_level)?;
            let tool = DbTool::new(&db, chain.clone())?;

            bench(
                mode,
                bench_kind,
                compression,
                phf,
                &tool,
                || {
                    for num in row_indexes.iter() {
                        Header::decompress(
                            cursor
                                .row_by_number_with_cols::<0b01, 2>(num - self.from)
                                .unwrap()
                                .ok_or(ProviderError::HeaderNotFound((*num as u64).into()))
                                .unwrap()[0],
                        )
                        .unwrap();
                        // TODO: replace with below when provider re-uses cursor
                        // let h = provider.header_by_number(num as u64)?.unwrap();
                    }
                },
                |provider| {
                    for num in row_indexes.iter() {
                        provider.header_by_number(*num as u64).unwrap().unwrap();
                    }
                },
            );

            // For random walk
            row_indexes.shuffle(&mut rng);
        }

        let db = open_db_read_only(db_path, log_level)?;
        let tool = DbTool::new(&db, chain.clone())?;
        let num = row_indexes[rng.gen_range(0..row_indexes.len())];
        bench(
            mode,
            BenchKind::RandomOne,
            compression,
            phf,
            &tool,
            || {
                Header::decompress(
                    cursor
                        .row_by_number_with_cols::<0b01, 2>((num - self.from) as usize)
                        .unwrap()
                        .ok_or(ProviderError::HeaderNotFound((num as u64).into()))
                        .unwrap()[0],
                )
                .unwrap();
            },
            |provider| {
                provider.header_by_number(num as u64).unwrap().unwrap();
            },
        );

        Ok(())
    }
}
