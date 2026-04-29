//! Snap sync lifecycle controller.
//!
//! The controller owns Phase A header download and then spawns the engine-driven
//! [`SnapSyncOrchestrator`](crate::orchestrator::SnapSyncOrchestrator) for
//! Phase B state download and BAL catch-up.

use crate::{SnapSyncError, SnapSyncEvent, SnapSyncOutcome};
use alloy_consensus::BlockHeader;
use futures::FutureExt;
use reth_config::config::EtlConfig;
use reth_consensus::noop::NoopConsensus;
use reth_db_api::{
    table::Value,
    transaction::{DbTx, DbTxMut},
};
use reth_downloaders::headers::reverse_headers::ReverseHeadersDownloaderBuilder;
use reth_network_p2p::{headers::client::HeadersClient, snap::client::SnapClient};
use reth_primitives_traits::{FullBlockHeader, NodePrimitives};
use reth_provider::{
    providers::StaticFileWriter, DatabaseProviderFactory, HeaderProvider,
    StaticFileProviderFactory, StorageSettingsCache,
};
use reth_stages::stages::HeaderStage;
use reth_stages_api::{ExecInput, Stage, StageCheckpoint, StageExt, StageId};
use reth_storage_api::{
    DBProvider, HeaderSyncGapProvider, NodePrimitivesProvider, StageCheckpointWriter, StateWriter,
};
use reth_tasks::Runtime;
use std::task::{Context, Poll};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

/// Events emitted by [`SnapSyncController`].
#[derive(Debug)]
pub enum SnapSyncControlEvent {
    /// Phase B started and engine events can now be forwarded through the sender.
    Started(UnboundedSender<SnapSyncEvent>),
    /// Snap sync finished.
    Finished(Result<SnapSyncOutcome, SnapSyncError>),
    /// A controller task was dropped or Phase A failed.
    TaskDropped(String),
}

/// Snap sync lifecycle control surface.
pub trait SnapSyncControl: Send {
    /// Returns `true` if snap sync is active.
    fn is_active(&self) -> bool;

    /// Starts snap sync toward the target block hash.
    fn start(&mut self, target_hash: alloy_primitives::B256) -> bool;

    /// Polls snap sync for its next lifecycle event.
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<SnapSyncControlEvent>;
}

/// Header type used by snap sync for the given provider factory.
pub type SnapSyncHeader<F> =
    <<<F as DatabaseProviderFactory>::ProviderRW as NodePrimitivesProvider>::Primitives as NodePrimitives>::BlockHeader;

/// A snap sync controller that manages the snap sync lifecycle.
///
/// Snap sync runs in two phases:
/// - **Phase A**: download headers 1..pivot via `HeaderStage` so that static files are populated
///   before any state download begins.
/// - **Phase B**: hand off to [`SnapSyncOrchestrator`](crate::orchestrator::SnapSyncOrchestrator)
///   for state download and BAL catch-up.
#[derive(Debug)]
pub struct SnapSyncController<C, F> {
    client: C,
    factory: F,
    runtime: Runtime,
    /// The target hash passed to [`Self::start`], kept until Phase A completes.
    target_hash: Option<alloy_primitives::B256>,
    /// State of the snap sync: `None` = idle, `Some` = running.
    state: Option<SnapSyncState>,
}

/// Running state of snap sync.
#[derive(Debug)]
enum SnapSyncState {
    /// Phase A: downloading headers via `HeaderStage`.
    DownloadingHeaders { result_rx: oneshot::Receiver<Result<(), String>> },
    /// Phase B: state download via orchestrator.
    DownloadingState {
        /// Sender for forwarding engine events to the orchestrator.
        ///
        /// Kept alive so the orchestrator's receiver doesn't close prematurely.
        #[expect(dead_code)]
        events_tx: UnboundedSender<SnapSyncEvent>,
        /// Receiver for the orchestrator result.
        result_rx: oneshot::Receiver<Result<SnapSyncOutcome, SnapSyncError>>,
    },
}

