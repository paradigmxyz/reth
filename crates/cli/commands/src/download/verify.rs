use super::{manifest::OutputFileChecksum, progress::ArchiveVerificationProgress};
use blake3::Hasher;
use eyre::Result;
use reth_fs_util as fs;
use std::{io::Read, path::Path};

/// Verifies and cleans up extracted output files in one target directory.
pub(crate) struct OutputVerifier<'a> {
    /// Directory containing the output files declared by the manifest.
    target_dir: &'a Path,
}

impl<'a> OutputVerifier<'a> {
    /// Creates a verifier for one extraction target directory.
    pub(crate) const fn new(target_dir: &'a Path) -> Self {
        Self { target_dir }
    }

    /// Returns `true` only when every declared output file exists and matches size and BLAKE3.
    /// Returns `false` if any file is missing, mismatched, or no outputs were declared.
    pub(crate) fn verify(&self, output_files: &[OutputFileChecksum]) -> Result<bool> {
        self.verify_with_progress(output_files, None)
    }

    /// Returns `true` only when every declared output file exists and matches size and BLAKE3,
    /// updating the optional verification progress as file bytes are hashed.
    pub(crate) fn verify_with_progress(
        &self,
        output_files: &[OutputFileChecksum],
        mut progress: Option<&mut ArchiveVerificationProgress<'_>>,
    ) -> Result<bool> {
        if output_files.is_empty() {
            return Ok(false);
        }

        for expected in output_files {
            let output_path = self.target_dir.join(&expected.path);
            let meta = match fs::metadata(&output_path) {
                Ok(meta) => meta,
                Err(_) => return Ok(false),
            };
            if meta.len() != expected.size {
                return Ok(false);
            }

            let actual = Self::file_blake3_hex(&output_path, progress.as_deref_mut())?;
            if !actual.eq_ignore_ascii_case(&expected.blake3) {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Removes any declared output files so a fresh archive attempt can restart cleanly.
    pub(crate) fn cleanup(&self, output_files: &[OutputFileChecksum]) {
        for output in output_files {
            let _ = fs::remove_file(self.target_dir.join(&output.path));
        }
    }

    /// Computes the hex-encoded BLAKE3 checksum for one plain output file.
    fn file_blake3_hex(
        path: &Path,
        mut progress: Option<&mut ArchiveVerificationProgress<'_>>,
    ) -> Result<String> {
        let mut file = fs::open(path)?;
        let mut hasher = Hasher::new();
        let mut buf = [0_u8; 64 * 1024];

        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            if let Some(progress) = progress.as_deref_mut() {
                progress.record_verified(n as u64);
            }
        }

        Ok(hasher.finalize().to_hex().to_string())
    }
}
