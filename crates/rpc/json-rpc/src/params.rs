use crate::{invalid_params, ByteStr, ErrorObject};
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde_json::value::RawValue;
use std::{borrow::Cow, slice};

/// Raw JSON parameters of a request.
#[derive(Clone, Debug, Default)]
pub struct Params(Option<ByteStr>);

impl Params {
    /// Creates new parameters from raw JSON.
    pub fn new(raw: Option<ByteStr>) -> Self {
        Self(raw.filter(|raw| !raw.trim().is_empty()))
    }

    /// Returns the raw JSON, if any.
    pub fn as_str(&self) -> Option<&str> {
        self.0.as_deref()
    }

    /// Returns `true` if no parameters were provided.
    pub const fn is_none(&self) -> bool {
        self.0.is_none()
    }

    /// Parses the parameters as `T`, treating missing parameters as `null`.
    pub fn parse<'a, T: Deserialize<'a>>(&'a self) -> Result<T, ErrorObject> {
        serde_json::from_str(self.as_str().unwrap_or("null")).map_err(invalid_params)
    }

    /// Parses the only positional parameter as `T`.
    pub fn one<'a, T: Deserialize<'a>>(&'a self) -> Result<T, ErrorObject> {
        self.parse::<[T; 1]>().map(|[value]| value)
    }

    /// Returns an iterator-like parser over positional parameters.
    pub fn sequence(&self) -> ParamsSequence<'_> {
        ParamsSequence(Seq::Array(self.as_str().map(str::trim_start).unwrap_or("")))
    }

    /// Returns an iterator-like parser over parameters given either by position or by name.
    ///
    /// Each entry of `names` lists the keys accepted for the parameter at that position when the
    /// parameters are a JSON object. Unknown keys are ignored.
    pub fn sequence_named<'a>(
        &'a self,
        names: &'a [&'a [&'a str]],
    ) -> Result<ParamsSequence<'a>, ErrorObject> {
        match self.as_str().map(str::trim_start) {
            Some(json) if json.starts_with('{') => {
                let values = serde_json::from_str(json).map_err(invalid_params)?;
                Ok(ParamsSequence(Seq::Object { values, names: names.iter() }))
            }
            _ => Ok(self.sequence()),
        }
    }
}

/// Parameter parser created by [`Params::sequence`] or [`Params::sequence_named`].
#[derive(Clone, Debug)]
pub struct ParamsSequence<'a>(Seq<'a>);

