use futures_util::Future;
use reth::tasks::TaskSpawner;
use reth_basic_payload_builder::{HeaderForPayload, PayloadBuilder, PayloadConfig};
use reth_node_api::PayloadKind;
use reth_payload_builder::{KeepPayloadJobAlive, PayloadBuilderError, PayloadJob};

use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// A [PayloadJob] that builds empty blocks.
pub struct EmptyBlockPayloadJob<Tasks, Builder>
where
    Builder: PayloadBuilder,
{
    /// The configuration for how the payload will be created.
    pub(crate) config: PayloadConfig<Builder::Attributes, HeaderForPayload<Builder::BuiltPayload>>,
    /// How to spawn building tasks
    pub(crate) _executor: Tasks,
    /// The type responsible for building payloads.
    ///
    /// See [PayloadBuilder]
    pub(crate) builder: Builder,
}

impl<Tasks, Builder> PayloadJob for EmptyBlockPayloadJob<Tasks, Builder>
where
    Tasks: TaskSpawner + Clone + 'static,
    Builder: PayloadBuilder + Unpin + 'static,
    Builder::Attributes: Unpin + Clone,
    Builder::BuiltPayload: Unpin + Clone,
{
    type PayloadAttributes = Builder::Attributes;
    type ResolvePayloadFuture =
        futures_util::future::Ready<Result<Self::BuiltPayload, PayloadBuilderError>>;
    type BuiltPayload = Builder::BuiltPayload;

    fn best_payload(&self) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let payload = self.builder.build_empty_payload(self.config.clone())?;
        Ok(payload)
    }

    fn payload_attributes(&self) -> Result<Self::PayloadAttributes, PayloadBuilderError> {
        Ok(self.config.attributes.clone())
    }

    fn resolve_kind(
        &mut self,
        _kind: PayloadKind,
    ) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive) {
        let payload = self.best_payload();
        (futures_util::future::ready(payload), KeepPayloadJobAlive::No)
    }
}

/// A [PayloadJob] is a future that's being polled by the `PayloadBuilderService`
impl<Tasks, Builder> Future for EmptyBlockPayloadJob<Tasks, Builder>
where
    Tasks: TaskSpawner + Clone + 'static,
    Builder: PayloadBuilder + Unpin + 'static,
    Builder::Attributes: Unpin + Clone,
    Builder::BuiltPayload: Unpin + Clone,
{
    type Output = Result<(), PayloadBuilderError>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}
