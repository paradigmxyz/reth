//! Streams subscriptions providers for `eth_subscribe`.

use crate::{EthApiTypes, RpcConvert, RpcNodeCore};
use alloy_rpc_types_eth::{Filter, Log};
use futures::StreamExt;
use reth_chain_state::CanonStateSubscriptions;
use reth_rpc_convert::RpcHeader;
use reth_rpc_eth_types::logs_utils;
use tracing::error;

/// Provides streams subscriptions for `eth_subscribe`.
///
/// Override the default methods to inject additional data sources (e.g. flashblocks).
pub trait EthSubscriptions:
    RpcNodeCore + EthApiTypes<RpcConvert: RpcConvert<Primitives = Self::Primitives>>
{
    /// Returns a stream that yields matching logs from canonical chain updates.
    fn log_stream(&self, filter: Filter) -> impl futures::Stream<Item = Log> + Send + Unpin {
        self.provider()
            .canonical_state_stream()
            .map(move |canon_state| canon_state.block_receipts())
            .flat_map(futures::stream::iter)
            .flat_map(move |(block_receipts, removed)| {
                let all_logs = logs_utils::matching_block_logs_with_tx_hashes(
                    &filter,
                    block_receipts.block,
                    block_receipts.timestamp,
                    block_receipts.tx_receipts.iter().map(|(tx, receipt)| (*tx, receipt)),
                    removed,
                );
                futures::stream::iter(all_logs)
            })
    }

    /// Returns a stream that yields new block headers from canonical chain updates.
    fn header_stream(
        &self,
    ) -> impl futures::Stream<Item = RpcHeader<Self::NetworkTypes>> + Send + Unpin {
        let converter = self.converter();
        self.provider().canonical_state_stream().flat_map(move |new_chain| {
            let headers = new_chain
                .committed()
                .blocks_iter()
                .filter_map(|block| {
                    match converter.convert_header(block.clone_sealed_header(), block.rlp_length())
                    {
                        Ok(header) => Some(header),
                        Err(err) => {
                            error!(target = "rpc", %err, "Failed to convert header");
                            None
                        }
                    }
                })
                .collect::<Vec<_>>();
            futures::stream::iter(headers)
        })
    }
}
