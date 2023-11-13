use super::{
    bench::{bench, BenchKind},
    Command, Compression, PerfectHashingFunction,
};
use rand::{seq::SliceRandom, Rng};
use reth_db::{database::Database, open_db_read_only, snapshot::ReceiptMask};
use reth_interfaces::db::LogLevel;
use reth_primitives::{
    snapshot::{Filters, InclusionFilter},
    ChainSpec, Receipt, SnapshotSegment,
};
use reth_provider::{
    providers::SnapshotProvider, DatabaseProviderRO, ProviderError, ProviderFactory,
    ReceiptProvider, TransactionsProvider, TransactionsProviderExt,
};
use reth_snapshot::{segments, segments::Segment};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

impl Command {
    pub(crate) fn generate_receipts_snapshot<DB: Database>(
        &self,
        provider: &DatabaseProviderRO<'_, DB>,
        compression: Compression,
        inclusion_filter: InclusionFilter,
        phf: PerfectHashingFunction,
    ) -> eyre::Result<()> {
        let range = self.block_range();
        let filters = if self.with_filters {
            Filters::WithFilters(inclusion_filter, phf)
        } else {
            Filters::WithoutFilters
        };

        let segment = segments::Receipts::new(compression, filters);

        segment.snapshot::<DB>(provider, range.clone())?;

        // Default name doesn't have any configuration
        let default_name: PathBuf = SnapshotSegment::Receipts.filename(&range).into();
        let new_name: PathBuf = SnapshotSegment::Receipts
            .filename_with_configuration(filters, compression, &range)
            .into();

        std::fs::rename(default_name, new_name)?;

        Ok(())
    }

    pub(crate) fn bench_receipts_snapshot(
        &self,
        db_path: &Path,
        log_level: Option<LogLevel>,
        chain: Arc<ChainSpec>,
        compression: Compression,
        inclusion_filter: InclusionFilter,
        phf: PerfectHashingFunction,
    ) -> eyre::Result<()> {
        let filters = if self.with_filters {
            Filters::WithFilters(inclusion_filter, phf)
        } else {
            Filters::WithoutFilters
        };

        let block_range = self.from..=(self.from + self.block_interval - 1);

        let mut rng = rand::thread_rng();

        let tx_range = ProviderFactory::new(open_db_read_only(db_path, log_level)?, chain.clone())
            .provider()?
            .transaction_range_by_block_range(block_range.clone())?;

        let mut row_indexes = tx_range.clone().collect::<Vec<_>>();

        let path = SnapshotSegment::Receipts
            .filename_with_configuration(filters, compression, &block_range)
            .into();

        let provider = SnapshotProvider::default();
        let jar_provider =
            provider.get_segment_provider(SnapshotSegment::Receipts, self.from, Some(path))?;
        let mut cursor = jar_provider.cursor()?;

        for bench_kind in [BenchKind::Walk, BenchKind::RandomAll] {
            bench(
                bench_kind,
                (open_db_read_only(db_path, log_level)?, chain.clone()),
                SnapshotSegment::Receipts,
                filters,
                compression,
                || {
                    for num in row_indexes.iter() {
                        cursor
                            .get_one::<ReceiptMask<Receipt>>((*num).into())?
                            .ok_or(ProviderError::ReceiptNotFound((*num).into()))?;
                    }
                    Ok(())
                },
                |provider| {
                    for num in row_indexes.iter() {
                        provider
                            .receipt(*num)?
                            .ok_or(ProviderError::ReceiptNotFound((*num).into()))?;
                    }
                    Ok(())
                },
            )?;

            // For random walk
            row_indexes.shuffle(&mut rng);
        }

        // BENCHMARK QUERYING A RANDOM RECEIPT BY NUMBER
        {
            let num = row_indexes[rng.gen_range(0..row_indexes.len())];
            bench(
                BenchKind::RandomOne,
                (open_db_read_only(db_path, log_level)?, chain.clone()),
                SnapshotSegment::Receipts,
                filters,
                compression,
                || {
                    Ok(cursor
                        .get_one::<ReceiptMask<Receipt>>(num.into())?
                        .ok_or(ProviderError::ReceiptNotFound(num.into()))?)
                },
                |provider| {
                    Ok(provider
                        .receipt(num as u64)?
                        .ok_or(ProviderError::ReceiptNotFound((num as u64).into()))?)
                },
            )?;
        }

        // BENCHMARK QUERYING A RANDOM RECEIPT BY HASH
        {
            let num = row_indexes[rng.gen_range(0..row_indexes.len())] as u64;
            let tx_hash =
                ProviderFactory::new(open_db_read_only(db_path, log_level)?, chain.clone())
                    .transaction_by_id(num)?
                    .ok_or(ProviderError::ReceiptNotFound(num.into()))?
                    .hash();

            bench(
                BenchKind::RandomHash,
                (open_db_read_only(db_path, log_level)?, chain.clone()),
                SnapshotSegment::Receipts,
                filters,
                compression,
                || {
                    Ok(cursor
                        .get_one::<ReceiptMask<Receipt>>((&tx_hash).into())?
                        .ok_or(ProviderError::ReceiptNotFound(tx_hash.into()))?)
                },
                |provider| {
                    Ok(provider
                        .receipt_by_hash(tx_hash)?
                        .ok_or(ProviderError::ReceiptNotFound(tx_hash.into()))?)
                },
            )?;
        }
        Ok(())
    }
}
