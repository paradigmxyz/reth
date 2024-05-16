//! Collection of various stream utilities for consensus engine.

use futures::Stream;
use reth_beacon_consensus::BeaconEngineMessage;
use reth_engine_primitives::EngineTypes;
use std::path::PathBuf;
use tokio_util::either::Either;

pub mod engine_store;
use engine_store::EngineStoreStream;

pub mod skip_fcu;
use skip_fcu::EngineSkipFcu;

pub mod skip_new_payload;
use skip_new_payload::EngineSkipNewPayload;

/// The collection of stream extensions for engine API message stream.
pub trait EngineMessageStreamExt<Engine: EngineTypes>:
    Stream<Item = BeaconEngineMessage<Engine>>
{
    /// Skips the specified number of [BeaconEngineMessage::ForkchoiceUpdated] messages from the
    /// engine message stream.
    fn skip_fcu(self, count: usize) -> EngineSkipFcu<Self>
    where
        Self: Sized,
    {
        EngineSkipFcu::new(self, count)
    }

    /// If the count is [Some], returns the stream that skips the specified number of
    /// [BeaconEngineMessage::ForkchoiceUpdated] messages. Otherwise, returns `Self`.
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

    /// Skips the specified number of [BeaconEngineMessage::NewPayload] messages from the
    /// engine message stream.
    fn skip_new_payload(self, count: usize) -> EngineSkipNewPayload<Self>
    where
        Self: Sized,
    {
        EngineSkipNewPayload::new(self, count)
    }

    /// If the count is [Some], returns the stream that skips the specified number of
    /// [BeaconEngineMessage::NewPayload] messages. Otherwise, returns `Self`.
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
}

impl<Engine, T> EngineMessageStreamExt<Engine> for T
where
    Engine: EngineTypes,
    T: Stream<Item = BeaconEngineMessage<Engine>>,
{
}