#[derive(Clone, Debug)]
enum Seq<'a> {
    Array(&'a str),
    Object { values: FxHashMap<Cow<'a, str>, &'a RawValue>, names: slice::Iter<'a, &'a [&'a str]> },
}

impl<'a> ParamsSequence<'a> {
    /// Parses the next parameter, returning `None` if it is missing.
    fn next_inner<T: Deserialize<'a>>(&mut self) -> Result<Option<T>, ErrorObject> {
        let json = match &mut self.0 {
            Seq::Array(json) => json,
            Seq::Object { values, names } => {
                let Some(raw) =
                    names.next().and_then(|keys| keys.iter().find_map(|k| values.get(*k)))
                else {
                    return Ok(None)
                };
                return serde_json::from_str(raw.get()).map(Some).map_err(invalid_params)
            }
        };
        let mut rest = *json;
        match rest.as_bytes().first() {
            None => return Ok(None),
            Some(b']') => {
                *json = "";
                return Ok(None)
            }
            Some(b'[') if rest[1..].trim_start().starts_with(']') => {
                *json = "";
                return Ok(None)
            }
            Some(b'[' | b',') => rest = &rest[1..],
            Some(_) => {
                *json = "";
                return Err(invalid_params(format_args!(
                    "Expected one of '[', ']' or ',' but found {rest:?}"
                )));
            }
        }

        let mut iter = serde_json::Deserializer::from_str(rest).into_iter::<T>();
        match iter.next() {
            None => {
                *json = "";
                Ok(None)
            }
            Some(Ok(value)) => {
                *json = rest[iter.byte_offset()..].trim_start();
                Ok(Some(value))
            }
            Some(Err(err)) => {
                *json = "";
                Err(invalid_params(err))
            }
        }
    }

    /// Parses the next parameter.
    #[expect(clippy::should_implement_trait)]
    pub fn next<T: Deserialize<'a>>(&mut self) -> Result<T, ErrorObject> {
        let name = match &self.0 {
            Seq::Array(_) => None,
            Seq::Object { names, .. } => names.as_slice().first().and_then(|keys| keys.first()),
        };
        self.next_inner()?.ok_or_else(|| match name {
            Some(name) => invalid_params(format_args!("Missing param {name:?}")),
            None => invalid_params("No more params"),
        })
    }

    /// Parses the next optional parameter, returning `None` for `null` or missing values.
    pub fn optional_next<T: Deserialize<'a>>(&mut self) -> Result<Option<T>, ErrorObject> {
        self.next_inner::<Option<T>>().map(Option::flatten)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(s: &'static str) -> Params {
        Params::new(Some(s.into()))
    }

    #[test]
    fn sequence() {
        let p = params(r#" [true, 10, "foo"] "#);
        let mut seq = p.sequence();
        assert!(seq.next::<bool>().unwrap());
        assert_eq!(seq.next::<i32>().unwrap(), 10);
        assert_eq!(seq.next::<&str>().unwrap(), "foo");
        assert!(seq.next::<u8>().is_err());

        let p = params("[1, 2, null]");
        let mut seq = p.sequence();
        let parsed: [Option<u32>; 4] = std::array::from_fn(|_| seq.optional_next().unwrap());
        assert_eq!(parsed, [Some(1), Some(2), None, None]);

        for empty in ["[]", "[ ]"] {
            let p = params(empty);
            assert_eq!(p.sequence().optional_next::<u8>().unwrap(), None);
            assert!(p.sequence().next::<u8>().is_err());
        }

        let p = Params::new(None);
        assert_eq!(p.sequence().optional_next::<u8>().unwrap(), None);
        assert!(p.sequence().next::<u8>().is_err());
    }

    #[test]
    fn sequence_named() {
        const NAMES: &[&[&str]] = &[&["block_number", "blockNumber"], &["full"], &["extra"]];

        let p = params(r#"{"blockNumber": 5, "full": true, "unknown": 1}"#);
        let mut seq = p.sequence_named(NAMES).unwrap();
        assert_eq!(seq.next::<u64>().unwrap(), 5);
        assert!(seq.next::<bool>().unwrap());
        assert_eq!(seq.optional_next::<u8>().unwrap(), None);
        assert!(seq.next::<u8>().is_err());

        let p = params(r#"{"full": true}"#);
        let mut seq = p.sequence_named(NAMES).unwrap();
        assert_eq!(
            seq.next::<u64>().unwrap_err().data().unwrap().get(),
            r#""Missing param \"block_number\"""#
        );

        let p = params("[5, true]");
        let mut seq = p.sequence_named(NAMES).unwrap();
        assert_eq!(seq.next::<u64>().unwrap(), 5);
        assert!(seq.next::<bool>().unwrap());

        assert!(params(r#"{"full": tru}"#).sequence_named(NAMES).is_err());
    }

    #[test]
    fn one_and_parse() {
        assert_eq!(params("[5]").one::<u8>().unwrap(), 5);
        assert!(params("[5, 6]").one::<u8>().is_err());
        assert_eq!(params("[5, 6]").parse::<Vec<u8>>().unwrap(), [5, 6]);
        assert_eq!(Params::new(None).parse::<Option<u8>>().unwrap(), None);
        assert_eq!(
            params("[true]").sequence().next::<u8>().unwrap_err().code(),
            crate::INVALID_PARAMS_CODE
        );
    }
}
