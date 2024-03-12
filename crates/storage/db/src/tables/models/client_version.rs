//! Client version model.

use reth_codecs::{derive_arbitrary, Compact};
use serde::{Deserialize, Serialize};

/// Client version that accessed the database.
#[derive_arbitrary(compact)]
#[derive(Clone, Eq, PartialEq, Debug, Default, Serialize, Deserialize)]
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

impl Compact for ClientVersion {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let Self { version, git_sha, build_timestamp } = self;
        version.into_bytes().to_compact(buf);
        git_sha.into_bytes().to_compact(buf);
        build_timestamp.into_bytes().to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (version, buf) = Vec::<u8>::from_compact(buf, len);
        let (git_sha, buf) = Vec::<u8>::from_compact(buf, len);
        let (build_timestamp, buf) = Vec::<u8>::from_compact(buf, len);
        let client_version = Self {
            version: unsafe { String::from_utf8_unchecked(version) },
            git_sha: unsafe { String::from_utf8_unchecked(git_sha) },
            build_timestamp: unsafe { String::from_utf8_unchecked(build_timestamp) },
        };
        (client_version, buf)
    }
}