impl<C, F> SnapSyncController<C, F> {
    /// Creates a new controller.
    pub fn new(client: C, factory: F, runtime: Runtime) -> Self {
        Self { client, factory, runtime, target_hash: None, state: None }
    }

    /// Returns `true` if snap sync is currently active.
    pub const fn is_active(&self) -> bool {
        self.state.is_some()
    }
}

impl<C, F> SnapSyncController<C, F>
where
    C: SnapClient + HeadersClient<Header = SnapSyncHeader<F>> + Clone + Send + Sync + 'static,
    F: DatabaseProviderFactory
        + StaticFileProviderFactory
        + HeaderSyncGapProvider<Header = SnapSyncHeader<F>>
        + Clone
        + Send
        + Sync
        + 'static,
    F::Provider: DBProvider + HeaderProvider + StorageSettingsCache,
    F::ProviderRW: DBProvider
        + NodePrimitivesProvider
        + StateWriter
        + StaticFileProviderFactory
        + StageCheckpointWriter,
    <F::Provider as DBProvider>::Tx: DbTx,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
    SnapSyncHeader<F>: Value + FullBlockHeader,
{
    /// Starts snap sync.
    ///
    /// Returns `false` if snap sync is already active.
    pub fn start(&mut self, target_hash: alloy_primitives::B256) -> bool {
        if self.is_active() {
            return false;
        }

        let (result_tx, result_rx) = oneshot::channel();
        let client = self.client.clone();
        let factory = self.factory.clone();

        self.runtime.spawn_critical_blocking_task("snap sync header download", async move {
            let result = Self::run_header_stage(client, factory, target_hash).await;
            let _ = result_tx.send(result);
        });

        self.target_hash = Some(target_hash);
        self.state = Some(SnapSyncState::DownloadingHeaders { result_rx });
        true
    }

    /// Phase A: resolve the pivot from peers and run `HeaderStage` to fill
    /// static files with headers 1..pivot.
    async fn run_header_stage(
        client: C,
        factory: F,
        target_hash: alloy_primitives::B256,
    ) -> Result<(), String> {
        tracing::info!(target: "sync::snap", %target_hash, "Phase A: resolving target header from peers");

        let target_header = client
            .get_header(alloy_eips::BlockHashOrNumber::Hash(target_hash))
            .await
            .map_err(|e| format!("failed to fetch target header: {e}"))?
            .into_data()
            .ok_or_else(|| "peer returned empty response for target header".to_string())?;
        let target_number = target_header.number();

        let pivot_number = target_number.saturating_sub(crate::PIVOT_OFFSET);
        if pivot_number == 0 {
            tracing::info!(target: "sync::snap", "Target too low for header stage, skipping Phase A");
            return Ok(());
        }

        let pivot_header = client
            .get_header(alloy_eips::BlockHashOrNumber::Number(pivot_number))
            .await
            .map_err(|e| format!("failed to fetch pivot header: {e}"))?
            .into_data()
            .ok_or_else(|| "peer returned empty response for pivot header".to_string())?;
        let pivot_sealed = reth_primitives_traits::SealedHeader::seal_slow(pivot_header);
        let pivot_hash = pivot_sealed.hash();

        tracing::info!(
            target: "sync::snap",
            target_number,
            pivot_number,
            %pivot_hash,
            "Phase A: downloading headers 1..{pivot_number}"
        );

        let (_tip_tx, tip_rx) = tokio::sync::watch::channel(pivot_hash);
        let downloader =
            ReverseHeadersDownloaderBuilder::default().build(client, NoopConsensus::arc());

        let mut stage = HeaderStage::new(factory.clone(), downloader, tip_rx, EtlConfig::default());

        let input =
            ExecInput { target: Some(pivot_number), checkpoint: Some(StageCheckpoint::new(0)) };

        <HeaderStage<F, _> as StageExt<F::ProviderRW>>::execute_ready(&mut stage, input)
            .await
            .map_err(|e| format!("header download failed: {e}"))?;

        let provider_rw = factory.database_provider_rw().map_err(|e| format!("db error: {e}"))?;
        let _output = Stage::<F::ProviderRW>::execute(&mut stage, &provider_rw, input)
            .map_err(|e| format!("header write failed: {e}"))?;

        provider_rw
            .save_stage_checkpoint(StageId::Headers, StageCheckpoint::new(pivot_number))
            .map_err(|e| format!("checkpoint save failed: {e}"))?;

        provider_rw.commit().map_err(|e| format!("commit failed: {e}"))?;

        factory
            .static_file_provider()
            .commit()
            .map_err(|e| format!("static file commit failed: {e}"))?;

        tracing::info!(target: "sync::snap", pivot_number, "Phase A complete: headers written to static files");
        Ok(())
    }

    /// Spawn the Phase B orchestrator and return the started event.
    fn start_orchestrator(&mut self) -> SnapSyncControlEvent {
        let target_hash = self.target_hash.take().expect("target_hash set during start()");
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();

        let orchestrator = crate::orchestrator::SnapSyncOrchestrator::new(
            self.client.clone(),
            self.factory.clone(),
        );

        self.runtime.spawn_critical_blocking_task("snap sync orchestrator", async move {
            let result = orchestrator.run(events_rx, target_hash).await;
            let _ = result_tx.send(result);
        });

        let started_tx = events_tx.clone();
        self.state = Some(SnapSyncState::DownloadingState { events_tx, result_rx });
        SnapSyncControlEvent::Started(started_tx)
    }

    /// Polls the controller for the next lifecycle event.
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<SnapSyncControlEvent> {
        let Some(state) = &mut self.state else {
            return Poll::Pending;
        };

        match state {
            SnapSyncState::DownloadingHeaders { result_rx } => match result_rx.poll_unpin(cx) {
                Poll::Ready(Ok(Ok(()))) => Poll::Ready(self.start_orchestrator()),
                Poll::Ready(Ok(Err(e))) => {
                    self.state = None;
                    self.target_hash = None;
                    Poll::Ready(SnapSyncControlEvent::TaskDropped(format!(
                        "snap sync header download failed: {e}"
                    )))
                }
                Poll::Ready(Err(_)) => {
                    self.state = None;
                    self.target_hash = None;
                    Poll::Ready(SnapSyncControlEvent::TaskDropped(
                        "snap sync header download task dropped".into(),
                    ))
                }
                Poll::Pending => Poll::Pending,
            },
            SnapSyncState::DownloadingState { result_rx, .. } => match result_rx.poll_unpin(cx) {
                Poll::Ready(Ok(result)) => {
                    self.state = None;
                    self.target_hash = None;
                    Poll::Ready(SnapSyncControlEvent::Finished(result))
                }
                Poll::Ready(Err(_)) => {
                    self.state = None;
                    self.target_hash = None;
                    Poll::Ready(SnapSyncControlEvent::TaskDropped("snap sync task dropped".into()))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

impl<C, F> SnapSyncControl for SnapSyncController<C, F>
where
    C: SnapClient + HeadersClient<Header = SnapSyncHeader<F>> + Clone + Send + Sync + 'static,
    F: DatabaseProviderFactory
        + StaticFileProviderFactory
        + HeaderSyncGapProvider<Header = SnapSyncHeader<F>>
        + Clone
        + Send
        + Sync
        + 'static,
    F::Provider: DBProvider + HeaderProvider + StorageSettingsCache,
    F::ProviderRW: DBProvider
        + NodePrimitivesProvider
        + StateWriter
        + StaticFileProviderFactory
        + StageCheckpointWriter,
    <F::Provider as DBProvider>::Tx: DbTx,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
    SnapSyncHeader<F>: Value + FullBlockHeader,
{
    fn is_active(&self) -> bool {
        SnapSyncController::is_active(self)
    }

    fn start(&mut self, target_hash: alloy_primitives::B256) -> bool {
        SnapSyncController::start(self, target_hash)
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<SnapSyncControlEvent> {
        SnapSyncController::poll(self, cx)
    }
}
