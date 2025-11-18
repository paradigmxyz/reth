//! XLayer-specific extensions for `EthConfig`

use super::config::EthConfig;
use crate::LegacyRpcConfig;

/// `XLayer` extension methods for `EthConfig`
impl EthConfig {
    /// Configures legacy RPC routing for historical data access.
    pub fn with_legacy_rpc(mut self, legacy_rpc_config: Option<LegacyRpcConfig>) -> Self {
        self.legacy_rpc_config = legacy_rpc_config;
        self
    }
}
