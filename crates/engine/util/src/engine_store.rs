//! Stores engine API messages to disk for later inspection and replay.

use alloy_rpc_types_engine::{CancunPayloadFields, ExecutionPayload, ForkchoiceState};
use futures::{Stream, StreamExt};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_engine_primitives::EngineTypes;
use reth_fs_util as fs;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    pin::Pin,
    task::{ready, Context, Poll},
    time::SystemTime,
};
use tracing::*;

/// A message from the engine API that has been stored to disk.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StoredEngineApiMessage<Attributes> {
    /// The on-disk representation of an `engine_forkchoiceUpdated` method call.
    ForkchoiceUpdated {
        /// The [`ForkchoiceState`] sent in the persisted call.
        state: ForkchoiceState,
        /// The payload attributes sent in the persisted call, if any.
        payload_attrs: Option<Attributes>,
    },
    /// The on-disk representation of an `engine_newPayload` method call.
    NewPayload {
        /// The [`ExecutionPayload`] sent in the persisted call.
        payload: ExecutionPayload,
        /// The Cancun-specific fields sent in the persisted call, if any.
        cancun_fields: Option<CancunPayloadFields>,
    },
}

/// This can read and write engine API messages in a specific directory.
#[derive(Debug)]
pub struct EngineMessageStore {
    /// The path to the directory that stores the engine API messages.
    path: PathBuf,
}

impl EngineMessageStore {
    /// Creates a new [`EngineMessageStore`] at the given path.
    ///
    /// The path is expected to be a directory, where individual message JSON files will be stored.
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Stores the received [`BeaconEngineMessage`] to disk, appending the `received_at` time to the
    /// path.
    pub fn on_message<Engine>(
        &self,
        msg: &BeaconEngineMessage<Engine>,
        received_at: SystemTime,
    ) -> eyre::Result<()>
    where
        Engine: EngineTypes,
    {
        fs::create_dir_all(&self.path)?; // ensure that store path had been created
        let timestamp = received_at.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis();
        match msg {
            BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx: _tx } => {
                let filename = format!("{}-fcu-{}.json", timestamp, state.head_block_hash);
                fs::write(
                    self.path.join(filename),
                    serde_json::to_vec(&StoredEngineApiMessage::ForkchoiceUpdated {
                        state: *state,
                        payload_attrs: payload_attrs.clone(),
                    })?,
                )?;
            }
            BeaconEngineMessage::NewPayload { payload, cancun_fields, tx: _tx } => {
                let filename = format!("{}-new_payload-{}.json", timestamp, payload.block_hash());
                fs::write(
                    self.path.join(filename),
                    serde_json::to_vec(
                        &StoredEngineApiMessage::<Engine::PayloadAttributes>::NewPayload {
                            payload: payload.clone(),
                            cancun_fields: cancun_fields.clone(),
                        },
                    )?,
                )?;
            }
            // noop
            BeaconEngineMessage::TransitionConfigurationExchanged => (),
        };
        Ok(())
    }

    /// Finds and iterates through any stored engine API message files, ordered by timestamp.
    pub fn engine_messages_iter(&self) -> eyre::Result<impl Iterator<Item = PathBuf>> {
        let mut filenames_by_ts = BTreeMap::<u64, Vec<PathBuf>>::default();
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            let filename = entry.file_name();
            if let Some(filename) = filename.to_str().filter(|n| n.ends_with(".json")) {
                if let Some(Ok(timestamp)) = filename.split('-').next().map(|n| n.parse::<u64>()) {
                    filenames_by_ts.entry(timestamp).or_default().push(entry.path());
                    tracing::debug!(target: "engine::store", timestamp, filename, "Queued engine API message");
                } else {
                    tracing::warn!(target: "engine::store", %filename, "Could not parse timestamp from filename")
                }
            } else {
                tracing::warn!(target: "engine::store", ?filename, "Skipping non json file");
            }
        }
        Ok(filenames_by_ts.into_iter().flat_map(|(_, paths)| paths))
    }
}

/// A wrapper stream that stores Engine API messages in
/// the specified directory.
#[derive(Debug)]
#[pin_project::pin_project]
pub struct EngineStoreStream<S> {
    /// Inner message stream.
    #[pin]
    stream: S,
    /// Engine message store.
    store: EngineMessageStore,
}

impl<S> EngineStoreStream<S> {
    /// Create new engine store stream wrapper.
    pub const fn new(stream: S, path: PathBuf) -> Self {
        Self { stream, store: EngineMessageStore::new(path) }
    }
}

impl<S, Engine> Stream for EngineStoreStream<S>
where
    S: Stream<Item = BeaconEngineMessage<Engine>>,
    Engine: EngineTypes,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        let next = ready!(this.stream.poll_next_unpin(cx));
        if let Some(msg) = &next {
            if let Err(error) = this.store.on_message(msg, SystemTime::now()) {
                error!(target: "engine::stream::store", ?msg, %error, "Error handling Engine API message");
            }
        }
        Poll::Ready(next)
    }
}
