use crate::{invalid_params, ByteStr, ErrorObject};
use serde::Deserialize;

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
        ParamsSequence(self.as_str().map(str::trim_start).unwrap_or(""))
    }
}

/// Positional parameter parser created by [`Params::sequence`].
#[derive(Clone, Copy, Debug)]
pub struct ParamsSequence<'a>(&'a str);

impl<'a> ParamsSequence<'a> {
    fn next_inner<T: Deserialize<'a>>(&mut self) -> Option<Result<T, ErrorObject>> {
        let mut json = self.0;
        match json.as_bytes().first()? {
            b']' => {
                self.0 = "";
                return None
            }
            b'[' | b',' => json = &json[1..],
            _ => {
                self.0 = "";
                return Some(Err(invalid_params(format_args!(
                    "Expected one of '[', ']' or ',' but found {json:?}"
                ))));
            }
        }

        let mut iter = serde_json::Deserializer::from_str(json).into_iter::<T>();
        match iter.next()? {
            Ok(value) => {
                self.0 = json[iter.byte_offset()..].trim_start();
                Some(Ok(value))
            }
            Err(err) => {
                self.0 = "";
                Some(Err(invalid_params(err)))
            }
        }
    }

    /// Parses the next parameter.
    #[expect(clippy::should_implement_trait)]
    pub fn next<T: Deserialize<'a>>(&mut self) -> Result<T, ErrorObject> {
        self.next_inner().unwrap_or_else(|| Err(invalid_params("No more params")))
    }

    /// Parses the next optional parameter, returning `None` for `null` or missing values.
    pub fn optional_next<T: Deserialize<'a>>(&mut self) -> Result<Option<T>, ErrorObject> {
        self.next_inner::<Option<T>>().unwrap_or(Ok(None))
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

        let p = Params::new(None);
        assert_eq!(p.sequence().optional_next::<u8>().unwrap(), None);
        assert!(p.sequence().next::<u8>().is_err());
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
