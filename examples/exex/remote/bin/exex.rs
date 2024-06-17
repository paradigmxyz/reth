use std::sync::Arc;

use exex_remote::proto::{
    remote_ex_ex_server::{RemoteExEx, RemoteExExServer},
    Notification as ProtoNotification, SubscribeRequest as ProtoSubscribeRequest,
};
use futures::TryFutureExt;
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

#[derive(Debug)]
struct ExExService {
    notifications: Arc<broadcast::Sender<ExExNotification>>,
}

#[tonic::async_trait]
impl RemoteExEx for ExExService {
    type SubscribeStream = ReceiverStream<Result<ProtoNotification, Status>>;

    async fn subscribe(
        &self,
        _request: Request<ProtoSubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let (tx, rx) = mpsc::channel(1);

        let mut notifications = self.notifications.subscribe();
        tokio::spawn(async move {
            while let Ok(notification) = notifications.recv().await {
                tx.send(Ok(ProtoNotification {
                    data: bincode::serialize(&notification)
                        .expect("failed to serialize notification"),
                }))
                .await
                .expect("failed to send notification to client");
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

async fn exex<Node: FullNodeComponents>(
    mut ctx: ExExContext<Node>,
    notifications: Arc<broadcast::Sender<ExExNotification>>,
) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.recv().await {
        match &notification {
            ExExNotification::ChainCommitted { new } => {
                info!(committed_chain = ?new.range(), "Received commit");
            }
            ExExNotification::ChainReorged { old, new } => {
                info!(from_chain = ?old.range(), to_chain = ?new.range(), "Received reorg");
            }
            ExExNotification::ChainReverted { old } => {
                info!(reverted_chain = ?old.range(), "Received revert");
            }
        };

        if let Some(committed_chain) = notification.committed_chain() {
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
        }

        let _ = notifications.send(notification);
    }

    Ok(())
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let notifications = Arc::new(broadcast::channel(1).0);

        let server = Server::builder()
            .add_service(RemoteExExServer::new(ExExService {
                notifications: notifications.clone(),
            }))
            .serve("[::1]:10000".parse().unwrap())
            .map_err(|err| err.into());

        let node = builder
            .node(EthereumNode::default())
            .install_exex("Remote", |ctx| async move { Ok(exex(ctx, notifications)) })
            .launch()
            .await?
            .wait_for_node_exit();

        futures::try_join!(server, node)?;

        Ok(())
    })
}
