use futures::Future;
use pin_project::pin_project;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::oneshot::{error::RecvError, Receiver};

/// Flatten a [Receiver] message in order to get rid of the [RecvError] result
#[derive(Debug)]
#[pin_project]
pub struct FlattenedResponse<T> {
    #[pin]
    receiver: Receiver<T>,
}

impl<T, E> Future for FlattenedResponse<Result<T, E>>
where
    E: From<RecvError>,
{
    type Output = Result<T, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        this.receiver.poll(cx).map(|r| match r {
            Ok(r) => r,
            Err(err) => Err(err.into()),
        })
    }
}

impl<T> From<Receiver<T>> for FlattenedResponse<T> {
    fn from(value: Receiver<T>) -> Self {
        Self { receiver: value }
    }
}
