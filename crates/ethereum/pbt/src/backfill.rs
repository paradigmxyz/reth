//! Staged execution authenticates PBT commitments and retires MPT work after finality.

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
    stages::MerkleStage, BlockErrorKind, ExecInput, ExecOutput, Stage, StageCheckpoint, StageError,
    StageId, UnwindInput, UnwindOutput,
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
        for id in [StageId::MerkleExecute, StageId::MerkleUnwind] {
            let merkle = pipeline.stage_by_id::<MerkleStage>(id).cloned().ok_or_else(|| {
                eyre::eyre!("PBT backfill requires a configured MerkleStage at {id}")
            })?;
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
        if !matches!(self.merkle, MerkleStage::Unwind { .. }) &&
            let Some(retired) = provider.mpt_retired_at()?
        {
            // Execution has authenticated the BAL against its output. Preparation replayed
            // that BAL and checked PBT roots; no auxiliary MPT root remains after disposal.
            let header = provider
                .sealed_header(input.target())?
                .ok_or_else(|| ProviderError::HeaderNotFound(input.target().into()))?;
            self.strategy.validate_retired_parent(header.header(), Some(retired))?;
            self.strategy.restore_parent(header.hash(), header.state_root(), header.timestamp())?;
            return Ok(ExecOutput::done(StageCheckpoint::new(input.target())))
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
        if provider.mpt_retired_at()?.is_some() {
            self.validate_unwind(provider, input)?;
            return Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
        }
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
        self.strategy.validate_retired_parent(header.header(), provider.mpt_retired_at()?)?;
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
        let mut end =
            number.saturating_add(u64::try_from(limits.max_blocks.get())?).min(input.target());
        let anchor = factory
            .sealed_header(number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(number.into()))?;
        let mut fetched = B256Map::default();
        let mut remaining = limits.max_bal_bytes.get();
        for next in number + 1..=end {
            if remaining == 0 {
                end = next - 1;
                break;
            }
            let header = factory
                .sealed_header(next)?
                .ok_or_else(|| ProviderError::HeaderNotFound(next.into()))?;
            let commitment = header
                .block_access_list_hash()
                .ok_or_else(|| eyre::eyre!("PBT backfill header has no BAL commitment"))?;
            if let Some(raw) = factory.bal_store().get_by_hash(header.hash())? {
                if raw.len() > remaining {
                    eyre::ensure!(next > number + 1, "PBT catch-up BAL byte budget exceeded");
                    end = next - 1;
                    break;
                }
                remaining -= raw.len();
            } else {
                let raw =
                    match download::download_bal(&client, header.hash(), commitment, remaining)
                        .await
                    {
                        Ok(raw) => raw,
                        Err(error) if error.is::<download::BalByteBudgetExceeded>() => {
                            if next == number + 1 {
                                return Err(error)
                            }
                            end = next - 1;
                            break;
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
        let tip =
            factory.sealed_header(end)?.ok_or_else(|| ProviderError::HeaderNotFound(end.into()))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retirement;
    use alloy_primitives::{keccak256, Address, Bytes};
    use reth_ethereum::{
        network::p2p::NoopFullBlockClient,
        provider::{
            db::{tables, transaction::DbTx},
            test_utils::{create_test_provider_factory, insert_headers},
            RawBal,
        },
        Block,
    };
    use reth_trie_pbt::{
        migration::DecodeLimits, persistence::SnapshotStore, AccountData, StateTree,
    };
    use std::num::NonZeroUsize;

    #[tokio::test]
    async fn replay_chunks_stop_at_the_byte_budget_but_reject_one_oversized_bal() -> eyre::Result<()>
    {
        for oversized in [false, true] {
            let directory = tempfile::tempdir()?;
            let store = Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default())?);
            let factory = create_test_provider_factory();
            let state = StateTree::new();
            let raw = if oversized {
                Bytes::from_static(&[0xc1, 0xc0])
            } else {
                Bytes::from_static(&[0xc0])
            };
            let mut headers = Vec::new();
            let mut parent = B256::ZERO;
            for number in 0..=3 {
                let mut header = Block::default().header;
                header.number = number;
                header.timestamp = number;
                header.parent_hash = parent;
                header.state_root = state.root();
                header.block_access_list_hash = Some(keccak256(&raw));
                let header = SealedHeader::seal_slow(header);
                parent = header.hash();
                factory.bal_store().insert(header.num_hash(), RawBal::new(raw.clone()))?;
                factory.bal_store().flush(&[header.num_hash()])?;
                headers.push(header);
            }
            insert_headers(&factory, &headers);
            let database = factory.provider_rw()?;
            for header in &headers {
                database.tx_ref().put::<tables::HeaderNumbers>(header.hash(), header.number())?;
            }
            database.commit()?;
            store.save_block_with_number(headers[0].hash(), 0, &state)?;
            let result = prepare::<_, NoopFullBlockClient>(
                factory,
                NoopFullBlockClient::default(),
                Arc::new(PbtStateRootStrategy::new(store.clone(), 0)),
                CatchupLimits {
                    max_blocks: NonZeroUsize::new(3).unwrap(),
                    max_bal_bytes: NonZeroUsize::new(1).unwrap(),
                },
                ExecInput { target: Some(3), checkpoint: Some(StageCheckpoint::new(0)) },
            )
            .await;
            if oversized {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "PBT catch-up BAL byte budget exceeded"
                );
                assert!(!store.contains_block(headers[1].hash())?);
            } else {
                result?;
                for header in &headers {
                    assert_eq!(store.verify_block(header.hash())?, state.root());
                }
            }
        }
        Ok(())
    }

    #[test]
    fn retired_backfill_skips_mpt_and_rejects_deep_unwind() -> eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default())?);
        let factory = create_test_provider_factory();
        let mut headers = Vec::new();
        let mut state = StateTree::new();
        for number in 0..=3 {
            state.set_account(
                Address::ZERO,
                AccountData { nonce: number + 1, ..Default::default() },
            )?;
            let mut header = Block::default().header;
            header.number = number;
            header.timestamp = number;
            header.state_root = state.root();
            let header = SealedHeader::seal_slow(header);
            if number < 3 {
                store.save_block(header.hash(), &state)?;
            }
            headers.push(header);
        }
        insert_headers(&factory, &headers);
        let provider = factory.provider_rw()?;
        provider.tx_ref().clear::<tables::AccountsTrie>()?;
        provider.tx_ref().clear::<tables::StoragesTrie>()?;
        provider
            .tx_ref()
            .put::<tables::ChainState>(tables::ChainStateKey::PbtMptRetiredBlock, 1)?;
        let mut stage = PbtMerkleStage::<_, NoopFullBlockClient> {
            merkle: MerkleStage::default_unwind(),
            strategy: Arc::new(PbtStateRootStrategy::new(store, 1)),
            limits: CatchupLimits::default(),
            factory: factory.clone(),
            client: NoopFullBlockClient::default(),
            pending: None,
            prepared: None,
        };
        // The unwind-only stage runs before execution/replay has verified target state.
        assert_eq!(
            stage.execute(
                &*provider,
                ExecInput { target: Some(3), checkpoint: Some(StageCheckpoint::new(1)) }
            )?,
            ExecOutput::done(StageCheckpoint::new(3))
        );
        stage.merkle = MerkleStage::default_execution();
        let output = stage.execute(
            &*provider,
            ExecInput { target: Some(2), checkpoint: Some(StageCheckpoint::new(1)) },
        )?;
        assert_eq!(output, ExecOutput::done(StageCheckpoint::new(2)));
        let unwind = UnwindInput { checkpoint: output.checkpoint, unwind_to: 1, bad_block: None };
        assert_eq!(stage.unwind(&*provider, unwind)?.checkpoint.block_number, 1);
        assert_eq!(stage.validate_unwind(&*provider, UnwindInput { unwind_to: 0, ..unwind })
            .unwrap_err().to_string(),
            "database integrity error occurred: cannot unwind past MPT retirement; re-anchor or resync");
        assert_eq!(provider.tx_ref().entries::<tables::AccountsTrie>()?, 0);
        assert_eq!(provider.tx_ref().entries::<tables::StoragesTrie>()?, 0);
        provider
            .tx_ref()
            .put::<tables::ChainState>(tables::ChainStateKey::LastFinalizedBlock, 0)?;
        provider.commit()?;
        assert_eq!(
            retirement::validate_startup(&stage.strategy, &factory).unwrap_err().to_string(),
            "persisted finality is below MPT retirement; re-anchor or resync"
        );
        let provider = factory.provider_rw()?;
        provider
            .tx_ref()
            .put::<tables::ChainState>(tables::ChainStateKey::LastFinalizedBlock, 1)?;
        provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(0))?;
        provider.commit()?;
        assert_eq!(
            retirement::validate_startup(&stage.strategy, &factory).unwrap_err().to_string(),
            "persisted state is below MPT retirement; re-anchor or resync"
        );
        let provider = factory.provider_rw()?;
        provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(2))?;
        provider.commit()?;
        retirement::validate_startup(&stage.strategy, &factory)?;
        Ok(())
    }
}
