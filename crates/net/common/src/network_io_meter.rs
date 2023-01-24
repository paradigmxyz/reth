//! Support for metering network IO. Takes heavy inspiration from https://github.com/libp2p/rust-libp2p/blob/master/src/bandwidth.rs

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
use metrics::Counter;
use reth_metrics_derive::Metrics;
use std::{
    convert::TryFrom as _,
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    time::{interval, Interval},
};

/// Meters network IO usage of streams
#[derive(Debug)]
struct NetworkIOMeterInner {
    /// Measures the number of bytes received
    ingress: AtomicU64,
    /// Number of bytes received during throughput recording period
    ingress_delta: AtomicU64,
    /// Last measured throughput of bytes received in terms of bytes/s.
    /// Stored as an [`AtomicU64`] but interpreted as an [`f64`] using `f64::from_bits`
    ingress_throughput: AtomicU64,
    /// Measures the number bytes sent
    egress: AtomicU64,
    /// Number of bytes sent during throughput recording period
    egress_delta: AtomicU64,
    /// Last measured throughput of bytes sent in terms of bytes/s.
    /// Stored as an [`AtomicU64`] but interpreted as an [`f64`] using `f64::from_bits`
    egress_throughput: AtomicU64,
    /// Duration of interval over which throughput is evaluated
    throughput_period: Duration,
}

/// Public shareable struct used for getting network IO info
#[derive(Clone, Debug)]
pub struct NetworkIOMeter {
    inner: Arc<NetworkIOMeterInner>,
}

impl NetworkIOMeter {
    /// Returns the total number of bytes that have been downloaded on all the streams.
    ///
    /// > **Note**: This method is by design subject to race conditions. The returned value should
    /// > only ever be used for statistics purposes.
    pub fn total_ingress(&self) -> u64 {
        self.inner.ingress.load(Ordering::Relaxed)
    }

    /// Returns the total number of bytes that have been uploaded on all the streams.
    ///
    /// > **Note**: This method is by design subject to race conditions. The returned value should
    /// > only ever be used for statistics purposes.
    pub fn total_egress(&self) -> u64 {
        self.inner.egress.load(Ordering::Relaxed)
    }

    pub fn ingress_throughput(&self) -> f64 {
        f64::from_bits(self.inner.ingress_throughput.load(Ordering::Relaxed))
    }

    pub fn egress_throughput(&self) -> f64 {
        f64::from_bits(self.inner.egress_throughput.load(Ordering::Relaxed))
    }
}

impl Default for NetworkIOMeter {
    fn default() -> Self {
        let network_io_meter = Self {
            inner: Arc::new(NetworkIOMeterInner {
                ingress: AtomicU64::new(0),
                ingress_delta: AtomicU64::new(0),
                ingress_throughput: AtomicU64::new(0),
                egress: AtomicU64::new(0),
                egress_delta: AtomicU64::new(0),
                egress_throughput: AtomicU64::new(0),
                throughput_period: Duration::from_secs(1),
            }),
        };

        tokio::spawn(ThroughputTask::new(
            network_io_meter.inner.throughput_period,
            network_io_meter.clone(),
        ));

        network_io_meter
    }
}

/// An endless Future used to calculate ingress & egress throughputs
/// for the given [`NetworkIOMeter`] on the given [`Interval`].
struct ThroughputTask {
    interval: Interval,
    meter: NetworkIOMeter,
}

impl ThroughputTask {
    fn new(period: Duration, meter: NetworkIOMeter) -> Self {
        Self { interval: interval(period), meter }
    }
}

impl Future for ThroughputTask {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        if this.interval.poll_tick(cx).is_ready() {
            let ingress_delta = this.meter.inner.ingress_delta.swap(0, Ordering::Relaxed);
            let egress_delta = this.meter.inner.egress_delta.swap(0, Ordering::Relaxed);

            this.meter.inner.ingress_throughput.store(
                f64::to_bits((ingress_delta as f64) / this.interval.period().as_secs_f64()),
                Ordering::Relaxed,
            );

            this.meter.inner.egress_throughput.store(
                f64::to_bits((egress_delta as f64) / this.interval.period().as_secs_f64()),
                Ordering::Relaxed,
            );
        }

        Poll::Pending
    }
}

/// Exposes metrics for the ingress and egress on a given
/// network IO meter
#[derive(Metrics)]
#[metrics(dynamic = true)]
struct NetworkIOMeterMetricsInner {
    /// Counts inbound bytes
    ingress_bytes: Counter,
    /// Counts outbound bytes
    egress_bytes: Counter,
}

