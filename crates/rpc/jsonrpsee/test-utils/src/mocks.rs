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

use std::{io, net::SocketAddr};

use futures_util::io::{BufReader, BufWriter};
use serde::{Deserialize, Serialize};
use soketto::handshake::{self, Error as SokettoError};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

pub use hyper::{HeaderMap, StatusCode, Uri};

type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Request Id
#[derive(Debug, PartialEq, Clone, Hash, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum Id {
    /// No id (notification)
    Null,
    /// Numeric id
    Num(u64),
    /// String id
    Str(String),
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: StatusCode,
    pub header: HeaderMap,
    pub body: String,
}

/// WebSocket client to construct with arbitrary payload to construct bad payloads.
pub struct WebSocketTestClient {
    tx: soketto::Sender<BufReader<BufWriter<Compat<TcpStream>>>>,
    rx: soketto::Receiver<BufReader<BufWriter<Compat<TcpStream>>>>,
}

impl std::fmt::Debug for WebSocketTestClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WebSocketTestClient")
    }
}

#[derive(Debug)]
pub enum WebSocketTestError {
    Redirect,
    RejectedWithStatusCode(u16),
    Soketto(SokettoError),
}

impl From<io::Error> for WebSocketTestError {
    fn from(err: io::Error) -> Self {
        WebSocketTestError::Soketto(SokettoError::Io(err))
    }
}

impl WebSocketTestClient {
    pub async fn new(url: SocketAddr) -> Result<Self, WebSocketTestError> {
        let socket = TcpStream::connect(url).await?;
        let mut client = handshake::Client::new(
            BufReader::new(BufWriter::new(socket.compat())),
            "test-client",
            "/",
        );
        match client.handshake().await {
            Ok(handshake::ServerResponse::Accepted { .. }) => {
                let (tx, rx) = client.into_builder().finish();
                Ok(Self { tx, rx })
            }
            Ok(handshake::ServerResponse::Redirect { .. }) => Err(WebSocketTestError::Redirect),
            Ok(handshake::ServerResponse::Rejected { status_code }) => {
                Err(WebSocketTestError::RejectedWithStatusCode(status_code))
            }
            Err(err) => Err(WebSocketTestError::Soketto(err)),
        }
    }

    pub async fn send_request_text(&mut self, msg: impl AsRef<str>) -> Result<String, Error> {
        self.send(msg).await?;
        let mut data = Vec::new();
        self.rx.receive_data(&mut data).await?;
        String::from_utf8(data).map_err(Into::into)
    }

    pub async fn send(&mut self, msg: impl AsRef<str>) -> Result<(), Error> {
        self.tx.send_text(msg).await?;
        self.tx.flush().await.map_err(Into::into)
    }

    pub async fn send_request_binary(&mut self, msg: &[u8]) -> Result<String, Error> {
        self.tx.send_binary(msg).await?;
        self.tx.flush().await?;
        self.receive().await
    }

    pub async fn receive(&mut self) -> Result<String, Error> {
        let mut data = Vec::new();
        self.rx.receive_data(&mut data).await?;
        String::from_utf8(data).map_err(Into::into)
    }

    pub async fn close(&mut self) -> Result<(), Error> {
        self.tx.close().await.map_err(Into::into)
    }
}
