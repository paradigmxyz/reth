//! `eth_` `PubSub` RPC handler implementation

use std::sync::Arc;

use alloy_consensus::{transaction::TxHashRef, BlockHeader, TxReceipt};
use alloy_primitives::{TxHash, U256};
use alloy_rpc_types_eth::{
    pubsub::{Params, PubSubSyncStatus, SubscriptionKind, SyncStatusMetadata},
    Filter, Header, Log,
};
use futures::StreamExt;
use jsonrpsee::{
    server::SubscriptionMessage, types::ErrorObject, PendingSubscriptionSink, SubscriptionSink,
};
use reth_chain_state::CanonStateSubscriptions;
use reth_network_api::NetworkInfo;
use reth_primitives_traits::{NodePrimitives, TransactionMeta};
use reth_rpc_convert::{build_convert_receipt_inputs, transaction::ConvertReceiptInput};
use reth_rpc_eth_api::{
    pubsub::EthPubSubApiServer, EthApiTypes, RpcConvert, RpcNodeCore, RpcTransaction,
};
use reth_rpc_eth_types::logs_utils;
use reth_rpc_server_types::result::{internal_rpc_err, invalid_params_rpc_err};
use reth_storage_api::BlockNumReader;
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use reth_transaction_pool::{NewTransactionEvent, PoolConsensusTx, TransactionPool};
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
    ///
    /// Subscription tasks are spawned via [`tokio::task::spawn`]
    pub fn new(eth_api: Eth) -> Self {
        Self::with_spawner(eth_api, Box::<TokioTaskExecutor>::default())
    }

    /// Creates a new, shareable instance.
    pub fn with_spawner(eth_api: Eth, subscription_task_spawner: Box<dyn TaskSpawner>) -> Self {
        let inner = EthPubSubInner { eth_api, subscription_task_spawner };
        Self { inner: Arc::new(inner) }
    }
}

