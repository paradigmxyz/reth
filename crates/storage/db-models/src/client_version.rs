//! Client version model.

use alloc::string::String;

/// Client version that accessed the database.
#[derive(Clone, Eq, PartialEq, Debug, Default)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClientVersion {
    /// Client version
    pub version: String,
    /// The git commit sha
    pub git_sha: String,
    /// Build timestamp
    pub build_timestamp: String,
}

impl ClientVersion {
    /// Returns `true` if no version fields are set.
    pub fn is_empty(&self) -> bool {
        self.version.is_empty() && self.git_sha.is_empty() && self.build_timestamp.is_empty()
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for ClientVersion {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        self.version.to_compact(buf);
        self.git_sha.to_compact(buf);
        self.build_timestamp.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (version, buf) = String::from_compact(buf, len);
        let (git_sha, buf) = String::from_compact(buf, len);
        let (build_timestamp, buf) = String::from_compact(buf, len);
        let client_version = Self { version, git_sha, build_timestamp };
        (client_version, buf)
    }
}
