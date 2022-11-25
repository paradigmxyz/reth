use futures::Stream;
use std::future::Future;

/// Abstraction for downloading several items at once.
///
/// A [`BatchDownload`] is a [`Future`] that represents a collection of download futures and
/// resolves once all of them finished.
///
/// This is similar to the [`futures::future::join_all`] function, but it's open to implementers how
/// this Future behaves exactly.
///
/// It is expected that the underlying futures return a [`Result`].
pub trait BatchDownload: Future<Output = Result<Vec<Self::Ok>, Self::Error>> {
    /// The `Ok` variant of the futures output in this batch.
    type Ok;
    /// The `Err` variant of the futures output in this batch.
    type Error;

    /// Consumes the batch future and returns a [`Stream`] that yields results as they become ready.
    fn into_stream_unordered(self) -> Box<dyn Stream<Item = Result<Self::Ok, Self::Error>>>;
}
