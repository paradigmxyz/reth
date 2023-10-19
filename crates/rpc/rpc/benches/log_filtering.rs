use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use rand::Rng;
use reth_db::{database::Database, test_utils::create_test_rw_db, DatabaseEnv};
use reth_interfaces::test_utils::generators::{random_block, random_log};
use reth_primitives::{logs_bloom, Address, Receipt, Receipts, B256, MAINNET, U256};
use reth_provider::{
    BlockWriter, BundleStateWithReceipts, LogHistoryWriter, OriginalValuesKnown, ProviderFactory,
};
use reth_rpc::eth::filter::FilterError;
use reth_rpc_types::{Filter, FilterBlockOption, Log};
use revm::db::BundleState;
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
enum LogOccurrence {
    SingleBlock,
    Random,
    Always,
}

impl LogOccurrence {
    const ALL: [LogOccurrence; 3] =
        [LogOccurrence::SingleBlock, LogOccurrence::Random, LogOccurrence::Always];
}

trait BenchLogFiltering<DB> {
    fn new(provider_factory: ProviderFactory<DB>) -> Self;

    fn get_logs_in_block_range(
        &self,
        filter: &Filter,
        from_block: u64,
        to_block: u64,
    ) -> Result<Vec<Log>, FilterError>;
}

fn rpc_log_filtering_by_address(c: &mut Criterion) {
    let mut group = c.benchmark_group("RPC Log Filtering By Address");

    let mut rng = rand::thread_rng();
    let target_filter_address = Address::random();

    use implementations::*;
    for num_blocks in [1_000, 10_000, 100_000] {
        for occurrence in LogOccurrence::ALL {
            let (db, expected) =
                generate_test_data(&mut rng, target_filter_address, num_blocks, occurrence);

            let filter = Filter {
                block_option: FilterBlockOption::Range {
                    from_block: Some(0.into()),
                    to_block: Some((num_blocks as u64).into()),
                },
                address: target_filter_address.into(),
                topics: Default::default(),
            };

            bench_log_filtering::<_, BloomFilterLogFiltering<_>>(
                &mut group,
                "BloomFilter",
                db.clone(),
                &expected,
                &filter,
                occurrence,
                num_blocks,
            );

            bench_log_filtering::<_, IndexLogFiltering<_>>(
                &mut group,
                "LogIndex",
                db.clone(),
                &expected,
                &filter,
                occurrence,
                num_blocks,
            );
        }
    }
}

fn bench_log_filtering<DB: Database + Clone, T: BenchLogFiltering<DB>>(
    group: &mut BenchmarkGroup<WallTime>,
    description: &str,
    db: ProviderFactory<DB>,
    expected_result: &[Log],
    filter: &Filter,
    occurrence: LogOccurrence,
    num_blocks: usize,
) {
    group.bench_function(
        format!(
            "log filtering | num blocks: {num_blocks} | occurrence: {occurrence:?} | {description}",
        ),
        |b| {
            b.iter(|| {
                {
                    let result = T::new(db.clone())
                        .get_logs_in_block_range(filter, 0, num_blocks as u64)
                        .expect("log filter success");
                    assert_eq!(result.len(), expected_result.len());
                    assert_eq!(result, expected_result);
                }
                std::hint::black_box(());
            });
        },
    );
}

