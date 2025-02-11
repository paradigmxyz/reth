use core::{
    fmt::Debug,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use reth_payload_builder::{KeepPayloadJobAlive, PayloadJob, PayloadJobGenerator};
use reth_payload_primitives::{
    BuiltPayload, PayloadBuilderAttributes, PayloadBuilderError, PayloadKind,
};

/// A [`PayloadJobGenerator`] that doesn't produce any useful payload.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct NoopPayloadJobGenerator<PA, BP> {
    _types: core::marker::PhantomData<(PA, BP)>,
}

impl<PA, BP> PayloadJobGenerator for NoopPayloadJobGenerator<PA, BP>
where
    PA: PayloadBuilderAttributes + Default + Debug + Send + Sync,
    BP: BuiltPayload + Default + Clone + Debug + Send + Sync + 'static,
{
    type Job = NoopPayloadJob<PA, BP>;

    fn new_payload_job(&self, _attr: PA) -> Result<Self::Job, PayloadBuilderError> {
        Ok(NoopPayloadJob::<PA, BP>::default())
    }
}

/// A [`PayloadJobGenerator`] that doesn't produce any payload.
#[derive(Debug, Default)]
pub struct NoopPayloadJob<PA, BP> {
    _types: core::marker::PhantomData<(PA, BP)>,
}

impl<PA, BP> Future for NoopPayloadJob<PA, BP> {
    type Output = Result<(), PayloadBuilderError>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

impl<PA, BP> PayloadJob for NoopPayloadJob<PA, BP>
where
    PA: PayloadBuilderAttributes + Default + Debug,
    BP: BuiltPayload + Default + Clone + Debug + 'static,
{
    type PayloadAttributes = PA;
    type ResolvePayloadFuture =
        futures_util::future::Ready<Result<Self::BuiltPayload, PayloadBuilderError>>;
    type BuiltPayload = BP;

    fn best_payload(&self) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        Ok(Self::BuiltPayload::default())
    }

    fn payload_attributes(&self) -> Result<Self::PayloadAttributes, PayloadBuilderError> {
        Ok(Self::PayloadAttributes::default())
    }

    fn resolve_kind(
        &mut self,
        _kind: PayloadKind,
    ) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive) {
        let fut = futures_util::future::ready(self.best_payload());
        (fut, KeepPayloadJobAlive::No)
    }
}
