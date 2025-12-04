//! Consist of types adjacent to the fee history cache and its configs

use alloy_rpc_client::RpcClient;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

/// Configuration for the transaction forwarder.
#[derive(Debug, PartialEq, Eq, Clone, Default, Serialize, Deserialize)]
pub struct ForwardConfig {
    /// The raw transaction forwarder.
    ///
    /// Default is `None`
    pub tx_forwarder: Option<Url>,
}

impl ForwardConfig {
    /// Builds an [`RpcClient`] from the forwarder URL, if configured.
    pub fn forwarder_client(&self) -> Option<RpcClient> {
        self.tx_forwarder.clone().map(RpcClient::new_http)
    }
}
