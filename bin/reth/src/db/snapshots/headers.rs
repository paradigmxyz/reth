use super::{
    bench::{bench, BenchKind},
    Command,
};
use crate::utils::DbTool;
use rand::{seq::SliceRandom, Rng};
use reth_db::{database::Database, open_db_read_only, table::Decompress, DatabaseEnvRO};
use reth_interfaces::db::LogLevel;
use reth_nippy_jar::NippyJar;
use reth_primitives::{ChainSpec, Compression, Filters, Header, PerfectHashingFunction};
use reth_provider::{HeaderProvider, ProviderError, ProviderFactory};
use reth_snapshot::segments::{Headers, Segment};
use std::{path::Path, sync::Arc};

impl Command {
    pub(crate) fn generate_headers_snapshot(
        &self,
        tool: &DbTool<'_, DatabaseEnvRO>,
        compression: Compression,
        phf: PerfectHashingFunction,
    ) -> eyre::Result<()> {
        let segment = Headers::new(
            compression,
            self.with_filters
                .then_some(Filters::WithFilters(phf))
                .unwrap_or(Filters::WithoutFilters),
        );
        segment.snapshot(&tool.db.tx()?, self.from..=(self.from + self.block_interval - 1))?;

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
        let segment = Headers::new(
            compression,
            self.with_filters
                .then_some(Filters::WithFilters(phf))
                .unwrap_or(Filters::WithoutFilters),
        );

        let range = self.from..=(self.from + self.block_interval - 1);

        let jar_config = (segment.segment(), compression, phf);
        let mut row_indexes = range.clone().collect::<Vec<_>>();
        let mut rng = rand::thread_rng();
        let mut dictionaries = None;
        let mut jar = NippyJar::load_without_header(&segment.get_file_path(&range))?;

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
                                .row_by_number_with_cols::<0b01, 2>((num - self.from) as usize)?
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
