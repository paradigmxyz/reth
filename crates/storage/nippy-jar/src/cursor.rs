use crate::{
    compression::{Compression, Compressors, Zstd},
    DataReader, NippyJar, NippyJarError, NippyJarHeader, RefRow,
};
use reth_fs_util::DirectFile;
use smallvec::SmallVec;
use std::{io, ops::Range, sync::Arc};
use zstd::bulk::Decompressor;

/// The column value ranges of a single row, mirroring the inline capacity of [`RefRow`].
///
/// The ranges are collected before they are resolved into slices because [`read_value`] borrows the
/// internal buffer mutably while filling it.
///
/// [`read_value`]: NippyJarCursor::read_value
type ValueRanges = SmallVec<[Range<usize>; 4]>;

/// Simple cursor implementation to retrieve data from [`NippyJar`].
#[derive(Clone)]
pub struct NippyJarCursor<'a, H = ()> {
    /// [`NippyJar`] which holds most of the required configuration to read from the file.
    jar: &'a NippyJar<H>,
    /// Data and offset reader.
    reader: Arc<DataReader>,
    /// Cursor-owned row data, reused across reads.
    internal_buffer: Vec<u8>,
    /// Scratch buffer for compressed input.
    compressed_buffer: Vec<u8>,
    /// Bounded read-ahead buffers live only as long as the cursor.
    data_buffer: ReadBuffer,
    offset_buffer: ReadBuffer,
    /// Cursor row position.
    row: u64,
}

impl<H: NippyJarHeader> std::fmt::Debug for NippyJarCursor<'_, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NippyJarCursor").field("config", &self.jar).finish_non_exhaustive()
    }
}

impl<'a, H: NippyJarHeader> NippyJarCursor<'a, H> {
    /// Creates a new instance of [`NippyJarCursor`] for the given [`NippyJar`].
    pub fn new(jar: &'a NippyJar<H>) -> Result<Self, NippyJarError> {
        Ok(Self {
            jar,
            reader: Arc::new(jar.open_data_reader()?),
            internal_buffer: Vec::new(),
            compressed_buffer: Vec::new(),
            data_buffer: ReadBuffer::new(),
            offset_buffer: ReadBuffer::new(),
            row: 0,
        })
    }

    /// Creates a new instance of [`NippyJarCursor`] with the specified [`NippyJar`] and data
    /// reader.
    pub const fn with_reader(
        jar: &'a NippyJar<H>,
        reader: Arc<DataReader>,
    ) -> Result<Self, NippyJarError> {
        Ok(Self {
            jar,
            reader,
            internal_buffer: Vec::new(),
            compressed_buffer: Vec::new(),
            data_buffer: ReadBuffer::new(),
            offset_buffer: ReadBuffer::new(),
            row: 0,
        })
    }

    /// Returns a reference to the related [`NippyJar`]
    pub const fn jar(&self) -> &NippyJar<H> {
        self.jar
    }

    /// Returns current row index of the cursor
    pub const fn row_index(&self) -> u64 {
        self.row
    }

    /// Resets cursor to the beginning.
    pub const fn reset(&mut self) {
        self.row = 0;
    }

    /// Returns a row by its number.
    pub fn row_by_number(&mut self, row: usize) -> Result<Option<RefRow<'_>>, NippyJarError> {
        self.row = row as u64;
        self.next_row()
    }

    /// Returns the current value and advances the row.
    pub fn next_row(&mut self) -> Result<Option<RefRow<'_>>, NippyJarError> {
        self.internal_buffer.clear();

        if self.row as usize >= self.jar.rows {
            // Has reached the end
            return Ok(None)
        }

        let mut row = ValueRanges::with_capacity(self.jar.columns);

        // Retrieve all column values from the row
        for column in 0..self.jar.columns {
            self.read_value(column, &mut row)?;
        }

        self.row += 1;

        Ok(Some(row.into_iter().map(|range| &self.internal_buffer[range]).collect()))
    }

    /// Returns a row by its number by using a `mask` to only read certain columns from the row.
    pub fn row_by_number_with_cols(
        &mut self,
        row: usize,
        mask: usize,
    ) -> Result<Option<RefRow<'_>>, NippyJarError> {
        self.row = row as u64;
        self.next_row_with_cols(mask)
    }

    /// Returns the current value and advances the row.
    ///
    /// Uses a `mask` to only read certain columns from the row.
    pub fn next_row_with_cols(&mut self, mask: usize) -> Result<Option<RefRow<'_>>, NippyJarError> {
        self.internal_buffer.clear();

        if self.row as usize >= self.jar.rows {
            // Has reached the end
            return Ok(None)
        }

        let columns = self.jar.columns;
        let mut row = ValueRanges::with_capacity(columns);

        for column in 0..columns {
            if mask & (1 << column) != 0 {
                self.read_value(column, &mut row)?
            }
        }
        self.row += 1;

        Ok(Some(row.into_iter().map(|range| &self.internal_buffer[range]).collect()))
    }

    fn read_offset(&mut self, index: usize) -> Result<u64, NippyJarError> {
        let size = self.reader.offset_size as usize;
        let from = index * size + 1;
        let mut bytes = [0; 8];
        bytes[..size].copy_from_slice(self.offset_buffer.read(
            &self.reader.offset_file,
            self.reader.offsets_len,
            from..from + size,
        )?);
        Ok(u64::from_le_bytes(bytes))
    }

    /// Takes the column index and reads the range value for the corresponding column.
    fn read_value(&mut self, column: usize, row: &mut ValueRanges) -> Result<(), NippyJarError> {
        // Find out the offset of the column value
        let offset_pos = self.row as usize * self.jar.columns + column;
        let value_offset = self.read_offset(offset_pos)? as usize;

        let column_offset_range = if self.jar.rows * self.jar.columns == offset_pos + 1 {
            // It's the last column of the last row
            value_offset..self.reader.size()
        } else {
            let next_value_offset = self.read_offset(offset_pos + 1)? as usize;
            value_offset..next_value_offset
        };

        let from = self.internal_buffer.len();
        if let Some(compression) = self.jar.compressor() {
            self.compressed_buffer.clear();
            self.compressed_buffer.extend_from_slice(self.data_buffer.read(
                &self.reader.data_file,
                self.reader.data_len,
                column_offset_range,
            )?);
            // The decompressors write into the spare capacity of the buffer, so it has to fit any
            // row of data. The buffer is only cleared between rows, so this reserves once.
            if self.internal_buffer.capacity() < self.jar.max_row_size {
                self.internal_buffer.reserve(self.jar.max_row_size - self.internal_buffer.len());
            }

            match compression {
                Compressors::Zstd(z) if z.use_dict => {
                    // If we are here, then for sure we have the necessary dictionaries and they're
                    // loaded (happens during deserialization). Otherwise, there's an issue
                    // somewhere else and we can't recover here anyway.
                    let dictionaries = z.dictionaries.as_ref().expect("dictionaries to exist")
                        [column]
                        .loaded()
                        .expect("dictionary to be loaded");
                    let mut decompressor = Decompressor::with_prepared_dictionary(dictionaries)?;
                    Zstd::decompress_with_dictionary(
                        &self.compressed_buffer,
                        &mut self.internal_buffer,
                        &mut decompressor,
                    )?;
                }
                _ => {
                    // Uses the chosen default decompressor
                    compression
                        .decompress_to(&self.compressed_buffer, &mut self.internal_buffer)?;
                }
            }
        } else {
            self.internal_buffer.extend_from_slice(self.data_buffer.read(
                &self.reader.data_file,
                self.reader.data_len,
                column_offset_range,
            )?);
        }

        row.push(from..self.internal_buffer.len());
        Ok(())
    }
}

