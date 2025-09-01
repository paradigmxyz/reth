use crate::{ExecutionPayloadBaseV1, FlashBlock};
use alloy_eips::{eip2718::WithEncoded, BlockNumberOrTag, Decodable2718};
use alloy_primitives::B256;
use eyre::{eyre, OptionExt};
use futures_util::{FutureExt, Stream, StreamExt};
use reth_chain_state::{CanonStateNotifications, CanonStateSubscriptions, ExecutedBlock};
use reth_errors::RethError;
use reth_evm::{
    execute::{BlockBuilder, BlockBuilderOutcome},
    ConfigureEvm,
};
use reth_execution_types::ExecutionOutcome;
use reth_primitives_traits::{
    AlloyBlockHeader, BlockTy, HeaderTy, NodePrimitives, ReceiptTy, RecoveredBlock,
    SignedTransaction,
};
use reth_revm::{cached::CachedReads, database::StateProviderDatabase, db::State};
use reth_rpc_eth_types::{EthApiError, PendingBlock};
use reth_storage_api::{noop::NoopProvider, BlockReaderIdExt, StateProviderFactory};
use std::{
    collections::BTreeMap,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::pin;
use tracing::{debug, error, info, trace};

/// The `FlashBlockService` maintains an in-memory [`PendingBlock`] built out of a sequence of
/// [`FlashBlock`]s.
#[derive(Debug)]
pub struct FlashBlockService<
    N: NodePrimitives,
    S,
    EvmConfig: ConfigureEvm<Primitives = N, NextBlockEnvCtx: Unpin>,
    Provider,
> {
    rx: S,
    current: Option<PendingBlock<N>>,
    blocks: FlashBlockSequence,
    evm_config: EvmConfig,
    provider: Provider,
    canon_receiver: CanonStateNotifications<N>,
    /// Cached state reads for the current block.
    /// Current `PendingBlock` is built out of a sequence of `FlashBlocks`, and executed again when
    /// fb received on top of the same block. Avoid redundant I/O across multiple executions
    /// within the same block.
    cached_state: Option<(B256, CachedReads)>,
}

impl<
        N: NodePrimitives,
        S,
        EvmConfig: ConfigureEvm<Primitives = N, NextBlockEnvCtx: From<ExecutionPayloadBaseV1> + Unpin>,
        Provider: StateProviderFactory
            + CanonStateSubscriptions<Primitives = N>
            + BlockReaderIdExt<
                Header = HeaderTy<N>,
                Block = BlockTy<N>,
                Transaction = N::SignedTx,
                Receipt = ReceiptTy<N>,
            >,
    > FlashBlockService<N, S, EvmConfig, Provider>
{
    /// Constructs a new `FlashBlockService` that receives [`FlashBlock`]s from `rx` stream.
    pub fn new(rx: S, evm_config: EvmConfig, provider: Provider) -> Self {
        Self {
            rx,
            current: None,
            blocks: FlashBlockSequence::new(),
            evm_config,
            canon_receiver: provider.subscribe_to_canonical_state(),
            provider,
            cached_state: None,
        }
    }

    /// Returns the cached reads at the given head hash.
    ///
    /// Returns a new cache instance if this is new `head` hash.
    fn cached_reads(&mut self, head: B256) -> CachedReads {
        if let Some((tracked, cache)) = self.cached_state.take() {
            if tracked == head {
                return cache
            }
        }

        // instantiate a new cache instance
        CachedReads::default()
    }

    /// Updates the cached reads at the given head hash
    fn update_cached_reads(&mut self, head: B256, cached_reads: CachedReads) {
        self.cached_state = Some((head, cached_reads));
    }

    /// Clear the state of the service, including:
    /// - All flashblocks sequence of the current pending block
    /// - Invalidate latest pending block built
    /// - Cache
    fn clear(&mut self) {
        self.blocks.clear();
        self.current.take();
        self.cached_state.take();
    }

    /// Returns the [`ExecutedBlock`] made purely out of [`FlashBlock`]s that were received using
    /// [`Self::add_flash_block`].
    /// Builds a pending block using the configured provider and pool.
    ///
    /// If the origin is the actual pending block, the block is built with withdrawals.
    ///
    /// After Cancun, if the origin is the actual pending block, the block includes the EIP-4788 pre
    /// block contract call using the parent beacon block root received from the CL.
    pub fn execute(&mut self) -> eyre::Result<PendingBlock<N>> {
        let latest = self
            .provider
            .latest_header()?
            .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()))?;
        let latest_hash = latest.hash();

        let attrs = self
            .blocks
            .base()
            .and_then(|v| v.base.clone())
            .ok_or_eyre("Missing base flashblock")?;

        if attrs.parent_hash != latest_hash {
            return Err(eyre!("The base flashblock is old"));
        }

        let state_provider = self.provider.history_by_block_hash(latest.hash())?;

        let mut request_cache = self.cached_reads(latest_hash);
        let cached_db = request_cache.as_db_mut(StateProviderDatabase::new(&state_provider));
        let mut state = State::builder().with_database(cached_db).with_bundle_update().build();

        let mut builder = self
            .evm_config
            .builder_for_next_block(&mut state, &latest, attrs.into())
            .map_err(RethError::other)?;

        builder.apply_pre_execution_changes()?;

        let transactions = self.blocks.iter_ready().flat_map(|v| v.diff.transactions.clone());

        for encoded in transactions {
            let tx = N::SignedTx::decode_2718_exact(encoded.as_ref())?;
            let signer = tx.try_recover()?;
            let tx = WithEncoded::new(encoded, tx.with_signer(signer));
            let _gas_used = builder.execute_transaction(tx)?;
        }

        let BlockBuilderOutcome { execution_result, block, hashed_state, .. } =
            builder.finish(NoopProvider::default())?;

        let execution_outcome = ExecutionOutcome::new(
            state.take_bundle(),
            vec![execution_result.receipts],
            block.number(),
            vec![execution_result.requests],
        );

        // update cached reads
        self.update_cached_reads(latest_hash, request_cache);

        Ok(PendingBlock::with_executed_block(
            Instant::now() + Duration::from_secs(1),
            ExecutedBlock {
                recovered_block: block.into(),
                execution_output: Arc::new(execution_outcome),
                hashed_state: Arc::new(hashed_state),
            },
        ))
    }

    /// Compares tip from the last notification of [`CanonStateSubscriptions`] with last computed
    /// pending block and verifies that the tip is the parent of the pending block.
    ///
    /// Returns:
    /// * `Ok(Some(true))` if tip == parent
    /// * `Ok(Some(false))` if tip != parent
    /// * `Ok(None)` if there weren't any new notifications or the pending block is not built
    /// * `Err` if the cannon state receiver returned an error
    fn verify_pending_block_integrity(
        &mut self,
        cx: &mut Context<'_>,
    ) -> eyre::Result<Option<bool>> {
        let mut tip = None;
        let fut = self.canon_receiver.recv();
        pin!(fut);

        while let Poll::Ready(result) = fut.poll_unpin(cx) {
            tip = result?.tip_checked().map(RecoveredBlock::hash);
        }

        Ok(tip
            .zip(self.current.as_ref().map(PendingBlock::parent_hash))
            .map(|(latest, parent)| latest == parent))
    }
}

