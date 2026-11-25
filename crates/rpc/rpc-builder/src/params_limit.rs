//! [`jsonrpsee`] layer that bounds the JSON objects and arrays in a request's params.

use jsonrpsee::{
    core::middleware::{Batch, BatchEntry, BatchEntryErr, Notification},
    server::middleware::rpc::RpcServiceT,
    types::{
        error::{ErrorObject, ErrorObjectOwned, INVALID_PARAMS_CODE},
        Request,
    },
    MethodResponse,
};
use memchr::{memchr2, memchr3};
use std::future::{ready, Future};
use tokio_util::either::Either;
use tower::Layer;

/// Rejects calls whose params hold more than a set number of JSON objects and arrays.
///
/// Methods decode their params before they run, so an element as small as `{}` can allocate a
/// full default value of the method's param type, and a request within the body size limit can
/// expand many times over in memory. Counting objects and arrays in the raw params bounds that
/// for every method, including those whose params have no limits of their own.
#[derive(Debug, Clone, Copy)]
pub struct RpcParamsLimit {
    /// The most objects and arrays a call's params may hold.
    max_containers: usize,
}

impl RpcParamsLimit {
    /// Creates a layer that rejects calls whose params hold more than `max_containers` JSON
    /// objects and arrays, counting the params array itself.
    pub const fn new(max_containers: usize) -> Self {
        Self { max_containers }
    }

    /// Returns the error for a call whose params exceed the limit, or `None` if they are within
    /// it.
    fn check(&self, req: &Request<'_>) -> Option<ErrorObjectOwned> {
        let params = req.params.as_deref()?;
        exceeds_containers(params.get(), self.max_containers).then(|| {
            ErrorObject::owned(
                INVALID_PARAMS_CODE,
                format!("params exceed {} JSON objects and arrays", self.max_containers),
                None::<()>,
            )
        })
    }
}

impl<S> Layer<S> for RpcParamsLimit {
    type Service = RpcParamsLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RpcParamsLimitService { inner, limit: *self }
    }
}

/// A [`RpcServiceT`] middleware that applies [`RpcParamsLimit`].
#[derive(Debug, Clone)]
pub struct RpcParamsLimitService<S> {
    /// The inner service being wrapped.
    inner: S,
    /// The limit applied to each call.
    limit: RpcParamsLimit,
}

impl<S> RpcServiceT for RpcParamsLimitService<S>
where
    S: RpcServiceT + Send + Sync + Clone + 'static,
{
    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
        match self.limit.check(&req) {
            Some(err) => Either::Left(ready(MethodResponse::error(req.id, err))),
            None => Either::Right(self.inner.call(req)),
        }
    }

    /// Batch entries are dispatched to the innermost service directly, so each call is checked
    /// here.
    fn batch<'a>(&self, mut batch: Batch<'a>) -> impl Future<Output = MethodResponse> + Send + 'a {
        for entry in batch.iter_mut() {
            if let Ok(BatchEntry::Call(req)) = entry &&
                let Some(err) = self.limit.check(req)
            {
                *entry = Err(BatchEntryErr::new(req.id.clone(), err));
            }
        }
        self.inner.batch(batch)
    }

    fn notification<'a>(
        &self,
        n: Notification<'a>,
    ) -> impl Future<Output = MethodResponse> + Send + 'a {
        self.inner.notification(n)
    }
}

/// Returns whether the JSON in `params` holds more than `max` objects and arrays.
///
/// `params` is valid JSON, so brackets only need to be told apart from string contents. The scan
/// jumps between quotes, brackets and escapes with [`memchr`], so long strings cost little.
fn exceeds_containers(params: &str, max: usize) -> bool {
    let bytes = params.as_bytes();
    let mut containers = 0usize;
    let mut pos = 0;
    while let Some(offset) = bytes.get(pos..).and_then(|rest| memchr3(b'"', b'{', b'[', rest)) {
        pos += offset + 1;
        if bytes[pos - 1] != b'"' {
            containers += 1;
            if containers > max {
                return true
            }
            continue
        }
        // Skip to the string's closing quote, stepping over each escaped byte.
        loop {
            let Some(offset) = bytes.get(pos..).and_then(|rest| memchr2(b'"', b'\\', rest)) else {
                return false
            };
            pos += offset + 1;
            if bytes[pos - 1] == b'"' {
                break
            }
            pos += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_objects_and_arrays() {
        assert!(!exceeds_containers("[{},{}]", 3));
        assert!(exceeds_containers("[{},{},{}]", 3));
        assert!(!exceeds_containers("[1,\"0x00\",true,null]", 1));
    }

    #[test]
    fn steps_over_escapes_at_string_end() {
        assert!(!exceeds_containers(r#"["\\\\",{}]"#, 2));
        assert!(!exceeds_containers(r#"["a\\\"[{"]"#, 1));
        assert!(exceeds_containers(r#"["\\\\",{},[]]"#, 2));
    }

    #[test]
    fn ignores_brackets_in_strings() {
        assert!(!exceeds_containers(r#"["{[{[","\"{[","\\",{}]"#, 2));
        assert!(exceeds_containers(r#"["\\",{},[]]"#, 2));
    }
}