fn generate_test_data<R: Rng>(
    rng: &mut R,
    target_filter_address: Address,
    num_blocks: usize,
    occurrence: LogOccurrence,
) -> (ProviderFactory<Arc<DatabaseEnv>>, Vec<Log>) {
    let block_range = 0..=(num_blocks - 1) as u64;

    // Pick a target block for [LogOccurrence::SingleBlock].
    let target_log_block =
        matches!(occurrence, LogOccurrence::SingleBlock).then(|| (num_blocks / 2) as u64);

    let mut parent_hash = None;
    let mut expected_result = Vec::new();
    let mut data = Vec::with_capacity(num_blocks);
    for block_idx in block_range.clone() {
        let tx_count = rng.gen_range(1..=10);
        let mut block = random_block(
            rng,
            block_idx,
            Some(parent_hash.unwrap_or(B256::random())),
            Some(tx_count),
            None,
        )
        .unseal();

        let should_block_include_target_address =
            matches!(occurrence, LogOccurrence::Random).then(|| rng.gen::<bool>());
        let receipts = block
            .body
            .iter()
            .map(|tx| {
                let logs_count = rng.gen_range::<u8, _>(1..=20);
                let mut logs = Vec::with_capacity(logs_count as usize);
                for _ in 0..logs_count {
                    let log_address = match occurrence {
                        LogOccurrence::SingleBlock => {
                            if Some(block.number) == target_log_block {
                                target_filter_address
                            } else {
                                rng.gen::<Address>()
                            }
                        }
                        LogOccurrence::Always => target_filter_address,
                        LogOccurrence::Random => {
                            if should_block_include_target_address == Some(true) {
                                target_filter_address
                            } else {
                                rng.gen::<Address>()
                            }
                        }
                    };
                    logs.push(random_log(rng, Some(log_address), None));
                }
                Receipt {
                    tx_type: tx.tx_type(),
                    success: true,
                    cumulative_gas_used: rng.gen_range(0..=tx.gas_limit()),
                    logs,
                }
            })
            .collect::<Vec<_>>();
        let senders = (0..block.body.len()).map(|_| rng.gen()).collect::<Vec<_>>();

        block.header.logs_bloom = logs_bloom(receipts.iter().flat_map(|r| r.logs.iter()));
        let block = block.seal_slow();
        parent_hash = Some(block.hash);

        let mut log_index = 0;
        for (tx_idx, (tx, receipt)) in block.body.iter().zip(receipts.iter()).enumerate() {
            for log in &receipt.logs {
                if log.address == target_filter_address {
                    expected_result.push(Log {
                        address: log.address,
                        topics: log.topics.clone(),
                        data: log.data.clone(),
                        block_hash: Some(block.hash),
                        block_number: Some(U256::from(block.number)),
                        transaction_hash: Some(tx.hash),
                        transaction_index: Some(U256::from(tx_idx)),
                        log_index: Some(U256::from(log_index)),
                        removed: false,
                    });
                }
                log_index += 1;
            }
        }

        data.push((block, senders, receipts));
    }

    let provider_factory = ProviderFactory::new(create_test_rw_db(), MAINNET.clone());
    let mut provider_rw = provider_factory.provider_rw().unwrap();
    for (block, senders, receipts) in data {
        let block_number = block.number;
        provider_rw.insert_block(block, Some(senders), None).unwrap();
        BundleStateWithReceipts::new(
            BundleState::default(),
            Receipts::from_vec(Vec::from([receipts.into_iter().map(Some).collect()])),
            block_number,
        )
        .write_to_db(provider_rw.tx_mut(), OriginalValuesKnown::Yes)
        .unwrap();
    }
    let (log_address_indices, _log_topic_indices, _) =
        provider_rw.compute_log_indexes(block_range).unwrap();
    provider_rw.insert_log_address_history_index(log_address_indices).unwrap();
    provider_rw.commit().unwrap();

    (provider_factory, expected_result)
}

mod implementations {
    use super::*;
    use reth_primitives::BlockHashOrNumber;
    use reth_provider::{
        BlockHashReader, BlockReader, HeaderProvider, ProviderFactory, ReceiptProvider,
    };
    use reth_rpc::eth::{filter::LogIndexFilter, logs_utils};
    use reth_rpc_types::FilteredParams;
    use std::{iter::StepBy, ops::RangeInclusive};

    #[derive(Debug)]
    struct BlockRangeInclusiveIter {
        iter: StepBy<RangeInclusive<u64>>,
        step: u64,
        end: u64,
    }

    impl BlockRangeInclusiveIter {
        fn new(range: RangeInclusive<u64>, step: u64) -> Self {
            Self { end: *range.end(), iter: range.step_by(step as usize + 1), step }
        }
    }

    impl Iterator for BlockRangeInclusiveIter {
        type Item = (u64, u64);

        fn next(&mut self) -> Option<Self::Item> {
            let start = self.iter.next()?;
            let end = (start + self.step).min(self.end);
            if start > end {
                return None
            }
            Some((start, end))
        }
    }

