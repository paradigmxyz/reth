use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Full};
use hyper::body::Incoming;
use std::{
    fmt,
    pin::Pin,
    task::{Context, Poll},
};
use tower::BoxError;

/// HTTP request with an [`HttpBody`].
pub type HttpRequest<B = HttpBody> = http::Request<B>;

/// HTTP response with an [`HttpBody`].
pub type HttpResponse<B = HttpBody> = http::Response<B>;

/// A type-erased HTTP body.
///
/// Received bodies and full or empty bodies are stored without boxing.
pub struct HttpBody(Inner);

enum Inner {
    Empty,
    Full(Full<Bytes>),
    Incoming(Incoming),
    Boxed(UnsyncBoxBody<Bytes, BoxError>),
}

impl HttpBody {
    /// Wraps a body.
    pub fn new<B>(body: B) -> Self
    where
        B: Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        Self(Inner::Boxed(body.map_err(Into::into).boxed_unsync()))
    }

    /// Returns an empty body.
    pub const fn empty() -> Self {
        Self(Inner::Empty)
    }
}

impl Default for HttpBody {
    fn default() -> Self {
        Self::empty()
    }
}

impl fmt::Debug for HttpBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpBody").finish_non_exhaustive()
    }
}

impl Body for HttpBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        match &mut self.0 {
            Inner::Empty => Poll::Ready(None),
            Inner::Full(body) => Pin::new(body).poll_frame(cx).map_err(|err| match err {}),
            Inner::Incoming(body) => Pin::new(body).poll_frame(cx).map_err(Into::into),
            Inner::Boxed(body) => Pin::new(body).poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.0 {
            Inner::Empty => true,
            Inner::Full(body) => body.is_end_stream(),
            Inner::Incoming(body) => body.is_end_stream(),
            Inner::Boxed(body) => body.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.0 {
            Inner::Empty => SizeHint::with_exact(0),
            Inner::Full(body) => body.size_hint(),
            Inner::Incoming(body) => body.size_hint(),
            Inner::Boxed(body) => body.size_hint(),
        }
    }
}

impl From<Incoming> for HttpBody {
    fn from(body: Incoming) -> Self {
        Self(Inner::Incoming(body))
    }
}

macro_rules! impl_from_full {
    ($($t:ty),*) => {$(
        impl From<$t> for HttpBody {
            fn from(value: $t) -> Self {
                Self(Inner::Full(Full::new(Bytes::from(value))))
            }
        }
    )*};
}

impl_from_full!(Bytes, String, Vec<u8>, &'static str);
