//! `eth_` `PubSub` RPC handler implementation

use std::sync::Arc;

use alloy_consensus::{transaction::TxHashRef, BlockHeader, TxReceipt};
use alloy_primitives::TxHash;
use alloy_rpc_types_eth::{
    pubsub::{
        Params, PubSubSyncStatus, SubscriptionKind, SyncStatusMetadata, TransactionReceiptsParams,
    },
    Filter, Log,
};
use futures::StreamExt;
use jsonrpsee::{
    server::SubscriptionMessage, types::ErrorObject, PendingSubscriptionSink, SubscriptionSink,
};
use reth_chain_state::CanonStateSubscriptions;
use reth_network_api::NetworkInfo;
use reth_primitives_traits::TransactionMeta;
use reth_rpc_convert::{transaction::ConvertReceiptInput, RpcHeader};
use reth_rpc_eth_api::{
    pubsub::EthPubSubApiServer, EthApiTypes, RpcConvert, RpcNodeCore, RpcTransaction,
};
use reth_rpc_eth_types::logs_utils;
use reth_rpc_server_types::result::{internal_rpc_err, invalid_params_rpc_err};
use reth_storage_api::BlockNumReader;
use reth_tasks::Runtime;
use reth_transaction_pool::{NewTransactionEvent, TransactionPool};
use serde::Serialize;
use tokio_stream::{
    wrappers::{BroadcastStream, ReceiverStream},
    Stream,
};
use tracing::error;

/// `Eth` pubsub RPC implementation.
///
/// This handles `eth_subscribe` RPC calls.
#[derive(Clone)]
pub struct EthPubSub<Eth> {
    /// All nested fields bundled together.
    inner: Arc<EthPubSubInner<Eth>>,
}

// === impl EthPubSub ===

impl<Eth> EthPubSub<Eth> {
    /// Creates a new, shareable instance.
    pub fn new(eth_api: Eth, subscription_task_spawner: Runtime) -> Self {
        let inner = EthPubSubInner { eth_api, subscription_task_spawner };
        Self { inner: Arc::new(inner) }
    }
}

impl<Eth> EthPubSub<Eth>
where
    Eth: RpcNodeCore + EthApiTypes<RpcConvert: RpcConvert<Primitives = Eth::Primitives>>,
{
    /// Returns the current sync status for the `syncing` subscription
    pub fn sync_status(&self, is_syncing: bool) -> PubSubSyncStatus {
        self.inner.sync_status(is_syncing)
    }

    /// Returns a stream that yields all transaction hashes emitted by the txpool.
    pub fn pending_transaction_hashes_stream(&self) -> impl Stream<Item = TxHash> {
        self.inner.pending_transaction_hashes_stream()
    }

    /// Returns a stream that yields all transactions emitted by the txpool.
    pub fn full_pending_transaction_stream(
        &self,
    ) -> impl Stream<Item = NewTransactionEvent<<Eth::Pool as TransactionPool>::Transaction>> {
        self.inner.full_pending_transaction_stream()
    }

    /// Returns a stream that yields all new RPC blocks.
    pub fn new_headers_stream(&self) -> impl Stream<Item = RpcHeader<Eth::NetworkTypes>> {
        self.inner.new_headers_stream()
    }

    /// Returns a stream that yields all logs that match the given filter.
    pub fn log_stream(&self, filter: Filter) -> impl Stream<Item = Log> {
        self.inner.log_stream(filter)
    }
}

