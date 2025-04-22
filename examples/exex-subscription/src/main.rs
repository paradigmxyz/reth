#![warn(unused_crate_dependencies)]
#![allow(unused_imports)] //will be removed in the future
#![allow(dead_code)] //will be removed in the future
use alloy_primitives::{Address, B256};
use clap::Parser;
use futures::TryStreamExt;
use jsonrpsee::{
    core::SubscriptionResult, proc_macros::rpc, tracing, PendingSubscriptionSink,
    SubscriptionMessage,
};
use reth::cli::Cli;
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
    key: B256,
    old_value: B256,
    new_value: B256,
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

use jsonrpsee::server::{ServerBuilder, ServerHandle};

async fn my_exex<Node: FullNodeComponents>(
    mut _ctx: ExExContext<Node>,
    _subscriptions: SubscriptionMap,
) -> eyre::Result<()> {
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
