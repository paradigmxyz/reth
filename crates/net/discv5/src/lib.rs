pub use discv5::{
    enr::CombinedKey, service::Service, Config as Discv5Config,
    ConfigBuilder as Discv5ConfigBuilder, Discv5, Event,
};

use futures_util::StreamExt;
use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, Stream};

pub struct Discv5Service {
    inner: ReceiverStream<Event>,
}

impl Discv5Service {
    // A constructor to create a new Discv5Service
    pub fn new(event_receiver: mpsc::Receiver<Event>) -> Self {
        Discv5Service { inner: ReceiverStream::new(event_receiver) }
    }
}

impl Stream for Discv5Service {
    type Item = Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let receiver = self.get_mut().inner.poll_next_unpin(cx);
        receiver
    }
}