impl<
        N: NodePrimitives,
        S: Stream<Item = eyre::Result<FlashBlock>> + Unpin,
        EvmConfig: ConfigureEvm<Primitives = N, NextBlockEnvCtx: From<ExecutionPayloadBaseV1> + Unpin>,
        Provider: StateProviderFactory
            + CanonStateSubscriptions<Primitives = N>
            + BlockReaderIdExt<
                Header = HeaderTy<N>,
                Block = BlockTy<N>,
                Transaction = N::SignedTx,
                Receipt = ReceiptTy<N>,
            > + Unpin,
    > Stream for FlashBlockService<N, S, EvmConfig, Provider>
{
    type Item = eyre::Result<Option<PendingBlock<N>>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // Consume new flashblocks while they're ready
        while let Poll::Ready(Some(result)) = this.rx.poll_next_unpin(cx) {
            match result {
                Ok(flashblock) => this.blocks.insert(flashblock),
                Err(err) => return Poll::Ready(Some(Err(err))),
            }
        }

        // Execute block if there are flashblocks but no last pending block
        let changed = if this.current.is_none() && !this.blocks.is_empty() {
            match this.execute() {
                Ok(block) => this.current = Some(block),
                Err(err) => return Poll::Ready(Some(Err(err))),
            }

            true
        } else {
            false
        };

        // Verify that pending block is following up to the canonical state
        match this.verify_pending_block_integrity(cx) {
            // Integrity check failed: erase last block
            Ok(Some(false)) => Poll::Ready(Some(Ok(None))),
            // Integrity check is OK or skipped: output last block
            Ok(Some(true) | None) => {
                if changed {
                    Poll::Ready(Some(Ok(this.current.clone())))
                } else {
                    Poll::Pending
                }
            }
            // Cannot check integrity: error occurred
            Err(err) => Poll::Ready(Some(Err(err))),
        }
    }
}

/// Simple wrapper around an ordered B-tree to keep track of a sequence of flashblocks by index
#[derive(Debug)]
struct FlashBlockSequence {
    inner: BTreeMap<u64, FlashBlock>,
}

impl FlashBlockSequence {
    const fn new() -> Self {
        Self { inner: BTreeMap::new() }
    }

    /// Inserts a new block into the sequence.
    ///
    /// A [`FlashBlock`] with index 0 resets the set.
    fn insert(&mut self, flashblock: FlashBlock) {
        if flashblock.index == 0 {
            trace!(number=%flashblock.block_number(), "Tracking new flashblock sequence");
            // Flash block at index zero resets the whole state
            self.clear();
            self.inner.insert(flashblock.index, flashblock);
            return
        }

        // only insert if we we previously received the same block, assume we received index 0
        if self.block_number() == Some(flashblock.metadata.block_number) {
            trace!(number=%flashblock.block_number(), index = %flashblock.index, block_count = self.inner.len() + 1  ,"Received followup flashblock");
            self.inner.insert(flashblock.index, flashblock);
        }
    }

    /// Returns the first block number
    fn block_number(&self) -> Option<u64> {
        Some(self.inner.values().next()?.metadata.block_number)
    }

    fn clear(&mut self) {
        self.inner.clear();
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Return base flashblock
    /// Base flashblock is the first flashblock of index 0 in the sequence
    fn base(&self) -> Option<&FlashBlock> {
        self.inner.get(&0)
    }

    /// Iterator over sequence of ready flashblocks
    /// A flashblocks is not ready if there's missing previous flashblocks, i.e. there's a gap in
    /// the sequence
    fn iter_ready(&self) -> impl Iterator<Item = &FlashBlock> {
        self.inner
            .values()
            .enumerate()
            .take_while(|(idx, block)| block.index == *idx as u64)
            .map(|(_, block)| block)
    }
}
