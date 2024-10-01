# Remote Execution Extensions

In this chapter, we will learn how to create an ExEx that emits all notifications to an external process.

We will use [Tonic](https://github.com/hyperium/tonic) to create a gRPC server and a client.
- The server binary will have the Reth client, our ExEx and the gRPC server.
- The client binary will have the gRPC client that connects to the server.

## Prerequisites

See [section](https://github.com/hyperium/tonic?tab=readme-ov-file#dependencies) of the Tonic documentation
to install the required dependencies.

## Create a new project

Let's create a new project. Don't forget to provide the `--lib` flag to `cargo new`,
because we will have two custom binaries in this project that we will create manually.

```console
$ cargo new --lib exex-remote
$ cd exex-remote
```

We will also need a bunch of dependencies. Some of them you know from the [Hello World](./hello-world.md) chapter,
but some of specific to what we need now.

```toml
[package]
name = "remote-exex"
version = "0.1.0"
edition = "2021"

[dependencies]
# reth
reth = { git = "https://github.com/paradigmxyz/reth.git" }
reth-exex = { git = "https://github.com/paradigmxyz/reth.git", features = ["serde"] }
reth-node-ethereum = { git = "https://github.com/paradigmxyz/reth.git"}
reth-tracing = { git = "https://github.com/paradigmxyz/reth.git" }

# async
tokio = { version = "1", features = ["full"] }
tokio-stream = "0.1"
futures-util = "0.3"

# grpc
tonic = "0.11"
prost = "0.12"
bincode = "1"

# misc
eyre = "0.6"

[build-dependencies]
tonic-build = "0.11"

[[bin]]
name = "exex"
path = "src/exex.rs"

[[bin]]
name = "consumer"
path = "src/consumer.rs"
```

We also added a build dependency for Tonic. We will use it to generate the Rust code for our
Protobuf definitions at compile time. Read more about using Tonic in the
[introductory tutorial](https://github.com/hyperium/tonic/blob/6a213e9485965db0628591e30577ed81cdaeaf2b/examples/helloworld-tutorial.md).

Also, we now have two separate binaries:
- `exex` is the server binary that will run the ExEx and the gRPC server.
- `consumer` is the client binary that will connect to the server and receive notifications.

### Create the Protobuf definitions

In the root directory of your project (not `src`), create a new directory called `proto` and a file called `exex.proto`.

We define a service called `RemoteExEx` that exposes a single method called `Subscribe`.
This method streams notifications to the client.

<div class="warning">

A proper way to represent the notification would be to define all fields in the schema, but it goes beyond the scope
of this chapter.

For an example of a full schema, see the [Remote ExEx](https://github.com/paradigmxyz/reth-exex-examples/blob/1f74410740ac996276a84ee72003f4f9cf041491/remote/proto/exex.proto) example.

</div>

```protobuf
syntax = "proto3";

package exex;

service RemoteExEx {
  rpc Subscribe(SubscribeRequest) returns (stream ExExNotification) {}
}

message SubscribeRequest {}

message ExExNotification {
  bytes data = 1;
}
```

To instruct Tonic to generate the Rust code using this `.proto`, add the following lines to your `lib.rs` file:
```rust,norun,noplayground,ignore
pub mod proto {
    tonic::include_proto!("exex");
}
```

## ExEx and gRPC server

We will now create the ExEx and the gRPC server in our `src/exex.rs` file.

### gRPC server

Let's create a minimal gRPC server that listens on the port `:10000`, and spawn it using
the [NodeBuilder](https://reth.rs/docs/reth/builder/struct.NodeBuilder.html)'s [task executor](https://reth.rs/docs/reth/tasks/struct.TaskExecutor.html).

```rust,norun,noplayground,ignore
use remote_exex::proto::{
    self,
    remote_ex_ex_server::{RemoteExEx, RemoteExExServer},
};
use reth_exex::ExExNotification;
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

struct ExExService {}

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
        let server = Server::builder()
            .add_service(RemoteExExServer::new(ExExService {}))
            .serve("[::1]:10000".parse().unwrap());

        let handle = builder.node(EthereumNode::default()).launch().await?;

        handle
            .node
            .task_executor
            .spawn_critical("gRPC server", async move {
                server.await.expect("failed to start gRPC server")
            });

        handle.wait_for_node_exit().await
    })
}
```

Currently, it does not send anything on the stream.
We need to create a communication channel between our future ExEx and this gRPC server
to send new `ExExNotification` on it.

Let's create this channel in the `main` function where we will have both gRPC server and ExEx initiated,
and save the sender part (that way we will be able to create new receivers) of this channel in our gRPC server.

```rust,norun,noplayground,ignore
// ...
use reth_exex::{ExExNotification};

struct ExExService {
    notifications: Arc<broadcast::Sender<ExExNotification>>,
}

...

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
            .launch()
            .await?;

        handle
            .node
            .task_executor
            .spawn_critical("gRPC server", async move {
                server.await.expect("failed to start gRPC server")
            });

        handle.wait_for_node_exit().await
    })
}
```

And with that, we're ready to handle incoming notifications, serialize them with [bincode](https://docs.rs/bincode/)
and send back to the client.

For each incoming request, we spawn a separate tokio task that will run in the background,
and then return the stream receiver to the client.

```rust,norun,noplayground,ignore
// ...

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

// ...
```

That's it for the gRPC server part! It doesn't receive anything on the `notifications` channel yet,
but we will fix it with our ExEx.

### ExEx

Now, let's define the ExEx part of our binary.

Our ExEx accepts a `notifications` channel and redirects all incoming `ExExNotification`s to it.

<div class="warning">

Don't forget to emit `ExExEvent::FinishedHeight`

</div>

```rust,norun,noplayground,ignore
// ...

use futures_util::StreamExt;
use reth_exex::{ExExContext, ExExEvent};

async fn remote_exex<Node: FullNodeComponents>(
    mut ctx: ExExContext<Node>,
    notifications: Arc<broadcast::Sender<ExExNotification>>,
) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.next().await {
        if let Some(committed_chain) = notification.committed_chain() {
            ctx.events
                .send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
        }

        info!("Notification sent to the gRPC server");
        let _ = notifications.send(notification);
    }

    Ok(())
}

// ...
```

All that's left is to connect all pieces together: install our ExEx in the node and pass the sender part
of communication channel to it.

```rust,norun,noplayground,ignore
// ...

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
            .install_exex("remote-exex", |ctx| async move {
                Ok(remote_exex(ctx, notifications))
            })
            .launch()
            .await?;

        handle
            .node
            .task_executor
            .spawn_critical("gRPC server", async move {
                server.await.expect("failed to start gRPC server")
            });

        handle.wait_for_node_exit().await
    })
}
```

### Full `exex.rs` code

<details>
<summary>Click to expand</summary>
  
```rust,norun,noplayground,ignore
use std::sync::Arc;

use futures_util::StreamExt;
use remote_exex::proto::{
    self,
    remote_ex_ex_server::{RemoteExEx, RemoteExExServer},
};
use reth::api::FullNodeComponents;
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;
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

                info!(?notification, "Notification sent to the gRPC client");
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

async fn remote_exex<Node: FullNodeComponents>(
    mut ctx: ExExContext<Node>,
    notifications: Arc<broadcast::Sender<ExExNotification>>,
) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.next().await {
        if let Some(committed_chain) = notification.committed_chain() {
            ctx.events
                .send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
        }

        info!(?notification, "Notification sent to the gRPC server");
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
            .serve("[::1]:10000".parse().unwrap());

        let handle = builder
            .node(EthereumNode::default())
            .install_exex("remote-exex", |ctx| async move {
                Ok(remote_exex(ctx, notifications))
            })
            .launch()
            .await?;

        handle
            .node
            .task_executor
            .spawn_critical("gRPC server", async move {
                server.await.expect("failed to start gRPC server")
            });

        handle.wait_for_node_exit().await
    })
}
```
</details>

## Consumer

Consumer will be a much simpler binary that just connects to our gRPC server and prints out all the notifications
it receives.

<div class="warning">

We need to increase maximum message encoding and decoding sizes to `usize::MAX`,
because notifications can get very heavy

</div>

```rust,norun,noplayground,ignore
use remote_exex::proto::{remote_ex_ex_client::RemoteExExClient, SubscribeRequest};
use reth_exex::ExExNotification;
use reth_tracing::{tracing::info, RethTracer, Tracer};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let _ = RethTracer::new().init()?;

    let mut client = RemoteExExClient::connect("http://[::1]:10000")
        .await?
        .max_encoding_message_size(usize::MAX)
        .max_decoding_message_size(usize::MAX);

    let mut stream = client.subscribe(SubscribeRequest {}).await?.into_inner();
    while let Some(notification) = stream.message().await? {
        let notification: ExExNotification = bincode::deserialize(&notification.data)?;

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
```

## Running

In one terminal window, we will run our ExEx and gRPC server. It will start syncing Reth on the Holesky chain
and use Etherscan in place of a real Consensus Client. Make sure to have `ETHERSCAN_API_KEY` on your env.

```console
export ETHERSCAN_API_KEY={YOUR_API_KEY} && cargo run --bin exex --release -- node --chain holesky --debug.etherscan
```

And in the other, we will run our consumer:

```console
cargo run --bin consumer --release
```

<img src="./assets/remote_exex.png" />
