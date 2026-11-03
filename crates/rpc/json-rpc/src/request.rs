use crate::{ErrorCode, Id, Params};
use bytes::Bytes;
use http::Extensions;
use serde::{
    de::{IgnoredAny, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
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
    /// Data attached by the transport and middleware, such as the
    /// [`ConnectionId`](crate::ConnectionId).
    pub extensions: Extensions,
}

impl Request {
    /// Creates a new request.
    pub fn new(method: impl Into<ByteStr>, params: Params, id: Id) -> Self {
        Self { id, method: method.into(), params, extensions: Extensions::new() }
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

    /// Returns the extensions.
    pub const fn extensions(&self) -> &Extensions {
        &self.extensions
    }

    /// Returns the extensions mutably.
    pub const fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.extensions
    }
}

/// A JSON-RPC notification, which is a call without an id that gets no response.
#[derive(Clone, Debug)]
pub struct Notification {
    /// Method name.
    pub method: ByteStr,
    /// Method parameters.
    pub params: Params,
    /// Data attached by the transport and middleware.
    pub extensions: Extensions,
}

impl Notification {
    /// Creates a new notification.
    pub fn new(method: impl Into<ByteStr>, params: Params) -> Self {
        Self { method: method.into(), params, extensions: Extensions::new() }
    }

    /// Returns the method name.
    pub fn method_name(&self) -> &str {
        &self.method
    }

    /// Returns the parameters.
    pub const fn params(&self) -> &Params {
        &self.params
    }

    /// Returns the extensions.
    pub const fn extensions(&self) -> &Extensions {
        &self.extensions
    }

    /// Returns the extensions mutably.
    pub const fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.extensions
    }
}

/// A parsed JSON-RPC message.
#[derive(Debug)]
pub(crate) enum Message {
    /// A method call.
    Call(Request),
    /// A notification, which gets no response.
    Notification(Notification),
    /// An invalid request, answered with this error and id.
    Invalid(Id, ErrorCode),
}

/// A single message or a batch, borrowing from the received buffer.
#[derive(Debug)]
pub(crate) enum Parsed<'a> {
    /// A single message.
    Single(Entry<'a>),
    /// A non-empty batch.
    Batch(Vec<Entry<'a>>),
}

/// Parses a message or batch in one pass over `buf`.
///
/// Returns a parse error if `buf` is not UTF-8 JSON, and an invalid request error for an empty
/// batch.
pub(crate) fn parse(buf: &Bytes) -> Result<Parsed<'_>, ErrorCode> {
    let json = std::str::from_utf8(buf).map_err(|_| ErrorCode::ParseError)?;
    if json.trim_ascii_start().starts_with('[') {
        let entries =
            serde_json::from_str::<Vec<Entry<'_>>>(json).map_err(|_| ErrorCode::ParseError)?;
        if entries.is_empty() {
            return Err(ErrorCode::InvalidRequest)
        }
        Ok(Parsed::Batch(entries))
    } else {
        serde_json::from_str(json).map(Parsed::Single).map_err(|_| ErrorCode::ParseError)
    }
}

/// The members of a JSON-RPC request object, which are validated by [`Entry::into_message`].
///
/// Deserializing only fails on invalid JSON, so a batch is parsed in one pass even if some of its
/// entries are invalid requests.
#[derive(Debug, Default)]
pub(crate) struct Entry<'a> {
    jsonrpc: Option<&'a RawValue>,
    id: Option<&'a RawValue>,
    method: Option<&'a RawValue>,
    params: Option<&'a RawValue>,
    /// Whether the entry is not an object or repeats a member.
    invalid: bool,
}

