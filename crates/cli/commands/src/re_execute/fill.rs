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
use std::sync::Arc;
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
                let verified = (|| -> eyre::Result<()> {
                    if self.verify_existing {
                        let mut pages = handle.block_on(store::MetadataPages::open(
                            &reader,
                            store::Cursor::initial(&catalog.context)?,
                            catalog.cursor.next,
                        ))?;
                        while pages.cursor.next < catalog.cursor.next {
                            let records = handle.block_on(pages.next_page())?;
                            for record in &records {
                                eyre::ensure!(!cancellation.is_cancelled(), "capture cancelled");
                                let bytes = handle
                                    .block_on(reader.get(store::key(record.start)))?
                                    .ok_or_else(|| eyre::eyre!("missing published record"))?;
                                let chunk = store::decode(&bytes, record)?;
                                let cases = handle.block_on(store::read_cases(&reader, record))?;
                                store::verify_cases(
                                    &chunk,
                                    &cases,
                                    record,
                                    &config,
                                    consensus,
                                    &cancellation,
                                )?;
                            }
                        }
                        eyre::ensure!(
                            pages.cursor == catalog.cursor,
                            "published metadata boundary mismatch"
                        );
                    }
                    Ok(())
                })();
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
                        object_store_max_retries: Some(3),
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
                let mut start = catalog.cursor.next;
                let mut metadata_bytes = 0;
                while start < end {
                    eyre::ensure!(!cancellation.is_cancelled(), "capture cancelled");
                    let stop = start + 1;
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
                        &cancellation,
                    )?);
                    let (transactions, gas) = chunk.replay(&config, consensus, &cancellation)?;
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
                        &cancellation,
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
                    let case_bytes = cases
                        .iter()
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
                    metadata_bytes += metadata.len();
                    let mut batch = WriteBatch::new();
                    for (key, value) in
                        [(store::key(start), &bytes), (store::metadata_key(start), &metadata)]
                            .into_iter()
                            .chain(
                                case_bytes
                                    .iter()
                                    .enumerate()
                                    .map(|(i, b)| (store::transaction_key(first_tx + i as u64), b)),
                            )
                    {
                        if let Some(stored) = handle.block_on(db.get(key))? {
                            eyre::ensure!(
                                stored.as_ref() == value,
                                "unpublished record conflicts with canonical capture"
                            );
                        } else {
                            batch.put(key, value);
                        }
                    }
                    // A prior writer may have durably written this entire block
                    // before publishing its checkpoint. Verify it above, but do
                    // not submit the empty batch rejected by SlateDB on resume.
                    if !batch.is_empty() {
                        handle.block_on(db.write(batch))?;
                    }
                    catalog.records.push(record);
                    // Publication frequency is runtime I/O batching, not part of the schema.
                    // Every batch above is a complete block; no partial block is published.
                    if stop != end &&
                        catalog.records.len() < store::PAGE_RECORDS &&
                        metadata_bytes < 8 * 1024 * 1024
                    {
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
                    let checked = (|| -> eyre::Result<()> {
                        let mut pages = handle.block_on(store::MetadataPages::open(
                            &reader,
                            page_cursor,
                            catalog.cursor.next,
                        ))?;
                        while pages.cursor.next < catalog.cursor.next {
                            let records = handle.block_on(pages.next_page())?;
                            for record in &records {
                                let stored = handle
                                    .block_on(reader.get(store::key(record.start)))?
                                    .ok_or_else(|| {
                                        eyre::eyre!("durable checkpoint is missing new record")
                                    })?;
                                store::decode(&stored, record)?;
                                handle.block_on(store::read_cases(&reader, record))?;
                            }
                        }
                        eyre::ensure!(
                            pages.cursor == catalog.cursor,
                            "durable metadata boundary mismatch"
                        );
                        Ok(())
                    })();
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
            let closed = handle.block_on(db.close());
            result?;
            closed?;
            Ok(())
        })
        .await?
    }
}