impl<N: NodePrimitives, Eth> EthPubSub<Eth>
where
    Eth: RpcNodeCore<
            Provider: BlockNumReader + CanonStateSubscriptions<Primitives = N>,
            Pool: TransactionPool,
            Network: NetworkInfo,
        > + EthApiTypes<
            RpcConvert: RpcConvert<
                Primitives: NodePrimitives<SignedTx = PoolConsensusTx<Eth::Pool>>,
            >,
        >,
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
    pub fn new_headers_stream(&self) -> impl Stream<Item = Header<N::BlockHeader>> {
        self.inner.new_headers_stream()
    }

    /// Returns a stream that yields all logs that match the given filter.
    pub fn log_stream(&self, filter: Filter) -> impl Stream<Item = Log> {
        self.inner.log_stream(filter)
    }

    /// The actual handler for an accepted [`EthPubSub::subscribe`] call.
    pub async fn handle_accepted(
        &self,
        accepted_sink: SubscriptionSink,
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> Result<(), ErrorObject<'static>> {
        #[allow(unreachable_patterns)]
        match kind {
            SubscriptionKind::NewHeads => {
                pipe_from_stream(accepted_sink, self.new_headers_stream()).await
            }
            SubscriptionKind::Logs => {
                // if no params are provided, used default filter params
                let filter = match params {
                    Some(Params::Logs(filter)) => *filter,
                    Some(Params::Bool(_)) => {
                        return Err(invalid_params_rpc_err("Invalid params for logs"))
                    }
                    _ => Default::default(),
                };
                pipe_from_stream(accepted_sink, self.log_stream(filter)).await
            }
            SubscriptionKind::NewPendingTransactions => {
                if let Some(params) = params {
                    match params {
                        Params::Bool(true) => {
                            // full transaction objects requested
                            let stream = self.full_pending_transaction_stream().filter_map(|tx| {
                                let tx_value = match self
                                    .inner
                                    .eth_api
                                    .tx_resp_builder()
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
                            return pipe_from_stream(accepted_sink, stream).await
                        }
                        Params::Bool(false) | Params::None => {
                            // only hashes requested
                        }
                        Params::Logs(_) => {
                            return Err(invalid_params_rpc_err(
                                "Invalid params for newPendingTransactions",
                            ))
                        }
                    }
                }

                pipe_from_stream(accepted_sink, self.pending_transaction_hashes_stream()).await
            }
            SubscriptionKind::Syncing => {
                // get new block subscription
                let mut canon_state = BroadcastStream::new(
                    self.inner.eth_api.provider().subscribe_to_canonical_state(),
                );
                // get current sync status
                let mut initial_sync_status = self.inner.eth_api.network().is_syncing();
                let current_sub_res = self.sync_status(initial_sync_status);

                // send the current status immediately
                let msg = SubscriptionMessage::new(
                    accepted_sink.method_name(),
                    accepted_sink.subscription_id(),
                    &current_sub_res,
                )
                .map_err(SubscriptionSerializeError::new)?;

                if accepted_sink.send(msg).await.is_err() {
                    return Ok(())
                }

                while canon_state.next().await.is_some() {
                    let current_syncing = self.inner.eth_api.network().is_syncing();
                    // Only send a new response if the sync status has changed
                    if current_syncing != initial_sync_status {
                        // Update the sync status on each new block
                        initial_sync_status = current_syncing;

                        // send a new message now that the status changed
                        let sync_status = self.sync_status(current_syncing);
                        let msg = SubscriptionMessage::new(
                            accepted_sink.method_name(),
                            accepted_sink.subscription_id(),
                            &sync_status,
                        )
                        .map_err(SubscriptionSerializeError::new)?;

                        if accepted_sink.send(msg).await.is_err() {
                            break
                        }
                    }
                }

                Ok(())
            }
            _ => Err(invalid_params_rpc_err("Unsupported subscription kind")),
        }
    }
}

// Additional impl block for transaction receipts functionality
impl<N: NodePrimitives<SignedTx: TxHashRef>, Eth> EthPubSub<Eth>
where
    Eth: RpcNodeCore<
            Provider: BlockNumReader + CanonStateSubscriptions<Primitives = N>,
            Pool: TransactionPool,
            Network: NetworkInfo,
        > + EthApiTypes<
            RpcConvert: RpcConvert<Primitives = N, Network = Eth::NetworkTypes, Error = Eth::Error>,
        >,
{
    /// Returns a stream that yields all transaction receipts from new blocks.
    pub fn transaction_receipts_stream(
        &self,
        hashes: Option<Vec<TxHash>>,
    ) -> impl Stream<Item = reth_rpc_convert::RpcReceipt<Eth::NetworkTypes>> {
        self.inner.transaction_receipts_stream(hashes)
    }
}

#[async_trait::async_trait]
impl<Eth> EthPubSubApiServer<RpcTransaction<Eth::NetworkTypes>> for EthPubSub<Eth>
where
    Eth: RpcNodeCore<
            Provider: BlockNumReader + CanonStateSubscriptions,
            Pool: TransactionPool,
            Network: NetworkInfo,
        > + EthApiTypes<
            RpcConvert: RpcConvert<
                Primitives: NodePrimitives<SignedTx = PoolConsensusTx<Eth::Pool>>,
            >,
        > + 'static,
{
    /// Handler for `eth_subscribe`
    async fn subscribe(
        &self,
        pending: PendingSubscriptionSink,
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> jsonrpsee::core::SubscriptionResult {
        let sink = pending.accept().await?;
        let pubsub = self.clone();
        self.inner.subscription_task_spawner.spawn(Box::pin(async move {
            let _ = pubsub.handle_accepted(sink, kind, params).await;
        }));

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
    subscription_task_spawner: Box<dyn TaskSpawner>,
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

impl<N: NodePrimitives, Eth> EthPubSubInner<Eth>
where
    Eth: RpcNodeCore<Provider: CanonStateSubscriptions<Primitives = N>>,
{
    /// Returns a stream that yields all new RPC blocks.
    fn new_headers_stream(&self) -> impl Stream<Item = Header<N::BlockHeader>> {
        self.eth_api.provider().canonical_state_stream().flat_map(|new_chain| {
            let headers = new_chain
                .committed()
                .blocks_iter()
                .map(|block| {
                    Header::from_consensus(
                        block.clone_sealed_header().into(),
                        None,
                        Some(U256::from(block.rlp_length())),
                    )
                })
                .collect::<Vec<_>>();
            futures::stream::iter(headers)
        })
    }

    /// Returns a stream that yields all logs that match the given filter.
    fn log_stream(&self, filter: Filter) -> impl Stream<Item = Log> {
        BroadcastStream::new(self.eth_api.provider().subscribe_to_canonical_state())
            .map(move |canon_state| {
                canon_state.expect("new block subscription never ends").block_receipts()
            })
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

impl<N: NodePrimitives<SignedTx: TxHashRef>, Eth> EthPubSubInner<Eth>
where
    Eth: RpcNodeCore<Provider: CanonStateSubscriptions<Primitives = N>>
        + EthApiTypes<
            RpcConvert: RpcConvert<Primitives = N, Network = Eth::NetworkTypes, Error = Eth::Error>,
        >,
{
    /// Returns a stream that yields all transaction receipts from new blocks.
    fn transaction_receipts_stream(
        &self,
        hashes: Option<Vec<TxHash>>,
    ) -> impl Stream<Item = reth_rpc_convert::RpcReceipt<Eth::NetworkTypes>> {
        let eth_api = self.eth_api.clone();
        self.eth_api.provider().canonical_state_stream().flat_map(move |new_chain| {
            let mut all_receipts = Vec::new();

            // Process all blocks in the committed chain - exactly like new_headers_stream pattern
            for (block, receipts) in new_chain.committed().blocks_and_receipts() {
                match &hashes {
                    // If no specific hashes requested, process all receipts
                    None => {
                        let inputs = build_convert_receipt_inputs(block, receipts);
                        if let Ok(rpc_receipts) = eth_api
                            .tx_resp_builder()
                            .convert_receipts_with_block(inputs, block.sealed_block())
                        {
                            all_receipts.extend(rpc_receipts);
                        }
                    }
                    // If specific hashes requested, filter during iteration to maintain proper
                    // gas/log indexing
                    Some(target_hashes) => {
                        use std::collections::HashSet;
                        let hash_set: HashSet<_> = target_hashes.iter().collect();

                        // Build inputs manually for filtered transactions, maintaining proper order
                        let block_number = block.header().number();
                        let base_fee = block.header().base_fee_per_gas();
                        let block_hash = block.hash();
                        let excess_blob_gas = block.header().excess_blob_gas();
                        let timestamp = block.header().timestamp();
                        let mut gas_used = 0;
                        let mut next_log_index = 0;

                        let filtered_inputs: Vec<_> = block
                            .transactions_recovered()
                            .zip(receipts.iter())
                            .enumerate()
                            .filter_map(|(idx, (tx, receipt))| {
                                let cumulative_gas_used = receipt.cumulative_gas_used();
                                let logs_len = receipt.logs().len();

                                let current_gas_used = cumulative_gas_used - gas_used;
                                let current_log_index = next_log_index;

                                // Update for next iteration
                                gas_used = cumulative_gas_used;
                                next_log_index += logs_len;

                                // Only include if hash matches
                                hash_set.contains(tx.tx_hash()).then(|| {
                                    let meta = TransactionMeta {
                                        tx_hash: *tx.tx_hash(),
                                        index: idx as u64,
                                        block_hash,
                                        block_number,
                                        base_fee,
                                        excess_blob_gas,
                                        timestamp,
                                    };

                                    ConvertReceiptInput {
                                        tx,
                                        gas_used: current_gas_used,
                                        next_log_index: current_log_index,
                                        meta,
                                        receipt: receipt.clone(),
                                    }
                                })
                            })
                            .collect();

                        if !filtered_inputs.is_empty() &&
                            let Ok(rpc_receipts) =
                                eth_api.tx_resp_builder().convert_receipts_with_block(
                                    filtered_inputs,
                                    block.sealed_block(),
                                )
                        {
                            all_receipts.extend(rpc_receipts);
                        }
                    }
                }
            }

            futures::stream::iter(all_receipts)
        })
    }
}
