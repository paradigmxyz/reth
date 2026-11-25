//! Bounded RPC input for `eth_callMany`, `debug_traceCallMany` and `trace_callMany`.

use alloy_rpc_types_eth::Bundle;
use reth_rpc_eth_types::{utils::CallManyBudget, EthApiError};
use serde::{
    de::{DeserializeOwned, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor},
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
            .map_err(invalid_params)?;
        CallManyBudget::check_calls(calls)?;
        serde_json::from_str(raw.get()).map_err(invalid_params)
    }
}

/// Maximum length of a decode error message, which can quote request input. Matches the cap
/// jsonrpsee applies to the params errors it reports itself.
const MAX_INVALID_PARAMS_LEN: usize = 1024;

/// Returns [`EthApiError::InvalidParams`] for a decode error, truncated to
/// [`MAX_INVALID_PARAMS_LEN`].
fn invalid_params(err: serde_json::Error) -> EthApiError {
    let mut msg = err.to_string();
    msg.truncate(msg.floor_char_boundary(MAX_INVALID_PARAMS_LEN));
    EthApiError::InvalidParams(msg)
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
struct BundleCalls {
    transactions: ArrayLen,
}

impl<'de> Deserialize<'de> for BundleCalls {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "Bundle",
            &["transactions", "blockOverride"],
            BundleCallsVisitor,
        )
    }
}

/// Reads the transactions of a bundle in either encoding [`Bundle`] accepts: an object, or an
/// array of its fields in order.
struct BundleCallsVisitor;

impl<'de> Visitor<'de> for BundleCallsVisitor {
    type Value = BundleCalls;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bundle")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut transactions = None;
        while let Some(key) = map.next_key::<BundleField>()? {
            match key {
                // Rejected like the typed decoder does, so the count covers every array it would
                // decode.
                BundleField::Transactions if transactions.is_some() => {
                    return Err(A::Error::duplicate_field("transactions"))
                }
                BundleField::Transactions => transactions = Some(map.next_value()?),
                BundleField::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(BundleCalls { transactions: transactions.unwrap_or_default() })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let transactions = seq.next_element()?.unwrap_or_default();
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(BundleCalls { transactions })
    }
}

/// The bundle fields the call count reads.
#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "camelCase")]
enum BundleField {
    Transactions,
    #[serde(other)]
    Other,
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
    fn counts_array_encoded_bundles() {
        let input = r#"[[[{}, {}], null], [[{}]]]"#;
        let parsed = parse::<Vec<Bundle<TransactionRequest>>>(input).unwrap();
        assert_eq!(parsed.iter().map(|bundle| bundle.transactions.len()).sum::<usize>(), 3);

        let input =
            format!("[[[{}], null]]", vec!["{}"; DEFAULT_MAX_CALL_MANY_CALLS + 1].join(","));
        let err = parse::<Vec<Bundle<TransactionRequest>>>(&input).unwrap_err();
        assert_eq!(err.to_string(), limit_err(DEFAULT_MAX_CALL_MANY_CALLS + 1));
    }

    /// A repeated `transactions` key is rejected before the typed decoder could decode the
    /// arrays the count skipped.
    #[test]
    fn rejects_duplicate_transactions_without_decoding() {
        let calls = vec!["{}"; DEFAULT_MAX_CALL_MANY_CALLS + 1].join(",");
        let input = format!(r#"[{{"transactions":[{calls}],"transactions":[]}}]"#);
        let err = parse::<Vec<Bundle<TransactionRequest>>>(&input).unwrap_err();
        assert_eq!(err.to_string(), "duplicate field `transactions` at line 1 column 3108");
    }

    #[test]
    fn rejects_malformed_input() {
        let err = parse::<Vec<Bundle<TransactionRequest>>>(r#"{"transactions":[]}"#).unwrap_err();
        let err: ErrorObjectOwned = err.into();
        assert_eq!(err.code(), -32602);
    }

    #[test]
    fn truncates_decode_errors() {
        let input = format!(r#""{}""#, "a".repeat(4 * MAX_INVALID_PARAMS_LEN));
        let err = parse::<Vec<Bundle<TransactionRequest>>>(&input).unwrap_err();
        assert_eq!(err.to_string().len(), MAX_INVALID_PARAMS_LEN);
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
