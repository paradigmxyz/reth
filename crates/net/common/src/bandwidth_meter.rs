//! Support for metering bandwidth.
//!
//! Takes heavy inspiration from <https://github.com/libp2p/rust-libp2p/blob/master/src/bandwidth.rs>

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

use crate::stream::HasRemoteAddr;
use std::{
    convert::TryFrom as _,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};

/// Meters bandwidth usage of streams
#[derive(Debug)]
struct BandwidthMeterInner {
    /// Measures the number of inbound packets
    inbound: AtomicU64,
    /// Measures the number of outbound packets
    outbound: AtomicU64,
}

/// Public shareable struct used for getting bandwidth metering info
#[derive(Clone, Debug)]
pub struct BandwidthMeter {
    inner: Arc<BandwidthMeterInner>,
}

impl BandwidthMeter {
    /// Returns the total number of bytes that have been downloaded on all the streams.
    ///
    /// > **Note**: This method is by design subject to race conditions. The returned value should
    /// > only ever be used for statistics purposes.
    pub fn total_inbound(&self) -> u64 {
        self.inner.inbound.load(Ordering::Relaxed)
    }

    /// Returns the total number of bytes that have been uploaded on all the streams.
    ///
    /// > **Note**: This method is by design subject to race conditions. The returned value should
    /// > only ever be used for statistics purposes.
    pub fn total_outbound(&self) -> u64 {
        self.inner.outbound.load(Ordering::Relaxed)
    }
}

impl Default for BandwidthMeter {
    fn default() -> Self {
        Self {
            inner: Arc::new(BandwidthMeterInner {
                inbound: AtomicU64::new(0),
                outbound: AtomicU64::new(0),
            }),
        }
    }
}

/// Wraps around a single stream that implements [`AsyncRead`] + [`AsyncWrite`] and meters the
/// bandwidth through it
#[derive(Debug)]
#[pin_project::pin_project]
pub struct MeteredStream<S> {
    /// The stream this instruments
    #[pin]
    inner: S,
    /// The [`BandwidthMeter`] struct this uses to meter bandwidth
    meter: BandwidthMeter,
}

impl<S> MeteredStream<S> {
    /// Creates a new [`MeteredStream`] wrapping around the provided stream,
    /// along with a new [`BandwidthMeter`]
    pub fn new(inner: S) -> Self {
        Self { inner, meter: BandwidthMeter::default() }
    }

    /// Creates a new [`MeteredStream`] wrapping around the provided stream,
    /// attaching the provided [`BandwidthMeter`]
    pub fn new_with_meter(inner: S, meter: BandwidthMeter) -> Self {
        Self { inner, meter }
    }

    /// Provides a reference to the [`BandwidthMeter`] attached to this [`MeteredStream`]
    pub fn get_bandwidth_meter(&self) -> &BandwidthMeter {
        &self.meter
    }

    /// Returns the wrapped stream
    pub fn inner(&self) -> &S {
        &self.inner
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
        this.meter
            .inner
            .inbound
            .fetch_add(u64::try_from(num_bytes).unwrap_or(u64::MAX), Ordering::Relaxed);
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
        this.meter
            .inner
            .outbound
            .fetch_add(u64::try_from(num_bytes).unwrap_or(u64::MAX), Ordering::Relaxed);
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

impl HasRemoteAddr for MeteredStream<TcpStream> {
    fn remote_addr(&self) -> Option<SocketAddr> {
        self.inner.remote_addr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream},
        net::TcpListener,
    };

    async fn duplex_stream_ping_pong(
        client: &mut MeteredStream<DuplexStream>,
        server: &mut MeteredStream<DuplexStream>,
    ) {
        let mut buf = [0u8; 4];

        client.write_all(b"ping").await.unwrap();
        server.read_exact(&mut buf).await.unwrap();

        server.write_all(b"pong").await.unwrap();
        client.read_exact(&mut buf).await.unwrap();
    }

    fn assert_bandwidth_counts(
        bandwidth_meter: &BandwidthMeter,
        expected_inbound: u64,
        expected_outbound: u64,
    ) {
        let actual_inbound = bandwidth_meter.total_inbound();
        assert_eq!(
            actual_inbound, expected_inbound,
            "Expected {expected_inbound} inbound bytes, but got {actual_inbound}",
        );

        let actual_outbound = bandwidth_meter.total_outbound();
        assert_eq!(
            actual_outbound, expected_outbound,
            "Expected {expected_outbound} inbound bytes, but got {actual_outbound}",
        );
    }

    #[tokio::test]
    async fn test_count_read_write() {
        // Taken in large part from https://docs.rs/tokio/latest/tokio/io/struct.DuplexStream.html#example

        let (client, server) = duplex(64);
        let mut metered_client = MeteredStream::new(client);
        let mut metered_server = MeteredStream::new(server);

        duplex_stream_ping_pong(&mut metered_client, &mut metered_server).await;

        assert_bandwidth_counts(metered_client.get_bandwidth_meter(), 4, 4);
        assert_bandwidth_counts(metered_server.get_bandwidth_meter(), 4, 4);
    }

    #[tokio::test]
    async fn test_read_equals_write_tcp() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();

        let client_stream = TcpStream::connect(server_addr).await.unwrap();
        let mut metered_client_stream = MeteredStream::new(client_stream);

        let client_meter = metered_client_stream.meter.clone();

        let handle = tokio::spawn(async move {
            let (server_stream, _) = listener.accept().await.unwrap();
            let mut metered_server_stream = MeteredStream::new(server_stream);

            let mut buf = [0u8; 4];

            metered_server_stream.read_exact(&mut buf).await.unwrap();

            assert_eq!(metered_server_stream.meter.total_inbound(), client_meter.total_outbound());
        });

        metered_client_stream.write_all(b"ping").await.unwrap();

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_multiple_streams_one_meter() {
        let (client_1, server_1) = duplex(64);
        let (client_2, server_2) = duplex(64);

        let shared_client_bandwidth_meter = BandwidthMeter::default();
        let shared_server_bandwidth_meter = BandwidthMeter::default();

        let mut metered_client_1 =
            MeteredStream::new_with_meter(client_1, shared_client_bandwidth_meter.clone());
        let mut metered_server_1 =
            MeteredStream::new_with_meter(server_1, shared_server_bandwidth_meter.clone());

        let mut metered_client_2 =
            MeteredStream::new_with_meter(client_2, shared_client_bandwidth_meter.clone());
        let mut metered_server_2 =
            MeteredStream::new_with_meter(server_2, shared_server_bandwidth_meter.clone());

        duplex_stream_ping_pong(&mut metered_client_1, &mut metered_server_1).await;
        duplex_stream_ping_pong(&mut metered_client_2, &mut metered_server_2).await;

        assert_bandwidth_counts(&shared_client_bandwidth_meter, 8, 8);
        assert_bandwidth_counts(&shared_server_bandwidth_meter, 8, 8);
    }
}
