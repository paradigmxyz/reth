use super::{
    extract::ensure_no_symlinks, manifest::OutputFileChecksum,
    progress::ArchiveVerificationProgress,
};
use blake3::Hasher;
use eyre::Result;
use rayon::prelude::*;
use reth_fs_util::{self as fs, FsPathError};
use std::{
    io::{ErrorKind, Read},
    mem,
    path::{Path, PathBuf},
};
use tracing::warn;

/// Read size per hash update. Large enough for `update_rayon` to split across threads.
const HASH_BUFFER_SIZE: usize = 4 * 1024 * 1024;

/// Verifies and cleans up extracted output files in one target directory.
pub(crate) struct OutputVerifier<'a> {
    /// Directory containing the output files declared by the manifest.
    target_dir: &'a Path,
    /// Custom location of static file outputs.
    static_files_dir: Option<&'a Path>,
}

impl<'a> OutputVerifier<'a> {
    /// Creates a verifier for one extraction target directory.
    pub(crate) const fn new(target_dir: &'a Path, static_files_dir: Option<&'a Path>) -> Self {
        Self { target_dir, static_files_dir }
    }

    /// Returns `true` only when every declared output file exists and matches size and BLAKE3.
    /// Returns `false` if any file is missing, mismatched, or no outputs were declared, and an
    /// error if a file exists but cannot be inspected or read.
    pub(crate) fn verify(&self, output_files: &[OutputFileChecksum]) -> Result<bool> {
        self.verify_with_progress(output_files, None)
    }

    /// Returns `true` only when every declared output file exists and matches size and BLAKE3,
    /// updating the optional verification progress as file bytes are hashed.
    ///
    /// Sizes are checked before any file is hashed, then files are hashed in parallel.
    pub(crate) fn verify_with_progress(
        &self,
        output_files: &[OutputFileChecksum],
        progress: Option<&ArchiveVerificationProgress<'_>>,
    ) -> Result<bool> {
        if output_files.is_empty() {
            return Ok(false);
        }

        let mut paths = Vec::with_capacity(output_files.len());
        for expected in output_files {
            let output_path = self.output_path(expected)?;
            match fs::metadata(&output_path) {
                Ok(meta) if meta.len() == expected.size => paths.push(output_path),
                Ok(_) => return Ok(false),
                Err(FsPathError::Metadata { source, .. })
                    if source.kind() == ErrorKind::NotFound =>
                {
                    return Ok(false)
                }
                Err(error) => return Err(error.into()),
            }
        }

        output_files
            .par_iter()
            .zip(paths)
            .map(|(expected, path)| {
                Self::file_blake3_hex(&path, expected.size, progress)
                    .map(|actual| actual.eq_ignore_ascii_case(&expected.blake3))
            })
            .find_any(|verified| !matches!(verified, Ok(true)))
            .unwrap_or(Ok(true))
    }

    /// Removes any declared output files so a fresh archive attempt can restart cleanly.
    ///
    /// Files are only removed if their path stays below the output root and no existing parent
    /// directory is a symlink. Extraction rejects link entries, so this guards against links
    /// already present in the datadir; the check is not atomic with the removal.
    pub(crate) fn cleanup(&self, output_files: &[OutputFileChecksum]) {
        for output in output_files {
            match self.removal_path(output) {
                Ok(path) => {
                    let _ = fs::remove_file(path);
                }
                Err(error) => {
                    warn!(target: "reth::cli", path = %output.path, %error, "Skipping output cleanup");
                }
            }
        }
    }

    /// Returns the path to remove for `output`, or an error if removing it could touch anything
    /// outside the output root.
    fn removal_path(&self, output: &OutputFileChecksum) -> Result<PathBuf> {
        let (root, relative) = self.resolve(output)?;
        if let Some(parent) = relative.parent() {
            ensure_no_symlinks(root, parent)?;
        }
        Ok(root.join(relative))
    }