#[async_trait::async_trait]
impl<Eth> EthPubSubApiServer<RpcTransaction<Eth::NetworkTypes>> for EthPubSub<Eth>
where
    Eth: RpcNodeCore + EthApiTypes<RpcConvert: RpcConvert<Primitives = Eth::Primitives>>,
{
    /// Handler for `eth_subscribe`
    async fn subscribe(
        &self,
        pending: PendingSubscriptionSink,
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> jsonrpsee::core::SubscriptionResult {
        // Register the underlying listener BEFORE `pending.accept()`. After
        // accept the client already has the subscription id and may begin
        // observing events; the spawned task is only enqueued, so any events
        // fired before the listener is wired up would otherwise be lost.
        #[allow(unreachable_patterns)]
        match kind {
            SubscriptionKind::NewHeads => {
                let canon_state = self.inner.eth_api.provider().canonical_state_stream();
                let inner = self.inner.clone();
                let sink = pending.accept().await?;
                self.inner.subscription_task_spawner.spawn_task(async move {
                    let stream = canon_state.flat_map(move |new_chain| {
                        let converter = inner.eth_api.converter();
                        let headers = new_chain
                            .committed()
                            .blocks_iter()
                            .filter_map(|block| {
                                match converter
                                    .convert_header(block.clone_sealed_header(), block.rlp_length())
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
                    });
                    let _ = pipe_from_stream(sink, stream).await;
                });
            }
            SubscriptionKind::Logs => {
                // if no params are provided, used default filter params
                let filter = match params {
                    Some(Params::Logs(filter)) => *filter,
                    Some(Params::Bool(_)) => {
                        pending.reject(invalid_params_rpc_err("Invalid params for logs")).await;
                        return Ok(());
                    }
                    _ => Default::default(),
                };
                let canon_state = self.inner.eth_api.provider().canonical_state_stream();
                let sink = pending.accept().await?;
                self.inner.subscription_task_spawner.spawn_task(async move {
                    let stream = canon_state
                        .map(move |canon_state| canon_state.block_receipts())
                        .flat_map(futures::stream::iter)
                        .flat_map(move |(block_receipts, removed)| {
                            let all_logs = logs_utils::matching_block_logs_with_tx_hashes(
                                &filter,
                                block_receipts.block,
                                block_receipts.timestamp,
                                block_receipts
                                    .tx_receipts
                                    .iter()
                                    .map(|(tx, receipt)| (*tx, receipt)),
                                removed,
                            );
                            futures::stream::iter(all_logs)
                        });
                    let _ = pipe_from_stream(sink, stream).await;
                });
            }
            SubscriptionKind::NewPendingTransactions => match params {
                Some(Params::Bool(true)) => {
                    // full transaction objects requested
                    let raw = self.inner.eth_api.pool().new_pending_pool_transactions_listener();
                    let inner = self.inner.clone();
                    let sink = pending.accept().await?;
                    self.inner.subscription_task_spawner.spawn_task(async move {
                        let stream = raw.filter_map(move |tx| {
                            let tx_value = match inner
                                .eth_api
                                .converter()
                                .fill_pending(tx.transaction.to_consensus())
                            {
                                Ok(tx) => Some(tx),
                                Err(err) => {
                                    error!(target = "rpc",
                                        %err,
                                        "Failed to fill transaction with block context"
                                    );
                                    None
                                }
                            };
                            std::future::ready(tx_value)
                        });
                        let _ = pipe_from_stream(sink, stream).await;
                    });
                }
                Some(Params::Bool(false) | Params::None) | None => {
                    // only hashes requested
                    let rx = self.inner.eth_api.pool().pending_transactions_listener();
                    let sink = pending.accept().await?;
                    self.inner.subscription_task_spawner.spawn_task(async move {
                        let stream = ReceiverStream::new(rx);
                        let _ = pipe_from_stream(sink, stream).await;
                    });
                }
                _ => {
                    pending
                        .reject(invalid_params_rpc_err("Invalid params for newPendingTransactions"))
                        .await;
                    return Ok(());
                }
            },
            SubscriptionKind::Syncing => {
                let canon_state = BroadcastStream::new(
                    self.inner.eth_api.provider().subscribe_to_canonical_state(),
                );
                let pubsub = self.clone();
                let sink = pending.accept().await?;
                self.inner.subscription_task_spawner.spawn_task(async move {
                    let mut canon_state = canon_state;
                    let mut initial_sync_status = pubsub.inner.eth_api.network().is_syncing();
                    let current_sub_res = pubsub.sync_status(initial_sync_status);

                    // send the current status immediately
                    let msg = match SubscriptionMessage::new(
                        sink.method_name(),
                        sink.subscription_id(),
                        &current_sub_res,
                    ) {
                        Ok(msg) => msg,
                        Err(_) => return,
                    };

                    if sink.send(msg).await.is_err() {
                        return;
                    }

                    while canon_state.next().await.is_some() {
                        let current_syncing = pubsub.inner.eth_api.network().is_syncing();
                        // Only send a new response if the sync status has changed
                        if current_syncing != initial_sync_status {
                            // Update the sync status on each new block
                            initial_sync_status = current_syncing;

                            // send a new message now that the status changed
                            let sync_status = pubsub.sync_status(current_syncing);
                            let msg = match SubscriptionMessage::new(
                                sink.method_name(),
                                sink.subscription_id(),
                                &sync_status,
                            ) {
                                Ok(msg) => msg,
                                Err(_) => return,
                            };
                            if sink.send(msg).await.is_err() {
                                break;
                            }
                        }
                    }
                });
            }
            SubscriptionKind::TransactionReceipts => {
                let filter = match params {
                    Some(Params::TransactionReceipts(filter)) => filter,
                    None | Some(Params::None) => TransactionReceiptsParams::default(),
                    _ => {
                        pending
                            .reject(invalid_params_rpc_err(
                                "Invalid params for transactionReceipts",
                            ))
                            .await;
                        return Ok(());
                    }
                };

                let canon_state = self.inner.eth_api.provider().canonical_state_stream();
                let inner = self.inner.clone();
                let sink = pending.accept().await?;
                self.inner.subscription_task_spawner.spawn_task(async move {
                    let stream = canon_state.flat_map(move |new_chain| {
                        let converter = inner.eth_api.converter();
                        // for each block in the new chain, build RPC receipts
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

                                // build ConvertReceiptInput for each tx+receipt pair
                                // (same logic as eth_getBlockReceipts HTTP endpoint)
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

                                        // apply transaction hash filter if provided
                                        let matches = match &filter.transaction_hashes {
                                            Some(hashes) if !hashes.is_empty() => {
                                                hashes.contains(tx.tx_hash())
                                            }
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

                                match converter.convert_receipts(inputs) {
                                    Ok(rpc_receipts) => Some(rpc_receipts),
                                    Err(err) => {
                                        error!(
                                            target = "rpc",
                                            %err,
                                            "Failed to convert receipts"
                                        );
                                        None
                                    }
                                }
                            })
                            .collect();

                        futures::stream::iter(results)
                    });
                    let _ = pipe_from_stream(sink, stream).await;
                });
            }
            _ => {
                pending.reject(invalid_params_rpc_err("Unsupported subscription kind")).await;
                return Ok(());
            }
        }

        Ok(())
    }
}

/// Helper to convert a serde error into an [`ErrorObject`]
#[derive(Debug, thiserror::Error)]
#[error("Failed to serialize subscription item: {0}")]
pub struct SubscriptionSerializeError(#[from] serde_json::Error);

impl SubscriptionSerializeError {
    const fn new(err: serde_json::Error) -> Self {
        Self(err)
    }
}

impl From<SubscriptionSerializeError> for ErrorObject<'static> {
    fn from(value: SubscriptionSerializeError) -> Self {
        internal_rpc_err(value.to_string())
    }
}

/// Pipes all stream items to the subscription sink.
async fn pipe_from_stream<T, St>(
    sink: SubscriptionSink,
    mut stream: St,
) -> Result<(), ErrorObject<'static>>
where
    St: Stream<Item = T> + Unpin,
    T: Serialize,
{
    loop {
        tokio::select! {
            _ = sink.closed() => {
                // connection dropped
                break Ok(())
            },
            maybe_item = stream.next() => {
                let item = match maybe_item {
                    Some(item) => item,
                    None => {
                        // stream ended
                        break  Ok(())
                    },
                };
                let msg = SubscriptionMessage::new(
                    sink.method_name(),
                    sink.subscription_id(),
                    &item
                ).map_err(SubscriptionSerializeError::new)?;

                if sink.send(msg).await.is_err() {
                    break Ok(());
                }
            }
        }
    }
}

impl<Eth> std::fmt::Debug for EthPubSub<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthPubSub").finish_non_exhaustive()
    }
}