/// Public shareable struct used for exposing network IO metrics
#[derive(Debug)]
pub struct NetworkIOMeterMetrics {
    inner: Arc<NetworkIOMeterMetricsInner>,
}

impl NetworkIOMeterMetrics {
    /// Creates an instance of [`NetworkIOMeterMetrics`] with the given scope
    pub fn new(scope: &str, labels: impl metrics::IntoLabels + Clone) -> Self {
        Self { inner: Arc::new(NetworkIOMeterMetricsInner::new_with_labels(scope, labels)) }
    }
}

/// Wraps around a single stream that implements [`AsyncRead`] + [`AsyncWrite`] and meters the
/// network IO through it
#[derive(Debug)]
#[pin_project::pin_project]
pub struct MeteredStream<S> {
    /// The stream this instruments
    #[pin]
    inner: S,
    /// The [`NetworkIOMeter`] struct this uses to meter network IO
    meter: NetworkIOMeter,
    /// An optional [`NetworkIOMeterMetrics`] struct expose metrics over the
    /// [`NetworkIOMeter`].
    metrics: Option<NetworkIOMeterMetrics>,
}

impl<S> MeteredStream<S> {
    /// Creates a new [`MeteredStream`] wrapping around the provided stream,
    /// along with a new [`NetworkIOMeter`]
    pub fn new(inner: S) -> Self {
        Self { inner, meter: NetworkIOMeter::default(), metrics: None }
    }

    /// Attaches the provided [`NetworkIOMeter`]
    pub fn set_meter(&mut self, meter: NetworkIOMeter) {
        self.meter = meter;
    }

    /// Provides a reference to the [`NetworkIOMeter`] attached to this [`MeteredStream`]
    pub fn get_network_io_meter(&self) -> &NetworkIOMeter {
        &self.meter
    }
}

impl<Stream: AsyncRead> AsyncRead for MeteredStream<Stream> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();
        let num_bytes_u64 = {
            let init_num_bytes = buf.filled().len();
            ready!(this.inner.poll_read(cx, buf))?;
            u64::try_from(buf.filled().len() - init_num_bytes).unwrap_or(u64::max_value())
        };
        let current_ingress =
            this.meter.inner.ingress.fetch_add(num_bytes_u64, Ordering::Relaxed) + num_bytes_u64;

        this.meter.inner.ingress_delta.fetch_add(num_bytes_u64, Ordering::Relaxed);

        if let Some(network_io_meter_metrics) = &this.metrics {
            network_io_meter_metrics.inner.ingress_bytes.absolute(current_ingress);
        }

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
        let num_bytes_u64 = { u64::try_from(num_bytes).unwrap_or(u64::max_value()) };
        let current_egress =
            this.meter.inner.egress.fetch_add(num_bytes_u64, Ordering::Relaxed) + num_bytes_u64;

        this.meter.inner.egress_delta.fetch_add(num_bytes_u64, Ordering::Relaxed);

        if let Some(network_io_meter_metrics) = &this.metrics {
            network_io_meter_metrics.inner.egress_bytes.absolute(current_egress);
        }

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

/// Allows exporting metrics for a type that contains a nested [`MeteredStream`]
pub trait MeterableStream {
    /// Attaches the provided [`NetworkIOMeterMetrics`]
    fn expose_metrics(&mut self, metrics: NetworkIOMeterMetrics);
}

impl<S> MeterableStream for MeteredStream<S> {
    fn expose_metrics(&mut self, metrics: NetworkIOMeterMetrics) {
        self.metrics = Some(metrics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream},
        net::{TcpListener, TcpStream},
    };

