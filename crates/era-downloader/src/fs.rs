use crate::EraMeta;
use alloy_primitives::{hex, hex::ToHexExt};
use eyre::{eyre, OptionExt};
use futures_util::{stream, Stream};
use reth_fs_util as fs;
use sha2::{Digest, Sha256};
use std::{fmt::Debug, io, io::BufRead, path::Path, str::FromStr};

/// Creates a new ordered asynchronous [`Stream`] of ERA1 files read from `dir`.
pub fn read_dir(
    dir: impl AsRef<Path> + Send + Sync + 'static,
) -> eyre::Result<impl Stream<Item = eyre::Result<EraLocalMeta>> + Send + Sync + 'static + Unpin> {
    let mut checksums = None;
    let mut entries = fs::read_dir(dir)?
        .filter_map(|entry| {
            (|| {
                let path = entry?.path();

                if path.extension() == Some("era1".as_ref()) {
                    if let Some(last) = path.components().next_back() {
                        let str = last.as_os_str().to_string_lossy().to_string();
                        let parts = str.split('-').collect::<Vec<_>>();

                        if parts.len() == 3 {
                            let number = usize::from_str(parts[1])?;

                            return Ok(Some((number, path.into_boxed_path())));
                        }
                    }
                }
                if path.file_name() == Some("checksums.txt".as_ref()) {
                    let file = fs::open(path)?;
                    let reader = io::BufReader::new(file);
                    let lines = reader.lines();
                    checksums = Some(lines);
                }

                Ok(None)
            })()
            .transpose()
        })
        .collect::<eyre::Result<Vec<_>>>()?;
    let mut checksums = checksums.ok_or_eyre("Missing file `checksums.txt` in the `dir`")?;

    entries.sort_by(|(left, _), (right, _)| left.cmp(right));

    Ok(stream::iter(entries.into_iter().map(move |(_, path)| {
        let expected_checksum =
            checksums.next().transpose()?.ok_or_eyre("Got less checksums than ERA files")?;
        let expected_checksum = hex::decode(expected_checksum)?;

        let mut hasher = Sha256::new();
        let mut reader = io::BufReader::new(fs::open(&path)?);

        io::copy(&mut reader, &mut hasher)?;
        let actual_checksum = hasher.finalize().to_vec();

        if actual_checksum != expected_checksum {
            return Err(eyre!(
                "Checksum mismatch, got: {}, expected: {}",
                actual_checksum.encode_hex(),
                expected_checksum.encode_hex()
            ));
        }

        Ok(EraLocalMeta::new(path))
    })))
}

/// Contains information about an ERA file that is on the local file-system and is read-only.
#[derive(Debug)]
pub struct EraLocalMeta {
    path: Box<Path>,
}

impl EraLocalMeta {
    const fn new(path: Box<Path>) -> Self {
        Self { path }
    }
}

impl<T: AsRef<Path>> PartialEq<T> for EraLocalMeta {
    fn eq(&self, other: &T) -> bool {
        self.as_ref().eq(other.as_ref())
    }
}

impl AsRef<Path> for EraLocalMeta {
    fn as_ref(&self) -> &Path {
        self.path.as_ref()
    }
}

impl EraMeta for EraLocalMeta {
    /// A no-op.
    fn mark_as_processed(self) -> eyre::Result<()> {
        Ok(())
    }
}
