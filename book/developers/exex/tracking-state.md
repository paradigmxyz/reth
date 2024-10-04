# Tracking State

In this chapter, we'll learn how to keep track of some state inside our ExEx.

Let's continue with our Hello World example from the [previous chapter](./hello-world.md).

### Turning ExEx into a struct

First, we need to turn our ExEx into a stateful struct.

Before, we had just an async function, but now we'll need to implement
the [`Future`](https://doc.rust-lang.org/std/future/trait.Future.html) trait manually.

<div class="warning">

Having a stateful async function is also possible, but it makes testing harder,
because you can't access variables inside the function to assert the state of your ExEx.

</div>

```rust,norun,noplayground,ignore
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures_util::StreamExt;
use reth::api::FullNodeComponents;
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;

struct MyExEx<Node: FullNodeComponents> {
    ctx: ExExContext<Node>,
}

impl<Node: FullNodeComponents> Future for MyExEx<Node> {
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        while let Some(notification) = ready!(this.ctx.notifications.poll_next_unpin(cx)) {
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
                this.ctx
                    .events
                    .send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
            }
        }

        Poll::Ready(Ok(()))
    }
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("my-exex", |ctx| async move { Ok(MyExEx { ctx }) })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
```

For those who are not familiar with how async Rust works on a lower level, that may seem scary,
but let's unpack what's going on here:

1. Our ExEx is now a `struct` that contains the context and implements the `Future` trait. It's now pollable (hence `await`-able).
1. We can't use `self` directly inside our `poll` method, and instead need to acquire a mutable reference to the data inside of the `Pin`.
   Read more about pinning in [the book](https://rust-lang.github.io/async-book/04_pinning/01_chapter.html).
1. We also can't use `await` directly inside `poll`, and instead need to poll futures manually.
   We wrap the call to `poll_recv(cx)` into a [`ready!`](https://doc.rust-lang.org/std/task/macro.ready.html) macro,
   so that if the channel of notifications has no value ready, we will instantly return `Poll::Pending` from our Future.
1. We initialize and return the `MyExEx` struct directly in the `install_exex` method, because it's a Future.

With all that done, we're now free to add more fields to our `MyExEx` struct, and track some state in them.

### Adding state

Our ExEx will count the number of transactions in each block and log it to the console.

```rust,norun,noplayground,ignore
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures_util::StreamExt;
use reth::{api::FullNodeComponents, primitives::BlockNumber};
use reth_exex::{ExExContext, ExExEvent};
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;

struct MyExEx<Node: FullNodeComponents> {
    ctx: ExExContext<Node>,
    /// First block that was committed since the start of the ExEx.
    first_block: Option<BlockNumber>,
    /// Total number of transactions committed.
    transactions: u64,
}

impl<Node: FullNodeComponents> MyExEx<Node> {
    fn new(ctx: ExExContext<Node>) -> Self {
        Self {
            ctx,
            first_block: None,
            transactions: 0,
        }
    }
}

impl<Node: FullNodeComponents> Future for MyExEx<Node> {
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        while let Some(notification) = ready!(this.ctx.notifications.poll_next_unpin(cx)) {
            if let Some(reverted_chain) = notification.reverted_chain() {
                this.transactions = this.transactions.saturating_sub(
                    reverted_chain
                        .blocks_iter()
                        .map(|b| b.body.len() as u64)
                        .sum(),
                );
            }

            if let Some(committed_chain) = notification.committed_chain() {
                this.first_block.get_or_insert(committed_chain.first().number);

                this.transactions += committed_chain
                    .blocks_iter()
                    .map(|b| b.body.len() as u64)
                    .sum::<u64>();

                this.ctx
                    .events
                    .send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
            }

            if let Some(first_block) = this.first_block {
                info!(%first_block, transactions = %this.transactions, "Total number of transactions");
            }
        }

        Poll::Ready(Ok(()))
    }
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("my-exex", |ctx| async move { Ok(MyExEx::new(ctx)) })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
```

As you can see, we added two fields to our ExEx struct:
- `first_block` to keep track of the first block that was committed since the start of the ExEx.
- `transactions` to keep track of the total number of transactions committed, accounting for reorgs and reverts.

We also changed our `match` block to two `if` clauses:
- First one checks if there's a reverted chain using `notification.reverted_chain()`. If there is:
    - We subtract the number of transactions in the reverted chain from the total number of transactions.
    - It's important to do the `saturating_sub` here, because if we just started our node and
      instantly received a reorg, our `transactions` field will still be zero.
- Second one checks if there's a committed chain using `notification.committed_chain()`. If there is:
    - We update the `first_block` field to the first block of the committed chain.
    - We add the number of transactions in the committed chain to the total number of transactions.
    - We send a `FinishedHeight` event back to the main node.

Finally, on every notification, we log the total number of transactions and
the first block that was committed since the start of the ExEx.
