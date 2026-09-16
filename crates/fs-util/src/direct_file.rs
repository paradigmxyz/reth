#[cfg(windows)]
use std::os::windows::fs::FileExt;
#[cfg(unix)]
use std::os::{fd::AsRawFd, unix::fs::FileExt};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

/// A positional file that bypasses the kernel data cache where supported.
///
/// Linux uses `O_DIRECT`, Apple platforms use `F_NOCACHE`. Other platforms and
/// filesystems that explicitly reject direct I/O use ordinary positional I/O.
/// Unaligned writes preserve adjacent bytes using read-modify-write and restore
/// the logical file length after writing a padded tail. Writers must be exclusive.
#[derive(Debug)]
pub struct DirectFile {
    file: File,
    position: u64,
    alignment: usize,
}

impl DirectFile {
    /// Opens a file for reading and optionally writing or creating it.
    pub fn open(path: impl AsRef<Path>, write: bool, create: bool) -> io::Result<Self> {
        Self::from_file(OpenOptions::new().read(true).write(write).create(create).open(path)?)
    }

    /// Enables cache bypass on an existing descriptor before any data I/O.
    pub fn from_file(file: File) -> io::Result<Self> {
        #[cfg(not(target_os = "linux"))]
        let alignment: usize = 1;
        #[cfg(target_os = "linux")]
        let alignment = {
            // Older kernels lack STATX_DIOALIGN. 4 KiB satisfies the usual block
            // device constraints; when reported, use the filesystem's requirements.
            let mut alignment: usize = 4096;
            // SAFETY: statx writes to an initialized struct and receives a valid fd
            // and a NUL-terminated empty path with AT_EMPTY_PATH.
            let mut stat: libc::statx = unsafe { std::mem::zeroed() };
            // SAFETY: the descriptor, path, and output pointer are valid.
            let result = unsafe {
                libc::statx(
                    file.as_raw_fd(),
                    c"".as_ptr(),
                    libc::AT_EMPTY_PATH,
                    libc::STATX_DIOALIGN,
                    &raw mut stat,
                )
            };
            if result == 0 && stat.stx_mask & libc::STATX_DIOALIGN != 0 {
                alignment = (stat.stx_dio_mem_align.max(stat.stx_dio_offset_align) as usize).max(1);
            }
            if alignment > 1 {
                // SAFETY: fcntl operates on a live descriptor with integer arguments.
                let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
                if flags == -1 {
                    return Err(io::Error::last_os_error());
                }
                // SAFETY: F_SETFL takes integer flags and a live descriptor.
                let result =
                    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_DIRECT) };
                if result == -1 {
                    let err = io::Error::last_os_error();
                    if !matches!(
                        err.raw_os_error(),
                        Some(libc::EINVAL | libc::EOPNOTSUPP | libc::ENOSYS)
                    ) {
                        return Err(err);
                    }
                    alignment = 1;
                }
            }
            alignment
        };
        #[cfg(target_vendor = "apple")]
        {
            // SAFETY: F_NOCACHE takes an integer flag and a live descriptor.
            if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) } == -1 {
                let err = io::Error::last_os_error();
                if !matches!(err.raw_os_error(), Some(libc::EINVAL | libc::ENOTSUP)) {
                    return Err(err);
                }
            }
        }
        if !alignment.is_power_of_two() {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "invalid direct I/O alignment"));
        }
        Ok(Self { file, position: 0, alignment })
    }

    pub(crate) const fn alignment(&self) -> usize {
        self.alignment
    }

    /// Reads into an already aligned buffer, avoiding an allocation and copy.
    pub(crate) fn read_aligned_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        debug_assert!(buf.as_ptr().align_offset(self.alignment) == 0);
        debug_assert!(buf.len().is_multiple_of(self.alignment));
        debug_assert!(offset.is_multiple_of(self.alignment as u64));
        let mut filled = 0;
        while filled < buf.len() {
            let n = match positional_read(&self.file, &mut buf[filled..], offset + filled as u64) {
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                other => other?,
            };
            if n == 0 {
                break;
            }
            filled += n;
            if !filled.is_multiple_of(self.alignment) {
                break;
            }
        }
        Ok(filled)
    }

    /// Returns filesystem metadata.
    pub fn metadata(&self) -> io::Result<Metadata> {
        self.file.metadata()
    }

    /// Truncates or extends the file without changing the stream position.
    pub fn set_len(&self, len: u64) -> io::Result<()> {
        self.file.set_len(len)
    }

    /// Persists file contents and metadata. Direct I/O alone is not a durability guarantee.
    pub fn sync_all(&self) -> io::Result<()> {
        self.file.sync_all()
    }

    /// Reads exactly the requested range, including unaligned ranges at EOF.
    pub fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let n = self.read_at(buf, offset)?;
        if n != buf.len() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(())
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let start = offset / self.alignment as u64 * self.alignment as u64;
        let skip = (offset - start) as usize;
        let len = skip
            .checked_add(buf.len())
            .and_then(|n| n.checked_next_multiple_of(self.alignment))
            .ok_or(io::ErrorKind::InvalidInput)?;
        let mut storage =
            vec![0; len.checked_add(self.alignment - 1).ok_or(io::ErrorKind::InvalidInput)?];
        let shift = storage.as_ptr().align_offset(self.alignment);
        let aligned = &mut storage[shift..shift + len];
        let mut filled = 0;
        while filled < len {
            let n = match positional_read(&self.file, &mut aligned[filled..], start + filled as u64)
            {
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                other => other?,
            };
            if n == 0 {
                break;
            }
            filled += n;
            // A short unaligned transfer reaches EOF; retrying it would violate O_DIRECT.
            if !filled.is_multiple_of(self.alignment) {
                break;
            }
        }
        let count = filled.saturating_sub(skip).min(buf.len());
        buf[..count].copy_from_slice(&aligned[skip..skip + count]);
        Ok(count)
    }
}

