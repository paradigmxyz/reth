//! `XLayer`: Legacy RPC routing utilities

use alloy_eips::BlockId;
use alloy_rpc_types_eth::BlockNumberOrTag;
use jsonrpsee::types::ErrorObjectOwned;
use serde::{Deserialize, Serialize};
use std::{future::Future, sync::Arc};

/// Trait for providing access to legacy RPC client for routing historical data.
pub trait LegacyRpc {
    /// Returns the legacy RPC client if configured.
    fn legacy_rpc_client(&self) -> Option<&Arc<reth_rpc_eth_types::LegacyRpcClient>>;
}

/// Check if a block number should be routed to legacy RPC
#[inline]
pub fn should_route_to_legacy(
    legacy_client: Option<&std::sync::Arc<reth_rpc_eth_types::LegacyRpcClient>>,
    number: BlockNumberOrTag,
) -> bool {
    if let Some(client) = legacy_client &&
        let BlockNumberOrTag::Number(n) = number
    {
        return n < client.cutoff_block();
    }

    false
}

/// Check if a `BlockId` should be routed to legacy RPC based on `cutoff_block`
#[inline]
pub fn should_route_block_id_to_legacy<Provider>(
    legacy_client: Option<&std::sync::Arc<reth_rpc_eth_types::LegacyRpcClient>>,
    provider: &Provider,
    block_id: Option<&BlockId>,
) -> Result<bool, ErrorObjectOwned>
where
    Provider: reth_storage_api::BlockNumReader,
{
    if legacy_client.is_none() {
        return Ok(false);
    }

    Ok(match block_id {
        Some(BlockId::Number(number)) => should_route_to_legacy(legacy_client, *number),
        Some(BlockId::Hash(hash)) => {
            provider.block_number(hash.block_hash).map_err(internal_rpc_err)?.is_none()
        }
        None => false,
    })
}

/// Convert any value through serde JSON (for type system compatibility)
#[inline]
pub fn convert_via_serde<T, U>(value: T) -> Result<U, ErrorObjectOwned>
where
    T: Serialize,
    U: for<'de> Deserialize<'de>,
{
    serde_json::to_value(value).and_then(serde_json::from_value).map_err(internal_rpc_err)
}

/// Convert Option<T> to Option<U> through serde
#[inline]
pub fn convert_option_via_serde<T, U>(value: Option<T>) -> Result<Option<U>, ErrorObjectOwned>
where
    T: Serialize,
    U: for<'de> Deserialize<'de>,
{
    match value {
        Some(v) => Ok(Some(convert_via_serde(v)?)),
        None => Ok(None),
    }
}

/// Helper to convert any error to internal RPC error
#[inline]
pub fn internal_rpc_err<E: std::fmt::Display>(e: E) -> ErrorObjectOwned {
    jsonrpsee::types::ErrorObjectOwned::owned(
        jsonrpsee::types::ErrorCode::InternalError.code(),
        e.to_string(),
        None::<()>,
    )
}

/// Helper to convert Box<dyn Error> to RPC error
#[inline]
pub fn boxed_err_to_rpc(e: Box<dyn std::error::Error + Send + Sync>) -> ErrorObjectOwned {
    internal_rpc_err(e.to_string())
}

/// Execute a legacy RPC call with timing and error logging
pub async fn exec_legacy<F, T>(
    method: &str,
    fut: F,
) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
where
    F: Future<Output = Result<T, Box<dyn std::error::Error + Send + Sync>>>,
{
    let start = std::time::Instant::now();
    match fut.await {
        Ok(result) => {
            tracing::info!(target: "rpc::eth::legacy", method, elapsed_ms = %start.elapsed().as_millis(), "← legacy");
            Ok(result)
        }
        Err(e) => {
            tracing::error!(target: "rpc::eth::legacy", method, elapsed_ms = %start.elapsed().as_millis(), error = %e, "← legacy (error)");
            Err(e)
        }
    }
}

