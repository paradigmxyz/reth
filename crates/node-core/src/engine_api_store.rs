//! Stores engine API messages to disk for later inspection and replay.

use reth_beacon_consensus::BeaconEngineMessage;
use reth_node_api::EngineTypes;
use reth_primitives::fs::{self};
use reth_rpc_types::{
    engine::{CancunPayloadFields, ForkchoiceState},
    ExecutionPayload,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, time::SystemTime};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::*;

/// A message from the engine API that has been stored to disk.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StoredEngineApiMessage<Attributes> {
    /// The on-disk representation of an `engine_forkchoiceUpdated` method call.
    ForkchoiceUpdated {
        /// The [ForkchoiceState] sent in the persisted call.
        state: ForkchoiceState,
        /// The payload attributes sent in the persisted call, if any.
        payload_attrs: Option<Attributes>,
    },
    /// The on-disk representation of an `engine_newPayload` method call.
    NewPayload {
        /// The [ExecutionPayload] sent in the persisted call.
        payload: ExecutionPayload,
        /// The Cancun-specific fields sent in the persisted call, if any.
        cancun_fields: Option<CancunPayloadFields>,
    },
}

/// This can read and write engine API messages in a specific directory.
#[derive(Debug)]
pub struct EngineApiStore {
    /// The path to the directory that stores the engine API messages.
    path: PathBuf,
}

impl EngineApiStore {
    /// Creates a new [EngineApiStore] at the given path.
    ///
    /// The path is expected to be a directory, where individual message JSON files will be stored.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Stores the received [BeaconEngineMessage] to disk, appending the `received_at` time to the
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
            BeaconEngineMessage::TransitionConfigurationExchanged |
            BeaconEngineMessage::EventListener(_) => (),
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

    /// Intercepts an incoming engine API message, storing it to disk and forwarding it to the
    /// engine channel.
    pub async fn intercept<Engine>(
        self,
        mut rx: UnboundedReceiver<BeaconEngineMessage<Engine>>,
        to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
    ) where
        Engine: EngineTypes,
        BeaconEngineMessage<Engine>: std::fmt::Debug,
    {
        while let Some(msg) = rx.recv().await {
            if let Err(error) = self.on_message(&msg, SystemTime::now()) {
                error!(target: "engine::intercept", ?msg, %error, "Error handling Engine API message");
            }
            let _ = to_engine.send(msg);
        }
    }
}
