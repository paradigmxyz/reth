//! A IPC connection.

use futures::{ready, Sink, Stream};
use jsonrpsee::types::Request;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

#[pin_project::pin_project]
pub(crate) struct IpcConn<T>(#[pin] T);

impl<T> Stream for IpcConn<T>
where
    T: Stream<Item = io::Result<String>>,
{
    type Item = Result<Option<Request<'static>>, ()>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        todo!()
        // fn on_request(msg: io::Result<String>) -> Result<Option<Request<'static>>,
        // serde_json::error::Error> {     let text = msg?;
        //     Ok(Some(serde_json::from_str(&text)?))
        // }
        // match ready!(self.project().0.poll_next(cx)) {
        //     Some(req) => Poll::Ready(Some(on_request(req))),
        //     _ => Poll::Ready(None),
        // }
    }
}

impl<T> Sink<String> for IpcConn<T>
where
    T: Sink<String, Error = io::Error>,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // NOTE: we always flush here this prevents any backpressure buffer in the underlying
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
