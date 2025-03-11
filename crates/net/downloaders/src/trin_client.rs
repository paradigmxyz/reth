use std::{marker::PhantomData, path::PathBuf, sync::Arc};

use alloy_primitives::{b256, Bytes, B256};
use alloy_rlp::{Decodable, Encodable};
use ethportal_api::{
    types::{
        distance::Distance, jsonrpc::request::HistoryJsonRpcRequest, network::Subnetwork,
        portal_wire::MAINNET,
    }, utils::bytes::hex_encode, ContentValue, Enr, HistoryContentKey, HistoryContentValue, OverlayContentKey
};
use portalnet::{
    config::PortalnetConfig,
    discovery::{Discovery, Discv5UdpSocket},
    events::{OverlayRequest, PortalnetEvents},
    overlay::config::FindContentConfig,
};
use reth_consensus::ConsensusError;
use reth_network_p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    priority::Priority,
};
use reth_network_peers::PeerId;
use reth_primitives_traits::{Block, FullBlock};
use serde_json::Value;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};
use tracing::trace;
use trin_history::{spawn_history_heartbeat, spawn_history_network};
use trin_storage::{config::StorageCapacityConfig, PortalStorageConfigFactory};
use trin_validation::oracle::HeaderOracle;
use utp_rs::socket::UtpSocket;

/// Default byte length of chunk to read from chain file.
///
/// Default is 1 GB.
pub const DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE: u64 = 1_000_000_000;

/// Front-end API for fetching chain data from a file.
///
/// Blocks are assumed to be written one after another in a file, as rlp bytes.
///
/// For example, if the file contains 3 blocks, the file is assumed to be encoded as follows:
/// rlp(block1) || rlp(block2) || rlp(block3)
///
/// Blocks are assumed to have populated transactions, so reading headers will also buffer
/// transactions in memory for use in the bodies stage.
///
/// This reads the entire file into memory, so it is not suitable for large files.
#[derive(Debug, Clone)]
pub struct TrinClient<B: Block = reth_primitives::Block> {
    trin: HistoryNetworkWrap,
    _phantom: PhantomData<B>,
}

/// An error that can occur when constructing and using a [`TrinClient`].
#[derive(Debug, Error)]
pub enum TrinClientError {
    /// An error occurred when validating a header from file.
    #[error(transparent)]
    Consensus(#[from] ConsensusError),

    /// An error occurred when opening or reading the file.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// An error occurred when decoding blocks, headers, or rlp headers from the file.
    #[error("{0}")]
    Rlp(alloy_rlp::Error, Vec<u8>),

    /// Custom error message.
    #[error("{0}")]
    Custom(&'static str),
}

impl From<&'static str> for TrinClientError {
    fn from(value: &'static str) -> Self {
        Self::Custom(value)
    }
}

impl<B: FullBlock> TrinClient<B> {
    /// Create a new [TrienClient].
    pub async fn new(datadir: PathBuf) -> Result<Self, TrinClientError> {
        let portalnet_config = PortalnetConfig::default();

        // Initialize base discovery protocol
        let mut discovery = Discovery::new(portalnet_config.clone(), MAINNET.clone()).unwrap();
        let _talk_req_rx = discovery.start().await.unwrap();
        let discovery = Arc::new(discovery);

        // Initialize and spawn uTP socket
        let (_utp_talk_reqs_tx, utp_talk_reqs_rx) = mpsc::unbounded_channel();

        // Initialize validation oracle TODO: NOOP
        let header_oracle = HeaderOracle::default();
        let header_oracle = Arc::new(RwLock::new(header_oracle));

        let portal_subnetworks = 1;
        let enr_cache_capacity = portalnet_config.utp_transfer_limit * 2 * portal_subnetworks;

        let discv5_utp_socket = Discv5UdpSocket::new(
            Arc::clone(&discovery),
            utp_talk_reqs_rx,
            header_oracle.clone(),
            enr_cache_capacity,
        );
        let utp_socket = UtpSocket::with_socket(discv5_utp_socket);
        let utp_socket = Arc::new(utp_socket);
        let storage_config_factory = PortalStorageConfigFactory::new(
            StorageCapacityConfig::Combined { total_mb: 0, subnetworks: vec![Subnetwork::History] },
            discovery.local_enr().node_id(),
            datadir,
        )
        .unwrap();

        let disable_history_storage = false;
        let storage_config =
            storage_config_factory.create(&Subnetwork::History, Distance::ZERO).unwrap();
        let (history_jsonrpc_tx, history_jsonrpc_rx) =
            mpsc::unbounded_channel::<HistoryJsonRpcRequest>();
        header_oracle.write().await.history_jsonrpc_tx = Some(history_jsonrpc_tx.clone());
        let (_history_event_tx, history_event_rx) = mpsc::unbounded_channel::<OverlayRequest>();
        let history_network = trin_history::network::HistoryNetwork::new(
            discovery.clone(),
            utp_socket,
            storage_config,
            portalnet_config.clone(),
            header_oracle,
            disable_history_storage,
        )
        .await
        .unwrap();
        let _event_stream = history_network.overlay.event_stream().await.unwrap();
        let history_network = Arc::new(history_network);

        tokio::spawn(async move {
                let events = PortalnetEvents::new(
                    _talk_req_rx,
                    (Some(_history_event_tx), Some(_event_stream)),
                    (None, None),
                    (None, None),
                    _utp_talk_reqs_tx,
                    MAINNET.clone(),
                )
                .await;
                events.start().await;
            });

        let _history_network_task =
            spawn_history_network(history_network.clone(), portalnet_config, history_event_rx);
        spawn_history_heartbeat(history_network.clone());

        tokio::spawn(_history_network_task);

        Ok(Self { trin: HistoryNetworkWrap(history_network), _phantom: Default::default() })
    }
}

impl<B: FullBlock> BodiesClient for TrinClient<B> {
    type Body = B::Body;
    type Output = BodiesFut<B::Body>;