/// A cursor-local window amortizes direct I/O during sequential scans. It is
/// discarded with the cursor, so inactive jars retain no cached file contents.
#[derive(Clone)]
struct ReadBuffer {
    start: usize,
    bytes: Vec<u8>,
}

impl ReadBuffer {
    const CAPACITY: usize = 64 * 1024;

    const fn new() -> Self {
        Self { start: 0, bytes: Vec::new() }
    }

    fn read(
        &mut self,
        file: &DirectFile,
        file_len: usize,
        range: Range<usize>,
    ) -> io::Result<&[u8]> {
        if !(range.start..=file_len).contains(&range.end) {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if range.is_empty() {
            return Ok(&[]);
        }
        if range.start < self.start || range.end > self.start + self.bytes.len() {
            // Align the window to avoid re-reading a partial direct-I/O block on
            // every refill. Oversized values use a window large enough for that value.
            let start = range.start / Self::CAPACITY * Self::CAPACITY;
            let end = start.saturating_add(Self::CAPACITY).max(range.end).min(file_len);
            // Invalidate before reading so an I/O error cannot expose stale bytes.
            self.bytes.clear();
            self.bytes.resize(end - start, 0);
            if let Err(err) = file.read_exact_at(&mut self.bytes, start as u64) {
                self.bytes.clear();
                return Err(err);
            }
            self.start = start;
        }
        Ok(&self.bytes[range.start - self.start..range.end - self.start])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn cursor_scans_and_seeks_across_read_windows() {
        for compressed in [false, true] {
            let file = tempfile::NamedTempFile::new().unwrap();
            let mut jar = NippyJar::new_without_header(2, file.path());
            if compressed {
                jar = jar.with_lz4();
            }
            let first: Vec<Vec<u8>> = (0u32..10000).map(|i| i.to_le_bytes().to_vec()).collect();
            let second: Vec<Vec<u8>> = (0..10000).map(|i| vec![(i % 251) as u8; i % 127]).collect();
            let jar = jar
                .freeze(vec![first.iter().cloned().map(Ok), second.iter().cloned().map(Ok)], 10000)
                .unwrap();
            let mut cursor = NippyJarCursor::new(&jar).unwrap();
            for i in 0..10000 {
                let row = cursor.next_row().unwrap().unwrap();
                assert_eq!(row[0], first[i]);
                assert_eq!(row[1], second[i]);
            }
            assert!(cursor.next_row().unwrap().is_none());
            for i in [9999, 0, 8191, 4096, 1] {
                let row = cursor.row_by_number_with_cols(i, 2).unwrap().unwrap();
                assert_eq!(row[0], second[i]);
            }
            cursor.reset();
            let row = cursor.next_row().unwrap().unwrap();
            assert_eq!(row[0], first[0]);
        }
    }

    #[test]
    fn read_buffer_boundaries_seeks_and_eof() {
        let mut temp = tempfile::NamedTempFile::new().unwrap();
        let expected: Vec<u8> =
            (0..ReadBuffer::CAPACITY * 3 + 17).map(|i| (i % 251) as u8).collect();
        temp.write_all(&expected).unwrap();
        let file = DirectFile::open(temp.path(), false, false).unwrap();
        let mut buffer = ReadBuffer::new();
        for range in [
            0..1,
            65530..65550,
            140000..140010,
            12..30,
            4..150000,
            expected.len() - 17..expected.len(),
            expected.len()..expected.len(),
        ] {
            assert_eq!(
                buffer.read(&file, expected.len(), range.clone()).unwrap(),
                &expected[range]
            );
        }
        assert_eq!(
            buffer
                .read(&file, expected.len(), expected.len()..expected.len() + 1)
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
