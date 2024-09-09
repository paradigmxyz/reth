//! Provides a local dev service engine that can be used to run a dev chain.
//!
//! [`LocalEngineService`] polls the payload builder based on a mining mode
//! which can be set to `Instant` or `Interval`. The `Instant` mode will
//! constantly poll the payload builder and initiate block building
//! with a single transaction. The `Interval` mode will initiate block
//! building at a fixed interval.

use crate::miner::MiningMode;
use futures::FutureExt;
use reth_beacon_consensus::EngineNodeTypes;
use reth_engine_tree::persistence::PersistenceHandle;
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{BuiltPayload, PayloadBuilderAttributes, PayloadTypes};
use reth_primitives::B256;
use reth_provider::ProviderFactory;
use reth_prune::Pruner;
use std::{
    future::Future,
    pin::{pin, Pin},
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;

/// Provides a local dev service engine that can be used to drive the
/// chain forward.
#[derive(Debug)]
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

        Self { payload_builder, payload_attributes, persistence_handle, head, mode }
    }

    /// Spawn a [`LocalEngineService`] on a new OS thread.
    pub fn spawn_new(
        payload_builder: PayloadBuilderHandle<N::Engine>,
        payload_attributes: <N::Engine as PayloadTypes>::PayloadAttributes,
        provider: ProviderFactory<N>,
        pruner: Pruner<N::DB, ProviderFactory<N>>,
        head: B256,
        mode: MiningMode,
    ) -> Result<(), LocalEngineServiceError> {
        let engine = Self::new(payload_builder, payload_attributes, provider, pruner, head, mode);

        let _ = tokio::spawn(async { engine.await });

        Ok(())
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
        let mut head = this.head;
        let mut ready = false;
        loop {
            // Wait for the interval or the pool to receive a transaction
            if !ready {
                let _ = ready!(pin!(&mut this.mode).poll(cx));
            }

            // TODO: for now we clone this.payload_attributes, waiting for better solution
            let attr = <N::Engine as PayloadTypes>::PayloadBuilderAttributes::try_new(
                head,
                this.payload_attributes.clone(),
            );

            // TODO: with this, we should keep trying to get a attribute because
            // continue means we skip the poll transaction, we shouldn't do this
            if attr.is_err() {
                continue;
            }

            let attr = attr.unwrap();
            let fut = pin!(this.payload_builder.new_payload(attr));
            let payload_id = ready!(fut.poll(cx));
            // TODO: again, we should probably not continue here but actually handle it
            if payload_id.is_err() {
                continue;
            }

            let payload_id = payload_id.unwrap();
            let fut = pin!(this.payload_builder.best_payload(payload_id));
            let best_payload = ready!(fut.poll(cx));
            if let Some(payload) = best_payload {
                // TODO don't unwrap
                let build_payload = payload.unwrap();
                // TODO don't unwrap
                let block = vec![build_payload.executed_block().unwrap()];
                let (tx, mut rx) = oneshot::channel();

                let _ = this.persistence_handle.save_blocks(block, tx);

                // Wait for the persistence_handle to complete
                let hash = ready!(rx.poll_unpin(cx));

                // TODO: again, we should probably not continue here but actually handle
                // it
                if hash.is_err() {
                    continue
                }
                let hash = hash.unwrap();
                if let Some(new_head) = hash {
                    head = new_head.hash;
                }
            }

            ready = false;
        }
    }
}
