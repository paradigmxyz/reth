use crate::{Id, Params};
use bytes::Bytes;
use serde::{de::IgnoredAny, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use std::{
    borrow::{Borrow, Cow},
    fmt,
    hash::{Hash, Hasher},
    ops::Deref,
    str::Utf8Error,
};

/// An immutable UTF-8 string backed by [`Bytes`].
///
/// Cloning is cheap, and strings parsed from a request share the request buffer.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteStr(Bytes);

impl ByteStr {
    /// Creates a new [`ByteStr`] from a static string.
    pub const fn from_static(s: &'static str) -> Self {
        Self(Bytes::from_static(s.as_bytes()))
    }

    /// Creates a new [`ByteStr`] from UTF-8 bytes.
    pub fn from_utf8(bytes: Bytes) -> Result<Self, Utf8Error> {
        std::str::from_utf8(&bytes)?;
        Ok(Self(bytes))
    }

    /// Returns the string slice.
    pub fn as_str(&self) -> &str {
        // SAFETY: every constructor checks or guarantees UTF-8.
        unsafe { std::str::from_utf8_unchecked(&self.0) }
    }

    /// Returns the underlying bytes.
    pub fn into_bytes(self) -> Bytes {
        self.0
    }

    /// Returns `s`, which must point into `buf`, as a slice of `buf`.
    pub(crate) fn slice_ref(buf: &Bytes, s: &str) -> Self {
        Self(buf.slice_ref(s.as_bytes()))
    }

    /// Returns `s` as a slice of `buf` if borrowed, or copies it otherwise.
    fn from_cow(buf: &Bytes, s: Cow<'_, str>) -> Self {
        match s {
            Cow::Borrowed(s) => Self::slice_ref(buf, s),
            Cow::Owned(s) => s.into(),
        }
    }
}

impl Deref for ByteStr {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for ByteStr {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for ByteStr {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl Hash for ByteStr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state)
    }
}

impl PartialEq<str> for ByteStr {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for ByteStr {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl From<&'static str> for ByteStr {
    fn from(s: &'static str) -> Self {
        Self::from_static(s)
    }
}

impl From<String> for ByteStr {
    fn from(s: String) -> Self {
        Self(s.into())
    }
}

impl From<Box<str>> for ByteStr {
    fn from(s: Box<str>) -> Self {
        Self(Bytes::from(s.into_boxed_bytes()))
    }
}

impl From<Box<RawValue>> for ByteStr {
    fn from(s: Box<RawValue>) -> Self {
        Box::<str>::from(s).into()
    }
}

impl fmt::Debug for ByteStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for ByteStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl Serialize for ByteStr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ByteStr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Into::into)
    }
}

/// A JSON-RPC method call.
#[derive(Clone, Debug)]
pub struct Request {
    /// Request id.
    pub id: Id,
    /// Method name.
    pub method: ByteStr,
    /// Method parameters.
    pub params: Params,
}

impl Request {
    /// Creates a new request.
    pub fn new(method: impl Into<ByteStr>, params: Params, id: Id) -> Self {
        Self { id, method: method.into(), params }
    }

    /// Returns the method name.
    pub fn method_name(&self) -> &str {
        &self.method
    }

    /// Returns the request id.
    pub const fn id(&self) -> &Id {
        &self.id
    }

    /// Returns the parameters.
    pub const fn params(&self) -> &Params {
        &self.params
    }
}

/// A parsed JSON-RPC message.
#[derive(Debug)]
pub(crate) enum Message {
    /// A method call.
    Call(Request),
    /// A notification, which gets no response.
    Notification,
    /// An invalid request, answered with this error and id.
    Invalid(Id, crate::ErrorCode),
}

#[derive(Deserialize)]
struct RawRequest<'a> {
    #[serde(borrow)]
    jsonrpc: Cow<'a, str>,
    #[serde(borrow, default, deserialize_with = "some_raw")]
    id: Option<&'a RawValue>,
    #[serde(borrow)]
    method: Cow<'a, str>,
    #[serde(borrow, default)]
    params: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct RawId<'a> {
    #[serde(borrow, default)]
    id: Option<&'a RawValue>,
}

/// Keeps an explicit `null` id, unlike `Option`, which maps it to `None`.
fn some_raw<'de, D: Deserializer<'de>>(d: D) -> Result<Option<&'de RawValue>, D::Error> {
    <&RawValue>::deserialize(d).map(Some)
}

/// Splits a buffer into its JSON-RPC messages, or returns `None` if it is not a batch.
pub(crate) fn split_batch(buf: &Bytes) -> Option<Result<Vec<Bytes>, crate::ErrorCode>> {
    let first = buf.iter().find(|b| !b.is_ascii_whitespace())?;
    if *first != b'[' {
        return None
    }
    let Ok(entries) = serde_json::from_slice::<Vec<&RawValue>>(buf) else {
        return Some(Err(crate::ErrorCode::ParseError))
    };
    if entries.is_empty() {
        return Some(Err(crate::ErrorCode::InvalidRequest))
    }
    Some(Ok(entries.into_iter().map(|e| buf.slice_ref(e.get().as_bytes())).collect()))
}