    /// Resolves archive paths consistently for verification and retry cleanup.
    pub(crate) fn output_path(&self, output: &OutputFileChecksum) -> Result<PathBuf> {
        self.resolve(output).map(|(root, relative)| root.join(relative))
    }

    /// Returns the output root and the validated path of `output` relative to it.
    fn resolve<'p>(&'p self, output: &'p OutputFileChecksum) -> Result<(&'p Path, &'p Path)> {
        let path = output.relative_path()?;
        if let Some(static_files_dir) = self.static_files_dir &&
            let Some(relative_path) = super::extract::static_file_relative_path(path)
        {
            return Ok((static_files_dir, relative_path))
        }
        Ok((self.target_dir, path))
    }

    /// Computes the hex-encoded BLAKE3 checksum for one plain output file.
    ///
    /// The next buffer is read while the current one is hashed, so disk reads overlap hashing
    /// instead of alternating with it.
    fn file_blake3_hex(
        path: &Path,
        size: u64,
        progress: Option<&ArchiveVerificationProgress<'_>>,
    ) -> Result<String> {
        let mut file = fs::open(path)?;
        let mut hasher = Hasher::new();
        let buf_len = size.min(HASH_BUFFER_SIZE as u64) as usize;
        let mut current = vec![0_u8; buf_len];
        let mut next = vec![0_u8; buf_len];

        let mut filled = read_full(&mut file, &mut current)?;
        while filled > 0 {
            let (_, read) = rayon::join(
                || {
                    hasher.update_rayon(&current[..filled]);
                },
                || read_full(&mut file, &mut next),
            );
            if let Some(progress) = progress {
                progress.record_verified(filled as u64);
            }
            filled = read?;
            mem::swap(&mut current, &mut next);
        }

        Ok(hasher.finalize().to_hex().to_string())
    }
}

