//! This module contains a type safe representation of a torrent's metainfo, as
//! well as utilities to construct it.

use crate::{info::storage::FileInfo, sha1::Sha1Hash};
use reqwest::Url;
pub use serde_bencode::Error as BencodeError;
use std::{
    fmt,
    path::{Path, PathBuf},
};
use tracing::warn;

pub(crate) type Result<T> = std::result::Result<T, MetainfoError>;

#[derive(Debug, thiserror::Error)]
pub enum MetainfoError {
    /// Holds bencode serialization or deserialization related errors.
    #[error(transparent)]
    Bencode(#[from] BencodeError),
    /// The torrent metainfo is not valid.
    #[error("Invalid torrent meta info")]
    InvalidMetainfo,
    /// The chain of piece hashes in the torrent metainfo file was not
    /// a multiple of 20, or is otherwise invalid and thus the torrent could not
    /// be started.
    #[error("Invalid torrent pieces")]
    InvalidPieces,
    /// The tracker URL is not a valid URL.
    #[error("Invalid tracker url: {0:?}")]
    InvalidTrackerUrl(#[from] url::ParseError),
}

/// The parsed and validated torrent metainfo file, containing necessary
/// arguments for starting a torrent.
#[derive(Clone)]
pub struct Metainfo {
    /// The name of the torrent, which is usually used to form the download
    /// path.
    pub name: String,
    /// This hash is used to identify a torrent with trackers and peers.
    pub info_hash: Sha1Hash,
    /// The concatenation of the 20 byte SHA-1 hash of each piece in torrent.
    /// This is used to verify the data sent to us by peers.
    pub pieces: Vec<u8>,
    /// The nominal lengths of a piece, that is, the length of all but
    /// potentially the last piece, which may be smaller.
    pub piece_len: u32,
    /// The paths and lenths of the files in torrent.
    pub files: Vec<FileInfo>,
    /// The trackers that we can announce to.
    /// The tier information is not currently present in this field as
    /// cratetorrent doesn't use it. In the future it may be added.
    pub trackers: Vec<Url>,
}

impl Metainfo {
    /// Parses from a byte buffer a new [`Metainfo`] instance, or aborts with an
    /// error.
    ///
    /// If the encoding itself is correct, the constructor may still fail if the
    /// metadata is not semantically correct (e.g. if the length of the `pieces`
    /// field is not a multiple of 20, or no valid files are encoded, etc).
    pub fn parse_bytes(buf: &[u8]) -> Result<Self> {
        let metainfo: raw::Metainfo = serde_bencode::from_bytes(buf)?;

        // the pieces field is a concatenation of 20 byte SHA-1 hashes, so it
        // must be a multiple of 20
        if metainfo.info.pieces.len() % 20 != 0 {
            return Err(MetainfoError::InvalidPieces)
        }

        // verify download structure and build up files metadata
        let mut files = Vec::new();
        if let Some(len) = metainfo.info.len {
            if metainfo.info.files.is_some() {
                warn!("Metainfo cannot contain both `length` and `files`");
                return Err(MetainfoError::InvalidMetainfo)
            }
            if len == 0 {
                warn!("File length is 0");
                return Err(MetainfoError::InvalidMetainfo)
            }

            // the path of this file is just the torrent name
            files.push(FileInfo {
                path: metainfo.info.name.clone().into(),
                len,
                torrent_offset: 0,
            });
        } else if let Some(raw_files) = &metainfo.info.files {
            if raw_files.is_empty() {
                warn!("Metainfo files must not be empty");
                return Err(MetainfoError::InvalidMetainfo)
            }

            // and sum up the file offsets in the torrent
            let mut torrent_offset = 0;
            for file in raw_files.iter() {
                // verify that the file length is non-zero
                if file.len == 0 {
                    warn!("File {:?} length is 0", file.path);
                    return Err(MetainfoError::InvalidMetainfo)
                }

                // verify that the path is not empty
                let path: PathBuf = file.path.iter().collect();
                if path == PathBuf::new() {
                    warn!("Path in metainfo is empty");
                    return Err(MetainfoError::InvalidMetainfo)
                }

                // verify that the path is not absolute
                if path.is_absolute() {
                    warn!("Path {:?} is absolute", path);
                    return Err(MetainfoError::InvalidMetainfo)
                }

                // verify that the path is not the root
                if path == Path::new("/") {
                    warn!("Path {:?} is root", path);
                    return Err(MetainfoError::InvalidMetainfo)
                }

                // file is now verified, we can collect it
                files.push(FileInfo { path, torrent_offset, len: file.len });

                // advance offset for next file
                torrent_offset += file.len;
            }
        } else {
            warn!("No `length` or `files` key present in metainfo");
            return Err(MetainfoError::InvalidMetainfo)
        }

        let mut trackers = Vec::with_capacity(metainfo.announce_list.len());
        if !metainfo.announce_list.is_empty() {
            for tier in metainfo.announce_list.iter() {
                for tracker in tier.iter() {
                    let url = Url::parse(tracker)?;
                    // the tracker may be over UDP, which we don't support (yet)
                    if url.scheme() == "http" || url.scheme() == "https" {
                        trackers.push(url);
                    }
                }
            }
        } else if let Some(tracker) = &metainfo.announce {
            let url = Url::parse(tracker)?;
            if url.scheme() == "http" || url.scheme() == "https" {
                trackers.push(url);
            }
        }

        // create info hash as a last step
        let info_hash = metainfo.info_hash()?;

        Ok(Self {
            name: metainfo.info.name,
            info_hash,
            pieces: metainfo.info.pieces,
            piece_len: metainfo.info.piece_len,
            files,
            trackers,
        })
    }

    /// Returns true if the download is for an archive.
    pub fn is_archive(&self) -> bool {
        self.files.len() > 1
    }

    /// Returns the total download size in bytes.
    ///
    /// Note that this is an O(n) operation for archive downloads, where n is
    /// the number of files, so the return value should ideally be cached.
    pub fn download_len(&self) -> u64 {
        self.files.iter().map(|f| f.len).sum()
    }

    /// Returns the number of pieces in this torrent.
    pub fn piece_count(&self) -> usize {
        self.pieces.len() / 20
    }
}

impl fmt::Debug for Metainfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Metainfo")
            .field("name", &self.name)
            .field("info_hash", &self.info_hash)
            .field("pieces", &"<pieces...>")
            .field("piece_len", &self.piece_len)
            .field("structure", &self.files)
            .finish()
    }
}

