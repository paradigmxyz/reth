//! Client version model.

use reth_codecs::{add_arbitrary_tests, Compact};
use serde::{Deserialize, Serialize};

/// Wrapper around `String`
#[derive(Clone, Eq, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct CompactString(String);

/// Client version that accessed the database.
#[derive(Clone, Eq, PartialEq, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct ClientVersion {
    /// Client version
    pub version: CompactString,
    /// The git commit sha
    pub git_sha: CompactString,
    /// Build timestamp
    pub build_timestamp: CompactString,
}

impl ClientVersion {
    /// Returns `true` if no version fields are set.
    pub fn is_empty(&self) -> bool {
        self.version.0.is_empty() && self.git_sha.0.is_empty() && self.build_timestamp.0.is_empty()
    }
}

impl From<String> for CompactString {
    fn from(s: String) -> Self {
        CompactString(s)
    }
}

impl Compact for CompactString {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        self.0.as_bytes().to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (vec, buf) = Vec::<u8>::from_compact(buf, len);
        let string = String::from_utf8(vec).unwrap(); // Safe conversion
        (CompactString(string), buf)
    }
}

impl Compact for ClientVersion {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        self.version.to_compact(buf);
        self.git_sha.to_compact(buf);
        self.build_timestamp.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (version, buf) = CompactString::from_compact(buf, len);
        let (git_sha, buf) = CompactString::from_compact(buf, len);
        let (build_timestamp, buf) = CompactString::from_compact(buf, len);
        let client_version = Self { version, git_sha, build_timestamp };
        (client_version, buf)
    }
}
