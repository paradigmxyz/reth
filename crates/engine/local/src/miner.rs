//! Contains the implementation of the mining mode for the local engine.

use alloy_consensus::BlockHeader;
use alloy_primitives::{TxHash, B256};
use alloy_rpc_types_engine::ForkchoiceState;
use eyre::OptionExt;
use futures_util::{stream::Fuse, StreamExt};
use reth_engine_primitives::{BeaconEngineMessage, EngineApiMessageVersion, EngineTypes};
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_builder_primitives::PayloadBuilder;
use reth_payload_primitives::{BuiltPayload, PayloadAttributesBuilder, PayloadKind, PayloadTypes};
use reth_provider::BlockReader;
use reth_transaction_pool::TransactionPool;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, UNIX_EPOCH},
};
use tokio::{
    sync::{mpsc::UnboundedSender, oneshot},
    time::Interval,
};
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;

/// A mining mode for the local dev engine.
#[derive(Debug)]
pub enum MiningMode {
    /// In this mode a block is built as soon as
    /// a valid transaction reaches the pool.
    Instant(Fuse<ReceiverStream<TxHash>>),
    /// In this mode a block is built at a fixed interval.
    Interval(Interval),
}

impl MiningMode {
    /// Constructor for a [`MiningMode::Instant`]
    pub fn instant<Pool: TransactionPool>(pool: Pool) -> Self {
        let rx = pool.pending_transactions_listener();
        Self::Instant(ReceiverStream::new(rx).fuse())
    }

    /// Constructor for a [`MiningMode::Interval`]
    pub fn interval(duration: Duration) -> Self {
        let start = tokio::time::Instant::now() + duration;
        Self::Interval(tokio::time::interval_at(start, duration))
    }
}

impl Future for MiningMode {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match this {
            Self::Instant(rx) => {
                // drain all transactions notifications
                if let Poll::Ready(Some(_)) = rx.poll_next_unpin(cx) {
                    return Poll::Ready(())
                }
                Poll::Pending
            }
            Self::Interval(interval) => {
                if interval.poll_tick(cx).is_ready() {
                    return Poll::Ready(())
                }
                Poll::Pending
            }
        }
    }
}

/// Local miner advancing the chain/
#[derive(Debug)]
pub struct LocalMiner<EngineT: EngineTypes, B> {
    /// The payload attribute builder for the engine
    payload_attributes_builder: B,
    /// Sender for events to engine.
    to_engine: UnboundedSender<BeaconEngineMessage<EngineT>>,
    /// The mining mode for the engine
    mode: MiningMode,
    /// The payload builder for the engine
    payload_builder: PayloadBuilderHandle<EngineT>,
    /// Timestamp for the next block.
    last_timestamp: u64,
    /// Stores latest mined blocks.
    last_block_hashes: Vec<B256>,
}

impl<EngineT, B> LocalMiner<EngineT, B>
where
    EngineT: EngineTypes,
    B: PayloadAttributesBuilder<<EngineT as PayloadTypes>::PayloadAttributes>,
{
    /// Spawns a new [`LocalMiner`] with the given parameters.
    pub fn spawn_new(
        provider: impl BlockReader,
        payload_attributes_builder: B,
        to_engine: UnboundedSender<BeaconEngineMessage<EngineT>>,
        mode: MiningMode,
        payload_builder: PayloadBuilderHandle<EngineT>,
    ) {
        let latest_header =
            provider.sealed_header(provider.best_block_number().unwrap()).unwrap().unwrap();

        let miner = Self {
            payload_attributes_builder,
            to_engine,
            mode,
            payload_builder,
            last_timestamp: latest_header.timestamp(),
            last_block_hashes: vec![latest_header.hash()],
        };

        // Spawn the miner
        tokio::spawn(miner.run());
    }

    /// Runs the [`LocalMiner`] in a loop, polling the miner and building payloads.
    async fn run(mut self) {
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
            head_block_hash: *self.last_block_hashes.last().expect("at least 1 block exists"),
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
        let (tx, rx) = oneshot::channel();
        self.to_engine.send(BeaconEngineMessage::ForkchoiceUpdated {
            state: self.forkchoice_state(),
            payload_attrs: None,
            tx,
            version: EngineApiMessageVersion::default(),
        })?;

        let res = rx.await??;
        if !res.forkchoice_status().is_valid() {
            eyre::bail!("Invalid fork choice update")
        }

        Ok(())
    }

    /// Generates payload attributes for a new block, passes them to FCU and inserts built payload
    /// through newPayload.
    async fn advance(&mut self) -> eyre::Result<()> {
        let timestamp = std::cmp::max(
            self.last_timestamp + 1,
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("cannot be earlier than UNIX_EPOCH")
                .as_secs(),
        );

        let (tx, rx) = oneshot::channel();
        self.to_engine.send(BeaconEngineMessage::ForkchoiceUpdated {
            state: self.forkchoice_state(),
            payload_attrs: Some(self.payload_attributes_builder.build(timestamp)),
            tx,
            version: EngineApiMessageVersion::default(),
        })?;

        let res = rx.await??.await?;
        if !res.payload_status.is_valid() {
            eyre::bail!("Invalid payload status")
        }

        let payload_id = res.payload_id.ok_or_eyre("No payload id")?;

        let Some(Ok(payload)) =
            self.payload_builder.resolve_kind(payload_id, PayloadKind::WaitForPending).await
        else {
            eyre::bail!("No payload")
        };

        let block = payload.block();

        let (tx, rx) = oneshot::channel();
        let (payload, sidecar) = EngineT::block_to_payload(payload.block().clone());
        self.to_engine.send(BeaconEngineMessage::NewPayload {
            payload,
            // todo: prague support
            sidecar,
            tx,
        })?;

        let res = rx.await??;

        if !res.is_valid() {
            eyre::bail!("Invalid payload")
        }

        self.last_timestamp = timestamp;
        self.last_block_hashes.push(block.hash());
        // ensure we keep at most 64 blocks
        if self.last_block_hashes.len() > 64 {
            self.last_block_hashes =
                self.last_block_hashes.split_off(self.last_block_hashes.len() - 64);
        }

        Ok(())
    }
}