    fn get_block_bodies_with_priority(
        &self,
        hashes: Vec<B256>,
        _priority: Priority,
    ) -> Self::Output {
        let trin = self.trin.clone();
        Box::pin(async move {
            let mut bodies = vec![];
            for hash in hashes {
                let content_key = HistoryContentKey::new_block_body(hash);
                let (content_bytes, _utp_transfer, _trace) = trin
                    .0
                    .overlay
                    .lookup_content(
                        content_key.clone(),
                        FindContentConfig {
                            is_trace: false,
                            timeout: Some(Duration::from_secs(5))

                        },
                    )
                    .await
                    .unwrap()
                    .unwrap();

                // Format as string.
                let content_response_string = Value::String(hex_encode(content_bytes));
                let content: Bytes = serde_json::from_value(content_response_string)
                    .map_err(|e| e.to_string())
                    .unwrap();

                let block = if let HistoryContentValue::BlockBody(
                    ethportal_api::BlockBody::Legacy(block_body_legacy),
                ) = HistoryContentValue::decode(&content_key, &content).unwrap()
                {
                    let mut tmp = vec![];
                    block_body_legacy.encode(&mut tmp);
                    B::Body::decode(&mut &tmp[..]).unwrap()
                } else {
                    unreachable!();
                };
                bodies.push(block);
            }

            Ok((PeerId::default(), bodies).into())
        })
    }
}

impl<B: FullBlock> DownloadClient for TrinClient<B> {
    fn report_bad_message(&self, _peer_id: PeerId) {
        trace!("Reported a bad message on a file client, the file may be corrupted or invalid");
        // noop
    }

    fn num_connected_peers(&self) -> usize {
        // no such thing as connected peers when we are just using a file
        1
    }
}

/// Wrapper over HistoryNetwork
#[derive(Clone)]
pub struct HistoryNetworkWrap(pub Arc<trin_history::network::HistoryNetwork>);

impl core::fmt::Debug for HistoryNetworkWrap {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "history")
    }
}

#[cfg(test)]
mod tests {
    use crate::trin_client::TrinClient;
    use alloy_primitives::b256;
    use reth_network_p2p::BodiesClient;
    use reth_primitives::{BlockTy, EthPrimitives};
    use std::time::Duration;
    use tempfile::env::temp_dir;

    #[tokio::test(flavor = "multi_thread")]
    async fn run_trin2() {
        reth_tracing::init_test_tracing();
        let datadir = temp_dir();

        // 0x8fe4a257e61805aeff5da6a7507bdb572b103bd97a7882590808e92d30b04b7e

        let client = TrinClient::<BlockTy<EthPrimitives>>::new(datadir).await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
        dbg!(
            &client
                .get_block_bodies(vec![b256!(
                    "0x5f890fe75b8a44e445fb32b3b26c9894b15d0758152950bb456d2b22080c1e7b"
                )])
                .await
        );
    }
}
