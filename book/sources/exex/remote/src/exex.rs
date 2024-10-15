use futures_util::TryStreamExt;
use remote_exex::proto::{
    self,
    remote_ex_ex_server::{RemoteExEx, RemoteExExServer},
};
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

struct ExExService {
    notifications: Arc<broadcast::Sender<ExExNotification>>,
}

#[tonic::async_trait]
impl RemoteExEx for ExExService {
    type SubscribeStream = ReceiverStream<Result<proto::ExExNotification, Status>>;

    async fn subscribe(
        &self,
        _request: Request<proto::SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let (tx, rx) = mpsc::channel(1);

        let mut notifications = self.notifications.subscribe();
        tokio::spawn(async move {
            while let Ok(notification) = notifications.recv().await {
                let proto_notification = proto::ExExNotification {
                    data: bincode::serialize(&notification).expect("failed to serialize"),
                };
                tx.send(Ok(proto_notification))
                    .await
                    .expect("failed to send notification to client");

                info!("Notification sent to the gRPC client");
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

async fn remote_exex<Node: FullNodeComponents>(
    mut ctx: ExExContext<Node>,
    notifications: Arc<broadcast::Sender<ExExNotification>>,
) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.try_next().await? {
        if let Some(committed_chain) = notification.committed_chain() {
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
        }

        info!("Notification sent to the gRPC server");
        let _ = notifications.send(notification);
    }

    Ok(())
}

// ANCHOR: snippet
fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let notifications = Arc::new(broadcast::channel(1).0);

        let server = Server::builder()
            .add_service(RemoteExExServer::new(ExExService {
                notifications: notifications.clone(),
            }))
            .serve("[::1]:10000".parse().unwrap());

        let handle = builder
            .node(EthereumNode::default())
            .install_exex("remote-exex", |ctx| async move { Ok(remote_exex(ctx, notifications)) })
            .launch()
            .await?;

        handle.node.task_executor.spawn_critical("gRPC server", async move {
            server.await.expect("failed to start gRPC server")
        });

        handle.wait_for_node_exit().await
    })
}
// ANCHOR_END: snippet
