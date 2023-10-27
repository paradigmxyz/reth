use super::{
    bench::{bench, BenchKind},
    Command, Compression, PerfectHashingFunction,
};
use rand::{seq::SliceRandom, Rng};
use reth_db::{database::Database, open_db_read_only, table::Decompress};
use reth_interfaces::db::LogLevel;
use reth_primitives::{
    snapshot::{Filters, InclusionFilter},
    ChainSpec, SnapshotSegment, TransactionSignedNoHash,
};
use reth_provider::{
    providers::SnapshotProvider, DatabaseProviderRO, ProviderError, ProviderFactory,
    TransactionsProvider, TransactionsProviderExt,
};
use reth_snapshot::{segments, segments::Segment};
use std::{path::Path, sync::Arc};

impl Command {
    pub(crate) fn generate_transactions_snapshot<DB: Database>(
        &self,
        provider: &DatabaseProviderRO<'_, DB>,
        compression: Compression,
        inclusion_filter: InclusionFilter,
        phf: PerfectHashingFunction,
    ) -> eyre::Result<()> {
        let segment = segments::Transactions::new(
            compression,
            if self.with_filters {
                Filters::WithFilters(inclusion_filter, phf)
            } else {
                Filters::WithoutFilters
            },
        );
        segment.snapshot::<DB>(provider, self.from..=(self.from + self.block_interval - 1))?;

        Ok(())
    }

    pub(crate) fn bench_transactions_snapshot(
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

        let path = SnapshotSegment::Transactions.filename_with_configuration(
            filters,
            compression,
            &block_range,
        );
        let provider = SnapshotProvider::default();
        let jar_provider =
            provider.get_segment_provider(SnapshotSegment::Transactions, self.from, Some(path))?;
        let mut cursor = jar_provider.cursor()?;

        for bench_kind in [BenchKind::Walk, BenchKind::RandomAll] {
            bench(
                bench_kind,
                (open_db_read_only(db_path, log_level)?, chain.clone()),
                SnapshotSegment::Transactions,
                filters,
                compression,
                || {
                    for num in row_indexes.iter() {
                        TransactionSignedNoHash::decompress(
                            cursor
                                .row_by_number_with_cols::<0b1, 1>(
                                    (num - tx_range.start()) as usize,
                                )?
                                .ok_or(ProviderError::TransactionNotFound((*num).into()))?[0],
                        )?
                        .with_hash();
                        // TODO: replace with below when eventually SnapshotProvider re-uses cursor
                        // provider.transaction_by_id(num as
                        // u64)?.ok_or(ProviderError::TransactionNotFound((*num).into()))?;
                    }
                    Ok(())
                },
                |provider| {
                    for num in row_indexes.iter() {
                        provider
                            .transaction_by_id(*num)?
                            .ok_or(ProviderError::TransactionNotFound((*num).into()))?;
                    }
                    Ok(())
                },
            )?;

            // For random walk
            row_indexes.shuffle(&mut rng);
        }

        // BENCHMARK QUERYING A RANDOM TRANSACTION BY NUMBER
        {
            let num = row_indexes[rng.gen_range(0..row_indexes.len())];
            bench(
                BenchKind::RandomOne,
                (open_db_read_only(db_path, log_level)?, chain.clone()),
                SnapshotSegment::Transactions,
                filters,
                compression,
                || {
                    Ok(TransactionSignedNoHash::decompress(
                        cursor
                            .row_by_number_with_cols::<0b1, 1>((num - tx_range.start()) as usize)?
                            .ok_or(ProviderError::TransactionNotFound((num as u64).into()))?[0],
                    )?
                    .with_hash())
                },
                |provider| {
                    Ok(provider
                        .transaction_by_id(num as u64)?
                        .ok_or(ProviderError::TransactionNotFound((num as u64).into()))?)
                },
            )?;
        }

        // BENCHMARK QUERYING A RANDOM TRANSACTION BY HASH
        {
            let num = row_indexes[rng.gen_range(0..row_indexes.len())] as u64;
            let transaction_hash =
                ProviderFactory::new(open_db_read_only(db_path, log_level)?, chain.clone())
                    .transaction_by_id(num)?
                    .ok_or(ProviderError::TransactionNotFound(num.into()))?
                    .hash();

            bench(
                BenchKind::RandomHash,
                (open_db_read_only(db_path, log_level)?, chain.clone()),
                SnapshotSegment::Transactions,
                filters,
                compression,
                || {
                    let transaction = TransactionSignedNoHash::decompress(
                        cursor
                            .row_by_key_with_cols::<0b1, 1>(transaction_hash.as_slice())?
                            .ok_or(ProviderError::TransactionNotFound(transaction_hash.into()))?[0],
                    )?;

                    // Might be a false positive, so in the real world we have to validate it
                    Ok(transaction.with_hash())
                },
                |provider| {
                    Ok(provider
                        .transaction_by_hash(transaction_hash)?
                        .ok_or(ProviderError::TransactionNotFound(transaction_hash.into()))?)
                },
            )?;
        }
        Ok(())
    }
}
