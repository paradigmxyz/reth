use super::{
    bench::{bench, BenchKind},
    Command, Compression, PerfectHashingFunction,
};
use rand::{seq::SliceRandom, Rng};
use reth_db::{open_db_read_only, snapshot::TransactionMask};
use reth_interfaces::db::LogLevel;
use reth_primitives::{
    snapshot::{Filters, InclusionFilter},
    ChainSpec, SnapshotSegment, StoredTransaction,
};
use reth_provider::{
    providers::SnapshotProvider, BlockNumReader, ProviderError, ProviderFactory,
    TransactionDataStore, TransactionsProvider, TransactionsProviderExt,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

impl Command {
    pub(crate) fn bench_transactions_snapshot(
        &self,
        db_path: &Path,
        transaction_data_store: Arc<dyn TransactionDataStore>,
        log_level: Option<LogLevel>,
        chain: Arc<ChainSpec>,
        compression: Compression,
        inclusion_filter: InclusionFilter,
        phf: PerfectHashingFunction,
    ) -> eyre::Result<()> {
        let provider_factory = ProviderFactory::new(
            Arc::new(open_db_read_only(db_path, log_level)?),
            transaction_data_store,
            chain.clone(),
        );
        let provider = provider_factory.provider()?;
        let tip = provider.last_block_number()?;
        let block_range =
            self.block_ranges(tip).first().expect("has been generated before").clone();

        let filters = if self.with_filters {
            Filters::WithFilters(inclusion_filter, phf)
        } else {
            Filters::WithoutFilters
        };

        let mut rng = rand::thread_rng();

        let tx_range = provider.transaction_range_by_block_range(block_range.clone())?;

        let mut row_indexes = tx_range.clone().collect::<Vec<_>>();

        let path: PathBuf = SnapshotSegment::Transactions
            .filename_with_configuration(filters, compression, &block_range, &tx_range)
            .into();
        let provider = SnapshotProvider::default();
        let jar_provider = provider.get_segment_provider_from_block(
            SnapshotSegment::Transactions,
            self.from,
            Some(&path),
        )?;
        let mut cursor = jar_provider.cursor()?;

        for bench_kind in [BenchKind::Walk, BenchKind::RandomAll] {
            bench(
                bench_kind,
                provider_factory.clone(),
                SnapshotSegment::Transactions,
                filters,
                compression,
                || {
                    for num in row_indexes.iter() {
                        cursor
                            .get_one::<TransactionMask<StoredTransaction>>((*num).into())?
                            .ok_or(ProviderError::TransactionNotFound((*num).into()))?
                            .as_no_hash()
                            .ok_or(ProviderError::MissingSnapshotTransactionData)?
                            .with_hash();
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
                provider_factory.clone(),
                SnapshotSegment::Transactions,
                filters,
                compression,
                || {
                    Ok(cursor
                        .get_one::<TransactionMask<StoredTransaction>>(num.into())?
                        .ok_or(ProviderError::TransactionNotFound(num.into()))?
                        .as_no_hash()
                        .ok_or(ProviderError::MissingSnapshotTransactionData)?
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
            let transaction_hash = provider_factory
                .transaction_by_id(num)?
                .ok_or(ProviderError::TransactionNotFound(num.into()))?
                .hash();

            bench(
                BenchKind::RandomHash,
                provider_factory.clone(),
                SnapshotSegment::Transactions,
                filters,
                compression,
                || {
                    Ok(cursor
                        .get_one::<TransactionMask<StoredTransaction>>((&transaction_hash).into())?
                        .ok_or(ProviderError::TransactionNotFound(transaction_hash.into()))?
                        .as_no_hash()
                        .ok_or(ProviderError::MissingSnapshotTransactionData)?
                        .with_hash())
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
