//! Archive capture and resumable, fenced publication of finite corpora.

use super::{
    chunk::Chunk,
    store::{self, CaseRecord, Catalog, Context, Record, Store, CONTEXT_KEY, MAX_RECORD_BYTES},
    transaction,
};
use crate::common::{
    AccessRights, CliComponentsBuilder, CliNodeComponents, CliNodeTypes, Environment,
    EnvironmentArgs,
};
use clap::Parser;
use reth_chainspec::{EthChainSpec, EthereumHardforks, Hardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_util::cancellation::CancellationToken;
use reth_evm::ConfigureEvm;
use reth_node_core::args::JitArgs;
use reth_provider::{
    providers::BlockchainProvider, BlockHashReader, BlockNumReader, BlockReader,
    DatabaseProviderFactory, TransactionVariant,
};
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::{BlockBodyIndicesProvider, DBProvider, StateProvider};
use slatedb::{
    config::{CompressionCodec, Settings},
    Db, WriteBatch,
};
use rayon::prelude::*;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tracing::{info, warn};

/// Ensure a pinned exclusive-end range has a durable, verified execution corpus.
#[derive(Debug, Parser)]
pub struct FillCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,
    /// First block (genesis is context, not an executed block).
    #[arg(long, default_value = "1")]
    from: u64,
    /// Exclusive end, clamped to the available head, as in re-execute.
    #[arg(long)]
    to: u64,
    /// Destination s3://bucket/prefix. Existing incompatible data is rejected.
    #[arg(long)]
    corpus: String,
    /// Optional S3-compatible endpoint. Credentials use the standard AWS chain.
    #[arg(long)]
    s3_endpoint: Option<String>,
    /// Re-read and fully replay published records instead of checking metadata only.
    #[arg(long)]
    verify_existing: bool,
    /// Capture worker threads. Blocks are captured concurrently and published in order by one
    /// writer. Defaults to the available parallelism.
    #[arg(long)]
    workers: Option<usize>,
    #[command(flatten)]
    pub jit: JitArgs,
}

