use eyre::Result;
use futures::{Future, FutureExt};
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_tracing::tracing::info;
use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};
use tracing::error;

use crate::network::DiscV5ExEx;

/// The ExEx struct, representing the initialization and execution of the ExEx.
pub struct ExEx<Node: FullNodeComponents> {
    exex: ExExContext<Node>,
    disc_v5: DiscV5ExEx,
}

impl<Node: FullNodeComponents> ExEx<Node> {
    pub fn new(exex: ExExContext<Node>, disc_v5: DiscV5ExEx) -> Self {
        Self { exex, disc_v5 }
    }
}

impl<Node: FullNodeComponents> Future for ExEx<Node> {
    type Output = Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Poll the Discv5 future until its drained
        loop {
            match self.disc_v5.poll_unpin(cx) {
                Poll::Ready(Ok(())) => {
                    info!("Discv5 task completed successfully");
                }
                Poll::Ready(Err(e)) => {
                    error!(?e, "Discv5 task encountered an error");
                    return Poll::Ready(Err(e));
                }
                Poll::Pending => {
                    // Exit match and continue to poll notifications
                    break;
                }
            }
        }

        // Continuously poll the ExExContext notifications
        loop {
            if let Some(notification) = ready!(self.exex.notifications.poll_recv(cx)) {
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
                }

                if let Some(committed_chain) = notification.committed_chain() {
                    self.exex
                        .events
                        .send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
                }
            }
        }
    }
}