impl Read for DirectFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.read_at(buf, self.position)?;
        self.position += n as u64;
        Ok(n)
    }
}

impl Write for DirectFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let end = self.position.checked_add(buf.len() as u64).ok_or(io::ErrorKind::InvalidInput)?;
        let old_len = self.file.metadata()?.len();
        let start = self.position / self.alignment as u64 * self.alignment as u64;
        let skip = (self.position - start) as usize;
        let len = skip
            .checked_add(buf.len())
            .and_then(|n| n.checked_next_multiple_of(self.alignment))
            .ok_or(io::ErrorKind::InvalidInput)?;
        let mut storage =
            vec![0; len.checked_add(self.alignment - 1).ok_or(io::ErrorKind::InvalidInput)?];
        let shift = storage.as_ptr().align_offset(self.alignment);
        let aligned = &mut storage[shift..shift + len];
        // Only edge blocks need preserving; the caller replaces all interior bytes.
        if skip != 0 && start < old_len {
            let n = (old_len - start).min(self.alignment as u64) as usize;
            self.read_exact_at(&mut aligned[..n], start)?;
        }
        let tail = (skip + buf.len()) / self.alignment * self.alignment;
        if !(skip + buf.len()).is_multiple_of(self.alignment) && start + (tail as u64) < old_len {
            let n = (old_len - start - tail as u64).min(self.alignment as u64) as usize;
            self.read_exact_at(&mut aligned[tail..tail + n], start + tail as u64)?;
        }
        aligned[skip..skip + buf.len()].copy_from_slice(buf);
        let mut written = 0;
        while written < len {
            let n = match positional_write(&self.file, &aligned[written..], start + written as u64)
            {
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                other => other?,
            };
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            written += n;
            if !written.is_multiple_of(self.alignment) {
                return Err(io::Error::other("unaligned short direct write"));
            }
        }
        if start + len as u64 > old_len.max(end) {
            self.file.set_len(old_len.max(end))?;
        }
        self.position = end;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for DirectFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.position = match pos {
            SeekFrom::Start(n) => n,
            SeekFrom::Current(n) => {
                self.position.checked_add_signed(n).ok_or(io::ErrorKind::InvalidInput)?
            }
            SeekFrom::End(n) => self
                .file
                .metadata()?
                .len()
                .checked_add_signed(n)
                .ok_or(io::ErrorKind::InvalidInput)?,
        };
        Ok(self.position)
    }
}

/// Owns aligned storage without requiring custom allocation or unsafe slice construction.
#[derive(Debug)]
pub(crate) struct AlignedBuffer {
    storage: Vec<u8>,
    shift: usize,
    len: usize,
}

impl AlignedBuffer {
    pub(crate) fn new(len: usize, alignment: usize) -> io::Result<Self> {
        let storage = vec![0; len.checked_add(alignment - 1).ok_or(io::ErrorKind::InvalidInput)?];
        let shift = storage.as_ptr().align_offset(alignment);
        Ok(Self { storage, shift, len })
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.storage[self.shift..self.shift + self.len]
    }
    pub(crate) fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.storage[self.shift..self.shift + self.len]
    }
    pub(crate) const fn allocated_bytes(&self) -> usize {
        self.storage.capacity()
    }
}