impl<C: ChainSpecParser> FillCommand<C> {
    /// Chain configuration, without opening a database.
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + Hardforks + EthereumHardforks>> FillCommand<C> {
    /// Capture missing blocks and independent transactions without persisted execution chunks.
    pub async fn execute<N>(
        self,
        components: impl CliComponentsBuilder<N>,
        runtime: reth_tasks::Runtime,
    ) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
    {
        eyre::ensure!(self.from > 0, "from must be positive");
        let store = Store::open(&self.corpus, self.s3_endpoint.as_deref()).await?;
        let chain = self.env.chain.clone();
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RO, runtime)?;
        let components = components(chain.clone());
        let cancellation = CancellationToken::new();
        let _guard = cancellation.drop_guard();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let expected =
                provider_factory.database_provider_ro()?.disable_long_read_transaction_safety();
            let best = expected.best_block_number()?;
            let end = self.to.min(best);
            if self.to > best {
                warn!(requested = self.to, best, "Clamping exclusive end to available head");
            }
            eyre::ensure!(end > self.from, "empty/reversed corpus range");
            let canonical_hash = |number| {
                expected
                    .block_hash(number)?
                    .ok_or_else(|| eyre::eyre!("missing canonical hash at {number}"))
            };
            eyre::ensure!(
                canonical_hash(0)? == chain.genesis_hash(),
                "source genesis does not match selected chain"
            );
            let first_tx = expected
                .block_body_indices(self.from)?
                .ok_or_else(|| eyre::eyre!("missing first block indices"))?
                .first_tx_num();
            let end_tx = expected
                .block_body_indices(end)?
                .ok_or_else(|| eyre::eyre!("missing exclusive-end block indices"))?
                .first_tx_num();
            let context = Context {
                schema: 5,
                chain_id: chain.chain_id(),
                genesis: chain.genesis_hash().to_string(),
                forks: chain.display_hardforks().to_string(),
                start: self.from,
                end,
                parent: canonical_hash(self.from - 1)?.to_string(),
                tip: canonical_hash(end - 1)?.to_string(),
                first_tx,
                end_tx,
            };
            let existing = handle.block_on(store.catalog())?;
            let (mut catalog, prior) = match existing {
                Some((catalog, version)) => {
                    eyre::ensure!(
                        catalog.context == context,
                        "corpus context conflicts with requested chain/range"
                    );
                    (catalog, Some(version))
                }
                None => (
                    Catalog {
                        cursor: store::Cursor::initial(&context)?,
                        context,
                        checkpoint: None,
                        records: Vec::new(),
                        complete: false,
                        generation: 0,
                    },
                    None,
                ),
            };
            let config = components.evm_config().clone().with_jit_support();
            let consensus = components.consensus();
            if catalog.checkpoint.is_some() {
                let reader = handle.block_on(store.reader(&catalog))?;
                // Errors must not skip closing the reader below.
                let verified = if self.verify_existing {
                    store::Cursor::initial(&catalog.context).and_then(|from| {
                        handle.block_on(store::for_each_published(
                            &reader,
                            from,
                            &catalog.cursor,
                            |record, bytes, cases| {
                                eyre::ensure!(!cancellation.is_cancelled(), "capture cancelled");
                                let chunk = store::decode(&bytes, record)?;
                                store::verify_cases(
                                    &chunk,
                                    &cases,
                                    record,
                                    &config,
                                    consensus,
                                    &cancellation,
                                )
                            },
                        ))
                    })
                } else {
                    Ok(())
                };
                handle.block_on(reader.close())?;
                verified?;
            }
            if catalog.complete {
                info!(
                    records = catalog.cursor.next - catalog.context.start,
                    verified = self.verify_existing,
                    "Corpus already complete; no writes"
                );
                return Ok(());
            }
            catalog.generation = catalog
                .generation
                .checked_add(1)
                .ok_or_else(|| eyre::eyre!("publication generation overflow"))?;
            // Claim before writer open: SlateDB fences database writes, while this
            // CAS token independently fences external catalog publication.
            let mut version = handle.block_on(store.publish(&catalog, prior))?;
            let db = handle.block_on(
                Db::builder(store.path.clone().join("data"), store.objects.clone())
                    .with_settings(Settings {
                        object_store_max_retries: store::SLATEDB_RETRIES,
                        compression_codec: Some(CompressionCodec::Zstd),
                        // Archival fill publishes checkpoints, not retention policy.
                        garbage_collector_options: None,
                        ..Default::default()
                    })
                    .build(),
            )?;
            let result = (|| -> eyre::Result<()> {
                let context_bytes = serde_json::to_vec(&catalog.context)?;
                if let Some(stored) = handle.block_on(db.get(CONTEXT_KEY))? {
                    eyre::ensure!(
                        stored.as_ref() == context_bytes,
                        "stored corpus context conflict"
                    );
                } else {
                    eyre::ensure!(
                        catalog.cursor.next == catalog.context.start,
                        "published corpus has no stored context"
                    );
                    handle.block_on(db.put(CONTEXT_KEY, &context_bytes))?;
                }
                let state = BlockchainProvider::new(provider_factory.clone())?;
                let cancellation = &cancellation;
                // Capture, replay and independent transaction execution are pure functions of
                // the archive DB, so workers run them concurrently. One writer publishes the
                // results strictly in block order, keeping pages, checkpoints and fencing as before.
                let produce = |start: u64| -> eyre::Result<Produced> {
                    let stop = start + 1;
                    let expected =
                        provider_factory.database_provider_ro()?.disable_long_read_transaction_safety();
                    let mut chunk = Arc::new(Chunk::capture(
                        start..stop,
                        &config,
                        consensus,
                        &expected,
                        |at| {
                            let provider = provider_factory
                                .database_provider_ro()?
                                .disable_long_read_transaction_safety();
                            let hash = provider.block_hash(at)?.ok_or_else(|| {
                                eyre::eyre!("missing historical canonical block {at}")
                            })?;
                            Ok(StateProviderDatabase(
                                state
                                    .state_provider_from_database(provider, hash)
                                    .into_evm_state_provider(),
                            ))
                        },
                        |number| {
                            provider_factory
                                .recovered_block(number.into(), TransactionVariant::NoHash)?
                                .ok_or_else(|| eyre::eyre!("missing canonical block {number}"))
                        },
                        cancellation,
                    )?);
                    let (transactions, gas) = chunk.replay(&config, consensus, cancellation)?;
                    let indices = expected
                        .block_body_indices(start)?
                        .ok_or_else(|| eyre::eyre!("missing canonical body indices"))?;
                    let first_tx = indices.first_tx_num();
                    eyre::ensure!(
                        indices.tx_count() == transactions,
                        "canonical transaction count mismatch"
                    );
                    let (cases, phases) = transaction::execute(
                        &config,
                        consensus,
                        &chunk,
                        first_tx,
                        None,
                        cancellation,
                    )?;
                    let captured =
                        Arc::get_mut(&mut chunk).expect("capture has no remaining views");
                    captured.phases = Some(phases);
                    for versions in captured.accounts.values_mut() {
                        versions.truncate(1);
                    }
                    for versions in captured.storage.values_mut() {
                        versions.truncate(1);
                    }
                    // Each standalone check is independent of the others, so a dense block's
                    // quadratic prior-delta replays spread over the pool.
                    let case_bytes = cases
                        .par_iter()
                        .enumerate()
                        .map(|(index, case)| {
                            case.replay(&config, &chunk, &cases[..index])?;
                            wincode::serialize(case).map_err(Into::into)
                        })
                        .collect::<eyre::Result<Vec<_>>>()?;
                    let bytes = wincode::serialize(chunk.as_ref())?;
                    let total_bytes = case_bytes.iter().try_fold(bytes.len(), |sum, b| {
                        sum.checked_add(b.len())
                            .ok_or_else(|| eyre::eyre!("serialized size overflow"))
                    })?;
                    eyre::ensure!(
                        total_bytes <= MAX_RECORD_BYTES,
                        "block and independent transactions exceed 64 MiB record limit"
                    );
                    let record = Record {
                        start,
                        end: stop,
                        parent: chunk.parent,
                        tip: chunk.blocks.last().expect("nonempty capture").hash,
                        bytes: total_bytes,
                        block_bytes: bytes.len(),
                        digest: blake3::hash(&bytes).to_hex().to_string(),
                        transactions,
                        gas,
                        first_tx,
                        cases: case_bytes
                            .iter()
                            .enumerate()
                            .map(|(index, b)| CaseRecord {
                                index: index as u64,
                                bytes: b.len(),
                                digest: blake3::hash(b).to_hex().to_string(),
                            })
                            .collect(),
                    };
                    let metadata = serde_json::to_vec(&record)?;
                    eyre::ensure!(metadata.len() <= MAX_RECORD_BYTES, "metadata record too large");
                    Ok(Produced { record, bytes, metadata, case_bytes })
                };
                let workers = self
                    .workers
                    .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
                    .max(1);
                // Bounds results held for in-order publication behind one slow block.
                let window = workers as u64 * 8;
                let begin = catalog.cursor.next;
                let next = AtomicU64::new(begin);
                let written = AtomicU64::new(begin);
                let failed = AtomicBool::new(false);
                info!(workers, from = begin, to = end, "Starting parallel corpus capture");
                std::thread::scope(|scope| -> eyre::Result<()> {
                    let (sender, receiver) = crossbeam_channel::unbounded::<eyre::Result<Produced>>();
                    for _ in 0..workers {
                        let sender = sender.clone();
                        let (next, written, failed, produce) = (&next, &written, &failed, &produce);
                        scope.spawn(move || loop {
                            let number = next.fetch_add(1, Ordering::Relaxed);
                            if number >= end {
                                return;
                            }
                            while number >= written.load(Ordering::Acquire) + window {
                                if failed.load(Ordering::Relaxed) || cancellation.is_cancelled() {
                                    return;
                                }
                                std::thread::sleep(Duration::from_millis(1));
                            }
                            if failed.load(Ordering::Relaxed) {
                                return;
                            }
                            let result = produce(number);
                            let error = result.is_err();
                            if sender.send(result).is_err() || error {
                                failed.store(true, Ordering::Relaxed);
                                return;
                            }
                        });
                    }
                    drop(sender);
                    let result = (|| -> eyre::Result<()> {
                        let mut pending = BTreeMap::new();
                        let mut metadata_bytes = 0;
                        // A superseded writer can only have written blocks of the page it
                        // never published, so reading keys back is needed for one page only.
                        let recheck_end = begin.saturating_add(store::PAGE_RECORDS as u64);
                        // One write per page (or per 64 MiB) instead of one durable write per block.
                        let mut batch = WriteBatch::new();
                        let mut batch_bytes = 0usize;
                        let mut start = begin;
                        while start < end {
                            eyre::ensure!(!cancellation.is_cancelled(), "capture cancelled");
                            let Some(Produced { record, bytes, metadata, case_bytes }) =
                                pending.remove(&start)
                            else {
                                let produced = receiver
                                    .recv()
                                    .map_err(|_| eyre::eyre!("corpus capture workers stopped"))??;
                                pending.insert(produced.record.start, produced);
                                continue;
                            };
                            let stop = record.end;
                            let (transactions, gas, first_tx) =
                                (record.transactions, record.gas, record.first_tx);
                            metadata_bytes += metadata.len();
                            for (key, value) in
                                [(store::key(start), &bytes), (store::metadata_key(start), &metadata)]
                                    .into_iter()
                                    .chain(case_bytes.iter().enumerate().map(|(i, b)| {
                                        (store::transaction_key(first_tx + i as u64), b)
                                    }))
                            {
                                let stored = if start < recheck_end {
                                    handle.block_on(db.get(key))?
                                } else {
                                    None
                                };
                                if let Some(stored) = stored {
                                    eyre::ensure!(
                                        stored.as_ref() == value,
                                        "unpublished record conflicts with canonical capture"
                                    );
                                } else {
                                    batch_bytes += value.len();
                                    batch.put(key, value);
                                }
                            }
                            catalog.records.push(record);
                            let seal = stop == end ||
                                catalog.records.len() >= store::PAGE_RECORDS ||
                                metadata_bytes >= 8 * 1024 * 1024;
                            // A prior writer may have durably written these blocks before
                            // publishing its checkpoint. Verify them above, but do not
                            // submit the empty batch rejected by SlateDB on resume.
                            if (seal || batch_bytes >= MAX_RECORD_BYTES) && !batch.is_empty() {
                                handle.block_on(
                                    db.write(std::mem::replace(&mut batch, WriteBatch::new())),
                                )?;
                                batch_bytes = 0;
                            }
                            written.store(stop, Ordering::Release);
                            // Publication frequency is runtime I/O batching, not part of the schema.
                            // Every batch above is a complete block; no partial block is published.
                            if !seal {
                                start = stop;
                                continue;
                            }
                            let page_cursor = catalog.cursor.clone();
                            catalog.seal()?;
                            metadata_bytes = 0;
                            handle.block_on(
                                db.put(store::CURSOR_KEY, serde_json::to_vec(&catalog.cursor)?),
                            )?;
                            let checkpoint = handle.block_on(store.checkpoint(&db))?;
                            // Check the current canonical pin, not merely the earlier RO snapshot.
                            let current = provider_factory.database_provider_ro()?;
                            eyre::ensure!(
                                current.block_hash(end - 1)?.map(|h| h.to_string()).as_ref() ==
                                    Some(&catalog.context.tip),
                                "canonical pin changed during capture"
                            );
                            catalog.checkpoint = Some(checkpoint);
                            catalog.complete = stop == end;
                            let reader = handle.block_on(store.reader(&catalog))?;
                            let checked = handle.block_on(store::for_each_published(
                                &reader,
                                page_cursor,
                                &catalog.cursor,
                                |record, bytes, _| store::decode(&bytes, record).map(drop),
                            ));
                            handle.block_on(reader.close())?;
                            checked?;
                            version = handle.block_on(store.publish(&catalog, Some(version)))?;
                            info!(
                                start,
                                end = stop,
                                blocks = catalog.cursor.next - catalog.context.start,
                                transactions,
                                gas,
                                complete = catalog.complete,
                                "Published verified corpus blocks and transactions"
                            );
                            start = stop;
                        }
                        Ok(())
                    })();
                    if result.is_err() {
                        failed.store(true, Ordering::Relaxed);
                    }
                    result
                })?;
                Ok(())
            })();
            let closed = handle.block_on(db.close());
            result?;
            closed?;
            Ok(())
        })
        .await?
    }
}

/// One captured, verified block awaiting in-order publication.
struct Produced {
    record: Record,
    bytes: Vec<u8>,
    metadata: Vec<u8>,
    case_bytes: Vec<Vec<u8>>,
}