impl<'a> Entry<'a> {
    /// Validates the entry, returning its method and params as slices of `buf`.
    pub(crate) fn into_message(self, buf: &Bytes, extensions: Extensions) -> Message {
        let id = match self.id.map(|id| parse_id(buf, id)) {
            None => None,
            Some(Some(id)) => Some(id),
            Some(None) => return Message::Invalid(Id::Null, ErrorCode::InvalidRequest),
        };
        let method = self.method.and_then(|method| parse_str(buf, method));
        let Some(method) = method.filter(|_| !self.invalid && self.jsonrpc.is_some_and(is_v2))
        else {
            return Message::Invalid(id.unwrap_or_default(), ErrorCode::InvalidRequest)
        };
        let params = self.params.filter(|params| params.get() != "null");
        let params = Params::new(params.map(|params| ByteStr::slice_ref(buf, params.get())));
        match id {
            Some(id) => Message::Call(Request { id, method, params, extensions }),
            None => Message::Notification(Notification { method, params, extensions }),
        }
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Entry<'a> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(EntryVisitor)
    }
}

struct EntryVisitor;

impl<'de> Visitor<'de> for EntryVisitor {
    type Value = Entry<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON-RPC request")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entry<'de>, A::Error> {
        let mut entry = Entry::default();
        while let Some(key) = map.next_key::<Cow<'de, str>>()? {
            let slot = match &*key {
                "jsonrpc" => &mut entry.jsonrpc,
                "id" => &mut entry.id,
                "method" => &mut entry.method,
                "params" => &mut entry.params,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                    continue
                }
            };
            entry.invalid |= slot.replace(map.next_value()?).is_some();
        }
        Ok(entry)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Entry<'de>, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Entry { invalid: true, ..Default::default() })
    }

    fn visit_bool<E>(self, _: bool) -> Result<Entry<'de>, E> {
        Ok(Entry { invalid: true, ..Default::default() })
    }

    fn visit_i64<E>(self, _: i64) -> Result<Entry<'de>, E> {
        Ok(Entry { invalid: true, ..Default::default() })
    }

    fn visit_u64<E>(self, _: u64) -> Result<Entry<'de>, E> {
        Ok(Entry { invalid: true, ..Default::default() })
    }

    fn visit_f64<E>(self, _: f64) -> Result<Entry<'de>, E> {
        Ok(Entry { invalid: true, ..Default::default() })
    }

    fn visit_str<E>(self, _: &str) -> Result<Entry<'de>, E> {
        Ok(Entry { invalid: true, ..Default::default() })
    }

    fn visit_unit<E>(self) -> Result<Entry<'de>, E> {
        Ok(Entry { invalid: true, ..Default::default() })
    }
}

/// Returns a JSON string as a slice of `buf`, or unescaped if it contains escapes.
fn parse_str(buf: &Bytes, raw: &RawValue) -> Option<ByteStr> {
    let s = raw.get();
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    if inner.contains('\\') {
        serde_json::from_str::<String>(s).ok().map(Into::into)
    } else {
        Some(ByteStr::slice_ref(buf, inner))
    }
}

fn is_v2(raw: &RawValue) -> bool {
    raw.get() == r#""2.0""# || serde_json::from_str::<String>(raw.get()).is_ok_and(|v| v == "2.0")
}

