//! Utils for testing purposes.

use crate::{
    traits::KeepPayloadJobAlive, EthBuiltPayload, EthPayloadBuilderAttributes,
    PayloadBuilderHandle, PayloadBuilderService, PayloadJob, PayloadJobGenerator,
};

use alloy_primitives::U256;
use reth_chain_state::{CanonStateNotification, ExecutedBlock};
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::{PayloadKind, PayloadTypes};
use reth_primitives::Block;
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
    T: PayloadTypes<
            PayloadBuilderAttributes = EthPayloadBuilderAttributes,
            BuiltPayload = EthBuiltPayload,
        > + 'static,
{
    PayloadBuilderService::new(Default::default(), futures_util::stream::empty())
}

/// Creates a new [`PayloadBuilderService`] for testing purposes and spawns it in the background.
pub fn spawn_test_payload_service<T>() -> PayloadBuilderHandle<T>
where
    T: PayloadTypes<
            PayloadBuilderAttributes = EthPayloadBuilderAttributes,
            BuiltPayload = EthBuiltPayload,
        > + 'static,
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
        attr: EthPayloadBuilderAttributes,
    ) -> Result<Self::Job, PayloadBuilderError> {
        Ok(TestPayloadJob { attr })
    }
}

/// A [`PayloadJobGenerator`] for testing purposes
#[derive(Debug)]
pub struct TestPayloadJob {
    attr: EthPayloadBuilderAttributes,
}

impl Future for TestPayloadJob {
    type Output = Result<(), PayloadBuilderError>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

impl PayloadJob for TestPayloadJob {
    type PayloadAttributes = EthPayloadBuilderAttributes;
    type ResolvePayloadFuture =
        futures_util::future::Ready<Result<EthBuiltPayload, PayloadBuilderError>>;
    type BuiltPayload = EthBuiltPayload;

    fn best_payload(&self) -> Result<EthBuiltPayload, PayloadBuilderError> {
        Ok(EthBuiltPayload::new(
            self.attr.payload_id(),
            Arc::new(Block::default().seal_slow()),
            U256::ZERO,
            Some(ExecutedBlock::default()),
            Some(Default::default()),
        ))
    }

    fn payload_attributes(&self) -> Result<EthPayloadBuilderAttributes, PayloadBuilderError> {
        Ok(self.attr.clone())
    }

    fn resolve_kind(
        &mut self,
        _kind: PayloadKind,
    ) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive) {
        let fut = futures_util::future::ready(self.best_payload());
        (fut, KeepPayloadJobAlive::No)
    }
}
