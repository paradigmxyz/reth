use crate::{PayloadBuilderError, PayloadEvents};
use alloy_rpc_types_engine::PayloadId;
use reth_payload_primitives::{PayloadKind, PayloadTypes};
use std::fmt::Debug;
use tokio::sync::oneshot;

/// A helper trait for internal usage to retrieve and resolve payloads.
#[async_trait::async_trait]
pub trait PayloadStoreExt<T: PayloadTypes>: Debug + Send + Sync + Unpin {
    /// Resolves the payload job and returns the best payload that has been built so far.
    async fn resolve_kind(
        &self,
        id: PayloadId,
        kind: PayloadKind,
    ) -> Option<Result<T::BuiltPayload, PayloadBuilderError>>;

    /// Resolves the payload job as fast and possible and returns the best payload that has been
    /// built so far.
    async fn resolve(&self, id: PayloadId) -> Option<Result<T::BuiltPayload, PayloadBuilderError>> {
        self.resolve_kind(id, PayloadKind::Earliest).await
    }

    /// Returns the best payload for the given identifier.
    async fn best_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<T::BuiltPayload, PayloadBuilderError>>;

    /// Returns the payload attributes associated with the given identifier.
    async fn payload_attributes(
        &self,
        id: PayloadId,
    ) -> Option<Result<T::PayloadBuilderAttributes, PayloadBuilderError>>;
}

#[async_trait::async_trait]
impl<T: PayloadTypes, P> PayloadStoreExt<T> for P
where
    P: PayloadBuilder<PayloadType = T>,
{
    async fn resolve_kind(
        &self,
        id: PayloadId,
        kind: PayloadKind,
    ) -> Option<Result<T::BuiltPayload, PayloadBuilderError>> {
        Some(PayloadBuilder::resolve_kind(self, id, kind).await?.map_err(Into::into))
    }

    async fn best_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<T::BuiltPayload, PayloadBuilderError>> {
        Some(PayloadBuilder::best_payload(self, id).await?.map_err(Into::into))
    }

    async fn payload_attributes(
        &self,
        id: PayloadId,
    ) -> Option<Result<T::PayloadBuilderAttributes, PayloadBuilderError>> {
        Some(PayloadBuilder::payload_attributes(self, id).await?.map_err(Into::into))
    }
}

/// A type that can request, subscribe to and resolve payloads.
#[async_trait::async_trait]
pub trait PayloadBuilder: Debug + Send + Sync + Unpin {
    /// The Payload type for the builder.
    type PayloadType: PayloadTypes;
    /// The error type returned by the builder.
    type Error: Into<PayloadBuilderError>;

    /// Sends a message to the service to start building a new payload for the given payload.
    ///
    /// Returns a receiver that will receive the payload id.
    fn send_new_payload(
        &self,
        attr: <Self::PayloadType as PayloadTypes>::PayloadBuilderAttributes,
    ) -> oneshot::Receiver<Result<PayloadId, Self::Error>>;

    /// Returns the best payload for the given identifier.
    async fn best_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<<Self::PayloadType as PayloadTypes>::BuiltPayload, Self::Error>>;

    /// Resolves the payload job and returns the best payload that has been built so far.
    async fn resolve_kind(
        &self,
        id: PayloadId,
        kind: PayloadKind,
    ) -> Option<Result<<Self::PayloadType as PayloadTypes>::BuiltPayload, Self::Error>>;

    /// Resolves the payload job as fast and possible and returns the best payload that has been
    /// built so far.
    async fn resolve(
        &self,
        id: PayloadId,
    ) -> Option<Result<<Self::PayloadType as PayloadTypes>::BuiltPayload, Self::Error>> {
        self.resolve_kind(id, PayloadKind::Earliest).await
    }

    /// Sends a message to the service to subscribe to payload events.
    /// Returns a receiver that will receive them.
    async fn subscribe(&self) -> Result<PayloadEvents<Self::PayloadType>, Self::Error>;

    /// Returns the payload attributes associated with the given identifier.
    async fn payload_attributes(
        &self,
        id: PayloadId,
    ) -> Option<Result<<Self::PayloadType as PayloadTypes>::PayloadBuilderAttributes, Self::Error>>;
}
