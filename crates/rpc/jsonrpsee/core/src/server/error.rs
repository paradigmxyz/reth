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

use crate::server::SubscriptionMessage;
use serde_json::value::RawValue;
use tokio::sync::mpsc;

/// Error that may occur during [`crate::server::MethodSink::try_send`] or [`crate::server::SubscriptionSink::try_send`].
#[derive(Debug, thiserror::Error)]
pub enum TrySendError {
	/// The connection channel is closed.
	#[error("The connection channel is closed")]
	Closed(SubscriptionMessage),
	/// The connection channel is full.
	#[error("The connection channel is full")]
	Full(SubscriptionMessage),
}

/// Error that may occur during [`crate::server::MethodSink::send`] or [`crate::server::SubscriptionSink::send`].
#[derive(Debug, thiserror::Error)]
#[error("The connection channel is closed")]
pub struct DisconnectError(pub SubscriptionMessage);

/// Error that may occur during [`crate::server::MethodSink::send_timeout`] or [`crate::server::SubscriptionSink::send_timeout`].
#[derive(Debug, thiserror::Error)]
pub enum SendTimeoutError {
	/// The data could not be sent because the timeout elapsed
	/// which most likely is that the channel is full.
	#[error("The connection channel timed out waiting on send operation")]
	Timeout(SubscriptionMessage),
	/// The connection channel is closed.
	#[error("The connection channel is closed")]
	Closed(SubscriptionMessage),
}

/// The error returned while accepting a subscription.
#[derive(Debug, Copy, Clone, thiserror::Error)]
#[error("The remote peer closed the connection")]
pub struct PendingSubscriptionAcceptError;

impl From<mpsc::error::SendError<Box<RawValue>>> for DisconnectError {
	fn from(e: mpsc::error::SendError<Box<RawValue>>) -> Self {
		DisconnectError(SubscriptionMessage::from_complete_message(e.0))
	}
}

impl From<mpsc::error::TrySendError<Box<RawValue>>> for TrySendError {
	fn from(e: mpsc::error::TrySendError<Box<RawValue>>) -> Self {
		match e {
			mpsc::error::TrySendError::Closed(m) => Self::Closed(SubscriptionMessage::from_complete_message(m)),
			mpsc::error::TrySendError::Full(m) => Self::Full(SubscriptionMessage::from_complete_message(m)),
		}
	}
}

impl From<mpsc::error::SendTimeoutError<Box<RawValue>>> for SendTimeoutError {
	fn from(e: mpsc::error::SendTimeoutError<Box<RawValue>>) -> Self {
		match e {
			mpsc::error::SendTimeoutError::Closed(m) => Self::Closed(SubscriptionMessage::from_complete_message(m)),
			mpsc::error::SendTimeoutError::Timeout(m) => Self::Timeout(SubscriptionMessage::from_complete_message(m)),
		}
	}
}
