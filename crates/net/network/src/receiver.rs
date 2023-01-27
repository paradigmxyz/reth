use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures::Future;
use tokio::sync::oneshot::{error::RecvError, Receiver};

pub struct ResolvedOneshotReceiver<T> {
    receiver: Receiver<T>,
}

impl<T, E> Future for ResolvedOneshotReceiver<Result<T, E>>
where
    E: From<RecvError>,
{
    type Output = Result<T, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = self.get_mut();
        Pin::new(&mut inner.receiver).poll(cx).map(|r| match r {
            Ok(r) => r,
            Err(err) => Err(err.into()),
        })
    }
}

impl<T> From<Receiver<T>> for ResolvedOneshotReceiver<T> {
    fn from(value: Receiver<T>) -> Self {
        Self { receiver: value }
    }
}
