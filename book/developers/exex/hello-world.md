# Hello World

Let's write a simple "Hello World" ExEx that emits a log every time a new chain of blocks is committed, reverted, or reorged.

### Create a project

First, let's create a new project for our ExEx

```console
cargo new --bin my-exex
cd my-exex
```

And add Reth as a dependency in `Cargo.toml`

```toml
[package]
name = "my-exex"
version = "0.1.0"
edition = "2021"

[dependencies]
reth = { git = "https://github.com/paradigmxyz/reth.git" } # Reth
reth-exex = { git = "https://github.com/paradigmxyz/reth.git" } # Execution Extensions
reth-node-ethereum = { git = "https://github.com/paradigmxyz/reth.git" } # Ethereum Node implementation
reth-tracing = { git = "https://github.com/paradigmxyz/reth.git" } # Logging

eyre = "0.6" # Easy error handling
futures-util = "0.3" # Stream utilities for consuming notifications
```

### Default Reth node

Now, let's jump to our `main.rs` and start by initializing and launching a default Reth node

```rust,norun,noplayground,ignore
use reth_node_ethereum::EthereumNode;

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder.node(EthereumNode::default()).launch().await?;

        handle.wait_for_node_exit().await
    })
}
```

You can already test that it works by running the binary and initializing the Holesky node in a custom datadir
(to not interfere with any instances of Reth you already have on your machine):

```console
$ cargo run -- init --chain holesky --datadir data

2024-06-12T16:48:06.420296Z  INFO reth init starting
2024-06-12T16:48:06.422380Z  INFO Opening storage db_path="data/db" sf_path="data/static_files"
2024-06-12T16:48:06.432939Z  INFO Verifying storage consistency.
2024-06-12T16:48:06.577673Z  INFO Genesis block written hash=0xb5f7f912443c940f21fd611f12828d75b53
4364ed9e95ca4e307729a4661bde4
```

### Simplest ExEx

The simplest ExEx is just an async function that never returns. We need to install it into our node

```rust,norun,noplayground,ignore
use reth::api::FullNodeComponents;
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;

async fn my_exex<Node: FullNodeComponents>(mut _ctx: ExExContext<Node>) -> eyre::Result<()> {
    loop {}
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("my-exex", |ctx| async move { Ok(my_exex(ctx)) })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
```

See that unused `_ctx`? That's the context that we'll use to listen to new notifications coming from the main node,
and send events back to it. It also contains all components that the node exposes to the ExEx.

Currently, our ExEx does absolutely nothing by running an infinite loop in an async function that never returns.

<div class="warning">

It's important that the future returned by the ExEx (`my_exex`) never resolves.

If you try running a node with an ExEx that exits, the node will exit as well.

</div>

### Hello World ExEx

Now, let's extend our simplest ExEx and start actually listening to new notifications, log them, and send events back to the main node

```rust,norun,noplayground,ignore
use futures_util::StreamExt;
use reth::api::FullNodeComponents;
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;

async fn my_exex<Node: FullNodeComponents>(mut ctx: ExExContext<Node>) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.next().await {
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
            ctx.events
                .send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
        }
    }

    Ok(())
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("my-exex", |ctx| async move { Ok(my_exex(ctx)) })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
```

Woah, there's a lot of new stuff here! Let's go through it step by step:

- First, we've added a `while let Some(notification) = ctx.notifications.recv().await` loop that waits for new notifications to come in.
   - The main node is responsible for sending notifications to the ExEx, so we're waiting for them to come in.
- Next, we've added a `match &notification { ... }` block that matches on the type of the notification.
   - In each case, we're logging the notification and the corresponding block range, be it a chain commit, revert, or reorg.
- Finally, we're checking if the notification contains a committed chain, and if it does, we're sending a `ExExEvent::FinishedHeight` event back to the main node using the `ctx.events.send` method.

<div class="warning">

Sending an `ExExEvent::FinishedHeight` event is a very important part of every ExEx.

It's the only way to communicate to the main node that the ExEx has finished processing the specified height
and it's safe to prune the associated data.

</div>

What we've arrived at is the [minimal ExEx example](https://github.com/paradigmxyz/reth-exex-examples/blob/4f3498f0cc00e038d6d8c32cd94fe82788862f49/minimal/src/main.rs) that we provide in the [reth-exex-examples](https://github.com/paradigmxyz/reth-exex-examples) repository.

## What's next?

Let's do something a bit more interesting, and see how you can [keep track of some state](./tracking-state.md) inside your ExEx.
