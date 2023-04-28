//! Utils for testing purposes.

use crate::{
    error::PayloadBuilderError, BuiltPayload, PayloadBuilderAttributes, PayloadBuilderHandle,
    PayloadBuilderService, PayloadJob, PayloadJobGenerator,
};
use reth_primitives::{Block, U256};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// Creates a new [PayloadBuilderService] for testing purposes.
pub fn test_payload_service(
) -> (PayloadBuilderService<TestPayloadJobGenerator>, PayloadBuilderHandle) {
    PayloadBuilderService::new(Default::default())
}

/// Creates a new [PayloadBuilderService] for testing purposes and spawns it in the background.
pub fn spawn_test_payload_service() -> PayloadBuilderHandle {
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
        attr: PayloadBuilderAttributes,
    ) -> Result<Self::Job, PayloadBuilderError> {
        Ok(TestPayloadJob { attr })
    }
}

/// A [PayloadJobGenerator] for testing purposes
#[derive(Debug)]
pub struct TestPayloadJob {
    attr: PayloadBuilderAttributes,
}

impl Future for TestPayloadJob {
    type Output = Result<(), PayloadBuilderError>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

impl PayloadJob for TestPayloadJob {
    fn best_payload(&self) -> Result<Arc<BuiltPayload>, PayloadBuilderError> {
        Ok(Arc::new(BuiltPayload::new(
            self.attr.payload_id(),
            Block::default().seal_slow(),
            U256::ZERO,
        )))
    }
}
