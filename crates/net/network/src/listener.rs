//! Contains connection-oriented interfaces.

use futures::{ready, Stream};

use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::net::{TcpListener, TcpStream};

/// A tcp connection listener.
///
/// Listens for incoming connections.
#[must_use = "Transport does nothing unless polled."]
#[pin_project::pin_project]
#[derive(Debug)]
pub struct ConnectionListener {
    /// Local address of the listener stream.
    local_address: SocketAddr,
    /// The active tcp listener for incoming connections.
    #[pin]
    incoming: TcpListenerStream,
}

impl ConnectionListener {
    /// Creates a new [`TcpListener`] that listens for incoming connections.
    pub async fn bind(addr: SocketAddr) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        Ok(Self::new(listener, local_addr))
    }

    /// Creates a new connection listener stream.
    pub(crate) fn new(listener: TcpListener, local_address: SocketAddr) -> Self {
        Self { local_address, incoming: TcpListenerStream { inner: listener } }
    }

    /// Polls the type to make progress.
    pub fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<ListenerEvent> {
        let this = self.project();
        match ready!(this.incoming.poll_next(cx)) {
            Some(Ok((stream, remote_addr))) => {
                if let Err(err) = stream.set_nodelay(true) {
                    tracing::warn!(target: "net", "set nodelay failed: {:?}", err);
                }
                Poll::Ready(ListenerEvent::Incoming { stream, remote_addr })
            }
            Some(Err(err)) => Poll::Ready(ListenerEvent::Error(err)),
            None => {
                Poll::Ready(ListenerEvent::ListenerClosed { local_address: *this.local_address })
            }
        }
    }

    /// Returns the socket address this listener listens on.
    pub fn local_address(&self) -> SocketAddr {
        self.local_address
    }
}

/// Event type produced by the [`TcpListenerStream`].
pub enum ListenerEvent {
    /// Received a new incoming.
    Incoming {
        /// Accepted connection
        stream: TcpStream,
        /// Address of the remote peer.
        remote_addr: SocketAddr,
    },
    /// Returned when the underlying connection listener has been closed.
    ///
    /// This is the case if the [`TcpListenerStream`] should ever return `None`
    ListenerClosed {
        /// Address of the closed listener.
        local_address: SocketAddr,
    },
    /// Encountered an error when accepting a connection.
    ///
    /// This is non-fatal error as the listener continues to listen for new connections to accept.
    Error(io::Error),
}

/// A stream of incoming [`TcpStream`]s.
#[derive(Debug)]
struct TcpListenerStream {
    /// listener for incoming connections.
    inner: TcpListener,
}

impl Stream for TcpListenerStream {
    type Item = io::Result<(TcpStream, SocketAddr)>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.poll_accept(cx) {
            Poll::Ready(Ok(conn)) => Poll::Ready(Some(Ok(conn))),
            Poll::Ready(Err(err)) => Poll::Ready(Some(Err(err))),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::pin_mut;
    use std::net::{Ipv4Addr, SocketAddrV4};
    use tokio::macros::support::poll_fn;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_incoming_listener() {
        let listener =
            ConnectionListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
                .await
                .unwrap();
        let local_addr = listener.local_address();

        tokio::task::spawn(async move {
            pin_mut!(listener);
            match poll_fn(|cx| listener.as_mut().poll(cx)).await {
                ListenerEvent::Incoming { .. } => {}
                _ => {
                    panic!("unexpected event")
                }
            }
        });

        let _ = TcpStream::connect(local_addr).await.unwrap();
    }
}