/// Route a request by `BlockNumberOrTag` to legacy RPC if below cutoff
#[macro_export]
macro_rules! route_by_number {
    ($method:literal, $self:ident, $number:ident, $legacy_call:expr, $local_expr:expr) => {{
        if $crate::helpers::should_route_to_legacy($self.legacy_rpc_client(), $number) {
            tracing::info!(target: "rpc::eth::legacy", method = $method, block = ?$number, "→ legacy");
            let result = $crate::helpers::exec_legacy($method, $legacy_call).await.map_err($crate::helpers::boxed_err_to_rpc)?;
            $crate::helpers::convert_option_via_serde(result)
        } else {
            $local_expr
        }
    }};
}

/// Route a request by `BlockId` to legacy RPC if below cutoff
#[macro_export]
macro_rules! route_by_block_id {
    ($method:literal, $self:ident, $block_id:ident, $legacy_call:expr, $local_expr:expr) => {{
        if $crate::helpers::should_route_block_id_to_legacy($self.legacy_rpc_client(), $self.provider(), Some(&$block_id))? {
            tracing::info!(target: "rpc::eth::legacy", method = $method, block = ?$block_id, "→ legacy");
            let result = $crate::helpers::exec_legacy($method, $legacy_call).await.map_err($crate::helpers::boxed_err_to_rpc)?;
            return $crate::helpers::convert_option_via_serde(result);
        }

        $local_expr
    }};
}

/// Route by optional `BlockId` (for state queries)
#[macro_export]
macro_rules! route_by_block_id_opt {
    ($method:literal, $self:ident, $block_id:ident, $legacy_call:expr, $local_expr:expr) => {{
        if $crate::helpers::should_route_block_id_to_legacy($self.legacy_rpc_client(), $self.provider(), $block_id.as_ref())? {
            tracing::info!(target: "rpc::eth::legacy", method = $method, block = ?$block_id, "→ legacy");
            return $crate::helpers::exec_legacy($method, $legacy_call).await.map_err($crate::helpers::boxed_err_to_rpc);
        }

        $local_expr
    }};
}

/// Try local first, then fallback to legacy (for hash-based queries)
#[macro_export]
macro_rules! try_local_then_legacy {
    ($method:literal, $self:ident, $key:ident, $local_expr:expr, $legacy_call:expr) => {{
        let local_result = $local_expr;
        if local_result.is_none() {
            if let Some(_legacy_client) = $self.legacy_rpc_client() {
                tracing::info!(target: "rpc::eth::legacy", method = $method, key = ?$key, "→ legacy (fallback)");
                let result = $crate::helpers::exec_legacy($method, $legacy_call).await.map_err($crate::helpers::boxed_err_to_rpc)?;
                return $crate::helpers::convert_option_via_serde(result);
            }
        }
        Ok(local_result)
    }};
}