    fn create_metered_duplex<'a>(
        client_meter: NetworkIOMeter,
        server_meter: NetworkIOMeter,
    ) -> (MeteredStream<DuplexStream>, MeteredStream<DuplexStream>) {
        let (client, server) = duplex(64);
        let (mut metered_client, mut metered_server) =
            (MeteredStream::new(client), MeteredStream::new(server));

        metered_client.set_meter(client_meter);
        metered_server.set_meter(server_meter);

        (metered_client, metered_server)
    }

    async fn duplex_stream_ping_pong(
        client: &mut MeteredStream<DuplexStream>,
        server: &mut MeteredStream<DuplexStream>,
    ) {
        let mut buf = [0u8; 4];

        client.write_all(b"ping").await.unwrap();
        server.read(&mut buf).await.unwrap();

        server.write_all(b"pong").await.unwrap();
        client.read(&mut buf).await.unwrap();
    }

    fn assert_io_counts(
        network_io_meter: &NetworkIOMeter,
        expected_ingress: u64,
        expected_egress: u64,
    ) {
        let actual_ingress = network_io_meter.total_ingress();
        assert_eq!(
            actual_ingress, expected_ingress,
            "Expected {expected_ingress} inbound bytes, but got {actual_ingress}",
        );

        let actual_egress = network_io_meter.total_egress();
        assert_eq!(
            actual_egress, expected_egress,
            "Expected {expected_egress} inbound bytes, but got {actual_egress}",
        );
    }

    #[tokio::test]
    async fn test_count_read_write() {
        // Taken in large part from https://docs.rs/tokio/latest/tokio/io/struct.DuplexStream.html#example

        let (mut metered_client, mut metered_server) =
            create_metered_duplex(NetworkIOMeter::default(), NetworkIOMeter::default());

        duplex_stream_ping_pong(&mut metered_client, &mut metered_server).await;

        assert_io_counts(metered_client.get_network_io_meter(), 4, 4);
        assert_io_counts(metered_server.get_network_io_meter(), 4, 4);
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

            metered_server_stream.read(&mut buf).await.unwrap();

            assert_eq!(metered_server_stream.meter.total_ingress(), client_meter.total_egress());
        });

        metered_client_stream.write_all(b"ping").await.unwrap();

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_multiple_streams_one_meter() {
        let shared_client_network_io_meter = NetworkIOMeter::default();
        let shared_server_network_io_meter = NetworkIOMeter::default();

        let (mut metered_client_1, mut metered_server_1) = create_metered_duplex(
            shared_client_network_io_meter.clone(),
            shared_server_network_io_meter.clone(),
        );
        let (mut metered_client_2, mut metered_server_2) = create_metered_duplex(
            shared_client_network_io_meter.clone(),
            shared_server_network_io_meter.clone(),
        );

        duplex_stream_ping_pong(&mut metered_client_1, &mut metered_server_1).await;
        duplex_stream_ping_pong(&mut metered_client_2, &mut metered_server_2).await;

        assert_io_counts(&shared_client_network_io_meter, 8, 8);
        assert_io_counts(&shared_server_network_io_meter, 8, 8);
    }

    struct ThroughputStatsTask<'a> {
        interval: Interval,
        meter: &'a NetworkIOMeter,
        max_throughputs: (f64, f64),
    }

    impl<'a> ThroughputStatsTask<'a> {
        pub fn new(
            interval: Interval,
            meter: &'a NetworkIOMeter,
        ) -> Self {
            Self {
                interval,
                meter,
                max_throughputs: (0.0, 0.0)
            }
        }
    }

    impl<'a> Future for ThroughputStatsTask<'a> {
        type Output = (f64, f64);

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();

            let ingress_throughput = this.meter.ingress_throughput();
            let egress_throughput = this.meter.egress_throughput();

            this.max_throughputs = (
                if ingress_throughput > this.max_throughputs.0 { ingress_throughput } else { this.max_throughputs.0 },
                if egress_throughput > this.max_throughputs.1 { egress_throughput } else { this.max_throughputs.1 },
            );

            if this.interval.poll_tick(cx).is_ready() {
                return Poll::Ready(this.max_throughputs)
            }

            Poll::Pending
        }
    }

    #[tokio::test]
    async fn test_throughput_basic() {
        let (mut metered_client, mut _metered_server) =
            create_metered_duplex(NetworkIOMeter::default(), NetworkIOMeter::default());

        let network_io_meter = metered_client.get_network_io_meter().clone();

        // Concurrently collect throughput stats & write out from stream.
        // Want to be sure that stats task starts first, but can't use `spawn` due to `'static` lifetime constraint...
        let ((_, max_egress_throughput), _) = tokio::join!(
            async {
                let mut one_s_interval = interval(Duration::from_secs(1));
                // Resolves immediately, want this to actually poll for 1s
                one_s_interval.tick().await;

                ThroughputStatsTask::new(
                    one_s_interval,
                    &network_io_meter,
                ).await
            },
            async {
                metered_client.write_all(b"ping").await.unwrap()
            }
        );

        assert_eq!(max_egress_throughput, 4.0);
    }
}
