#![warn(unused_crate_dependencies)]
#![allow(dead_code)]
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
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{mpsc, RwLock};
use tracing::info;

/// Subscription update format
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
struct StorageDiff {
    address: Address,
    key: U256,
    old_value: U256,
    new_value: U256,
}

type SubscriptionMap = Arc<RwLock<HashMap<Address, Vec<mpsc::Sender<StorageDiff>>>>>;

#[rpc(server, namespace = "watcher")]
pub trait StorageWatcherApi {
    #[subscription(name = "subscribeStorageChanges", item = StorageDiff)]
    fn subscribe_storage_changes(&self, address: Address) -> SubscriptionResult;
}

#[derive(Clone)]
struct StorageWatcherRpc {
    subscriptions: SubscriptionMap,
}

impl StorageWatcherRpc {
    fn new(subscriptions: SubscriptionMap) -> Self {
        Self { subscriptions }
    }
}

impl StorageWatcherApiServer for StorageWatcherRpc {
    fn subscribe_storage_changes(
        &self,
        pending: PendingSubscriptionSink,
        address: Address,
    ) -> SubscriptionResult {
        let subscriptions = self.subscriptions.clone();

        tokio::spawn(async move {
            let sink = match pending.accept().await {
                Ok(sink) => sink,
                Err(e) => {
                    println!("failed to accept subscription: {}", e);
                    return;
                }
            };

            let (tx, mut rx) = tokio::sync::mpsc::channel::<StorageDiff>(16);

            {
                let mut map = subscriptions.write().await;
                map.entry(address).or_default().push(tx);
            }

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
    subscriptions: SubscriptionMap,
) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.try_next().await? {
        match &notification {
            ExExNotification::ChainCommitted { new } => {
                info!(committed_chain = ?new.range(), "Received commit");
                let execution_outcome = new.execution_outcome();
                let subscriptions = subscriptions.read().await;
                for (address, senders) in subscriptions.iter() {
                    for change in &execution_outcome.bundle.state {
                        if change.0 == address {
                            for (key, slot) in &change.1.storage {
                                let diff = StorageDiff {
                                    address: *change.0,
                                    key: key.clone(),
                                    old_value: slot.original_value(),
                                    new_value: slot.present_value(),
                                };

                                for sender in senders {
                                    let _ = sender.send(diff.clone());
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
    reth::cli::Cli::parse_args().run(async move |builder, _| {
        let subscriptions: SubscriptionMap = Arc::new(RwLock::new(HashMap::new()));
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("my-exex", async move |ctx| Ok(my_exex(ctx, subscriptions)))
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
