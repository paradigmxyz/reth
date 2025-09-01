use crate::{ExecutionPayloadBaseV1, FlashBlock};
use alloy_eips::{eip2718::WithEncoded, BlockNumberOrTag, Decodable2718};
use alloy_primitives::B256;
use eyre::OptionExt;
use futures_util::{FutureExt, Stream, StreamExt};
use reth_chain_state::{CanonStateNotifications, CanonStateSubscriptions, ExecutedBlock};
use reth_errors::RethError;
use reth_evm::{
    execute::{BlockBuilder, BlockBuilderOutcome},
    ConfigureEvm,
};
use reth_execution_types::ExecutionOutcome;
use reth_primitives_traits::{
    AlloyBlockHeader, BlockTy, HeaderTy, NodePrimitives, ReceiptTy, SignedTransaction,
};
use reth_revm::{cached::CachedReads, database::StateProviderDatabase, db::State};
use reth_rpc_eth_types::{EthApiError, PendingBlock};
use reth_storage_api::{noop::NoopProvider, BlockReaderIdExt, StateProviderFactory};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::pin;
use tracing::{debug, error, info};

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
    blocks: Vec<FlashBlock>,
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
            blocks: Vec::new(),
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

    /// Adds the `block` into the collection.
    ///
    /// Depending on its index and associated block number, it may:
    /// * Be added to all the flashblocks received prior using this function.
    /// * Cause a reset of the flashblocks and become the sole member of the collection.
    /// * Be ignored.
    pub fn add_flash_block(&mut self, flashblock: FlashBlock) {
        // Flash block at index zero resets the whole state
        if flashblock.index == 0 {
            self.clear();
            self.blocks.push(flashblock);
        }
        // Flash block at the following index adds to the collection and invalidates built block
        else if flashblock.index == self.blocks.last().map(|last| last.index + 1).unwrap_or(0) {
            self.blocks.push(flashblock);
            self.current.take();
        }
        // Flash block at a different index is ignored
        else if let Some(pending_block) = self.current.as_ref() {
            // Delete built block if it corresponds to a different height
            if pending_block.block().header().number() == flashblock.metadata.block_number {
                info!(
                    message = "None sequential Flashblocks, keeping cache",
                    curr_block = %pending_block.block().header().number(),
                    new_block = %flashblock.metadata.block_number,
                );
            } else {
                error!(
                    message = "Received Flashblock for new block, zeroing Flashblocks until we receive a base Flashblock",
                    curr_block = %pending_block.block().header().number(),
                    new_block = %flashblock.metadata.block_number,
                );

                self.clear();
            }
        } else {
            debug!("ignoring {flashblock:?}");
        }
    }

    /// Returns the [`ExecutedBlock`] made purely out of [`FlashBlock`]s that were received using
    /// [`Self::add_flash_block`] on top of the latest state.
    ///
    /// Returns None if the flashblock doesn't attach to the latest header.
    fn execute(&mut self) -> eyre::Result<Option<PendingBlock<N>>> {
        let latest = self
            .provider
            .latest_header()?
            .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()))?;
        let latest_hash = latest.hash();

        let attrs = self
            .blocks
            .first()
            .and_then(|v| v.base.clone())
            .ok_or_eyre("Missing base flashblock")?;

        if attrs.parent_hash != latest_hash {
            // doesn't attach to the latest block
            return Ok(None)
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

        let transactions = self.blocks.iter().flat_map(|v| v.diff.transactions.clone());

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

        Ok(Some(PendingBlock::with_executed_block(
            Instant::now() + Duration::from_secs(1),
            ExecutedBlock {
                recovered_block: block.into(),
                execution_output: Arc::new(execution_outcome),
                hashed_state: Arc::new(hashed_state),
            },
        )))
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

        let mut new_flashblock = false;
        // consume new flashblocks while they're ready
        while let Poll::Ready(Some(result)) = this.rx.poll_next_unpin(cx) {
            match result {
                Ok(flashblock) => {
                    new_flashblock = true;
                    this.add_flash_block(flashblock)
                }
                Err(err) => return Poll::Ready(Some(Err(err))),
            }
        }

        // advance new canonical message, if any to reset flashblock
        {
            let fut = this.canon_receiver.recv();
            pin!(fut);
            if fut.poll_unpin(cx).is_ready() {
                // if we have a new canonical message, we know the currently tracked flashblock is
                // invalidated
                if this.current.take().is_some() {
                    return Poll::Ready(Some(Ok(None)))
                }
            }
        }

        if !new_flashblock && this.current.is_none() {
            // no new flashbblocks received since, block is still unchanged
            return Poll::Pending
        }

        // try to build a block on top of latest
        match this.execute() {
            Ok(Some(new_pending)) => {
                // built a new pending block
                this.current = Some(new_pending.clone());
                return Poll::Ready(Some(Ok(Some(new_pending))));
            }
            Ok(None) => {
                // nothing to do because tracked flashblock doesn't attach to latest
            }
            Err(err) => {
                // we can ignore this error
                debug!(%err, "failed to execute flashblock");
            }
        }

        Poll::Pending
    }
}