fn parse_id(buf: &Bytes, raw: &RawValue) -> Option<Id> {
    let s = raw.get();
    match s.as_bytes().first()? {
        b'n' if s == "null" => Some(Id::Null),
        b'"' => parse_str(buf, raw).map(Id::Str),
        _ => s.parse().ok().map(Id::Number),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;

    fn parse_one(s: &'static str) -> Message {
        let buf = Bytes::from_static(s.as_bytes());
        match parse(&buf) {
            Ok(Parsed::Single(entry)) => entry.into_message(&buf, Extensions::new()),
            Ok(Parsed::Batch(_)) => panic!("unexpected batch"),
            Err(code) => Message::Invalid(Id::Null, code),
        }
    }

    #[test]
    fn parse_call() {
        let Message::Call(req) =
            parse_one(r#"{"jsonrpc":"2.0","id":1,"method":"eth_call","params":[1, "a"]}"#)
        else {
            panic!()
        };
        assert_eq!(req.id, Id::Number(1));
        assert_eq!(req.method, "eth_call");
        assert_eq!(req.params.as_str(), Some(r#"[1, "a"]"#));

        let Message::Call(req) = parse_one(r#"{"jsonrpc":"2.0","id":"a\"b","method":"mA"}"#) else {
            panic!()
        };
        assert_eq!(req.id, Id::Str("a\"b".into()));
        assert_eq!(req.method, "mA");
        assert!(req.params.is_none());

        let Message::Call(req) = parse_one(r#"{"jsonrpc":"2.0","id":null,"method":"m"}"#) else {
            panic!()
        };
        assert_eq!(req.id, Id::Null);
    }

    #[test]
    fn parse_invalid() {
        assert!(matches!(parse_one(r#"{"jsonrpc":"2.0","method":"m"}"#), Message::Notification(_)));
        assert!(matches!(
            parse_one(r#"{"jsonrpc":"2.0","id":5,"method":1}"#),
            Message::Invalid(Id::Number(5), ErrorCode::InvalidRequest)
        ));
        assert!(matches!(
            parse_one(r#"{"jsonrpc":"1.0","id":5,"method":"m"}"#),
            Message::Invalid(Id::Number(5), ErrorCode::InvalidRequest)
        ));
        assert!(matches!(parse_one("{"), Message::Invalid(Id::Null, ErrorCode::ParseError)));
        assert!(matches!(parse_one("1"), Message::Invalid(Id::Null, ErrorCode::InvalidRequest)));
    }

    #[test]
    fn batch() {
        let buf = Bytes::from_static(
            br#" [{"a":1}, 2, ["2.0", 1, "m"], {"jsonrpc":"2.0","id":1,"method":"m"}]"#,
        );
        let Ok(Parsed::Batch(entries)) = parse(&buf) else { panic!() };
        let messages = entries
            .into_iter()
            .map(|entry| entry.into_message(&buf, Extensions::new()))
            .collect::<Vec<_>>();
        assert!(matches!(
            messages[..],
            [
                Message::Invalid(Id::Null, ErrorCode::InvalidRequest),
                Message::Invalid(Id::Null, ErrorCode::InvalidRequest),
                Message::Invalid(Id::Null, ErrorCode::InvalidRequest),
                Message::Call(_)
            ]
        ));
        assert!(matches!(parse(&Bytes::from_static(b"{}")), Ok(Parsed::Single(_))));
        assert_eq!(parse(&Bytes::from_static(b"[]")).unwrap_err(), ErrorCode::InvalidRequest);
        assert_eq!(parse(&Bytes::from_static(b"[")).unwrap_err(), ErrorCode::ParseError);
        assert_eq!(parse(&Bytes::from_static(b"[{}] 1")).unwrap_err(), ErrorCode::ParseError);
    }

    #[test]
    fn members() {
        let Message::Call(req) =
            parse_one(r#"{"params":null,"x":[{}],"method":"e\u0074h","id":"1","jsonrpc":"2.0"}"#)
        else {
            panic!()
        };
        assert_eq!(req.id, Id::Str("1".into()));
        assert_eq!(req.method, "eth");
        assert!(req.params.is_none());

        assert!(matches!(
            parse_one(r#"{"jsonrpc":"2.0","id":1,"method":"m","method":"n"}"#),
            Message::Invalid(Id::Number(1), ErrorCode::InvalidRequest)
        ));
        assert!(matches!(
            parse_one(r#"{"jsonrpc":"2.0","id":{},"method":"m"}"#),
            Message::Invalid(Id::Null, ErrorCode::InvalidRequest)
        ));
        let buf = Bytes::from_static(b"{\"method\":\"\xff\"}");
        assert_eq!(parse(&buf).unwrap_err(), ErrorCode::ParseError);
    }
}
