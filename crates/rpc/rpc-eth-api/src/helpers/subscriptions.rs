//! Streams subscriptions providers for `eth_subscribe`.

use crate::{EthApiTypes, RpcConvert, RpcNodeCore, RpcReceipt};
use alloy_consensus::{transaction::TxHashRef, BlockHeader, TxReceipt};
use alloy_rpc_types_eth::{
    pubsub::{Params, SubscriptionKind, TransactionReceiptsParams},
    Filter, Log,
};
use futures::StreamExt;
use jsonrpsee_types::ErrorObject;
use reth_chain_state::CanonStateSubscriptions;
use reth_primitives_traits::TransactionMeta;
use reth_rpc_convert::{transaction::ConvertReceiptInput, RpcHeader};
use reth_rpc_eth_types::logs_utils;
use reth_rpc_server_types::result::invalid_params_rpc_err;
use tracing::error;

/// Provides streams subscriptions for `eth_subscribe`.
///
/// Override the default methods to inject additional data sources (e.g. flashblocks).
pub trait EthSubscriptions:
    RpcNodeCore + EthApiTypes<RpcConvert: RpcConvert<Primitives = Self::Primitives>>
{
    /// Ensures the subscription params are supported by this subscription provider.
    ///
    /// The default log stream only watches canonical chain updates, so pending block log filters
    /// are rejected instead of being silently treated as canonical log subscriptions. Providers
    /// that support pending log streams can override this together with [`Self::log_stream`].
    fn ensure_subscription_params_supported(
        &self,
        kind: SubscriptionKind,
        params: Option<&Params>,
    ) -> Result<(), ErrorObject<'static>> {
        if let (SubscriptionKind::Logs, Some(Params::Logs(filter))) = (kind, params) {
            let (from_block, to_block) = filter.block_option.as_range();
            if from_block.is_some_and(|block| block.is_pending()) ||
                to_block.is_some_and(|block| block.is_pending())
            {
                return Err(invalid_params_rpc_err(
                    "pending block filters are not supported for logs subscriptions",
                ));
            }
        }

        Ok(())
    }

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

    /// Returns a stream that yields matching transaction receipts from canonical chain updates.
    fn transaction_receipts_stream(
        &self,
        filter: TransactionReceiptsParams,
    ) -> impl futures::Stream<Item = Vec<RpcReceipt<Self::NetworkTypes>>> + Send + Unpin {
        let converter = self.converter();
        self.provider().canonical_state_stream().flat_map(move |new_chain| {
            let results: Vec<_> = new_chain
                .committed()
                .blocks_and_receipts()
                .filter_map(|(block, receipts)| {
                    let block_hash = block.hash();
                    let block_number = block.number();
                    let base_fee = block.base_fee_per_gas();
                    let excess_blob_gas = block.excess_blob_gas();
                    let timestamp = block.timestamp();

                    let mut gas_used: u64 = 0;
                    let mut next_log_index: usize = 0;

                    let inputs: Vec<_> = block
                        .transactions_recovered()
                        .zip(receipts.iter())
                        .enumerate()
                        .filter_map(|(idx, (tx, receipt))| {
                            let gas_used_before = gas_used;
                            let next_log_index_before = next_log_index;
                            let cumulative_gas_used = receipt.cumulative_gas_used();

                            gas_used = cumulative_gas_used;
                            next_log_index += receipt.logs().len();

                            let matches = match &filter.transaction_hashes {
                                Some(hashes) if !hashes.is_empty() => hashes.contains(tx.tx_hash()),
                                _ => true,
                            };

                            matches.then(|| ConvertReceiptInput {
                                tx,
                                gas_used: cumulative_gas_used - gas_used_before,
                                next_log_index: next_log_index_before,
                                meta: TransactionMeta {
                                    tx_hash: *tx.tx_hash(),
                                    index: idx as u64,
                                    block_hash,
                                    block_number,
                                    base_fee,
                                    excess_blob_gas,
                                    timestamp,
                                },
                                receipt: receipt.clone(),
                            })
                        })
                        .collect();

                    if inputs.is_empty() {
                        return None;
                    }

                    match converter.convert_receipts_with_block(inputs, block.sealed_block()) {
                        Ok(rpc_receipts) => Some(rpc_receipts),
                        Err(err) => {
                            error!(target = "rpc", %err, "Failed to convert receipts");
                            None
                        }
                    }
                })
                .collect();

            futures::stream::iter(results)
        })
    }
}
