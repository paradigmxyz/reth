//! Utils for testing purposes.

use crate::{
    error::PayloadBuilderError, traits::KeepPayloadJobAlive, EthBuiltPayload,
    EthPayloadBuilderAttributes, PayloadBuilderHandle, PayloadBuilderService, PayloadJob,
    PayloadJobGenerator,
};
use reth_node_api::EngineTypes;
use reth_primitives::{Block, U256};
use reth_provider::CanonStateNotification;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

/// Creates a new [PayloadBuilderService] for testing purposes.
pub fn test_payload_service<Engine>() -> (
    PayloadBuilderService<
        TestPayloadJobGenerator,
        futures_util::stream::Empty<CanonStateNotification>,
        Engine,
    >,
    PayloadBuilderHandle<Engine>,
)
where
    Engine: EngineTypes<
            PayloadBuilderAttributes = EthPayloadBuilderAttributes,
            BuiltPayload = EthBuiltPayload,
        > + 'static,
{
    PayloadBuilderService::new(Default::default(), futures_util::stream::empty())
}

/// Creates a new [PayloadBuilderService] for testing purposes and spawns it in the background.
pub fn spawn_test_payload_service<Engine>() -> PayloadBuilderHandle<Engine>
where
    Engine: EngineTypes<
            PayloadBuilderAttributes = EthPayloadBuilderAttributes,
            BuiltPayload = EthBuiltPayload,
        > + 'static,
{
    let (service, handle) = test_payload_service();
    tokio::spawn(service);
    handle
}

/// A [PayloadJobGenerator] for testing purposes
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

/// A [PayloadJobGenerator] for testing purposes
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
        Ok(EthBuiltPayload::new(self.attr.payload_id(), Block::default().seal_slow(), U256::ZERO))
    }

    fn payload_attributes(&self) -> Result<EthPayloadBuilderAttributes, PayloadBuilderError> {
        Ok(self.attr.clone())
    }

    fn resolve(&mut self) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive) {
        let fut = futures_util::future::ready(self.best_payload());
        (fut, KeepPayloadJobAlive::No)
    }
}