/// Route to legacy RPC with JSON deserialization if condition is met
#[macro_export]
macro_rules! route_by_condition {
    ($method:literal, $condition:expr, $key:expr, $legacy_call:expr) => {{
        if $condition {
            tracing::info!(target: "rpc::eth::legacy", method = $method, key = ?$key, "→ legacy");
            let result = $crate::helpers::exec_legacy($method, $legacy_call)
                .await
                .map_err($crate::helpers::boxed_err_to_rpc)?;
            return serde_json::from_value(result).map_err(|e| {
                jsonrpsee::types::ErrorObjectOwned::owned(
                    -32603,
                    format!("Failed to deserialize legacy response: {}", e),
                    None::<()>,
                )
            });
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, B256, U256};
    use reth_storage_api::BlockNumReader;
    use reth_storage_errors::provider::{ProviderError, ProviderResult};
    use std::{collections::HashMap, sync::Arc, time::Duration};

    /// Helper to create a test legacy client
    fn create_test_client(cutoff_block: u64) -> Arc<reth_rpc_eth_types::LegacyRpcClient> {
        let config = reth_rpc_eth_types::LegacyRpcConfig::new(
            cutoff_block,
            "http://test:8545".to_string(),
            Duration::from_secs(30),
        );
        Arc::new(reth_rpc_eth_types::LegacyRpcClient::from_config(&config).unwrap())
    }

    #[test]
    fn test_convert_simple_types() {
        // Test primitive types
        let num: u64 = 42;
        let result: u64 = convert_via_serde(num).unwrap();
        assert_eq!(result, 42);

        let addr = Address::from([1u8; 20]);
        let result: Address = convert_via_serde(addr).unwrap();
        assert_eq!(result, addr);

        let hash = B256::from([2u8; 32]);
        let result: B256 = convert_via_serde(hash).unwrap();
        assert_eq!(result, hash);
    }

    #[test]
    fn test_convert_u256() {
        let value = U256::from(1234567890u64);
        let result: U256 = convert_via_serde(value).unwrap();
        assert_eq!(result, value);

        // Test large numbers
        let large = U256::from_str_radix("ffffffffffffffffffffffffffffffff", 16).unwrap();
        let result: U256 = convert_via_serde(large).unwrap();
        assert_eq!(result, large);
    }

    #[test]
    fn test_convert_option_types() {
        // Test Some
        let value = Some(42u64);
        let result: Option<u64> = convert_option_via_serde(value).unwrap();
        assert_eq!(result, Some(42));

        // Test None
        let value: Option<u64> = None;
        let result: Option<u64> = convert_option_via_serde(value).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_convert_block_header_fields() {
        // Test converting common block fields
        use serde_json::json;

        let block_json = json!({
            "number": "0x1",
            "hash": "0x0000000000000000000000000000000000000000000000000000000000000001",
            "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
            "timestamp": "0x123456",
            "gasLimit": "0x1000000",
            "gasUsed": "0x5000",
        });

        // This tests the core serde conversion logic
        let result: serde_json::Value = convert_via_serde(block_json.clone()).unwrap();
        assert_eq!(result, block_json);
    }

    #[test]
    fn test_convert_transaction_fields() {
        use serde_json::json;

        let tx_json = json!({
            "hash": "0x0000000000000000000000000000000000000000000000000000000000000001",
            "from": "0x0000000000000000000000000000000000000001",
            "to": "0x0000000000000000000000000000000000000002",
            "value": "0x1000",
            "gas": "0x5208",
            "gasPrice": "0x3b9aca00",
            "nonce": "0x0",
        });

        let result: serde_json::Value = convert_via_serde(tx_json.clone()).unwrap();
        assert_eq!(result, tx_json);
    }

    #[test]
    fn test_convert_nested_structures() {
        use serde_json::json;

        // Test nested arrays and objects
        let nested = json!({
            "transactions": [
                {"hash": "0x01", "value": "0x100"},
                {"hash": "0x02", "value": "0x200"},
            ],
            "uncles": [],
        });

        let result: serde_json::Value = convert_via_serde(nested.clone()).unwrap();
        assert_eq!(result, nested);
    }

    #[test]
    fn test_serde_conversion_preserves_hex_encoding() {
        // This is critical - hex strings must be preserved correctly
        use serde_json::json;

        let data = json!({
            "blockNumber": "0xabc123",
            "blockHash": "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        });

        let result: serde_json::Value = convert_via_serde(data.clone()).unwrap();
        assert_eq!(result["blockNumber"], "0xabc123");
        assert_eq!(
            result["blockHash"],
            "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
        );
    }

    #[test]
    fn test_should_route_to_legacy_below_cutoff() {
        let client = create_test_client(1000000);

        // Below cutoff - should route to legacy
        assert!(should_route_to_legacy(Some(&client), BlockNumberOrTag::Number(100)));
        assert!(should_route_to_legacy(Some(&client), BlockNumberOrTag::Number(999999)));
    }

    #[test]
    fn test_should_route_to_legacy_at_and_above_cutoff() {
        let client = create_test_client(1000000);

        // At cutoff - should NOT route to legacy
        assert!(!should_route_to_legacy(Some(&client), BlockNumberOrTag::Number(1000000)));

        // Above cutoff - should NOT route to legacy
        assert!(!should_route_to_legacy(Some(&client), BlockNumberOrTag::Number(1000001)));
        assert!(!should_route_to_legacy(Some(&client), BlockNumberOrTag::Number(2000000)));
    }

    #[test]
    fn test_should_route_to_legacy_special_tags() {
        let client = create_test_client(1000000);

        // Special tags should NOT route to legacy (handled locally)
        assert!(!should_route_to_legacy(Some(&client), BlockNumberOrTag::Latest));
        assert!(!should_route_to_legacy(Some(&client), BlockNumberOrTag::Earliest));
        assert!(!should_route_to_legacy(Some(&client), BlockNumberOrTag::Pending));
        assert!(!should_route_to_legacy(Some(&client), BlockNumberOrTag::Finalized));
        assert!(!should_route_to_legacy(Some(&client), BlockNumberOrTag::Safe));
    }

    #[test]
    fn test_should_route_to_legacy_no_client() {
        // No client - should never route to legacy
        assert!(!should_route_to_legacy(None, BlockNumberOrTag::Number(100)));
        assert!(!should_route_to_legacy(None, BlockNumberOrTag::Latest));
    }

    #[test]
    fn test_should_route_block_id_to_legacy() {
        let client = create_test_client(1000000);
        let provider_missing = MockBlockProvider::default();

        // BlockId::Number below cutoff
        let block_id = BlockId::Number(BlockNumberOrTag::Number(100));
        assert!(should_route_block_id_to_legacy(Some(&client), &provider_missing, Some(&block_id))
            .unwrap());

        // BlockId::Number above cutoff
        let block_id = BlockId::Number(BlockNumberOrTag::Number(1000001));
        assert!(!should_route_block_id_to_legacy(
            Some(&client),
            &provider_missing,
            Some(&block_id)
        )
        .unwrap());

        // BlockId::Hash - routes if missing locally
        let missing_hash = BlockId::Hash(B256::from([1u8; 32]).into());
        assert!(should_route_block_id_to_legacy(
            Some(&client),
            &provider_missing,
            Some(&missing_hash)
        )
        .unwrap());

        // BlockId::Hash - stays local if present
        let present_hash_value = B256::from([2u8; 32]);
        let mut provider_present = MockBlockProvider::default();
        provider_present.insert_hash(present_hash_value, 0);
        let present_hash = BlockId::Hash(present_hash_value.into());
        assert!(!should_route_block_id_to_legacy(
            Some(&client),
            &provider_present,
            Some(&present_hash)
        )
        .unwrap());

        // None - should NOT route
        assert!(!should_route_block_id_to_legacy(Some(&client), &provider_missing, None).unwrap());
    }

    #[test]
    fn test_edge_case_cutoff_at_zero() {
        let client = create_test_client(0);

        // Everything should be handled locally
        assert!(!should_route_to_legacy(Some(&client), BlockNumberOrTag::Number(0)));
        assert!(!should_route_to_legacy(Some(&client), BlockNumberOrTag::Number(1)));
        assert!(!should_route_to_legacy(Some(&client), BlockNumberOrTag::Number(1000)));
    }

    #[derive(Default)]
    struct MockBlockProvider {
        hashes: HashMap<B256, u64>,
    }

    impl MockBlockProvider {
        fn insert_hash(&mut self, hash: B256, number: u64) {
            self.hashes.insert(hash, number);
        }
    }

    impl reth_storage_api::BlockHashReader for MockBlockProvider {
        fn block_hash(&self, _number: u64) -> ProviderResult<Option<B256>> {
            Ok(None)
        }

        fn canonical_hashes_range(&self, _start: u64, _end: u64) -> ProviderResult<Vec<B256>> {
            Ok(Vec::new())
        }
    }

    impl BlockNumReader for MockBlockProvider {
        fn chain_info(&self) -> ProviderResult<reth_chainspec::ChainInfo> {
            Err(ProviderError::UnsupportedProvider)
        }

        fn best_block_number(&self) -> ProviderResult<u64> {
            Ok(0)
        }

        fn last_block_number(&self) -> ProviderResult<u64> {
            Ok(0)
        }

        fn block_number(&self, hash: B256) -> ProviderResult<Option<u64>> {
            Ok(self.hashes.get(&hash).copied())
        }
    }

    #[test]
    fn test_edge_case_cutoff_at_max() {
        let client = create_test_client(u64::MAX);

        // Everything should route to legacy (except special tags)
        assert!(should_route_to_legacy(Some(&client), BlockNumberOrTag::Number(0)));
        assert!(should_route_to_legacy(Some(&client), BlockNumberOrTag::Number(1000000)));
        assert!(should_route_to_legacy(Some(&client), BlockNumberOrTag::Number(u64::MAX - 1)));

        // But not u64::MAX itself
        assert!(!should_route_to_legacy(Some(&client), BlockNumberOrTag::Number(u64::MAX)));
    }

    #[test]
    fn test_internal_rpc_err_preserves_message() {
        let err = internal_rpc_err("Test error message");
        assert_eq!(err.code(), jsonrpsee::types::ErrorCode::InternalError.code());
        assert!(err.message().contains("Test error message"));
    }

    #[test]
    fn test_boxed_err_to_rpc() {
        let err: Box<dyn std::error::Error + Send + Sync> =
            Box::new(std::io::Error::new(std::io::ErrorKind::Other, "IO error"));
        let rpc_err = boxed_err_to_rpc(err);
        assert!(rpc_err.message().contains("IO error"));
    }

    #[test]
    fn test_convert_error_on_incompatible_types() {
        use serde_json::json;

        // Try to convert string to number - should fail
        let value = json!("not a number");
        let result: Result<u64, _> = convert_via_serde(value);
        assert!(result.is_err());

        if let Err(err) = result {
            // The error message might be different depending on serde version
            // Just check that it contains error-related keywords
            let msg = err.message();
            assert!(
                msg.contains("Deserialization") || msg.contains("error") || msg.contains("invalid"),
                "Error message should indicate deserialization failure, got: {}",
                msg
            );
        }
    }

    #[tokio::test]
    async fn test_exec_legacy_success() {
        // Test successful execution with timing
        let result = exec_legacy("test_method", async {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(42u64)
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42u64);
    }

    #[tokio::test]
    async fn test_exec_legacy_failure() {
        // Test error propagation with timing
        let result = exec_legacy("test_method", async {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            Err::<u64, _>(Box::new(std::io::Error::new(std::io::ErrorKind::Other, "Test error"))
                as Box<dyn std::error::Error + Send + Sync>)
        })
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Test error"));
    }

    #[tokio::test]
    async fn test_exec_legacy_timing_accuracy() {
        use std::time::Instant;

        // Test that timing is approximately accurate
        let start = Instant::now();
        let _ = exec_legacy("test_method", async {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        })
        .await;

        let elapsed = start.elapsed();
        // Should take at least 50ms
        assert!(elapsed.as_millis() >= 50);
        // But not more than 100ms (allowing some overhead)
        assert!(elapsed.as_millis() < 100);
    }

    #[tokio::test]
    async fn test_exec_legacy_different_return_types() {
        // Test with String return type
        let result = exec_legacy("test_method", async {
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>("success".to_string())
        })
        .await;
        assert_eq!(result.unwrap(), "success");

        // Test with Option return type
        let result = exec_legacy("test_method", async {
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(Some(100u64))
        })
        .await;
        assert_eq!(result.unwrap(), Some(100u64));

        // Test with None
        let result = exec_legacy("test_method", async {
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(None::<u64>)
        })
        .await;
        assert_eq!(result.unwrap(), None);
    }
}