mod raw {
    //! Contains the types that we directly deserialize into, but is not to be
    //! used by the rest of the crate, as the validity of the parsed structure
    //! is not ensured at this level. The semantic validation happens in the
    //! [`super::Metainfo`] type, which is essentially a mapping of
    //! [`Metainfo`], but with semantic requirements encoded in the type
    //! system.
    use super::{Result, Sha1Hash};
    use serde::{Deserialize, Serialize};
    use sha1::Digest;

    #[derive(Deserialize)]
    pub(crate) struct Metainfo {
        pub(crate) info: Info,
        pub(crate) announce: Option<String>,
        #[serde(default)]
        #[serde(rename = "announce-list")]
        pub(crate) announce_list: Vec<Vec<String>>,
    }

    impl Metainfo {
        /// Creates a SHA-1 hash of the encoded `info` field's value.
        pub(crate) fn info_hash(&self) -> Result<Sha1Hash> {
            let info = serde_bencode::to_bytes(&self.info)?;
            Ok(Sha1Hash::digest(&info))
        }
    }

    #[derive(Serialize, Deserialize)]
    pub(crate) struct Info {
        pub(crate) name: String,
        #[serde(with = "serde_bytes")]
        pub(crate) pieces: Vec<u8>,
        #[serde(rename = "piece length")]
        pub(crate) piece_len: u32,
        #[serde(rename = "length")]
        pub(crate) len: Option<u64>,
        pub(crate) files: Option<Vec<File>>,
        /// This is not currently used but needs to be kept in here so that we
        /// can encode back a valid info hash for hashing.
        pub(crate) private: Option<u8>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub(crate) struct File {
        pub(crate) path: Vec<String>,
        #[serde(rename = "length")]
        pub(crate) len: u64,
    }
}
