//! Sha1 support

use sha1::{Digest, Sha1};
use std::{
    convert::{TryFrom, TryInto},
    fmt, fs,
    io::{self, BufReader, Error},
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::AsyncWrite;

/// Length of a SHA-1 hash.
pub const SHA_HASH_LEN: usize = 20;

/// Peers are identified by a hash.
pub(crate) type PeerId = ShaHash;

/// SHA-1 hash wrapper type for performing operations on the hash.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ShaHash {
    hash: [u8; SHA_HASH_LEN],
}

impl ShaHash {
    /// Create a ShaHash by hashing the given bytes.
    #[inline]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self { hash: Sha1::digest(bytes).as_slice().try_into().unwrap() }
    }

    /// Create a random Sha1 hash.
    pub fn random() -> Self {
        use rand::distributions::Distribution;
        let mut rng = rand::rngs::OsRng;
        Self { hash: rand::distributions::Standard.sample(&mut rng) }
    }

    /// Returns the array that holds the hash.
    #[inline]
    pub fn bytes(&self) -> &[u8; 20] {
        &self.hash
    }

    /// Returns the `SHA-1` hash of the entire file.
    pub fn for_file_sync(file: &Path) -> io::Result<Self> {
        let mut file = BufReader::new(fs::File::open(file)?);
        let mut hasher = Sha1::new();
        io::copy(&mut file, &mut hasher)?;
        Ok(hasher.finalize().as_slice().try_into().unwrap())
    }

    /// Returns the `SHA-1` hash of the entire file asynchronously.
    pub async fn for_file(file: &Path) -> io::Result<Self> {
        let mut file = tokio::io::BufReader::new(tokio::fs::File::open(file).await?);
        let mut hasher = Sha1::new();
        let mut w = AsyncWriteSha1(&mut hasher);
        tokio::io::copy(&mut file, &mut w).await?;
        Ok(hasher.finalize().as_slice().try_into().unwrap())
    }

    /// The length of the hash
    #[inline]
    pub const fn len() -> usize {
        SHA_HASH_LEN
    }
}

/// Helper to support async write into SHA-1 digest.
///
/// This always updates the hasher as soon as new data is written. And consumes the entire data.
struct AsyncWriteSha1<'a>(&'a mut Sha1);

impl<'a> AsyncWrite for AsyncWriteSha1<'a> {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        self.get_mut().0.update(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }
}

impl AsRef<[u8]> for ShaHash {
    fn as_ref(&self) -> &[u8] {
        &self.hash
    }
}

impl From<ShaHash> for [u8; SHA_HASH_LEN] {
    fn from(val: ShaHash) -> [u8; SHA_HASH_LEN] {
        val.hash
    }
}

impl From<[u8; SHA_HASH_LEN]> for ShaHash {
    fn from(sha_hash: [u8; SHA_HASH_LEN]) -> ShaHash {
        ShaHash { hash: sha_hash }
    }
}

impl TryFrom<&[u8]> for ShaHash {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let data = value;
        if data.len() < SHA_HASH_LEN {
            Err(())
        } else {
            let hash: [u8; SHA_HASH_LEN] = data[..SHA_HASH_LEN].try_into().map_err(|_| ())?;

            Ok(Self { hash })
        }
    }
}

impl PartialEq<[u8]> for ShaHash {
    fn eq(&self, other: &[u8]) -> bool {
        other == &self.hash[..]
    }
}

impl fmt::Display for ShaHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for i in self.hash.iter() {
            write!(f, "{:08x}", i)?
        }
        Ok(())
    }
}
