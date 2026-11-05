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

//! Convenience wrapper for a stream (AsyncRead + AsyncWrite) which can either be plain TCP or TLS.

use std::io::Error as IoError;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use pin_project::pin_project;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

/// Stream to represent either a unencrypted or encrypted socket stream.
#[pin_project(project = EitherStreamProj)]
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum EitherStream {
	/// Unencrypted socket stream.
	Plain(#[pin] TcpStream),
	/// Encrypted socket stream.
	#[cfg(feature = "tls")]
	Tls(#[pin] tokio_rustls::client::TlsStream<TcpStream>),
}

impl AsyncRead for EitherStream {
	fn poll_read(
		self: Pin<&mut Self>,
		cx: &mut Context,
		buf: &mut tokio::io::ReadBuf<'_>,
	) -> Poll<Result<(), IoError>> {
		match self.project() {
			EitherStreamProj::Plain(stream) => AsyncRead::poll_read(stream, cx, buf),
			#[cfg(feature = "tls")]
			EitherStreamProj::Tls(stream) => AsyncRead::poll_read(stream, cx, buf),
		}
	}
}

impl AsyncWrite for EitherStream {
	fn poll_write(self: Pin<&mut Self>, cx: &mut Context, buf: &[u8]) -> Poll<Result<usize, IoError>> {
		match self.project() {
			EitherStreamProj::Plain(stream) => AsyncWrite::poll_write(stream, cx, buf),
			#[cfg(feature = "tls")]
			EitherStreamProj::Tls(stream) => AsyncWrite::poll_write(stream, cx, buf),
		}
	}

	fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), IoError>> {
		match self.project() {
			EitherStreamProj::Plain(stream) => AsyncWrite::poll_flush(stream, cx),
			#[cfg(feature = "tls")]
			EitherStreamProj::Tls(stream) => AsyncWrite::poll_flush(stream, cx),
		}
	}

	fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), IoError>> {
		match self.project() {
			EitherStreamProj::Plain(stream) => AsyncWrite::poll_shutdown(stream, cx),
			#[cfg(feature = "tls")]
			EitherStreamProj::Tls(stream) => AsyncWrite::poll_shutdown(stream, cx),
		}
	}
}
