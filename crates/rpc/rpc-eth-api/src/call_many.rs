//! Bounded RPC input for `eth_callMany`, `debug_traceCallMany` and `trace_callMany`.

use alloy_rpc_types_eth::Bundle;
use reth_rpc_eth_types::{utils::CallManyBudget, EthApiError};
use serde::{
    de::{DeserializeOwned, IgnoredAny, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::value::RawValue;
use std::fmt;

/// The calls of a `*_callMany` request, decoded only once their number is within the limit.
///
/// RPC params are decoded before the handler runs, so a plain `Vec` would allocate a full
/// transaction request for every `{}` in the input before [`CallManyBudget`] could reject it.
/// This keeps the raw input until [`Self::into_inner`] has counted the calls without decoding
/// them.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum CallManyRequest<T> {
    /// Raw RPC input, retained until the handler checks the call limit.
    Raw(Box<RawValue>),
    /// Typed calls supplied by an RPC client or internal caller.
    Parsed(T),
}

impl<T> From<T> for CallManyRequest<T> {
    fn from(calls: T) -> Self {
        Self::Parsed(calls)
    }
}

impl<'de, T> Deserialize<'de> for CallManyRequest<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Box::<RawValue>::deserialize(deserializer).map(Self::Raw)
    }
}

impl<T: CallManyCalls + DeserializeOwned> CallManyRequest<T> {
    /// Decodes RPC input after checking its call count with [`CallManyBudget::check_calls`].
    ///
    /// Returns [`EthApiError::InvalidParams`] if the input is malformed or has too many calls.
    pub fn into_inner(self) -> Result<T, EthApiError> {
        let raw = match self {
            Self::Raw(raw) => raw,
            Self::Parsed(calls) => return Ok(calls),
        };
        let calls = T::count_calls(&mut serde_json::Deserializer::from_str(raw.get()))
            .map_err(|err| EthApiError::InvalidParams(err.to_string()))?;
        CallManyBudget::check_calls(calls)?;
        serde_json::from_str(raw.get()).map_err(|err| EthApiError::InvalidParams(err.to_string()))
    }
}

/// A list of calls whose number can be read from its JSON input without decoding the calls.
pub trait CallManyCalls {
    /// Returns the number of calls in the input, counted the way [`CallManyBudget`] counts them.
    fn count_calls<'de, D: Deserializer<'de>>(deserializer: D) -> Result<usize, D::Error>;
}

impl<TxReq> CallManyCalls for Vec<Bundle<TxReq>> {
    /// An empty bundle counts as one call, like [`CallManyBudget::for_bundles`].
    fn count_calls<'de, D: Deserializer<'de>>(deserializer: D) -> Result<usize, D::Error> {
        deserializer.deserialize_seq(BundlesVisitor)
    }
}

impl<TxReq, T> CallManyCalls for Vec<(TxReq, T)> {
    fn count_calls<'de, D: Deserializer<'de>>(deserializer: D) -> Result<usize, D::Error> {
        ArrayLen::deserialize(deserializer).map(|len| len.0)
    }
}

/// Counts the calls of a list of bundles.
struct BundlesVisitor;

impl<'de> Visitor<'de> for BundlesVisitor {
    type Value = usize;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an array of bundles")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut calls = 0usize;
        while let Some(bundle) = seq.next_element::<BundleCalls>()? {
            calls = calls.saturating_add(bundle.transactions.0.max(1));
        }
        Ok(calls)
    }
}

/// The transactions of a bundle, counted without decoding them. Other fields are skipped.
#[derive(Deserialize)]
struct BundleCalls {
    #[serde(default)]
    transactions: ArrayLen,
}

/// The length of a JSON array, counted without decoding its elements.
#[derive(Default)]
struct ArrayLen(usize);

impl<'de> Deserialize<'de> for ArrayLen {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_seq(ArrayLenVisitor)
    }
}

/// Counts the elements of a JSON array.
struct ArrayLenVisitor;

