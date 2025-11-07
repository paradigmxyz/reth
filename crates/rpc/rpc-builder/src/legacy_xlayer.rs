//! XLayer-specific Legacy RPC configuration builder

use reth_rpc_eth_types::LegacyRpcConfig;
use std::time::Duration;
use tracing::info;

/// Build legacy RPC config from CLI arguments
///
/// Returns `Some(LegacyRpcConfig)` if both URL and cutoff block are provided,
/// otherwise returns `None`.
pub(crate) fn build_legacy_rpc_config(
    legacy_rpc_url: Option<&String>,
    legacy_cutoff_block: Option<u64>,
    legacy_rpc_timeout: Option<&String>,
) -> Option<LegacyRpcConfig> {
    if let (Some(url), Some(cutoff)) = (legacy_rpc_url, legacy_cutoff_block) {
        let timeout = legacy_rpc_timeout
            .and_then(|s| humantime::parse_duration(s).ok())
            .unwrap_or(Duration::from_secs(30));

        info!(target: "reth::cli", legacy_url = %url, cutoff = cutoff, timeout = ?timeout, "Legacy RPC routing enabled");

        Some(LegacyRpcConfig::new(cutoff, url.clone(), timeout))
    } else {
        None
    }
}

