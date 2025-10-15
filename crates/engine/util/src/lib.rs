//! Collection of various stream utilities for consensus engine.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use futures::{Future, Stream};
use reth_engine_primitives::BeaconEngineMessage;
use reth_payload_primitives::PayloadTypes;
use std::path::PathBuf;
use tokio_util::either::Either;

pub mod engine_store;
use engine_store::EngineStoreStream;

pub mod skip_fcu;
use skip_fcu::EngineSkipFcu;

pub mod skip_new_payload;
use skip_new_payload::EngineSkipNewPayload;

pub mod reorg;
use reorg::EngineReorg;

/// The result type for `maybe_reorg` method.
type MaybeReorgResult<S, T, Provider, Evm, Validator, E> =
    Result<Either<EngineReorg<S, T, Provider, Evm, Validator>, S>, E>;

/// The collection of stream extensions for engine API message stream.
pub trait EngineMessageStreamExt<T: PayloadTypes>: Stream<Item = BeaconEngineMessage<T>> {
    /// Skips the specified number of [`BeaconEngineMessage::ForkchoiceUpdated`] messages from the
    /// engine message stream.
    fn skip_fcu(self, count: usize) -> EngineSkipFcu<Self>
    where
        Self: Sized,
    {
        EngineSkipFcu::new(self, count)
    }

    /// If the count is [Some], returns the stream that skips the specified number of
    /// [`BeaconEngineMessage::ForkchoiceUpdated`] messages. Otherwise, returns `Self`.
    fn maybe_skip_fcu(self, maybe_count: Option<usize>) -> Either<EngineSkipFcu<Self>, Self>
    where
        Self: Sized,
    {
        if let Some(count) = maybe_count {
            Either::Left(self.skip_fcu(count))
        } else {
            Either::Right(self)
        }
    }

    /// Skips the specified number of [`BeaconEngineMessage::NewPayload`] messages from the
    /// engine message stream.
    fn skip_new_payload(self, count: usize) -> EngineSkipNewPayload<Self>
    where
        Self: Sized,
    {
        EngineSkipNewPayload::new(self, count)
    }

    /// If the count is [Some], returns the stream that skips the specified number of
    /// [`BeaconEngineMessage::NewPayload`] messages. Otherwise, returns `Self`.
    fn maybe_skip_new_payload(
        self,
        maybe_count: Option<usize>,
    ) -> Either<EngineSkipNewPayload<Self>, Self>
    where
        Self: Sized,
    {
        if let Some(count) = maybe_count {
            Either::Left(self.skip_new_payload(count))
        } else {
            Either::Right(self)
        }
    }

    /// Stores engine messages at the specified location.
    fn store_messages(self, path: PathBuf) -> EngineStoreStream<Self>
    where
        Self: Sized,
    {
        EngineStoreStream::new(self, path)
    }

    /// If the path is [Some], returns the stream that stores engine messages at the specified
    /// location. Otherwise, returns `Self`.
    fn maybe_store_messages(
        self,
        maybe_path: Option<PathBuf>,
    ) -> Either<EngineStoreStream<Self>, Self>
    where
        Self: Sized,
    {
        if let Some(path) = maybe_path {
            Either::Left(self.store_messages(path))
        } else {
            Either::Right(self)
        }
    }

    /// Creates reorgs with specified frequency.
    fn reorg<Provider, Evm, Validator>(
        self,
        provider: Provider,
        evm_config: Evm,
        payload_validator: Validator,
        frequency: usize,
        depth: Option<usize>,
    ) -> EngineReorg<Self, T, Provider, Evm, Validator>
    where
        Self: Sized,
    {
        EngineReorg::new(
            self,
            provider,
            evm_config,
            payload_validator,
            frequency,
            depth.unwrap_or_default(),
        )
    }

    /// If frequency is [Some], returns the stream that creates reorgs with
    /// specified frequency. Otherwise, returns `Self`.
    ///
    /// The `payload_validator_fn` closure is only called if `frequency` is `Some`,
    /// allowing for lazy initialization of the validator.
    fn maybe_reorg<Provider, Evm, Validator, E, F, Fut>(
        self,
        provider: Provider,
        evm_config: Evm,
        payload_validator_fn: F,
        frequency: Option<usize>,
        depth: Option<usize>,
    ) -> impl Future<Output = MaybeReorgResult<Self, T, Provider, Evm, Validator, E>> + Send
    where
        Self: Sized + Send,
        Provider: Send,
        Evm: Send,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Validator, E>> + Send,
    {
        async move {
            if let Some(frequency) = frequency {
                let validator = payload_validator_fn().await?;
                Ok(Either::Left(reorg::EngineReorg::new(
                    self,
                    provider,
                    evm_config,
                    validator,
                    frequency,
                    depth.unwrap_or_default(),
                )))
            } else {
                Ok(Either::Right(self))
            }
        }
    }
}

impl<T, S> EngineMessageStreamExt<T> for S
where
    T: PayloadTypes,
    S: Stream<Item = BeaconEngineMessage<T>>,
{
}
