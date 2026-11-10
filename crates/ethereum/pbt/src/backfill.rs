//! Staged execution retains MPT nodes while BAL replay authenticates PBT commitments.

use crate::{download, CatchupLimits, Startup};
use alloy_primitives::{map::B256Map, B256};
use reth_engine_tree::{
    backfill::PipelineSync, tree::state_root_strategy::pbt::PbtStateRootStrategy,
};
use reth_ethereum::{
    consensus::ConsensusError,
    network::p2p::BlockAccessListsClient,
    node::builder::sync::{BackfillContext, BackfillSyncBuilder},
    primitives::{AlloyBlockHeader, BlockHeader, GotExpected, SealedHeader},
    provider::{
        db::transaction::DbTxMut, providers::ProviderNodeTypes, BalProvider, ChangeSetReader,
        DBProvider, HeaderProvider, ProviderError, ProviderFactory, StageCheckpointReader,
        StageCheckpointWriter, StatsReader, StorageChangeSetReader, StorageSettingsCache,
        TrieWriter,
    },
    EthPrimitives,
};
use reth_stages::{
    stages::MerkleStage, BlockErrorKind, ExecInput, ExecOutput, Stage, StageError, StageId,
    UnwindInput, UnwindOutput,
};
use reth_trie_pbt::migration::mpt_root;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
    time::Duration,
};
use tracing::{Instrument, Span};

/// PBT-aware staged backfill, retaining the MPT through activation.
#[derive(Debug)]
pub struct PbtBackfill {
    strategy: Arc<PbtStateRootStrategy>,
    limits: CatchupLimits,
}

impl PbtBackfill {
    pub(super) const fn new(strategy: Arc<PbtStateRootStrategy>, limits: CatchupLimits) -> Self {
        Self { strategy, limits }
    }
}

impl<N, C> BackfillSyncBuilder<N, C> for PbtBackfill
where
    N: ProviderNodeTypes<Primitives = EthPrimitives>,
    C: BlockAccessListsClient + Clone + 'static,
{
    type Backfill = PipelineSync<N>;

    fn build(self, ctx: BackfillContext<N, C>) -> eyre::Result<Self::Backfill> {
        let factory = ctx.provider_factory().clone();
        let client = ctx.client().clone();
        let runtime = ctx.runtime().clone();
        let mut pipeline = ctx.into_pipeline();
        for merkle in [MerkleStage::default_execution(), MerkleStage::default_unwind()] {
            pipeline.replace_stage(PbtMerkleStage {
                merkle,
                strategy: self.strategy.clone(),
                limits: self.limits,
                factory: factory.clone(),
                client: client.clone(),
                pending: None,
                prepared: None,
            })?;
        }
        Ok(PipelineSync::new(pipeline, runtime))
    }
}

struct PbtMerkleStage<N: ProviderNodeTypes, C> {
    merkle: MerkleStage,
    strategy: Arc<PbtStateRootStrategy>,
    limits: CatchupLimits,
    factory: ProviderFactory<N>,
    client: C,
    pending: Option<(ExecInput, PrepareFuture)>,
    prepared: Option<ExecInput>,
}

type PrepareFuture = Pin<Box<dyn Future<Output = eyre::Result<()>> + Send>>;

impl<N, C, Provider> Stage<Provider> for PbtMerkleStage<N, C>
where
    N: ProviderNodeTypes<Primitives = EthPrimitives>,
    C: BlockAccessListsClient + Clone + 'static,
    Provider: DBProvider<Tx: DbTxMut>
        + TrieWriter
        + StatsReader
        + HeaderProvider
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + StageCheckpointReader
        + StageCheckpointWriter,
{
    fn id(&self) -> StageId {
        match self.merkle {
            MerkleStage::Unwind { .. } => StageId::MerkleUnwind,
            _ => StageId::MerkleExecute,
        }
    }

    fn poll_execute_ready(
        &mut self,
        cx: &mut Context<'_>,
        input: ExecInput,
    ) -> Poll<Result<(), StageError>> {
        if matches!(self.merkle, MerkleStage::Unwind { .. }) ||
            input.target_reached() ||
            self.prepared == Some(input)
        {
            return Poll::Ready(Ok(()));
        }
        if self.pending.as_ref().is_some_and(|(pending, _)| *pending != input) {
            self.pending = None;
        }
        let (_, pending) = self.pending.get_or_insert_with(|| {
            (
                input,
                Box::pin(
                    prepare(
                        self.factory.clone(),
                        self.client.clone(),
                        self.strategy.clone(),
                        self.limits,
                        input,
                    )
                    .instrument(Span::current()),
                ),
            )
        });
        let result = ready!(pending.as_mut().poll(cx));
        self.pending = None;
        match result {
            Ok(()) => {
                self.prepared = Some(input);
                Poll::Ready(Ok(()))
            }
            Err(error) => Poll::Ready(Err(match error.downcast::<StageError>() {
                Ok(error) => error,
                Err(error) => StageError::Fatal(error.into()),
            })),
        }
    }

    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()));
        }
        let strategy = &self.strategy;
        self.merkle.execute_with_root_validator(provider, input, |mpt, header, _| {
            verify(strategy, mpt, header)
        })
    }

    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        self.pending = None;
        self.prepared = None;
        let strategy = &self.strategy;
        self.merkle.unwind_with_root_validator(provider, input, |mpt, header, _| {
            verify(strategy, mpt, header)
        })
    }

    fn validate_unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<(), StageError> {
        let header = provider
            .sealed_header(input.unwind_to)?
            .ok_or_else(|| ProviderError::HeaderNotFound(input.unwind_to.into()))?;
        self.strategy.restore_parent(header.hash(), header.state_root(), header.timestamp())?;
        Ok(())
    }
}

