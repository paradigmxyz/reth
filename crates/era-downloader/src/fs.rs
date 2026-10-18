use crate::{EraMeta, BLOCKS_PER_FILE};
use alloy_primitives::{hex, hex::ToHexExt, BlockNumber};
use eyre::{eyre, OptionExt};
use futures_util::{stream, Stream, StreamExt};
use reth_era::common::file_ops::EraFileType;
use reth_fs_util as fs;
use sha2::{Digest, Sha256};
use std::{fmt::Debug, fs::DirEntry, io, io::BufRead, path::Path, str::FromStr};

/// Creates an ordered stream of checksum-verified execution archives read from `dir`.
///
/// Directory enumeration and checksum sidecar setup happen synchronously before returning. The
/// directory must contain `checksums.txt`, with one hexadecimal SHA-256 digest per archive index
/// starting at zero. After skipping `start_from`'s file index in both sequences, checksums are
/// paired positionally with sorted files. Selected files must therefore be contiguous and begin at
/// that index; missing files do not cause checksum lines to be skipped.
///
/// Poll this stream within a Tokio runtime. Each file is opened and hashed on a blocking worker,
/// one at a time, preserving order and backpressure. Missing runtime access, task failures, invalid
/// checksums and file access errors are yielded as stream errors.
pub fn read_dir(
    dir: impl AsRef<Path> + Send + Sync + 'static,
    start_from: BlockNumber,
) -> eyre::Result<impl Stream<Item = eyre::Result<EraLocalMeta>> + Send + Sync + 'static + Unpin> {
    let mut checksums = None;

    // read all the files in the given dir and also pick up the checksums file
    let entries = sorted_era_entries(
        dir,
        |ty| matches!(ty, EraFileType::Era1 | EraFileType::Ere),
        |path| {
            if path.file_name() == Some("checksums.txt".as_ref()) {
                let reader = io::BufReader::new(fs::open(path)?);
                checksums = Some(reader.lines());
            }
            Ok(())
        },
    )?;
    let mut checksums = checksums.ok_or_eyre("Missing file `checksums.txt` in the `dir`")?;

    let start_index = start_from as usize / BLOCKS_PER_FILE;
    for _ in 0..start_index {
        // skip the first entries in the checksums iterator so that both iters align
        checksums.next().transpose()?.ok_or_eyre("Got less checksums than ERA files")?;
    }

    Ok(Box::pin(stream::iter(entries.into_iter().skip_while(move |(n, _)| *n < start_index)).then(
        move |(_, path)| {
            let expected_checksum = checksums
                .next()
                .transpose()
                .map_err(eyre::Report::from)
                .and_then(|checksum| checksum.ok_or_eyre("Got less checksums than ERA files"));
            async move {
                let expected_checksum = expected_checksum?;
                let runtime = tokio::runtime::Handle::try_current()?;
                runtime
                    .spawn_blocking(move || {
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
                    })
                    .await?
            }
        },
    )))
}

/// Creates a new ordered asynchronous [`Stream`] of consensus `.era` files read from `dir`.
///
/// Unlike [`read_dir`], consensus `.era` files ship no `checksums.txt`, and their filenames encode
/// an era (slot) number rather than a block number. Files are streamed in ascending era order; the
/// import pipeline filters out blocks already present, so no block-level `start_from` skipping is
/// done here.
pub fn read_era_dir(
    dir: impl AsRef<Path> + Send + Sync + 'static,
) -> eyre::Result<impl Stream<Item = eyre::Result<EraLocalMeta>> + Send + Sync + 'static + Unpin> {
    let entries = sorted_era_entries(dir, |ty| ty == EraFileType::Era, |_| Ok(()))?;

    Ok(stream::iter(entries.into_iter().map(|(_, path)| Ok(EraLocalMeta::new(path)))))
}

