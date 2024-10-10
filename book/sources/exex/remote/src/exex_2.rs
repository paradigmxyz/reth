use remote_exex::proto::{
    self,
    remote_ex_ex_server::{RemoteExEx, RemoteExExServer},
};
use reth_exex::ExExNotification;
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

#[allow(dead_code)]
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
        let (_tx, rx) = mpsc::channel(1);

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let notifications = Arc::new(broadcast::channel(1).0);

        let server = Server::builder()
            .add_service(RemoteExExServer::new(ExExService {
                notifications: notifications.clone(),
            }))
            .serve("[::1]:10000".parse().unwrap());

        let handle = builder.node(EthereumNode::default()).launch().await?;

        handle.node.task_executor.spawn_critical("gRPC server", async move {
            server.await.expect("failed to start gRPC server")
        });

        handle.wait_for_node_exit().await
    })
}
