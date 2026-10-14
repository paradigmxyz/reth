//! Archive capture and resumable, fenced publication of finite corpora.

use super::{
    chunk::Chunk,
    store::{self, Catalog, Context, Record, Store, BYTE_BUDGET, CONTEXT_KEY},
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
use reth_storage_api::{DBProvider, StateProvider};
use slatedb::{config::Settings, Db};
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
    /// Independently verified chunk size. Replay must use the same boundaries.
    #[arg(long, default_value = "5000")]
    blocks_per_chunk: u64,
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
    /// Capture missing chunks. No existing corpus is overwritten on context conflict.
    pub async fn execute<N>(
        self,
        components: impl CliComponentsBuilder<N>,
        runtime: reth_tasks::Runtime,
    ) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
    {
        eyre::ensure!(
            self.from > 0 && self.blocks_per_chunk > 0,
            "from and blocks-per-chunk must be positive"
        );
        let store = Store::open(&self.corpus, self.s3_endpoint.as_deref()).await?;
        let chain = self.env.chain.clone();
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RO, runtime)?;
        let components = components(chain.clone());
        let cancellation = CancellationToken::new();
        let _guard = cancellation.drop_guard();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let expected = provider_factory.database_provider_ro()?.disable_long_read_transaction_safety();
            let best = expected.best_block_number()?;
            let end = self.to.min(best);
            if self.to > best { warn!(requested=self.to, best, "Clamping exclusive end to available head"); }
            eyre::ensure!(end > self.from, "empty/reversed corpus range");
            let canonical_hash = |number| expected.block_hash(number)?.ok_or_else(|| eyre::eyre!("missing canonical hash at {number}"));
            eyre::ensure!(canonical_hash(0)? == chain.genesis_hash(), "source genesis does not match selected chain");
            let context = Context { schema: 1, chain_id: chain.chain_id(), genesis: chain.genesis_hash().to_string(),
                forks: chain.display_hardforks().to_string(), start: self.from, end, parent: canonical_hash(self.from-1)?.to_string(),
                tip: canonical_hash(end-1)?.to_string(), chunk_blocks: self.blocks_per_chunk };
            let existing = handle.block_on(store.catalog())?;
            let (mut catalog, prior) = match existing {
                Some((catalog, version)) => {
                    eyre::ensure!(catalog.context == context, "corpus context conflicts with requested chain/range/chunk configuration");
                    (catalog, Some(version))
                }
                None => (Catalog { context, checkpoint: None, records: Vec::new(), complete: false, generation: 0 }, None),
            };
            let config = components.evm_config().clone().with_jit_support();
            let consensus = components.consensus();
            if catalog.checkpoint.is_some() {
                let reader = handle.block_on(store.reader(&catalog))?;
                let verified = (|| -> eyre::Result<()> {
                    if self.verify_existing {
                        for record in &catalog.records {
                            eyre::ensure!(!cancellation.is_cancelled(), "capture cancelled");
                            let bytes = handle.block_on(reader.get(store::key(record.start)))?.ok_or_else(|| eyre::eyre!("missing published record"))?;
                            let chunk = store::decode(&bytes, record)?;
                            eyre::ensure!(chunk.replay(&config, consensus, &cancellation)? == (record.transactions, record.gas), "published replay totals mismatch");
                        }
                    }
                    Ok(())
                })();
                handle.block_on(reader.close())?;
                verified?;
            }
            if catalog.complete {
                info!(records=catalog.records.len(), verified=self.verify_existing, "Corpus already complete; no writes");
                return Ok(());
            }
            catalog.generation = catalog.generation.checked_add(1).ok_or_else(|| eyre::eyre!("publication generation overflow"))?;
            // Claim before writer open: SlateDB fences database writes, while this
            // CAS token independently fences external catalog publication.
            let mut version = handle.block_on(store.publish(&catalog, prior))?;
            let db = handle.block_on(Db::builder(store.path.clone().join("data"), store.objects.clone())
                .with_settings(Settings { object_store_max_retries: Some(3), ..Default::default() }).build())?;
            let result = (|| -> eyre::Result<()> {
                let context_bytes = serde_json::to_vec(&catalog.context)?;
                if let Some(stored) = handle.block_on(db.get(CONTEXT_KEY))? {
                    eyre::ensure!(stored.as_ref() == context_bytes, "stored corpus context conflict");
                } else {
                    eyre::ensure!(catalog.records.is_empty(), "published corpus has no stored context");
                    handle.block_on(db.put(CONTEXT_KEY, &context_bytes))?;
                }
                let state = BlockchainProvider::new(provider_factory.clone())?;
                let mut start = catalog.records.last().map(|r| r.end).unwrap_or(self.from);
                while start < end {
                    eyre::ensure!(!cancellation.is_cancelled(), "capture cancelled");
                    let stop = start.saturating_add(self.blocks_per_chunk).min(end);
                    let chunk = Arc::new(Chunk::capture(start..stop, &config, consensus, &expected,
                        |at| {
                            let provider = provider_factory.database_provider_ro()?.disable_long_read_transaction_safety();
                            let hash = provider.block_hash(at)?.ok_or_else(|| eyre::eyre!("missing historical canonical block {at}"))?;
                            Ok(StateProviderDatabase(state.state_provider_from_database(provider, hash).into_evm_state_provider()))
                        },
                        |number| provider_factory.recovered_block(number.into(), TransactionVariant::NoHash)?.ok_or_else(|| eyre::eyre!("missing canonical block {number}")),
                        &cancellation)?);
                    let (transactions, gas) = chunk.replay(&config, consensus, &cancellation)?;
                    let bytes = wincode::serialize(chunk.as_ref())?;
                    eyre::ensure!(bytes.len() <= BYTE_BUDGET, "record exceeds 64 MiB serialized budget; choose smaller chunks in a fresh prefix");
                    let record = Record { start, end: stop, parent: chunk.parent, tip: chunk.blocks.last().expect("nonempty capture").hash,
                        bytes: bytes.len(), digest: blake3::hash(&bytes).to_hex().to_string(), transactions, gas };
                    if let Some(stored) = handle.block_on(db.get(store::key(start)))? {
                        eyre::ensure!(stored.as_ref() == bytes, "unpublished corpus record conflicts with canonical capture");
                    } else {
                        handle.block_on(db.put(store::key(start), &bytes))?;
                    }
                    let checkpoint = handle.block_on(store.checkpoint(&db))?;
                    // Check the current canonical pin, not merely the earlier RO snapshot.
                    let current = provider_factory.database_provider_ro()?;
                    eyre::ensure!(current.block_hash(end-1)?.map(|h| h.to_string()).as_ref() == Some(&catalog.context.tip), "canonical pin changed during capture");
                    catalog.records.push(record);
                    catalog.checkpoint = Some(checkpoint);
                    catalog.complete = stop == end;
                    let reader = handle.block_on(store.reader(&catalog))?;
                    let checked = (|| -> eyre::Result<()> {
                        let stored = handle.block_on(reader.get(store::key(start)))?.ok_or_else(|| eyre::eyre!("durable checkpoint is missing new record"))?;
                        store::decode(&stored, catalog.records.last().expect("new record"))?;
                        Ok(())
                    })();
                    handle.block_on(reader.close())?;
                    checked?;
                    version = handle.block_on(store.publish(&catalog, Some(version)))?;
                    info!(start, end=stop, transactions, gas, complete=catalog.complete, "Published verified corpus chunk");
                    start = stop;
                }
                Ok(())
            })();
            let closed = handle.block_on(db.close());
            result?;
            closed?;
            Ok(())
        }).await?
    }
}