impl<'de> Visitor<'de> for ArrayLenVisitor {
    type Value = ArrayLen;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an array")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut len = 0usize;
        while seq.next_element::<IgnoredAny>()?.is_some() {
            len += 1;
        }
        Ok(ArrayLen(len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::map::HashSet;
    use alloy_rpc_types_eth::TransactionRequest;
    use jsonrpsee_types::error::ErrorObjectOwned;
    use reth_rpc_server_types::constants::DEFAULT_MAX_CALL_MANY_CALLS;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Size of the request body a public RPC proxy accepts.
    const BODY_LIMIT: usize = 4 * 1024 * 1024;

    static DECODED: AtomicUsize = AtomicUsize::new(0);

    /// A transaction request that counts how often it is decoded.
    #[derive(Debug, Serialize)]
    struct CountedRequest;

    impl<'de> Deserialize<'de> for CountedRequest {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            IgnoredAny::deserialize(deserializer)?;
            DECODED.fetch_add(1, Ordering::Relaxed);
            Ok(Self)
        }
    }

    fn bundles(counts: &[usize]) -> String {
        let bundles = counts
            .iter()
            .map(|&count| format!(r#"{{"transactions":[{}]}}"#, vec!["{}"; count].join(",")))
            .collect::<Vec<_>>()
            .join(",");
        format!("[{bundles}]")
    }

    fn trace_calls(call: &str, count: usize) -> String {
        format!("[{}]", vec![call; count].join(","))
    }

    fn parse<T: CallManyCalls + DeserializeOwned>(input: &str) -> Result<T, EthApiError> {
        serde_json::from_str::<CallManyRequest<T>>(input).unwrap().into_inner()
    }

    fn limit_err(calls: usize) -> String {
        format!("call count {calls} exceeds limit {DEFAULT_MAX_CALL_MANY_CALLS}")
    }

    #[test]
    fn accepts_call_limit() {
        let input = bundles(&[DEFAULT_MAX_CALL_MANY_CALLS - 1, 1]);
        let parsed = parse::<Vec<Bundle<TransactionRequest>>>(&input).unwrap();
        assert_eq!(
            parsed.iter().map(|bundle| bundle.transactions.len()).sum::<usize>(),
            DEFAULT_MAX_CALL_MANY_CALLS
        );

        let input = trace_calls(r#"[{},["trace"]]"#, DEFAULT_MAX_CALL_MANY_CALLS);
        let parsed = parse::<Vec<(TransactionRequest, HashSet<String>)>>(&input).unwrap();
        assert_eq!(parsed.len(), DEFAULT_MAX_CALL_MANY_CALLS);
    }

    #[test]
    fn bounds_calls_across_bundles() {
        let input = bundles(&[DEFAULT_MAX_CALL_MANY_CALLS, 1]);
        let err = parse::<Vec<Bundle<TransactionRequest>>>(&input).unwrap_err();
        assert_eq!(err.to_string(), limit_err(DEFAULT_MAX_CALL_MANY_CALLS + 1));
        let err: ErrorObjectOwned = err.into();
        assert_eq!(err.code(), -32602);
    }

    #[test]
    fn counts_empty_bundles() {
        let input = bundles(&vec![0; DEFAULT_MAX_CALL_MANY_CALLS + 1]);
        let err = parse::<Vec<Bundle<TransactionRequest>>>(&input).unwrap_err();
        assert_eq!(err.to_string(), limit_err(DEFAULT_MAX_CALL_MANY_CALLS + 1));
    }

    /// A request body of empty objects is rejected before any call is decoded, so it can't expand
    /// into a full transaction request per `{}`.
    #[test]
    fn rejects_empty_object_amplification_without_decoding() {
        let calls = BODY_LIMIT / 3;
        let err = parse::<Vec<Bundle<CountedRequest>>>(&bundles(&[calls])).unwrap_err();
        assert_eq!(err.to_string(), limit_err(calls));

        let calls = BODY_LIMIT / 8;
        let input = trace_calls("[{},[]]", calls);
        let err = parse::<Vec<(CountedRequest, HashSet<String>)>>(&input).unwrap_err();
        assert_eq!(err.to_string(), limit_err(calls));

        assert_eq!(DECODED.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn rejects_malformed_input() {
        let err = parse::<Vec<Bundle<TransactionRequest>>>(r#"{"transactions":[]}"#).unwrap_err();
        let err: ErrorObjectOwned = err.into();
        assert_eq!(err.code(), -32602);
    }

    #[test]
    fn serializes_like_calls() {
        let calls = vec![Bundle::<TransactionRequest>::default()];
        assert_eq!(
            serde_json::to_value(CallManyRequest::from(calls.clone())).unwrap(),
            serde_json::to_value(calls).unwrap()
        );
    }
}
