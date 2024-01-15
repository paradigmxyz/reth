mod message;
use parking_lot::RwLock;
use reth_interfaces::clayer::ClayerConsensus;
use std::{collections::VecDeque, sync::Arc};
use tracing::error;

use tokio::sync::{
    mpsc,
    mpsc::{Receiver, Sender},
};

#[derive(Debug, Clone)]
pub struct ClayerConsensusEngine {
    pub inner: Arc<RwLock<ClayerConsensusEngineInner>>,
}

impl ClayerConsensusEngine {
    pub fn new(is_validator: bool) -> Self {
        Self { inner: Arc::new(RwLock::new(ClayerConsensusEngineInner::new(is_validator))) }
    }

    pub fn is_validator(&self) -> bool {
        self.inner.read().is_validator
    }
}

impl ClayerConsensus for ClayerConsensusEngine {
    /// Returns pending consensus listener
    fn pending_consensus_listener(&self) -> Receiver<reth_primitives::Bytes> {
        self.inner.write().pending_consensus_listener()
    }

    /// push data received from network into cache
    fn push_cache(&self, data: reth_primitives::Bytes) {
        self.inner.write().push_cache(data);
    }
    /// pop data received from network out cache
    fn pop_cache(&self) -> Option<reth_primitives::Bytes> {
        self.inner.write().pop_cache()
    }
    /// broadcast consensus
    fn broadcast_consensus(&self, data: reth_primitives::Bytes) {
        self.inner.read().broadcast_consensus(data);
    }
}

#[derive(Debug, Clone)]
pub struct ClayerConsensusEngineInner {
    pub is_validator: bool,
    queued: VecDeque<reth_primitives::Bytes>,
    sender: Option<Sender<reth_primitives::Bytes>>,
}

impl ClayerConsensusEngineInner {
    pub fn new(is_validator: bool) -> Self {
        Self { is_validator, queued: VecDeque::default(), sender: None }
    }

    fn pending_consensus_listener(&mut self) -> Receiver<reth_primitives::Bytes> {
        let (sender, rx) = mpsc::channel(1024);
        self.sender = Some(sender);
        rx
    }

    fn push_cache(&mut self, data: reth_primitives::Bytes) {
        self.queued.push_back(data);
    }

    fn pop_cache(&mut self) -> Option<reth_primitives::Bytes> {
        self.queued.pop_front()
    }

    fn broadcast_consensus(&self, data: reth_primitives::Bytes) {
        if let Some(sender) = &self.sender {
            match sender.try_send(data) {
                Ok(()) => {}
                Err(err) => {
                    error!(target:"consensus::cl","broadcast_consensus error {:?}",err);
                }
            }
        }
    }
}