fn positional_read(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    #[cfg(unix)]
    {
        file.read_at(buf, offset)
    }
    #[cfg(windows)]
    {
        file.seek_read(buf, offset)
    }
    #[cfg(not(any(unix, windows)))]
    {
        with_position(file, offset, |file| file.read(buf))
    }
}

fn positional_write(file: &File, buf: &[u8], offset: u64) -> io::Result<usize> {
    #[cfg(unix)]
    {
        file.write_at(buf, offset)
    }
    #[cfg(windows)]
    {
        file.seek_write(buf, offset)
    }
    #[cfg(not(any(unix, windows)))]
    {
        with_position(file, offset, |file| file.write(buf))
    }
}

#[cfg(not(any(unix, windows)))]
fn with_position<T>(
    file: &File,
    offset: u64,
    operation: impl FnOnce(&mut &File) -> io::Result<T>,
) -> io::Result<T> {
    // Platforms without FileExt need a seek/read or seek/write pair. Serialize
    // the pair so shared readers cannot race on the descriptor's stream position.
    static POSITION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard =
        POSITION_LOCK.lock().map_err(|_| io::Error::other("file position lock poisoned"))?;
    let mut file = file;
    file.seek(SeekFrom::Start(offset))?;
    operation(&mut file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unaligned_reads_writes_and_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("direct");
        let mut file = DirectFile::open(&path, true, true).unwrap();
        // Exercise alignment and read-modify-write on Apple/fallback platforms too.
        file.alignment = file.alignment.max(4096);
        let mut expected: Vec<u8> = (0..12345).map(|i| (i % 251) as u8).collect();
        file.write_all(&expected).unwrap();
        assert_eq!(file.metadata().unwrap().len(), expected.len() as u64);
        for (offset, len) in [(0, 1), (1, 8), (4093, 16), (4096, 4096), (12340, 5)] {
            let mut actual = vec![0; len];
            file.read_exact_at(&mut actual, offset as u64).unwrap();
            assert_eq!(actual, expected[offset..offset + len]);
        }
        file.seek(SeekFrom::Start(4093)).unwrap();
        file.write_all(&[42; 19]).unwrap();
        expected[4093..4112].fill(42);
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut actual = Vec::new();
        file.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, expected);
        file.set_len(4100).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        file.write_all(&[99; 3]).unwrap();
        expected.truncate(4100);
        expected.extend([99; 3]);
        file.sync_all().unwrap();
        let reader = DirectFile::open(&path, false, false).unwrap();
        let mut actual = vec![0; expected.len()];
        reader.read_exact_at(&mut actual, 0).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(reader.metadata().unwrap().len(), 4103);
        assert_eq!(
            reader.read_exact_at(&mut [0; 4], 4100).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(reader.read_at(&mut [0; 4], 50000).unwrap(), 0);
    }

    #[test]
    fn mixed_writes_match_in_memory_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = DirectFile::open(dir.path().join("mixed"), true, true).unwrap();
        file.alignment = file.alignment.max(4096);
        let mut expected = Vec::new();
        for step in 0..64usize {
            let offset = (step * 7907) % 16384;
            let len = (step * 137) % 5000 + 1;
            let bytes = vec![step as u8; len];
            file.seek(SeekFrom::Start(offset as u64)).unwrap();
            file.write_all(&bytes).unwrap();
            expected.resize(expected.len().max(offset + len), 0);
            expected[offset..offset + len].copy_from_slice(&bytes);
            if step % 7 == 0 {
                let len = expected.len() / 2;
                file.set_len(len as u64).unwrap();
                expected.truncate(len);
            }
            assert_eq!(file.metadata().unwrap().len(), expected.len() as u64);
            let mut actual = vec![0; expected.len()];
            file.read_exact_at(&mut actual, 0).unwrap();
            assert_eq!(actual, expected, "step {step}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn uses_o_direct() {
        let dir = tempfile::tempdir().unwrap();
        let file = DirectFile::open(dir.path().join("flags"), true, true).unwrap();
        // CI uses a local block filesystem; do not silently test only the fallback.
        assert!(file.alignment > 1);
        // SAFETY: F_GETFL reads flags from a live descriptor.
        let flags = unsafe { libc::fcntl(file.file.as_raw_fd(), libc::F_GETFL) };
        assert_ne!(flags & libc::O_DIRECT, 0);
    }
}
