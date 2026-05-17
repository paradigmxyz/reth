//! Utils for testing purposes.

use crate::{
    service::BuildNewPayload, traits::KeepPayloadJobAlive, EthBuiltPayload, PayloadBuilderHandle,
    PayloadBuilderService, PayloadJob, PayloadJobGenerator,
};

use alloy_consensus::Block;
use alloy_primitives::U256;
use alloy_rpc_types::engine::PayloadId;
use reth_chain_state::CanonStateNotification;
use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::{PayloadKind, PayloadTypes};
use reth_primitives_traits::Block as _;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// Creates a new [`PayloadBuilderService`] for testing purposes.
pub fn test_payload_service<T>() -> (
    PayloadBuilderService<
        TestPayloadJobGenerator,
        futures_util::stream::Empty<CanonStateNotification>,
        T,
    >,
    PayloadBuilderHandle<T>,
)
where
    T: PayloadTypes<PayloadAttributes = EthPayloadAttributes, BuiltPayload = EthBuiltPayload>
        + 'static,
{
    PayloadBuilderService::new(Default::default(), futures_util::stream::empty())
}

/// Creates a new [`PayloadBuilderService`] for testing purposes and spawns it in the background.
pub fn spawn_test_payload_service<T>() -> PayloadBuilderHandle<T>
where
    T: PayloadTypes<PayloadAttributes = EthPayloadAttributes, BuiltPayload = EthBuiltPayload>
        + 'static,
{
    let (service, handle) = test_payload_service();
    tokio::spawn(service);
    handle
}

/// A [`PayloadJobGenerator`] for testing purposes
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct TestPayloadJobGenerator;

impl PayloadJobGenerator for TestPayloadJobGenerator {
    type Job = TestPayloadJob;

    fn new_payload_job(
        &self,
        input: BuildNewPayload<EthPayloadAttributes>,
        _id: PayloadId,
    ) -> Result<Self::Job, PayloadBuilderError> {
        Ok(TestPayloadJob { attr: input.attributes })
    }
}

/// A [`PayloadJob`] for testing purposes
#[derive(Debug)]
pub struct TestPayloadJob {
    attr: EthPayloadAttributes,
}

impl Future for TestPayloadJob {
    type Output = Result<(), PayloadBuilderError>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

impl PayloadJob for TestPayloadJob {
    type PayloadAttributes = EthPayloadAttributes;
    type ResolvePayloadFuture =
        futures_util::future::Ready<Result<EthBuiltPayload, PayloadBuilderError>>;
    type BuiltPayload = EthBuiltPayload;

    fn best_payload(&self) -> Result<EthBuiltPayload, PayloadBuilderError> {
        Ok(EthBuiltPayload::new(
            Arc::new(Block::<_>::default().seal_slow()),
            U256::ZERO,
            Some(Default::default()),
            None,
        ))
    }

    fn payload_attributes(&self) -> Result<EthPayloadAttributes, PayloadBuilderError> {
        Ok(self.attr.clone())
    }

    fn payload_timestamp(&self) -> Result<u64, PayloadBuilderError> {
        Ok(self.attr.timestamp)
    }

    fn resolve_kind(
        &mut self,
        _kind: PayloadKind,
    ) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive) {
        let fut = futures_util::future::ready(self.best_payload());
        (fut, KeepPayloadJobAlive::No)
    }
}
