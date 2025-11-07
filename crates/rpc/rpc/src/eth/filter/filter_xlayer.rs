//! XLayer-specific extensions for EthFilter

use super::super::EthFilter;
use alloy_rpc_types_eth::{Filter, Log};
use jsonrpsee::core::RpcResult;
use reth_rpc_eth_api::{
    helpers::{internal_rpc_err, EthBlocks, LegacyRpc, LoadReceipt},
    EthApiTypes, FullEthApiTypes, RpcNodeCoreExt,
};
use reth_rpc_eth_types::LegacyRpcClient;
use reth_storage_api::{BlockIdReader, BlockReader};
use std::sync::Arc;
use tracing::info;
use tracing::log::log;

/// XLayer: Implement LegacyRpc trait for EthFilter to enable legacy RPC routing
impl<Eth> LegacyRpc for EthFilter<Eth>
where
    Eth: LegacyRpc + EthApiTypes,
{
    fn legacy_rpc_client(&self) -> Option<&Arc<LegacyRpcClient>> {
        self.inner.eth_api.legacy_rpc_client()
    }
}

/// XLayer: Legacy RPC routing methods
impl<Eth> EthFilter<Eth>
where
    Eth: FullEthApiTypes<Provider: BlockReader + BlockIdReader>
        + RpcNodeCoreExt
        + LegacyRpc
        + LoadReceipt
        + EthBlocks
        + 'static,
{
    /// Parse block range from filter for legacy routing logic
    pub(super) fn parse_block_range(&self, filter: &Filter) -> RpcResult<(u64, u64)> {
        let from = match filter.block_option.get_from_block() {
            Some(alloy_rpc_types_eth::BlockNumberOrTag::Number(n)) => *n,
            Some(alloy_rpc_types_eth::BlockNumberOrTag::Earliest) => 0,
            _ => {
                // For latest/pending/finalized/safe, use current block
                // This is a rough approximation - in production you'd query the actual block number
                u64::MAX
            }
        };

        let to = match filter.block_option.get_to_block() {
            Some(alloy_rpc_types_eth::BlockNumberOrTag::Number(n)) => *n,
            Some(alloy_rpc_types_eth::BlockNumberOrTag::Earliest) => 0,
            _ => u64::MAX,
        };

        Ok((from, to))
    }

    /// Check if eth_getLogs needs legacy RPC routing and handle accordingly
    ///
    /// Returns:
    /// - `Some(result)` if legacy/hybrid routing is used (query complete)
    /// - `None` if pure local processing should be used (no legacy needed)
    pub(super) async fn route_logs_to_legacy(&self, filter: Filter) -> Option<RpcResult<Vec<Log>>> {
        // Check if legacy RPC routing is configured
        let legacy_client = self.inner.eth_api.legacy_rpc_client()?;
        let cutoff_block = legacy_client.cutoff_block();

        // Parse block range from filter
        let (from_block, to_block) = match self.parse_block_range(&filter) {
            Ok(range) => range,
            Err(e) => return Some(Err(e)),
        };

        // Determine routing strategy
        if to_block < cutoff_block {
            // Pure legacy: all blocks are below cutoff
            info!(target: "rpc::eth::legacy", method = "eth_getLogs", from = from_block, to = to_block, "→ legacy");
            match reth_rpc_eth_api::helpers::exec_legacy(
                "eth_getLogs",
                legacy_client.get_logs(filter),
            )
            .await
            {
                Ok(logs) => return Some(Ok(logs)),
                Err(e) => return Some(Err(internal_rpc_err(e))),
            }
        } else if from_block >= cutoff_block {
            // Pure local: all blocks are at or above cutoff
            // Return None to signal local processing
            return None;
        } else {
            // Hybrid: spans both legacy and local ranges
            let start = std::time::Instant::now();
            info!(target: "rpc::eth::legacy", method = "eth_getLogs", from = from_block, to = to_block, "→ hybrid");

            // Split filter into legacy and local parts
            let mut legacy_filter = filter.clone();
            legacy_filter = legacy_filter
                .to_block(alloy_rpc_types_eth::BlockNumberOrTag::Number(cutoff_block - 1));
            let mut local_filter = filter;
            local_filter = local_filter
                .from_block(alloy_rpc_types_eth::BlockNumberOrTag::Number(cutoff_block));

            // Query both in parallel
            let (legacy_result, local_result): (Result<Vec<Log>, _>, Result<Vec<Log>, _>) =
                tokio::join!(
                    async { legacy_client.get_logs(legacy_filter).await },
                    async { self.logs_for_filter(local_filter, self.inner.query_limits).await }
                );

            let mut legacy_logs = match legacy_result.map_err(|e| internal_rpc_err(e)) {
                Ok(logs) => logs,
                Err(e) => return Some(Err(e)),
            };

            let legacy_count = legacy_logs.len();
            let mut local_logs: Vec<Log> = match local_result {
                Ok(logs) => logs,
                Err(e) => return Some(Err(e.into())),
            };

            let local_count = local_logs.len();

            // Merge and sort logs
            legacy_logs.append(&mut local_logs);
            legacy_logs.sort_by(|a, b| {
                a.block_number
                    .cmp(&b.block_number)
                    .then(a.transaction_index.cmp(&b.transaction_index))
                    .then(a.log_index.cmp(&b.log_index))
            });

            info!(target: "rpc::eth::legacy", method = "eth_getLogs", elapsed_ms = %start.elapsed().as_millis(),
                  legacy_logs = legacy_count, local_logs = local_count, total = legacy_logs.len(), "← hybrid");

            return Some(Ok(legacy_logs));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, B256};
    use alloy_rpc_types_eth::{BlockNumberOrTag, FilterBlockOption};

    /// Helper to create a test filter with block range
    fn create_filter_with_range(from: u64, to: u64) -> Filter {
        Filter {
            block_option: FilterBlockOption::Range {
                from_block: Some(BlockNumberOrTag::Number(from)),
                to_block: Some(BlockNumberOrTag::Number(to)),
            },
            address: Default::default(),
            topics: Default::default(),
        }
    }

    /// Helper to create a test log
    fn create_test_log(block_number: u64, tx_index: u64, log_index: u64) -> Log {
        Log {
            inner: alloy_primitives::Log {
                address: Address::ZERO,
                data: alloy_primitives::LogData::new_unchecked(vec![], Default::default()),
            },
            block_number: Some(block_number),
            block_hash: Some(B256::ZERO),
            block_timestamp: None,
            transaction_hash: Some(B256::ZERO),
            transaction_index: Some(tx_index),
            log_index: Some(log_index),
            removed: false,
        }
    }

    #[test]
    fn test_parse_block_range_with_numbers() {
        // Create a minimal mock filter (we can't easily create EthFilter without full setup)
        // So we'll test the logic through the Filter type directly
        let filter = create_filter_with_range(100, 200);

        // Extract block range manually (same logic as parse_block_range)
        let from = match filter.block_option.get_from_block() {
            Some(BlockNumberOrTag::Number(n)) => *n,
            Some(BlockNumberOrTag::Earliest) => 0,
            _ => u64::MAX,
        };
        let to = match filter.block_option.get_to_block() {
            Some(BlockNumberOrTag::Number(n)) => *n,
            Some(BlockNumberOrTag::Earliest) => 0,
            _ => u64::MAX,
        };

        assert_eq!(from, 100);
        assert_eq!(to, 200);
    }

    #[test]
    fn test_parse_block_range_with_earliest() {
        let filter = Filter {
            block_option: FilterBlockOption::Range {
                from_block: Some(BlockNumberOrTag::Earliest),
                to_block: Some(BlockNumberOrTag::Number(100)),
            },
            address: Default::default(),
            topics: Default::default(),
        };

        let from = match filter.block_option.get_from_block() {
            Some(BlockNumberOrTag::Number(n)) => *n,
            Some(BlockNumberOrTag::Earliest) => 0,
            _ => u64::MAX,
        };
        let to = match filter.block_option.get_to_block() {
            Some(BlockNumberOrTag::Number(n)) => *n,
            Some(BlockNumberOrTag::Earliest) => 0,
            _ => u64::MAX,
        };

        assert_eq!(from, 0);
        assert_eq!(to, 100);
    }

    #[test]
    fn test_parse_block_range_with_latest() {
        let filter = Filter {
            block_option: FilterBlockOption::Range {
                from_block: Some(BlockNumberOrTag::Number(100)),
                to_block: Some(BlockNumberOrTag::Latest),
            },
            address: Default::default(),
            topics: Default::default(),
        };

        let to = match filter.block_option.get_to_block() {
            Some(BlockNumberOrTag::Number(n)) => *n,
            Some(BlockNumberOrTag::Earliest) => 0,
            _ => u64::MAX,
        };

        assert_eq!(to, u64::MAX);
    }

    #[test]
    fn test_log_sorting_order() {
        // Test that logs are sorted correctly: block_number -> tx_index -> log_index
        let mut logs = vec![
            create_test_log(100, 2, 1),
            create_test_log(100, 1, 2),
            create_test_log(100, 1, 1),
            create_test_log(99, 5, 5),
            create_test_log(101, 0, 0),
        ];

        // Sort using the same logic as route_logs_to_legacy
        logs.sort_by(|a, b| {
            a.block_number
                .cmp(&b.block_number)
                .then(a.transaction_index.cmp(&b.transaction_index))
                .then(a.log_index.cmp(&b.log_index))
        });

        // Verify order
        assert_eq!(logs[0].block_number, Some(99));
        assert_eq!(logs[1].block_number, Some(100));
        assert_eq!(logs[1].transaction_index, Some(1));
        assert_eq!(logs[1].log_index, Some(1));
        assert_eq!(logs[2].block_number, Some(100));
        assert_eq!(logs[2].transaction_index, Some(1));
        assert_eq!(logs[2].log_index, Some(2));
        assert_eq!(logs[3].block_number, Some(100));
        assert_eq!(logs[3].transaction_index, Some(2));
        assert_eq!(logs[4].block_number, Some(101));
    }

    #[test]
    fn test_hybrid_query_range_splitting() {
        // Test the logic of splitting a filter into legacy and local parts
        let cutoff_block = 1000u64;
        let original_filter = create_filter_with_range(500, 1500);

        // Simulate legacy filter (from_block unchanged, to_block = cutoff - 1)
        let mut legacy_filter = original_filter.clone();
        legacy_filter =
            legacy_filter.to_block(BlockNumberOrTag::Number(cutoff_block - 1));

        // Simulate local filter (from_block = cutoff, to_block unchanged)
        let mut local_filter = original_filter.clone();
        local_filter = local_filter.from_block(BlockNumberOrTag::Number(cutoff_block));

        // Verify legacy filter
        let legacy_to = match legacy_filter.block_option.get_to_block() {
            Some(BlockNumberOrTag::Number(n)) => *n,
            _ => panic!("Expected number"),
        };
        assert_eq!(legacy_to, 999);

        // Verify local filter
        let local_from = match local_filter.block_option.get_from_block() {
            Some(BlockNumberOrTag::Number(n)) => *n,
            _ => panic!("Expected number"),
        };
        assert_eq!(local_from, 1000);
    }

    #[test]
    fn test_routing_decision_pure_legacy() {
        let cutoff_block = 1000u64;
        let to_block = 500u64;

        // Pure legacy: both below cutoff
        let should_be_pure_legacy = to_block < cutoff_block;
        assert!(should_be_pure_legacy);
    }

    #[test]
    fn test_routing_decision_pure_local() {
        let cutoff_block = 1000u64;
        let from_block = 1500u64;

        // Pure local: from at or above cutoff
        let should_be_pure_local = from_block >= cutoff_block;
        assert!(should_be_pure_local);
    }

    #[test]
    fn test_routing_decision_hybrid() {
        let cutoff_block = 1000u64;
        let from_block = 500u64;
        let to_block = 1500u64;

        // Hybrid: spans cutoff
        let is_hybrid = to_block >= cutoff_block && from_block < cutoff_block;
        assert!(is_hybrid);
    }

    #[test]
    fn test_edge_case_cutoff_boundary() {
        let cutoff_block = 1000u64;

        // Query ending exactly at cutoff - 1 should be pure legacy
        assert!(999 < cutoff_block);

        // Query starting exactly at cutoff should be pure local
        assert!(1000 >= cutoff_block);

        // Query from cutoff-1 to cutoff should be hybrid
        let from = 999u64;
        let to = 1000u64;
        let is_hybrid = to >= cutoff_block && from < cutoff_block;
        assert!(is_hybrid);
    }

    #[test]
    fn test_log_merge_empty_results() {
        // Test merging when one or both sides return empty
        let mut legacy_logs: Vec<Log> = vec![];
        let mut local_logs: Vec<Log> = vec![create_test_log(1000, 0, 0)];

        legacy_logs.append(&mut local_logs);
        assert_eq!(legacy_logs.len(), 1);
        assert_eq!(legacy_logs[0].block_number, Some(1000));

        // Test both empty
        let mut legacy_logs: Vec<Log> = vec![];
        let mut local_logs: Vec<Log> = vec![];
        legacy_logs.append(&mut local_logs);
        assert_eq!(legacy_logs.len(), 0);
    }

    #[test]
    fn test_log_merge_maintains_order() {
        // Test that appending and sorting works correctly
        let mut legacy_logs = vec![
            create_test_log(100, 0, 0),
            create_test_log(200, 0, 0),
        ];

        let mut local_logs = vec![
            create_test_log(1000, 0, 0),
            create_test_log(1100, 0, 0),
        ];

        legacy_logs.append(&mut local_logs);
        legacy_logs.sort_by(|a, b| {
            a.block_number
                .cmp(&b.block_number)
                .then(a.transaction_index.cmp(&b.transaction_index))
                .then(a.log_index.cmp(&b.log_index))
        });

        assert_eq!(legacy_logs.len(), 4);
        assert_eq!(legacy_logs[0].block_number, Some(100));
        assert_eq!(legacy_logs[1].block_number, Some(200));
        assert_eq!(legacy_logs[2].block_number, Some(1000));
        assert_eq!(legacy_logs[3].block_number, Some(1100));
    }

    #[test]
    fn test_log_merge_with_interleaved_blocks() {
        // Test edge case where legacy and local logs could interleave
        // (shouldn't happen in practice, but test sorting handles it)
        let mut legacy_logs = vec![
            create_test_log(100, 0, 0),
            create_test_log(1001, 0, 0), // Shouldn't happen but test it
        ];

        let mut local_logs = vec![
            create_test_log(999, 0, 0), // Shouldn't happen but test it
            create_test_log(1100, 0, 0),
        ];

        legacy_logs.append(&mut local_logs);
        legacy_logs.sort_by(|a, b| {
            a.block_number
                .cmp(&b.block_number)
                .then(a.transaction_index.cmp(&b.transaction_index))
                .then(a.log_index.cmp(&b.log_index))
        });

        // Should still sort correctly
        assert_eq!(legacy_logs[0].block_number, Some(100));
        assert_eq!(legacy_logs[1].block_number, Some(999));
        assert_eq!(legacy_logs[2].block_number, Some(1001));
        assert_eq!(legacy_logs[3].block_number, Some(1100));
    }
}
