use core::fmt;

use futures_channel::mpsc;
use futures_util::sink::SinkExt;
use futures_util::stream::{SplitSink, SplitStream, StreamExt};
use gloo_net::websocket::{Message, WebSocketError, futures::WebSocket};
use jsonrpsee_core::client::{ReceivedMessage, TransportReceiverT, TransportSenderT};

/// Web-sys transport error that can occur.
#[derive(Debug, thiserror::Error)]
pub enum Error {
	/// Internal send error
	#[error("Could not send message: {0}")]
	SendError(#[from] mpsc::SendError),
	/// Sender went away
	#[error("Sender went away couldn't receive the message")]
	SenderDisconnected,
	/// Error that occurred in `JS context`.
	#[error("JS Error: {0:?}")]
	Js(String),
	/// WebSocket error
	#[error(transparent)]
	WebSocket(WebSocketError),
	/// Operation not supported
	#[error("Operation not supported")]
	NotSupported,
}

/// Sender.
pub struct Sender(SplitSink<WebSocket, Message>);

impl fmt::Debug for Sender {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Sender").finish()
	}
}

/// Receiver.
pub struct Receiver(SplitStream<WebSocket>);

impl fmt::Debug for Receiver {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Receiver").finish()
	}
}

impl TransportSenderT for Sender {
	type Error = Error;

	fn send(&mut self, msg: String) -> impl Future<Output = Result<(), Self::Error>> {
		async {
			self.0.send(Message::Text(msg)).await.map_err(|e| Error::WebSocket(e))?;
			Ok(())
		}
	}
}

impl TransportReceiverT for Receiver {
	type Error = Error;

	fn receive(&mut self) -> impl Future<Output = Result<ReceivedMessage, Self::Error>> {
		async {
			match self.0.next().await {
				Some(Ok(msg)) => match msg {
					Message::Bytes(bytes) => Ok(ReceivedMessage::Bytes(bytes)),
					Message::Text(txt) => Ok(ReceivedMessage::Text(txt)),
				},
				Some(Err(err)) => Err(Error::WebSocket(err)),
				None => Err(Error::SenderDisconnected),
			}
		}
	}
}

/// Create a transport sender & receiver pair.
pub async fn connect(url: impl AsRef<str>) -> Result<(Sender, Receiver), Error> {
	let websocket = WebSocket::open(url.as_ref()).map_err(|e| Error::Js(e.to_string()))?;
	let (write, read) = websocket.split();

	Ok((Sender(write), Receiver(read)))
}
