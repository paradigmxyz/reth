//! A utility Future to asynchronously wait until a node has finished syncing.

use futures::Stream;
use pin_project::pin_project;
use reth_network_api::NetworkInfo;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};

/// This future resolves once the node is no longer syncing: [`NetworkInfo::is_syncing`].
#[must_use = "futures do nothing unless polled"]
#[pin_project]
#[derive(Debug)]
pub struct SyncListener<N, St> {
    #[pin]
    tick: St,
    network_info: N,
}

impl<N, St> SyncListener<N, St> {
    /// Create a new [`SyncListener`] using the given tick stream.
    pub const fn new(network_info: N, tick: St) -> Self {
        Self { tick, network_info }
    }
}

impl<N, St, Out> Future for SyncListener<N, St>
where
    N: NetworkInfo,
    St: Stream<Item = Out> + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        if !this.network_info.is_syncing() {
            return Poll::Ready(());
        }

        loop {
            let tick_event = ready!(this.tick.as_mut().poll_next(cx));

            match tick_event {
                Some(_) => {
                    if !this.network_info.is_syncing() {
                        return Poll::Ready(());
                    }
                }
                None => return Poll::Ready(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_rpc_types_admin::EthProtocolInfo;
    use futures::stream;
    use reth_network_api::{NetworkError, NetworkStatus};
    use std::{
        net::{IpAddr, SocketAddr},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };

    #[derive(Clone)]
    struct TestNetwork {
        syncing: Arc<AtomicBool>,
    }

    impl NetworkInfo for TestNetwork {
        fn local_addr(&self) -> SocketAddr {
            (IpAddr::from([0, 0, 0, 0]), 0).into()
        }

        async fn network_status(&self) -> Result<NetworkStatus, NetworkError> {
            #[allow(deprecated)]
            Ok(NetworkStatus {
                client_version: "test".to_string(),
                protocol_version: 5,
                eth_protocol_info: EthProtocolInfo {
                    network: 1,
                    difficulty: None,
                    genesis: Default::default(),
                    config: Default::default(),
                    head: Default::default(),
                },
                capabilities: vec![],
            })
        }

        fn chain_id(&self) -> u64 {
            1
        }

        fn is_syncing(&self) -> bool {
            self.syncing.load(Ordering::SeqCst)
        }

        fn is_initially_syncing(&self) -> bool {
            self.is_syncing()
        }
    }

    #[tokio::test]
    async fn completes_immediately_if_not_syncing() {
        let network = TestNetwork { syncing: Arc::new(AtomicBool::new(false)) };
        let fut = SyncListener::new(network, stream::pending::<()>());
        fut.await;
    }

    #[tokio::test]
    async fn resolves_when_syncing_stops() {
        use tokio::sync::mpsc::unbounded_channel;
        use tokio_stream::wrappers::UnboundedReceiverStream;

        let syncing = Arc::new(AtomicBool::new(true));
        let network = TestNetwork { syncing: syncing.clone() };
        let (tx, rx) = unbounded_channel();
        let listener = SyncListener::new(network, UnboundedReceiverStream::new(rx));
        let handle = tokio::spawn(listener);

        syncing.store(false, Ordering::Relaxed);
        let _ = tx.send(());

        handle.await.unwrap();
    }
}
