use exex_remote::proto::{
    remote_ex_ex_client::RemoteExExClient, ExExNotification as ProtoExExNotification, Notification,
    SubscribeRequest,
};
use reth_exex::ExExNotification;
use reth_tracing::{tracing::info, RethTracer, Tracer};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let _ = RethTracer::new().init()?;

    let mut client = RemoteExExClient::connect("http://[::1]:10000").await?;

    let mut stream = client
        .subscribe(SubscribeRequest {
            notifications: vec![
                ProtoExExNotification::ChainCommitted as i32,
                ProtoExExNotification::ChainReorged as i32,
                ProtoExExNotification::ChainReverted as i32,
            ],
        })
        .await?
        .into_inner();

    while let Some(Notification { data }) = stream.message().await? {
        let notification = bincode::deserialize::<ExExNotification>(&data)?;

        match notification {
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
    }

    Ok(())
}
