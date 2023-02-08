use futures_util::stream::Stream;
use num_traits::One;
use std::{
    cmp::Ordering,
    collections::{binary_heap::PeekMut, BinaryHeap},
    ops::Add,
    pin::Pin,
    task::{Context, Poll},
};

/// A Stream type that emits key-value pairs in sequential order of the key.
#[pin_project::pin_project]
#[must_use = "stream does nothing unless polled"]
pub(crate) struct SequentialPairStream<Key: Ord, Value, St> {
    /// The next item we expect from the stream
    next: Key,
    /// buffered entries
    pending: BinaryHeap<OrderedItem<Key, Value>>,
    #[pin]
    stream: St,
    done: bool,
}

// === impl SequentialPairStream ===

impl<Key: Ord, Value, St> SequentialPairStream<Key, Value, St> {
    /// Returns a new [SequentialPairStream] that emits the items of the given stream in order
    /// starting at the given start point.
    pub(crate) fn new(start: Key, stream: St) -> Self {
        Self { next: start, pending: Default::default(), stream, done: false }
    }
}

/// implements Stream for any underlying Stream that returns a result
impl<Key, Value, St, Err> Stream for SequentialPairStream<Key, Value, St>
where
    Key: Ord + Copy + Add<Output = Key> + One,
    St: Stream<Item = Result<(Key, Value), Err>>,
{
    type Item = Result<(Key, Value), Err>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        'outer: loop {
            // try to drain buffered items
            while let Some(maybe_next) = this.pending.peek_mut() {
                match (maybe_next.0).cmp(&*this.next) {
                    Ordering::Less => {
                        PeekMut::pop(maybe_next);
                        continue
                    }
                    Ordering::Equal => {
                        let next = PeekMut::pop(maybe_next);
                        *this.next = *this.next + Key::one();
                        return Poll::Ready(Some(Ok(next.into())))
                    }
                    Ordering::Greater => {
                        if *this.done {
                            let next = PeekMut::pop(maybe_next);
                            return Poll::Ready(Some(Ok(next.into())))
                        }
                        break
                    }
                }
            }

            if *this.done {
                return Poll::Ready(None)
            }

            loop {
                match this.stream.as_mut().poll_next(cx) {
                    Poll::Pending => break,
                    Poll::Ready(item) => match item {
                        Some(Ok((k, v))) => {
                            if k == *this.next {
                                *this.next = *this.next + Key::one();
                                return Poll::Ready(Some(Ok((k, v))))
                            }
                            this.pending.push(OrderedItem(k, v));
                        }
                        Some(err @ Err(_)) => return Poll::Ready(Some(err)),
                        None => {
                            *this.done = true;
                            continue 'outer
                        }
                    },
                }
            }

            return Poll::Pending
        }
    }
}

/// The item a [SequentialPairStream] emits
struct OrderedItem<Key, Value>(Key, Value);

impl<Key, Value> From<OrderedItem<Key, Value>> for (Key, Value) {
    fn from(value: OrderedItem<Key, Value>) -> Self {
        (value.0, value.1)
    }
}

impl<Key: Ord, Value> Eq for OrderedItem<Key, Value> {}

impl<Key: Ord, Value> PartialEq for OrderedItem<Key, Value> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<Key: Ord, Value> PartialOrd for OrderedItem<Key, Value> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(other.0.cmp(&self.0))
    }
}

impl<Key: Ord, Value> Ord for OrderedItem<Key, Value> {
    fn cmp(&self, other: &Self) -> Ordering {
        // binary heap is max-heap
        other.0.cmp(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{stream, TryStreamExt};
    use rand::{seq::SliceRandom, thread_rng};

    #[tokio::test]
    async fn test_ordered_stream() {
        let values: Vec<_> = (0..10).map(|i| (i, i)).collect();

        let mut input = values.clone();
        input.shuffle(&mut thread_rng());
        let stream = stream::iter(input.into_iter().map(Ok::<_, ()>));
        let ordered = SequentialPairStream::new(values[0].0, stream);
        let received = ordered.try_collect::<Vec<_>>().await.unwrap();
        assert_eq!(received, values);
    }
}