/// Scans `dir` for ERA files whose type satisfies `accept`, returning them sorted by the number
/// parsed from the `<network>-<number>-...<ext>` filename.
///
/// Files that don't match `accept` are passed to `on_other`, letting callers pick up sidecar files
/// such as `checksums.txt`.
fn sorted_era_entries(
    dir: impl AsRef<Path>,
    accept: impl Fn(EraFileType) -> bool,
    mut on_other: impl FnMut(&Path) -> eyre::Result<()>,
) -> eyre::Result<Vec<(usize, Box<Path>)>> {
    let mut entries = fs::read_dir(dir)?
        .filter_map(|entry| parse_era_entry(entry, &accept, &mut on_other).transpose())
        .collect::<eyre::Result<Vec<_>>>()?;

    entries.sort_by_key(|(number, _)| *number);

    Ok(entries)
}

/// Parses one directory entry, returning `Some((number, path))` for an accepted ERA file.
///
/// Non-matching entries are forwarded to `on_other` (e.g. to pick up `checksums.txt`).
fn parse_era_entry(
    entry: io::Result<DirEntry>,
    accept: &impl Fn(EraFileType) -> bool,
    on_other: &mut impl FnMut(&Path) -> eyre::Result<()>,
) -> eyre::Result<Option<(usize, Box<Path>)>> {
    let path = entry?.path();

    if let Some(name) = path.file_name().and_then(|name| name.to_str()) &&
        EraFileType::from_filename(name).is_some_and(accept)
    {
        let parts = name.split('-').collect::<Vec<_>>();

        if parts.len() >= 3 {
            let number = usize::from_str(parts[1])?;

            return Ok(Some((number, path.into_boxed_path())));
        }
    } else {
        on_other(&path)?;
    }

    Ok(None)
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
    fn mark_as_processed(&self) -> eyre::Result<()> {
        Ok(())
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_stream_reports_missing_runtime_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("mainnet-00000-deadbeef.era1"), b"data").unwrap();
        fs::write(dir.path().join("checksums.txt"), Sha256::digest(b"data").encode_hex()).unwrap();
        let mut stream = read_dir(dir.path().to_owned(), 0).unwrap();
        let error = futures::executor::block_on(stream.next()).unwrap().unwrap_err();
        assert!(error
            .downcast_ref::<tokio::runtime::TryCurrentError>()
            .unwrap()
            .is_missing_context());
    }

    #[tokio::test]
    async fn checksum_verification_follows_consumption_order() {
        let dir = local_files();
        let mut stream = read_dir(dir.path().to_owned(), 0).unwrap();
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.path(), dir.path().join("mainnet-00000-deadbeef.era1"));

        // No verification of the second file should have completed before it was requested.
        let second_path = dir.path().join("mainnet-00001-deadbeef.era1");
        fs::remove_file(&second_path).unwrap();
        let error = stream.next().await.unwrap().unwrap_err();
        let fs::FsPathError::Open { source, path } =
            error.downcast_ref::<fs::FsPathError>().unwrap()
        else {
            panic!("expected missing archive error: {error:?}");
        };
        assert_eq!(source.kind(), io::ErrorKind::NotFound);
        assert_eq!(path, &second_path);
    }

    #[tokio::test]
    async fn missing_checksum_does_not_prevent_earlier_items() {
        let dir = local_files();
        fs::write(dir.path().join("checksums.txt"), Sha256::digest(b"first").encode_hex()).unwrap();
        let mut stream = read_dir(dir.path().to_owned(), 0).unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap().path(),
            dir.path().join("mainnet-00000-deadbeef.era1")
        );
        assert_eq!(
            stream.next().await.unwrap().unwrap_err().to_string(),
            "Got less checksums than ERA files"
        );
    }

    fn local_files() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("mainnet-00000-deadbeef.era1"), b"first").unwrap();
        fs::write(dir.path().join("mainnet-00001-deadbeef.era1"), b"second").unwrap();
        fs::write(
            dir.path().join("checksums.txt"),
            format!(
                "{}\n{}\n",
                Sha256::digest(b"first").encode_hex(),
                Sha256::digest(b"second").encode_hex()
            ),
        )
        .unwrap();
        dir
    }
}
