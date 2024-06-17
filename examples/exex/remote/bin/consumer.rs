use exex_remote::{
    codec::from_u8_slice,
    proto::{
        remote_ex_ex_client::RemoteExExClient, ExExNotification as ProtoExExNotification,
        Notification, SubscribeRequest,
    },
};

#[tokio::main]
async fn main() -> eyre::Result<()> {
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
        // TODO(alexey): it doesn't work, because data inside the notification is boxed.
        //  We need to implement a proper encoding via Serde.
        // let notification = unsafe { from_u8_slice(&data) };
        // println!("{:?}", notification);
        println!("{:?}", data);
    }

    Ok(())
}