/// Container type `EthPubSub`
#[derive(Clone)]
struct EthPubSubInner<EthApi> {
    /// The `eth` API.
    eth_api: EthApi,
    /// The type that's used to spawn subscription tasks.
    subscription_task_spawner: Runtime,
}

// == impl EthPubSubInner ===

impl<Eth> EthPubSubInner<Eth>
where
    Eth: RpcNodeCore<Provider: BlockNumReader>,
{
    /// Returns the current sync status for the `syncing` subscription
    fn sync_status(&self, is_syncing: bool) -> PubSubSyncStatus {
        if is_syncing {
            let current_block = self
                .eth_api
                .provider()
                .chain_info()
                .map(|info| info.best_number)
                .unwrap_or_default();
            PubSubSyncStatus::Detailed(SyncStatusMetadata {
                syncing: true,
                starting_block: 0,
                current_block,
                highest_block: Some(current_block),
            })
        } else {
            PubSubSyncStatus::Simple(false)
        }
    }
}

impl<Eth> EthPubSubInner<Eth>
where
    Eth: RpcNodeCore<Pool: TransactionPool>,
{
    /// Returns a stream that yields all transaction hashes emitted by the txpool.
    fn pending_transaction_hashes_stream(&self) -> impl Stream<Item = TxHash> {
        ReceiverStream::new(self.eth_api.pool().pending_transactions_listener())
    }

    /// Returns a stream that yields all transactions emitted by the txpool.
    fn full_pending_transaction_stream(
        &self,
    ) -> impl Stream<Item = NewTransactionEvent<<Eth::Pool as TransactionPool>::Transaction>> {
        self.eth_api.pool().new_pending_pool_transactions_listener()
    }
}

impl<Eth> EthPubSubInner<Eth>
where
    Eth: EthApiTypes<RpcConvert: RpcConvert<Primitives = Eth::Primitives>> + RpcNodeCore,
{
    /// Returns a stream that yields all new RPC blocks.
    fn new_headers_stream(&self) -> impl Stream<Item = RpcHeader<Eth::NetworkTypes>> {
        let converter = self.eth_api.converter();
        self.eth_api.provider().canonical_state_stream().flat_map(|new_chain| {
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

    /// Returns a stream that yields all logs that match the given filter.
    fn log_stream(&self, filter: Filter) -> impl Stream<Item = Log> {
        self.eth_api
            .provider()
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
}