async fn prepare<N, C>(
    factory: ProviderFactory<N>,
    client: C,
    strategy: Arc<PbtStateRootStrategy>,
    limits: CatchupLimits,
    input: ExecInput,
) -> eyre::Result<()>
where
    N: ProviderNodeTypes<Primitives = EthPrimitives>,
    C: BlockAccessListsClient + Clone + 'static,
{
    let mut number = input.checkpoint().block_number;
    while number < input.target() {
        let end =
            number.saturating_add(u64::try_from(limits.max_blocks.get())?).min(input.target());
        let anchor = factory
            .sealed_header(number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(number.into()))?;
        let tip =
            factory.sealed_header(end)?.ok_or_else(|| ProviderError::HeaderNotFound(end.into()))?;
        let mut fetched = B256Map::default();
        let mut remaining = limits.max_bal_bytes.get();
        for next in number + 1..=end {
            let header = factory
                .sealed_header(next)?
                .ok_or_else(|| ProviderError::HeaderNotFound(next.into()))?;
            let commitment = header
                .block_access_list_hash()
                .ok_or_else(|| eyre::eyre!("PBT backfill header has no BAL commitment"))?;
            if let Some(raw) = factory.bal_store().get_by_hash(header.hash())? {
                remaining = remaining
                    .checked_sub(raw.len())
                    .ok_or_else(|| eyre::eyre!("PBT catch-up BAL byte budget exceeded"))?;
            } else {
                let raw =
                    match download::download_bal(&client, header.hash(), commitment, remaining)
                        .await
                    {
                        Ok(raw) => raw,
                        Err(error) if error.is::<download::BalByteBudgetExceeded>() => {
                            return Err(error)
                        }
                        Err(error) => {
                            // An unavailable peer must stall replay, without a tight stage retry
                            // loop.
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            return Err(StageError::Recoverable(error.into()).into());
                        }
                    };
                remaining = remaining
                    .checked_sub(raw.len())
                    .ok_or_else(|| eyre::eyre!("PBT catch-up BAL byte budget exceeded"))?;
                fetched.insert(header.hash(), raw);
            }
        }
        let replay_factory = factory.clone();
        let replay_strategy = strategy.clone();
        let span = Span::current();
        tokio::task::spawn_blocking(move || {
            let _entered = span.enter();
            Startup { catchup: limits, ..Default::default() }.catch_up_with_bal_fetch(
                &replay_strategy,
                &replay_factory,
                &anchor,
                &tip,
                |hash, _, _| {
                    fetched
                        .remove(&hash)
                        .ok_or_else(|| eyre::eyre!("PBT prefetched BAL is unavailable"))
                },
            )
        })
        .await??;
        number = end;
    }
    Ok(())
}

fn verify<H: BlockHeader>(
    strategy: &PbtStateRootStrategy,
    mpt: B256,
    header: SealedHeader<H>,
) -> Result<(), StageError> {
    let state = strategy.restore_parent(header.hash(), header.state_root(), header.timestamp())?;
    let expected = mpt_root(&state);
    if mpt != expected {
        return Err(StageError::Block {
            block: Box::new(header.block_with_parent()),
            error: BlockErrorKind::Validation(ConsensusError::BodyStateRootDiff(
                GotExpected { got: mpt, expected }.into(),
            )),
        });
    }
    Ok(())
}
