//! Support for monitoring bandwidth. Takes heavy inspiration from https://github.com/libp2p/rust-libp2p/blob/master/src/bandwidth.rs

// Copyright 2019 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use std::{
    convert::TryFrom as _,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};


/// Monitors bandwidth usage of TCP streams
pub struct BandwidthMeterInner {
    /// Measures the number of inbound packets
    inbound: AtomicU64,
    /// Measures the number of outbound packets
    outbound: AtomicU64,
}

impl BandwidthMeterInner {
    /// Returns a new [`BandwidthMonitor`].
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inbound: AtomicU64::new(0),
            outbound: AtomicU64::new(0),
        })
    }

    /// Returns the total number of bytes that have been downloaded on all the streams.
    ///
    /// > **Note**: This method is by design subject to race conditions. The returned value should
    /// >           only ever be used for statistics purposes.
    pub fn total_inbound(&self) -> u64 {
        self.inbound.load(Ordering::Relaxed)
    }

    /// Returns the total number of bytes that have been uploaded on all the streams.
    ///
    /// > **Note**: This method is by design subject to race conditions. The returned value should
    /// >           only ever be used for statistics purposes.
    pub fn total_outbound(&self) -> u64 {
        self.outbound.load(Ordering::Relaxed)
    }
}

type BandwidthMeter = Arc<BandwidthMeterInner>;

/// Wraps around a single stream that implements [`AsyncRead`] + [`AsyncWrite`] and monitors the bandwidth through it
#[pin_project::pin_project]
pub(crate) struct MeteredStream<S> {
    /// The stream this instruments
    #[pin]
    inner: S,
    /// The [`BandwidthMeter`] struct this uses to monitor bandwidth
    monitor: BandwidthMeter,
}

impl<S> MeteredStream<S> {
    fn new(inner: S) -> Self {
        Self {
            inner,
            monitor: BandwidthMeterInner::new()
        }
    }
}

impl<Stream: AsyncRead> AsyncRead for MeteredStream<Stream> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();
        let num_bytes = {
            let init_num_bytes = buf.filled().len();
            ready!(this.inner.poll_read(cx, buf))?;
            buf.filled().len() - init_num_bytes
        };
        this.monitor.inbound.fetch_add(
            u64::try_from(num_bytes).unwrap_or(u64::max_value()),
            Ordering::Relaxed,
        );
        Poll::Ready(Ok(()))
    }
}

impl<Stream: AsyncWrite> AsyncWrite for MeteredStream<Stream> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.project();
        let num_bytes = ready!(this.inner.poll_write(cx, buf))?;
        this.monitor.outbound.fetch_add(
            u64::try_from(num_bytes).unwrap_or(u64::max_value()),
            Ordering::Relaxed,
        );
        Poll::Ready(Ok(num_bytes))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.project();
        this.inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.project();
        this.inner.poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{duplex, AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream}
    };
    
    #[tokio::test]
    async fn test_count_read_write() {
        // Taken in large part from https://docs.rs/tokio/latest/tokio/io/struct.DuplexStream.html#example

        let (client, server) = duplex(64);
        let mut monitored_client = MeteredStream::new(client);
        let mut monitored_server = MeteredStream::new(server);

        monitored_client.write_all(b"ping").await.unwrap();
        // Assert that the client stream wrote 4 bytes
        let client_outbound = monitored_client.monitor.total_outbound();
        assert_eq!(client_outbound, 4, "Expected client to write 4 bytes, but it wrote {}", client_outbound);

        let mut buf = [0u8; 4];
        monitored_server.read(&mut buf).await.unwrap();
        // Assert that the server stream read 4 bytes
        let server_inbound = monitored_server.monitor.total_inbound();
        assert_eq!(server_inbound, 4, "Expected server to read 4 bytes, but it read {}", server_inbound);

        monitored_server.write_all(b"pong").await.unwrap();
        // Assert that the server stream wrote 4 bytes
        let server_outbound = monitored_server.monitor.total_outbound();
        assert_eq!(server_outbound, 4, "Expected server to write 4 bytes, but it wrote {}", server_outbound);

        monitored_client.read(&mut buf).await.unwrap();
        // Assert that the client stream read 4 bytes
        let client_inbound = monitored_client.monitor.total_inbound();
        assert_eq!(client_inbound, 4, "Expected client to read 4 bytes, but it read {}", client_inbound);
    }

    #[tokio::test]
    async fn test_read_equals_write_tcp() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();

        let client_stream = TcpStream::connect(server_addr).await.unwrap();
        let mut metered_client_stream = MeteredStream::new(client_stream);

        let client_meter = metered_client_stream.monitor.clone();

        let handle = tokio::spawn(async move {
            let (server_stream, _) = listener.accept().await.unwrap();
            let mut metered_server_stream = MeteredStream::new(server_stream);

            let mut buf = [0u8; 4];

            metered_server_stream.read(&mut buf).await.unwrap();

            assert_eq!(metered_server_stream.monitor.total_inbound(), client_meter.total_outbound());
        });

        metered_client_stream.write_all(b"ping").await.unwrap();

        handle.await.unwrap();
    }

}