/// Reads until `buf` is full or the reader is exhausted, returning the number of bytes read.
///
/// Full buffers keep every `update_rayon` call aligned, so BLAKE3 can hash it as parallel
/// subtrees.
fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checksum(path: &str, data: &[u8]) -> OutputFileChecksum {
        OutputFileChecksum {
            path: path.into(),
            size: data.len() as u64,
            blake3: blake3::hash(data).to_hex().to_string(),
        }
    }

    #[test]
    fn verify_detects_each_kind_of_output_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let large: Vec<u8> = (0..2 * HASH_BUFFER_SIZE + 123).map(|i| (i % 251) as u8).collect();
        let small = b"small".to_vec();
        fs::write(dir.path().join("large"), &large).unwrap();
        fs::write(dir.path().join("small"), &small).unwrap();
        let outputs = vec![checksum("large", &large), checksum("small", &small)];
        let verifier = OutputVerifier::new(dir.path(), None);

        assert!(verifier.verify(&outputs).unwrap(), "matching multi-buffer outputs verify");

        let mut uppercase = outputs.clone();
        uppercase[0].blake3 = uppercase[0].blake3.to_ascii_uppercase();
        assert!(verifier.verify(&uppercase).unwrap(), "checksums compare case-insensitively");

        let mut flipped = large.clone();
        flipped[HASH_BUFFER_SIZE + 7] ^= 1;
        fs::write(dir.path().join("large"), &flipped).unwrap();
        assert!(!verifier.verify(&outputs).unwrap(), "one flipped byte fails verification");

        fs::write(dir.path().join("large"), &large[1..]).unwrap();
        assert!(!verifier.verify(&outputs).unwrap(), "size mismatch fails verification");

        fs::remove_file(dir.path().join("large")).unwrap();
        assert!(!verifier.verify(&outputs).unwrap(), "missing file fails verification");

        assert!(!verifier.verify(&[]).unwrap(), "archives without outputs never verify");
    }

    #[cfg(unix)]
    #[test]
    fn verify_returns_metadata_errors_other_than_not_found() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file"), b"data").unwrap();
        let verifier = OutputVerifier::new(dir.path(), None);

        assert!(!verifier.verify(&[checksum("missing", b"data")]).unwrap());
        assert!(
            verifier.verify(&[checksum("file/child", b"data")]).is_err(),
            "a path through a regular file is an error, not a missing output"
        );
    }

    #[test]
    fn custom_static_files_verification_and_cleanup() {
        let datadir = tempfile::tempdir().unwrap();
        let static_dir = tempfile::tempdir().unwrap();
        let default_file = datadir.path().join("static_files/headers");
        fs::create_dir_all(default_file.parent().unwrap()).unwrap();
        fs::write(&default_file, b"headers").unwrap();
        let outputs = [OutputFileChecksum {
            path: "./static_files/headers".into(),
            size: 7,
            blake3: blake3::hash(b"headers").to_hex().to_string(),
        }];
        let verifier = OutputVerifier::new(datadir.path(), Some(static_dir.path()));
        assert!(!verifier.verify(&outputs).unwrap());
        let custom_file = static_dir.path().join("headers");
        fs::write(&custom_file, b"headers").unwrap();
        assert!(verifier.verify(&outputs).unwrap());
        verifier.cleanup(&outputs);
        assert!(!custom_file.exists());
        assert_eq!(fs::read(default_file).unwrap(), b"headers");
    }

    #[test]
    fn cleanup_ignores_output_paths_outside_target() {
        let root = tempfile::tempdir().unwrap();
        let datadir = root.path().join("datadir");
        fs::create_dir_all(&datadir).unwrap();
        let outside = root.path().join("outside");
        fs::write(&outside, b"keep").unwrap();
        let inside = datadir.join("inside");
        fs::write(&inside, b"remove").unwrap();

        let outputs = [
            checksum(outside.to_str().unwrap(), b"keep"),
            checksum("../outside", b"keep"),
            checksum("db/../../outside", b"keep"),
            checksum("inside", b"remove"),
        ];
        let verifier = OutputVerifier::new(&datadir, None);
        assert!(verifier.verify(&outputs).is_err(), "invalid paths are never verified");
        verifier.cleanup(&outputs);

        assert_eq!(fs::read(&outside).unwrap(), b"keep");
        assert!(!inside.exists());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_does_not_follow_symlinked_parents() {
        let root = tempfile::tempdir().unwrap();
        let datadir = root.path().join("datadir");
        let static_dir = root.path().join("static");
        let outside = root.path().join("outside");
        fs::create_dir_all(&datadir).unwrap();
        fs::create_dir_all(&static_dir).unwrap();
        fs::create_dir_all(outside.join("nested")).unwrap();
        for path in ["mdbx.dat", "headers", "nested/headers"] {
            fs::write(outside.join(path), b"keep").unwrap();
        }
        std::os::unix::fs::symlink(&outside, datadir.join("db")).unwrap();
        std::os::unix::fs::symlink(&outside, static_dir.join("link")).unwrap();
        std::os::unix::fs::symlink(&outside, static_dir.join("nested")).unwrap();

        let outputs = [
            checksum("db/mdbx.dat", b"keep"),
            checksum("static_files/link/headers", b"keep"),
            checksum("static_files/nested/nested/headers", b"keep"),
        ];
        for static_files_dir in [None, Some(static_dir.as_path())] {
            OutputVerifier::new(&datadir, static_files_dir).cleanup(&outputs);
        }

        for path in ["mdbx.dat", "headers", "nested/headers"] {
            assert_eq!(fs::read(outside.join(path)).unwrap(), b"keep");
        }

        // A symlink as the output itself is unlinked without touching its target.
        std::os::unix::fs::symlink(outside.join("mdbx.dat"), datadir.join("link")).unwrap();
        OutputVerifier::new(&datadir, None).cleanup(&[checksum("link", b"keep")]);
        assert!(datadir.join("link").symlink_metadata().is_err());
        assert_eq!(fs::read(outside.join("mdbx.dat")).unwrap(), b"keep");
    }
}
