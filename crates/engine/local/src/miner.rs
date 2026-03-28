//! Contains the implementation of the mining mode for the local engine.

use alloy_primitives::{TxHash, B256};
use alloy_rpc_types_engine::ForkchoiceState;
use eyre::OptionExt;
use futures_util::{stream::Fuse, Stream, StreamExt};
use reth_engine_primitives::ConsensusEngineHandle;
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{
    BuiltPayload, EngineApiMessageVersion, PayloadAttributesBuilder, PayloadKind, PayloadTypes,
};
use reth_primitives_traits::{HeaderTy, SealedHeaderFor};
use reth_storage_api::BlockReader;
use reth_transaction_pool::TransactionPool;
use std::{
    collections::VecDeque,
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::Interval;
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;

/// A mining mode for the local dev engine.
pub enum MiningMode<Pool: TransactionPool + Unpin> {
    /// In this mode a block is built as soon as
    /// a valid transaction reaches the pool.
    /// If `max_transactions` is set, a block is built when that many transactions have
    /// accumulated.
    Instant {
        /// The transaction pool.
        pool: Pool,
        /// Stream of transaction notifications.
        rx: Fuse<ReceiverStream<TxHash>>,
        /// Maximum number of transactions to accumulate before mining a block.
        /// If None, mine immediately when any transaction arrives.
        max_transactions: Option<usize>,
        /// Counter for accumulated transactions (only used when `max_transactions` is set).
        accumulated: usize,
    },
    /// In this mode a block is built at a fixed interval.
    Interval(Interval),
    /// In this mode a block is built when the trigger stream yields a value.
    ///
    /// This is a general-purpose trigger that can be fired on demand, for example via a channel
    /// or any other [`Stream`] implementation.
    Trigger(Pin<Box<dyn Stream<Item = ()> + Send + Sync>>),
}

impl<Pool: TransactionPool + Unpin> fmt::Debug for MiningMode<Pool> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Instant { max_transactions, accumulated, .. } => f
                .debug_struct("Instant")
                .field("max_transactions", max_transactions)
                .field("accumulated", accumulated)
                .finish(),
            Self::Interval(interval) => f.debug_tuple("Interval").field(interval).finish(),
            Self::Trigger(_) => f.debug_tuple("Trigger").finish(),
        }
    }
}

impl<Pool: TransactionPool + Unpin> MiningMode<Pool> {
    /// Constructor for a [`MiningMode::Instant`]
    pub fn instant(pool: Pool, max_transactions: Option<usize>) -> Self {
        let rx = pool.pending_transactions_listener();
        Self::Instant { pool, rx: ReceiverStream::new(rx).fuse(), max_transactions, accumulated: 0 }
    }

    /// Constructor for a [`MiningMode::Interval`]
    pub fn interval(duration: Duration) -> Self {
        let start = tokio::time::Instant::now() + duration;
        Self::Interval(tokio::time::interval_at(start, duration))
    }

    /// Constructor for a [`MiningMode::Trigger`]
    ///
    /// Accepts any stream that yields `()` values, each of which triggers a new block to be
    /// mined. This can be backed by a channel, a custom stream, or any other async source.
    pub fn trigger(trigger: impl Stream<Item = ()> + Send + Sync + 'static) -> Self {
        Self::Trigger(Box::pin(trigger))
    }
}

impl<Pool: TransactionPool + Unpin> Future for MiningMode<Pool> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match this {
            Self::Instant { pool, rx, max_transactions, accumulated } => {
                // Poll for new transaction notifications
                while let Poll::Ready(Some(_)) = rx.poll_next_unpin(cx) {
                    if pool.pending_and_queued_txn_count().0 == 0 {
                        continue;
                    }
                    if let Some(max_tx) = max_transactions {
                        *accumulated += 1;
                        // If we've reached the max transactions threshold, mine a block
                        if *accumulated >= *max_tx {
                            *accumulated = 0; // Reset counter for next block
                            return Poll::Ready(());
                        }
                    } else {
                        // If no max_transactions is set, mine immediately
                        return Poll::Ready(());
                    }
                }
                Poll::Pending
            }
            Self::Interval(interval) => {
                if interval.poll_tick(cx).is_ready() {
                    return Poll::Ready(())
                }
                Poll::Pending
            }
            Self::Trigger(trigger) => {
                if trigger.poll_next_unpin(cx).is_ready() {
                    return Poll::Ready(())
                }
                Poll::Pending
            }
        }
    }
}