    pub struct BloomFilterLogFiltering<DB> {
        provider_factory: ProviderFactory<DB>,
    }

    impl<DB: Database> BenchLogFiltering<DB> for BloomFilterLogFiltering<DB> {
        fn new(provider_factory: ProviderFactory<DB>) -> Self {
            Self { provider_factory }
        }

        fn get_logs_in_block_range(
            &self,
            filter: &Filter,
            from_block: u64,
            to_block: u64,
        ) -> Result<Vec<Log>, FilterError> {
            let provider = self.provider_factory.provider()?;

            let mut all_logs = Vec::new();
            let filter_params = FilteredParams::new(Some(filter.clone()));

            // derive bloom filters from filter input
            let address_filter = FilteredParams::address_filter(&filter.address);
            let topics_filter = FilteredParams::topics_filter(&filter.topics);

            // loop over the range of new blocks and check logs if the filter matches the log's
            // bloom filter
            for (from, to) in BlockRangeInclusiveIter::new(from_block..=to_block, u64::MAX - 1) {
                let headers = provider.headers_range(from..=to)?;

                for (idx, header) in headers.iter().enumerate() {
                    // these are consecutive headers, so we can use the parent hash of the next
                    // block to get the current header's hash
                    let num_hash: BlockHashOrNumber = headers
                        .get(idx + 1)
                        .map(|h| h.parent_hash.into())
                        .unwrap_or_else(|| header.number.into());

                    // only if filter matches
                    if FilteredParams::matches_address(header.logs_bloom, &address_filter) &&
                        FilteredParams::matches_topics(header.logs_bloom, &topics_filter)
                    {
                        if let Some(block_hash) = provider.convert_block_hash(num_hash)? {
                            let block = provider.block(num_hash)?;
                            let receipts = provider.receipts_by_block(num_hash)?;
                            if let Some((block, receipts)) = block.zip(receipts) {
                                logs_utils::append_matching_block_logs(
                                    &mut all_logs,
                                    &filter_params,
                                    (block.number, block_hash).into(),
                                    block.body.into_iter().map(|tx| tx.hash()).zip(receipts),
                                    false,
                                );
                            }
                        }
                    }
                }
            }

            Ok(all_logs)
        }
    }

    pub struct IndexLogFiltering<DB> {
        provider_factory: ProviderFactory<DB>,
    }

    impl<DB: Database> BenchLogFiltering<DB> for IndexLogFiltering<DB> {
        fn new(provider_factory: ProviderFactory<DB>) -> Self {
            Self { provider_factory }
        }

        fn get_logs_in_block_range(
            &self,
            filter: &Filter,
            from_block: u64,
            to_block: u64,
        ) -> Result<Vec<Log>, FilterError> {
            let provider = self.provider_factory.provider()?;

            let mut all_logs = Vec::new();
            let filter_params = FilteredParams::new(Some(filter.clone()));

            // Create log index filter
            let mut log_index_filter = LogIndexFilter::new(from_block..=to_block);
            if let Some(filter_address) = filter.address.to_value_or_array() {
                log_index_filter.install_address_filter(&provider, &filter_address)?;
            }
            let topics = filter
                .topics
                .clone()
                .into_iter()
                .filter_map(|t| t.to_value_or_array())
                .collect::<Vec<_>>();
            if !topics.is_empty() {
                log_index_filter.install_topic_filter(&provider, &topics)?;
            }

            // loop over the range of new blocks and check logs if the filter matches the log's
            // bloom filter
            for block_number in log_index_filter.iter() {
                let Some(block) = provider.block(block_number.into())? else { continue };

                let block_hash = block.hash_slow();
                let receipts = provider.receipts_by_block(block.number.into())?.unwrap_or_default();

                logs_utils::append_matching_block_logs(
                    &mut all_logs,
                    &filter_params,
                    (block.number, block_hash).into(),
                    block.body.into_iter().map(|tx| tx.hash()).zip(receipts),
                    false,
                );
            }

            Ok(all_logs)
        }
    }
}

criterion_group!(log_filtering, rpc_log_filtering_by_address);
criterion_main!(log_filtering);
