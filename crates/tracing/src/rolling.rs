//! A log file writer that rotates by size.

use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    path::PathBuf,
};

/// Writes to `path` and rotates it once it reaches `max_size_bytes`.
///
/// Rotation renames `path` to `path.1`, `path.1` to `path.2` and so on, keeping at most
/// `max_files` rotated files. The size check runs before each write, so a file can exceed the
/// limit by one write.
#[derive(Debug)]
pub(crate) struct RollingFileWriter {
    path: PathBuf,
    max_size_bytes: u64,
    max_files: usize,
    file_size: u64,
    writer: Option<BufWriter<File>>,
}

impl RollingFileWriter {
    /// Opens `path` for appending, creating it if needed.
    pub(crate) fn new(path: PathBuf, max_size_bytes: u64, max_files: usize) -> io::Result<Self> {
        let mut writer = Self { path, max_size_bytes, max_files, file_size: 0, writer: None };
        writer.writer = Some(writer.open()?);
        Ok(writer)
    }

    /// Returns the path of the rotated file with index `n`, or the active file for `0`.
    fn rotated_path(&self, n: usize) -> PathBuf {
        if n == 0 {
            return self.path.clone()
        }
        let mut path = OsString::from(&self.path);
        path.push(format!(".{n}"));
        path.into()
    }

    /// Opens the active file for appending and records its current size.
    fn open(&mut self) -> io::Result<BufWriter<File>> {
        let file = OpenOptions::new().append(true).create(true).open(&self.path)?;
        self.file_size = file.metadata()?.len();
        Ok(BufWriter::new(file))
    }

    /// Closes the active file and shifts every file one index up, dropping the oldest.
    fn rotate(&mut self) -> io::Result<()> {
        if let Some(mut writer) = self.writer.take() {
            writer.flush()?;
        }
        let last = self.max_files.max(1);
        match fs::remove_file(self.rotated_path(last)) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        for n in (0..last).rev() {
            match fs::rename(self.rotated_path(n), self.rotated_path(n + 1)) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
                _ => {}
            }
        }
        Ok(())
    }
}

impl Write for RollingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.file_size >= self.max_size_bytes &&
            let Err(err) = self.rotate()
        {
            // This writer backs the tracing subscriber, so the error can't be logged.
            eprintln!("WARNING: Failed to rotate log file {}: {err}", self.path.display());
        }
        let writer = match self.writer.take() {
            Some(writer) => writer,
            None => self.open()?,
        };
        self.writer.insert(writer).write_all(buf)?;
        self.file_size = self.file_size.saturating_add(buf.len() as u64);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.writer.as_mut() {
            Some(writer) => writer.flush(),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn rotates_by_size_and_keeps_max_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reth.log");
        let mut writer = RollingFileWriter::new(path.clone(), 4, 2).unwrap();

        for line in ["aaaa", "bbbb", "cccc", "dddd"] {
            writer.write_all(line.as_bytes()).unwrap();
        }
        writer.flush().unwrap();

        assert_eq!(read(&path), "dddd");
        assert_eq!(read(&writer.rotated_path(1)), "cccc");
        assert_eq!(read(&writer.rotated_path(2)), "bbbb");
        assert!(!writer.rotated_path(3).exists());
    }

    #[test]
    fn appends_to_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reth.log");
        fs::write(&path, "abc").unwrap();

        let mut writer = RollingFileWriter::new(path.clone(), 4, 1).unwrap();
        writer.write_all(b"d").unwrap();
        writer.write_all(b"e").unwrap();
        writer.flush().unwrap();

        assert_eq!(read(&writer.rotated_path(1)), "abcd");
        assert_eq!(read(&path), "e");
    }
}
