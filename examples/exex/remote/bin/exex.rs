use example_exex_remote::proto::{
    remote_ex_ex_server::{RemoteExEx, RemoteExExServer},
    ExExNotification as ProtoExExNotification, SubscribeRequest as ProtoSubscribeRequest,
};
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

#[derive(Debug)]
struct ExExService {
    notifications: broadcast::Sender<ExExNotification>,
}

#[tonic::async_trait]
impl RemoteExEx for ExExService {
    type SubscribeStream = ReceiverStream<Result<ProtoExExNotification, Status>>;

    async fn subscribe(
        &self,
        _request: Request<ProtoSubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let (tx, rx) = mpsc::channel(1);

        let mut notifications = self.notifications.subscribe();
        tokio::spawn(async move {
            while let Ok(notification) = notifications.recv().await {
                tx.send(Ok((&notification).try_into().expect("failed to encode")))
                    .await
                    .expect("failed to send notification to client");
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

async fn exex<Node: FullNodeComponents>(
    mut ctx: ExExContext<Node>,
    notifications: broadcast::Sender<ExExNotification>,
) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.recv().await {
        if let Some(committed_chain) = notification.committed_chain() {
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
        }

        let _ = notifications.send(notification);
    }

    Ok(())
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let notifications = broadcast::channel(1).0;

        let server = Server::builder()
            .add_service(RemoteExExServer::new(ExExService {
                notifications: notifications.clone(),
            }))
            .serve("[::1]:10000".parse().unwrap());

        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Remote", |ctx| async move { Ok(exex(ctx, notifications)) })
            .launch()
            .await?;

        handle.node.task_executor.spawn_critical("gRPC server", async move {
            server.await.expect("gRPC server crashed")
        });

        handle.wait_for_node_exit().await
    })
}
