//! An ExEx example that installs a new RPC subscription endpoint that emit storage changes for a
//! requested address.

use alloy_primitives::{Address, U256};
use clap::Parser;
use futures::TryStreamExt;
use jsonrpsee::{
    core::SubscriptionResult, proc_macros::rpc, tracing, PendingSubscriptionSink,
    SubscriptionMessage,
};
use reth_ethereum::{
    exex::{ExExContext, ExExEvent, ExExNotification},
    node::{api::FullNodeComponents, EthereumNode},
};
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info};

/// Subscription update format for storage changes.
/// This is the format that will be sent to the client when a storage change occurs.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
struct StorageDiff {
    address: Address,
    key: U256,
    old_value: U256,
    new_value: U256,
}

/// Subscription request format for storage changes.
struct SubscriptionRequest {
    /// The address to subscribe to.
    address: Address,
    /// The response channel to send the subscription updates to.
    response: oneshot::Sender<mpsc::UnboundedReceiver<StorageDiff>>,
}

/// Subscription request format for storage changes.
type SubscriptionSender = mpsc::UnboundedSender<SubscriptionRequest>;

/// API to subscribe to storage changes for a specific Ethereum address.
#[rpc(server, namespace = "watcher")]
pub trait StorageWatcherApi {
    /// Subscribes to storage changes for a given Ethereum address and streams `StorageDiff`
    /// updates.
    #[subscription(name = "subscribeStorageChanges", item = StorageDiff)]
    fn subscribe_storage_changes(&self, address: Address) -> SubscriptionResult;
}

/// API implementation for the storage watcher.
#[derive(Clone)]
struct StorageWatcherRpc {
    /// The subscription sender to send subscription requests to.
    subscriptions: SubscriptionSender,
}

impl StorageWatcherRpc {
    /// Creates a new [`StorageWatcherRpc`] instance with the given subscription sender.
    fn new(subscriptions: SubscriptionSender) -> Self {
        Self { subscriptions }
    }
}

impl StorageWatcherApiServer for StorageWatcherRpc {
    fn subscribe_storage_changes(
        &self,
        pending: PendingSubscriptionSink,
        address: Address,
    ) -> SubscriptionResult {
        let subscription = self.subscriptions.clone();

        tokio::spawn(async move {
            let sink = match pending.accept().await {
                Ok(sink) => sink,
                Err(e) => {
                    error!("failed to accept subscription: {e}");
                    return;
                }
            };

            let (resp_tx, resp_rx) = oneshot::channel();
            subscription.send(SubscriptionRequest { address, response: resp_tx }).unwrap();

            let Ok(mut rx) = resp_rx.await else { return };

            while let Some(diff) = rx.recv().await {
                let msg = SubscriptionMessage::from_json(&diff).expect("serialize");
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });

        Ok(())
    }
}

async fn my_exex<Node: FullNodeComponents>(
    mut ctx: ExExContext<Node>,
    _subscriptions_senderr: SubscriptionSender,
) -> eyre::Result<()> {
    let subscriptions: HashMap<Address, Vec<mpsc::UnboundedSender<StorageDiff>>> = HashMap::new();

    while let Some(notification) = ctx.notifications.try_next().await? {
        match &notification {
            ExExNotification::ChainCommitted { new } => {
                info!(committed_chain = ?new.range(), "Received commit");
                let execution_outcome = new.execution_outcome();

                for (address, senders) in &subscriptions {
                    for change in &execution_outcome.bundle.state {
                        if change.0 == address {
                            for (key, slot) in &change.1.storage {
                                let diff = StorageDiff {
                                    address: *change.0,
                                    key: *key,
                                    old_value: slot.original_value(),
                                    new_value: slot.present_value(),
                                };

                                for sender in senders {
                                    let _ = sender.send(diff);
                                }
                            }
                        }
                    }
                }
            }
            ExExNotification::ChainReorged { old, new } => {
                info!(from_chain = ?old.range(), to_chain = ?new.range(), "Received reorg");
            }
            ExExNotification::ChainReverted { old } => {
                info!(reverted_chain = ?old.range(), "Received revert");
            }
        }

        if let Some(committed_chain) = notification.committed_chain() {
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
        }
    }
    Ok(())
}

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    enable_ext: bool,
}

fn main() -> eyre::Result<()> {
    reth_ethereum::cli::Cli::parse_args().run(async move |builder, _| {
        let (subscriptions_tx, subscriptions_rx) = mpsc::unbounded_channel::<SubscriptionRequest>();
        let subscriptions_sender: SubscriptionSender = subscriptions_tx.clone();

        let handle = builder
            .node(EthereumNode::default())
            .install_exex("my-exex", async move |ctx| Ok(my_exex(ctx, subscriptions_sender)))
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
