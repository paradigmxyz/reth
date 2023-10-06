use super::{
    bench::{bench, BenchKind},
    Command, Compression, PerfectHashingFunction, Rows, Snapshots,
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
use reth_provider::{HeaderProvider, ProviderError, ProviderFactory};
use std::{path::Path, sync::Arc};
use tables::*;

impl Command {
    pub(crate) fn generate_headers_snapshot(
        &self,
        tool: &DbTool<'_, DatabaseEnvRO>,
        compression: Compression,
        phf: PerfectHashingFunction,
    ) -> eyre::Result<()> {
        let mut jar = self.prepare_jar(2, (Snapshots::Headers, compression, phf), tool, || {
            // Generates the dataset to train a zstd dictionary if necessary, with the most recent
            // rows (at most 1000).
            let dataset = tool.db.view(|tx| {
                let mut cursor = tx.cursor_read::<reth_db::RawTable<reth_db::Headers>>()?;
                let v1 = cursor
                    .walk_back(Some(RawKey::from((self.from + self.block_interval - 1) as u64)))?
                    .take(self.block_interval.min(1000))
                    .map(|row| row.map(|(_key, value)| value.into_value()).expect("should exist"))
                    .collect::<Vec<_>>();
                let mut cursor = tx.cursor_read::<reth_db::RawTable<reth_db::HeaderTD>>()?;
                let v2 = cursor
                    .walk_back(Some(RawKey::from((self.from + self.block_interval - 1) as u64)))?
                    .take(self.block_interval.min(1000))
                    .map(|row| row.map(|(_key, value)| value.into_value()).expect("should exist"))
                    .collect::<Vec<_>>();
                Ok::<Rows, eyre::Error>(vec![v1, v2])
            })??;
            Ok(dataset)
        })?;

        tool.db.view(|tx| {
            // Hacky type inference. TODO fix
            let mut none_vec = Some(vec![vec![vec![0u8]].into_iter()]);
            let _ = none_vec.take();

            // Generate list of hashes for filters & PHF
            let mut cursor = tx.cursor_read::<RawTable<CanonicalHeaders>>()?;
            let mut hashes = None;
            if self.with_filters {
                hashes = Some(
                    cursor
                        .walk(Some(RawKey::from(self.from as u64)))?
                        .take(self.block_interval)
                        .map(|row| {
                            row.map(|(_key, value)| value.into_value()).map_err(|e| e.into())
                        }),
                );
            }

            create_snapshot_T1_T2::<Headers, HeaderTD, BlockNumber>(
                tx,
                self.from as u64..=(self.from as u64 + self.block_interval as u64),
                None,
                // We already prepared the dictionary beforehand
                none_vec,
                hashes,
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
        compression: Compression,
        phf: PerfectHashingFunction,
    ) -> eyre::Result<()> {
        let mode = Snapshots::Headers;
        let jar_config = (mode, compression, phf);
        let mut row_indexes = (self.from..(self.from + self.block_interval)).collect::<Vec<_>>();
        let mut rng = rand::thread_rng();
        let mut dictionaries = None;
        let mut jar = NippyJar::load_without_header(&self.get_file_path(jar_config))?;

        let (provider, decompressors) = self.prepare_jar_provider(&mut jar, &mut dictionaries)?;
        let mut cursor = if !decompressors.is_empty() {
            provider.cursor_with_decompressors(decompressors)
        } else {
            provider.cursor()
        };

        for bench_kind in [BenchKind::Walk, BenchKind::RandomAll] {
            bench(
                bench_kind,
                (open_db_read_only(db_path, log_level)?, chain.clone()),
                jar_config,
                || {
                    for num in row_indexes.iter() {
                        Header::decompress(
                            cursor
                                .row_by_number_with_cols::<0b01, 2>(num - self.from)?
                                .ok_or(ProviderError::HeaderNotFound((*num as u64).into()))?[0],
                        )?;
                        // TODO: replace with below when eventually SnapshotProvider re-uses cursor
                        // provider.header_by_number(num as
                        // u64)?.ok_or(ProviderError::HeaderNotFound((*num as u64).into()))?;
                    }
                    Ok(())
                },
                |provider| {
                    for num in row_indexes.iter() {
                        provider
                            .header_by_number(*num as u64)?
                            .ok_or(ProviderError::HeaderNotFound((*num as u64).into()))?;
                    }
                    Ok(())
                },
            )?;

            // For random walk
            row_indexes.shuffle(&mut rng);
        }

        // BENCHMARK QUERYING A RANDOM HEADER BY NUMBER
        {
            let num = row_indexes[rng.gen_range(0..row_indexes.len())];
            bench(
                BenchKind::RandomOne,
                (open_db_read_only(db_path, log_level)?, chain.clone()),
                jar_config,
                || {
                    Header::decompress(
                        cursor
                            .row_by_number_with_cols::<0b01, 2>((num - self.from) as usize)?
                            .ok_or(ProviderError::HeaderNotFound((num as u64).into()))?[0],
                    )?;
                    Ok(())
                },
                |provider| {
                    provider
                        .header_by_number(num as u64)?
                        .ok_or(ProviderError::HeaderNotFound((num as u64).into()))?;
                    Ok(())
                },
            )?;
        }

        // BENCHMARK QUERYING A RANDOM HEADER BY HASH
        {
            let num = row_indexes[rng.gen_range(0..row_indexes.len())] as u64;
            let header_hash =
                ProviderFactory::new(open_db_read_only(db_path, log_level)?, chain.clone())
                    .header_by_number(num)?
                    .ok_or(ProviderError::HeaderNotFound(num.into()))?
                    .hash_slow();

            bench(
                BenchKind::RandomHash,
                (open_db_read_only(db_path, log_level)?, chain.clone()),
                jar_config,
                || {
                    let header = Header::decompress(
                        cursor
                            .row_by_key_with_cols::<0b01, 2>(header_hash.as_slice())?
                            .ok_or(ProviderError::HeaderNotFound(header_hash.into()))?[0],
                    )?;

                    // Might be a false positive, so in the real world we have to validate it
                    assert!(header.hash_slow() == header_hash);
                    Ok(())
                },
                |provider| {
                    provider
                        .header(&header_hash)?
                        .ok_or(ProviderError::HeaderNotFound(header_hash.into()))?;
                    Ok(())
                },
            )?;
        }
        Ok(())
    }
}
