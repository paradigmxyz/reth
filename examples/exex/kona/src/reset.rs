//! Reset Provider for Kona's Derivation Pipeline

use std::sync::Mutex;
use std::sync::Arc;
use async_trait::async_trait;
use kona_derive::builder::ResetProvider;
use kona_primitives::{BlockInfo, SystemConfig};

/// TipState stores the tip information for the derivation pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct TipState {
    /// The origin [BlockInfo].
    /// This is used downstream by [kona_derive] to reset the origin
    /// of the [kona_derive::stages::BatchQueue] and l1 block list.
    origin: BlockInfo,
    /// The [SystemConfig] is used in two places.
    ///
    system_config: SystemConfig,
}

impl TipState {
    /// Creates a new [TipState].
    pub fn new(origin: BlockInfo, system_config: SystemConfig) -> Self {
        Self { origin, system_config }
    }

    /// Retrieves a copy of the [BlockInfo].
    pub fn origin(&self) -> BlockInfo {
        self.origin
    }

    /// Retrieves a copy of the [SystemConfig].
    pub fn system_config(&self) -> SystemConfig {
        self.system_config
    }

    /// Sets the block info.
    pub fn set_origin(&mut self, new_bi: BlockInfo) {
        self.origin = new_bi;
    }

    // /// Sets the system config.
    // pub fn set_system_config(&mut self, new_config: SystemConfig) {
    //     self.system_config = new_config;
    // }
}

/// The ExExResetProvider implements the [ResetProvider] trait.
#[derive(Debug, Clone)]
pub struct ExExResetProvider(Arc<Mutex<TipState>>);

impl ExExResetProvider {
    /// Creates a new [ExExResetProvider].
    pub fn new(ts: Arc<Mutex<TipState>>) -> Self {
        Self(ts)
    }
}

#[async_trait]
impl ResetProvider for ExExResetProvider {
    /// Returns the current [BlockInfo] for the pipeline to reset.
    async fn block_info(&self) -> BlockInfo {
        self.0.lock().map(|ts| ts.origin()).unwrap_or_default()
    }

    /// Returns the current [SystemConfig] for the pipeline to reset.
    async fn system_config(&self) -> SystemConfig {
        self.0.lock().map(|ts| ts.system_config()).unwrap_or_default()
    }
}