/// Local miner advancing the chain
#[derive(Debug)]
pub struct LocalMiner<T: PayloadTypes, B, Pool: TransactionPool + Unpin> {
    /// The payload attribute builder for the engine
    payload_attributes_builder: B,
    /// Sender for events to engine.
    to_engine: ConsensusEngineHandle<T>,
    /// The mining mode for the engine
    mode: MiningMode<Pool>,
    /// The payload builder for the engine
    payload_builder: PayloadBuilderHandle<T>,
    /// Latest block in the chain so far.
    last_header: SealedHeaderFor<<T::BuiltPayload as BuiltPayload>::Primitives>,
    /// Stores latest mined blocks.
    last_block_hashes: VecDeque<B256>,
}

impl<T, B, Pool> LocalMiner<T, B, Pool>
where
    T: PayloadTypes,
    B: PayloadAttributesBuilder<
        T::PayloadAttributes,
        HeaderTy<<T::BuiltPayload as BuiltPayload>::Primitives>,
    >,
    Pool: TransactionPool + Unpin,
{
    /// Spawns a new [`LocalMiner`] with the given parameters.
    pub fn new(
        provider: impl BlockReader<Header = HeaderTy<<T::BuiltPayload as BuiltPayload>::Primitives>>,
        payload_attributes_builder: B,
        to_engine: ConsensusEngineHandle<T>,
        mode: MiningMode<Pool>,
        payload_builder: PayloadBuilderHandle<T>,
    ) -> Self {
        let last_header =
            provider.sealed_header(provider.best_block_number().unwrap()).unwrap().unwrap();

        Self {
            payload_attributes_builder,
            to_engine,
            mode,
            payload_builder,
            last_block_hashes: VecDeque::from([last_header.hash()]),
            last_header,
        }
    }

    /// Runs the [`LocalMiner`] in a loop, polling the miner and building payloads.
    pub async fn run(mut self) {
        let mut fcu_interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                // Wait for the interval or the pool to receive a transaction
                _ = &mut self.mode => {
                    if let Err(e) = self.advance().await {
                        error!(target: "engine::local", "Error advancing the chain: {:?}", e);
                    }
                }
                // send FCU once in a while
                _ = fcu_interval.tick() => {
                    if let Err(e) = self.update_forkchoice_state().await {
                        error!(target: "engine::local", "Error updating fork choice: {:?}", e);
                    }
                }
            }
        }
    }

    /// Returns current forkchoice state.
    fn forkchoice_state(&self) -> ForkchoiceState {
        ForkchoiceState {
            head_block_hash: *self.last_block_hashes.back().expect("at least 1 block exists"),
            safe_block_hash: *self
                .last_block_hashes
                .get(self.last_block_hashes.len().saturating_sub(32))
                .expect("at least 1 block exists"),
            finalized_block_hash: *self
                .last_block_hashes
                .get(self.last_block_hashes.len().saturating_sub(64))
                .expect("at least 1 block exists"),
        }
    }

    /// Sends a FCU to the engine.
    async fn update_forkchoice_state(&self) -> eyre::Result<()> {
        let state = self.forkchoice_state();
        let res = self
            .to_engine
            .fork_choice_updated(state, None, EngineApiMessageVersion::default())
            .await?;

        if !res.is_valid() {
            eyre::bail!("Invalid fork choice update {state:?}: {res:?}")
        }

        Ok(())
    }

    /// Generates payload attributes for a new block, passes them to FCU and inserts built payload
    /// through newPayload.
    async fn advance(&mut self) -> eyre::Result<()> {
        let res = self
            .to_engine
            .fork_choice_updated(
                self.forkchoice_state(),
                Some(self.payload_attributes_builder.build(&self.last_header)),
                EngineApiMessageVersion::default(),
            )
            .await?;

        if !res.is_valid() {
            eyre::bail!("Invalid payload status")
        }

        let payload_id = res.payload_id.ok_or_eyre("No payload id")?;

        let Some(Ok(payload)) =
            self.payload_builder.resolve_kind(payload_id, PayloadKind::WaitForPending).await
        else {
            eyre::bail!("No payload")
        };

        let header = payload.block().sealed_header().clone();
        let payload = T::block_to_payload(payload.block().clone());
        let res = self.to_engine.new_payload(payload).await?;

        if !res.is_valid() {
            eyre::bail!("Invalid payload")
        }

        self.last_block_hashes.push_back(header.hash());
        self.last_header = header;
        // ensure we keep at most 64 blocks
        if self.last_block_hashes.len() > 64 {
            self.last_block_hashes.pop_front();
        }

        Ok(())
    }
}
