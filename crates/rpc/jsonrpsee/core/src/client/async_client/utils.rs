// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use futures_util::stream::FuturesUnordered;
use futures_util::{Stream, StreamExt, Future};
use pin_project::pin_project;

#[pin_project]
pub(crate) struct IntervalStream<S>(#[pin] Option<S>);

impl<S> IntervalStream<S> {
	/// Creates a stream which never returns any elements.
	pub(crate) fn pending() -> Self {
		Self(None)
	}

	/// Creates a stream which produces elements with interval of `period`.
	#[cfg(feature = "async-client")]
	pub(crate) fn new(s: S) -> Self {
		Self(Some(s))
	}
}

impl<S: Stream> Stream for IntervalStream<S> {
	type Item = ();

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		if let Some(mut stream) = self.project().0.as_pin_mut() {
			match stream.poll_next_unpin(cx) {
				Poll::Pending => Poll::Pending,
				Poll::Ready(Some(_)) => Poll::Ready(Some(())),
				Poll::Ready(None) => Poll::Ready(None),
			}
		} else {
			// NOTE: this will not be woken up again and it's by design
			// to be a pending stream that never returns.
			Poll::Pending
		}
	}
}

#[allow(unused)]
pub(crate) enum InactivityCheck {
	Disabled,
	Enabled { inactive_dur: Duration, last_active: std::time::Instant, count: usize, max_count: usize }
}

impl InactivityCheck {
	#[cfg(feature = "async-client")]
	pub(crate) fn new(_inactive_dur: Duration, _max_count: usize) -> Self {
		Self::Enabled { inactive_dur: _inactive_dur, last_active: std::time::Instant::now(), count: 0, max_count: _max_count }
	}

	pub(crate) fn is_inactive(&mut self) -> bool {
		match self {
			Self::Disabled => false,
			Self::Enabled { inactive_dur, last_active, count, max_count, .. } => {
				if last_active.elapsed() >= *inactive_dur {
					*count += 1;
				}

				count >= max_count
			}
 		}
	}

	pub(crate) fn mark_as_active(&mut self) {
		if let Self::Enabled { last_active, .. } = self {
			*last_active = std::time::Instant::now();
		}
	}
}



/// A wrapper around `FuturesUnordered` that doesn't return `None` when it's empty.
pub(crate) struct MaybePendingFutures<Fut> {
	futs: FuturesUnordered<Fut>,
	waker: Option<Waker>,
}

impl<Fut> MaybePendingFutures<Fut> {
	pub(crate) fn new() -> Self {
		Self { futs: FuturesUnordered::new(), waker: None }
	}

	pub(crate) fn push(&mut self, fut: Fut) {
		self.futs.push(fut);

		if let Some(w) = self.waker.take() {
			w.wake();
		}
	}
}

impl<Fut: Future> Stream for MaybePendingFutures<Fut> {
	type Item = Fut::Output;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		if self.futs.is_empty() {
			self.waker = Some(cx.waker().clone());
			return Poll::Pending;
		}

		self.futs.poll_next_unpin(cx)
	}
}
