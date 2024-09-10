//! Provides a local dev service engine that can be used to run a dev chain.
//!
//! [`LocalEngineService`] polls the payload builder based on a mining mode
//! which can be set to `Instant` or `Interval`. The `Instant` mode will
//! constantly poll the payload builder and initiate block building
//! with a single transaction. The `Interval` mode will initiate block
//! building at a fixed interval.

use crate::miner::MiningMode;
use futures_util::{future::BoxFuture, FutureExt};
use reth_beacon_consensus::EngineNodeTypes;
use reth_engine_tree::persistence::PersistenceHandle;
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{BuiltPayload, PayloadBuilderAttributes, PayloadTypes};
use reth_primitives::B256;
use reth_provider::ProviderFactory;
use reth_prune::Pruner;
use std::{
    fmt::Formatter,
    future::Future,
    pin::{pin, Pin},
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;
use tracing::debug;

/// Provides a local dev service engine that can be used to drive the
/// chain forward.
pub struct LocalEngineService<N>
where
    N: EngineNodeTypes,
{
    /// The payload builder for the engine
    payload_builder: PayloadBuilderHandle<N::Engine>,
    /// The attributes for the payload builder
    payload_attributes: <N::Engine as PayloadTypes>::PayloadAttributes,
    /// A handle to the persistence layer
    persistence_handle: PersistenceHandle,
    /// The hash of the current head
    head: B256,
    /// The mining mode for the engine
    mode: MiningMode,
    /// Payload request task
    payload_request_state: RequestState,
}

impl<N: EngineNodeTypes> std::fmt::Debug for LocalEngineService<N> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEngineService")
            .field("payload_builder", &self.payload_builder)
            .field("payload_attributes", &self.payload_attributes)
            .field("persistence_handle", &self.persistence_handle)
            .field("head", &self.head)
            .field("mode", &self.mode)
            .field("payload_request_state", &"...")
            .finish()
    }
}

/// The internal state of the [`LocalEngineService`]'s polling task
#[derive(Default)]
struct RequestState {
    ready: bool,
    task: Option<BoxFuture<'static, eyre::Result<B256>>>,
}

impl<N> LocalEngineService<N>
where
    N: EngineNodeTypes,
{
    /// Constructor for [`LocalEngineService`].
    pub fn new(
        payload_builder: PayloadBuilderHandle<N::Engine>,
        payload_attributes: <N::Engine as PayloadTypes>::PayloadAttributes,
        provider: ProviderFactory<N>,
        pruner: Pruner<N::DB, ProviderFactory<N>>,
        head: B256,
        mode: MiningMode,
    ) -> Self {
        let persistence_handle = PersistenceHandle::spawn_service(provider, pruner);

        Self {
            payload_builder,
            payload_attributes,
            persistence_handle,
            head,
            mode,
            payload_request_state: RequestState::default(),
        }
    }

    /// Spawn a [`LocalEngineService`] on a new OS thread.
    pub fn spawn_new(
        payload_builder: PayloadBuilderHandle<N::Engine>,
        payload_attributes: <N::Engine as PayloadTypes>::PayloadAttributes,
        provider: ProviderFactory<N>,
        pruner: Pruner<N::DB, ProviderFactory<N>>,
        head: B256,
        mode: MiningMode,
    ) {
        let engine = Self::new(payload_builder, payload_attributes, provider, pruner, head, mode);

        let _ = tokio::spawn(async { engine.await });
    }
}

/// Run the [`LocalEnginService`] as a Future. The service will poll the payload builder
/// with two varying modes, [`MiningMode::AutoSeal`] or [`MiningMode::Interval`]
/// which will respectively either execute the block as soon as it finds an
/// available payload or build the block based on an interval.
impl<N> Future for LocalEngineService<N>
where
    N: EngineNodeTypes,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            // Wait for the interval or the pool to receive a transaction
            if !this.payload_request_state.ready {
                let _ = ready!(pin!(&mut this.mode).poll(cx));
                this.payload_request_state.ready = true;
            }

            if this.payload_request_state.task.is_none() {
                let head = this.head;
                let attributes = this.payload_attributes.clone();
                let payload_builder = this.payload_builder.clone();
                let persistence_handle = this.persistence_handle.clone();

                let task = Box::pin(async move {
                    let attr = <N::Engine as PayloadTypes>::PayloadBuilderAttributes::try_new(
                        head, attributes,
                    )
                    .map_err(|_| eyre::eyre!("failed to fetch payload attributes"))?;

                    let rx = payload_builder.send_new_payload(attr);
                    let id = rx.await??;

                    let payload = payload_builder
                        .best_payload(id)
                        .await
                        .ok_or_else(|| eyre::eyre!("failed to fetch best payload"))??;

                    let block =
                        payload.executed_block().map(|block| vec![block]).unwrap_or_default();
                    let (tx, rx) = oneshot::channel();

                    let _ = persistence_handle.save_blocks(block, tx);

                    // Wait for the persistence_handle to complete
                    let new_head = rx.await?.ok_or_else(|| eyre::eyre!("missing new head"))?;

                    Result::<_, eyre::Error>::Ok(new_head.hash)
                });
                this.payload_request_state.task = Some(task);
            }

            ready = false;
        }
            if this.payload_request_state.task.is_some() {
                let mut task = this.payload_request_state.task.take().unwrap();
                match task.poll_unpin(cx) {
                    Poll::Ready(Ok(head)) => {
                        this.head = head;
                        this.payload_request_state.ready = false;
                        continue
                    }
                    Poll::Ready(Err(err)) => {
                        // if we get an error, retry directly by creating a new payload request task
                        debug!(target: "local_engine", ?err, "failed block production");
                        continue
                    }
                    Poll::Pending => {
                        this.payload_request_state.task = Some(task);
                        break
                    }
                }
            } else {
                break
            }
        }
        Poll::Pending
    }
}
    }
}
