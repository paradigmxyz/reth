//! A IPC connection.

use crate::stream_codec::StreamCodec;
use futures::{ready, Sink, Stream, StreamExt};
use std::{
    io,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_util::codec::Framed;

pub(crate) type JsonRpcStream<T> = Framed<T, StreamCodec>;

/// Wraps a stream of incoming connections.
#[pin_project::pin_project]
pub(crate) struct Incoming<T, Item> {
    #[pin]
    inner: T,
    _marker: PhantomData<Item>,
}
impl<T, Item> Incoming<T, Item>
where
    T: Stream<Item = io::Result<Item>> + Unpin + 'static,
    Item: AsyncRead + AsyncWrite,
{
    /// Create a new instance.
    pub(crate) fn new(inner: T) -> Self {
        Self { inner, _marker: Default::default() }
    }

    /// Polls to accept a new incoming connection to the endpoint.
    pub(crate) fn poll_accept(&mut self, cx: &mut Context<'_>) -> Poll<<Self as Stream>::Item> {
        let res = match ready!(self.poll_next_unpin(cx)) {
            None => Err(io::Error::new(io::ErrorKind::ConnectionAborted, "ipc connection closed")),
            Some(conn) => conn,
        };
        Poll::Ready(res)
    }
}

impl<T, Item> Stream for Incoming<T, Item>
where
    T: Stream<Item = io::Result<Item>> + 'static,
    Item: AsyncRead + AsyncWrite,
{
    type Item = io::Result<IpcConn<JsonRpcStream<Item>>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let res = match ready!(this.inner.poll_next(cx)) {
            Some(Ok(item)) => {
                let framed = IpcConn(tokio_util::codec::Decoder::framed(
                    StreamCodec::stream_incoming(),
                    item,
                ));
                Ok(framed)
            }
            Some(Err(err)) => Err(err),
            None => return Poll::Ready(None),
        };
        Poll::Ready(Some(res))
    }
}

#[pin_project::pin_project]
pub(crate) struct IpcConn<T>(#[pin] T);

impl<T> IpcConn<JsonRpcStream<T>>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    /// Create a response for when the server is busy and can't accept more requests.
    pub(crate) async fn reject_connection(self) {
        let mut parts = self.0.into_parts();
        let _ = parts.io.write_all(b"Too many connections. Please try again later.").await;
    }
}

impl<T> Stream for IpcConn<JsonRpcStream<T>>
where
    T: AsyncRead + AsyncWrite,
{
    type Item = io::Result<String>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.project().0.poll_next(cx)
    }
}

impl<T> Sink<String> for IpcConn<JsonRpcStream<T>>
where
    T: AsyncRead + AsyncWrite,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // NOTE: we always flush here this prevents buffering in the underlying
        // `Framed` impl that would cause stalled requests
        self.project().0.poll_flush(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: String) -> Result<(), Self::Error> {
        self.project().0.start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().0.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().0.poll_close(cx)
    }
}
