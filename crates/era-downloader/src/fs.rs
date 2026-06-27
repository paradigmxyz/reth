use crate::{EraMeta, BLOCKS_PER_FILE};
use alloy_primitives::{hex, hex::ToHexExt, BlockNumber};
use eyre::{eyre, OptionExt};
use futures_util::{stream, Stream};
use reth_era::common::file_ops::EraFileType;
use reth_fs_util as fs;
use sha2::{Digest, Sha256};
use std::{fmt::Debug, fs::DirEntry, io, io::BufRead, path::Path, str::FromStr};

/// Creates a new ordered asynchronous [`Stream`] of ERA1 files read from `dir`.
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

    Ok(stream::iter(entries.into_iter().skip_while(move |(n, _)| *n < start_index).map(
        move |(_, path)| {
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
/// parsed from the `<network>-<number>-<hash>.<ext>` filename.
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

        if parts.len() == 3 {
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
