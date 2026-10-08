use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Empty, Full};
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
pub struct HttpBody(UnsyncBoxBody<Bytes, BoxError>);

impl HttpBody {
    /// Wraps a body.
    pub fn new<B>(body: B) -> Self
    where
        B: Body<Data = Bytes> + Send + 'static,
        B::Error: Into<BoxError>,
    {
        Self(body.map_err(Into::into).boxed_unsync())
    }

    /// Returns an empty body.
    pub fn empty() -> Self {
        Self::new(Empty::new())
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
        Pin::new(&mut self.0).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.0.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.0.size_hint()
    }
}

macro_rules! impl_from_full {
    ($($t:ty),*) => {$(
        impl From<$t> for HttpBody {
            fn from(value: $t) -> Self {
                Self::new(Full::new(Bytes::from(value)))
            }
        }
    )*};
}

impl_from_full!(Bytes, String, Vec<u8>, &'static str);