/// Parses a single JSON-RPC message, borrowing strings from `buf`.
pub(crate) fn parse_message(buf: &Bytes) -> Message {
    match serde_json::from_slice::<RawRequest<'_>>(buf) {
        Ok(req) => {
            if req.jsonrpc != "2.0" {
                let id = req.id.and_then(|id| parse_id(buf, id)).unwrap_or(Id::Null);
                return Message::Invalid(id, crate::ErrorCode::InvalidRequest)
            }
            let Some(id) = req.id else { return Message::Notification };
            let Some(id) = parse_id(buf, id) else {
                return Message::Invalid(Id::Null, crate::ErrorCode::InvalidRequest)
            };
            let params = req.params.map(|p| ByteStr::slice_ref(buf, p.get()));
            Message::Call(Request {
                id,
                method: ByteStr::from_cow(buf, req.method),
                params: Params::new(params),
            })
        }
        Err(_) => match serde_json::from_slice::<RawId<'_>>(buf) {
            Ok(raw) => Message::Invalid(
                raw.id.and_then(|id| parse_id(buf, id)).unwrap_or(Id::Null),
                crate::ErrorCode::InvalidRequest,
            ),
            Err(_) if serde_json::from_slice::<IgnoredAny>(buf).is_ok() => {
                Message::Invalid(Id::Null, crate::ErrorCode::InvalidRequest)
            }
            Err(_) => Message::Invalid(Id::Null, crate::ErrorCode::ParseError),
        },
    }
}

fn parse_id(buf: &Bytes, raw: &RawValue) -> Option<Id> {
    let s = raw.get();
    match s.as_bytes().first()? {
        b'n' if s == "null" => Some(Id::Null),
        b'"' => {
            let inner = &s[1..s.len() - 1];
            if inner.contains('\\') {
                serde_json::from_str::<String>(s).ok().map(|s| Id::Str(s.into()))
            } else {
                Some(Id::Str(ByteStr::slice_ref(buf, inner)))
            }
        }
        _ => s.parse().ok().map(Id::Number),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;

    fn parse(s: &'static str) -> Message {
        parse_message(&Bytes::from_static(s.as_bytes()))
    }

    #[test]
    fn parse_call() {
        let Message::Call(req) =
            parse(r#"{"jsonrpc":"2.0","id":1,"method":"eth_call","params":[1, "a"]}"#)
        else {
            panic!()
        };
        assert_eq!(req.id, Id::Number(1));
        assert_eq!(req.method, "eth_call");
        assert_eq!(req.params.as_str(), Some(r#"[1, "a"]"#));

        let Message::Call(req) = parse(r#"{"jsonrpc":"2.0","id":"a\"b","method":"mA"}"#) else {
            panic!()
        };
        assert_eq!(req.id, Id::Str("a\"b".into()));
        assert_eq!(req.method, "mA");
        assert!(req.params.is_none());

        let Message::Call(req) = parse(r#"{"jsonrpc":"2.0","id":null,"method":"m"}"#) else {
            panic!()
        };
        assert_eq!(req.id, Id::Null);
    }

    #[test]
    fn parse_invalid() {
        assert!(matches!(parse(r#"{"jsonrpc":"2.0","method":"m"}"#), Message::Notification));
        assert!(matches!(
            parse(r#"{"jsonrpc":"2.0","id":5,"method":1}"#),
            Message::Invalid(Id::Number(5), ErrorCode::InvalidRequest)
        ));
        assert!(matches!(
            parse(r#"{"jsonrpc":"1.0","id":5,"method":"m"}"#),
            Message::Invalid(Id::Number(5), ErrorCode::InvalidRequest)
        ));
        assert!(matches!(parse("{"), Message::Invalid(Id::Null, ErrorCode::ParseError)));
        assert!(matches!(parse("1"), Message::Invalid(Id::Null, ErrorCode::InvalidRequest)));
    }

    #[test]
    fn split() {
        let buf = Bytes::from_static(br#" [{"a":1}, 2]"#);
        let entries = split_batch(&buf).unwrap().unwrap();
        assert_eq!(entries, [&br#"{"a":1}"#[..], b"2"]);
        assert!(split_batch(&Bytes::from_static(b"{}")).is_none());
        assert_eq!(split_batch(&Bytes::from_static(b"[]")), Some(Err(ErrorCode::InvalidRequest)));
        assert_eq!(split_batch(&Bytes::from_static(b"[")), Some(Err(ErrorCode::ParseError)));
    }
}
