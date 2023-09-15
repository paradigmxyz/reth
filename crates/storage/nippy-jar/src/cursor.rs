use crate::{compression::Compression, Filter, NippyJar, NippyJarError, PerfectHashingFunction};
use std::{
    clone::Clone,
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::Mutex,
};
use sucds::int_vectors::Access;
use zstd::bulk::Decompressor;

/// Simple cursor implementation to retrieve data from [`NippyJar`].
pub struct NippyJarCursor<'a> {
    /// [`NippyJar`] which holds most of the required configuration to read from the file.
    jar: &'a NippyJar,
    /// Optional dictionary decompressors.
    zstd_decompressors: Option<Mutex<Vec<Decompressor<'a>>>>,
    /// Data file.
    data_handle: File,
    /// Temporary buffer to unload data to (if necessary), without reallocating memory on each
    /// retrieval.
    tmp_buf: Vec<u8>,
    /// Cursor row position.
    row: u64,
    /// Cursor column position.
    col: u64,
}

impl<'a> std::fmt::Debug for NippyJarCursor<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NippyJarCursor {{ config: {:?} }}", self.jar)
    }
}

impl<'a> NippyJarCursor<'a> {
    pub fn new(
        config: &'a NippyJar,
        zstd_decompressors: Option<Mutex<Vec<Decompressor<'a>>>>,
    ) -> Result<Self, NippyJarError> {
        Ok(NippyJarCursor {
            jar: config,
            zstd_decompressors,
            data_handle: File::open(config.data_path())?,
            tmp_buf: vec![],
            row: 0,
            col: 0,
        })
    }

    /// Resets cursor to the beginning.
    pub fn reset(&mut self) {
        self.row = 0;
        self.col = 0;
    }

    /// Returns a row, searching it by an entry used during [`NippyJar::prepare_index`].
    ///
    /// **May return false positives.**
    ///
    /// Example usage would be querying a transactions file with a transaction hash which is **NOT**
    /// stored in file.
    pub fn row_by_filter(&mut self, value: &[u8]) -> Result<Option<Vec<Vec<u8>>>, NippyJarError> {
        if let (Some(filter), Some(phf)) = (&self.jar.filter, &self.jar.phf) {
            // TODO: is it worth to parallize both?

            // May have false positives
            if filter.contains(value)? {
                // May have false positives
                if let Some(row_index) = phf.get_index(value)? {
                    self.row =
                        self.jar.offsets_index.access(row_index as usize).expect("built from same")
                            as u64;
                    return self.next_row()
                }
            }
        }

        Ok(None)
    }

    /// Returns a row by its number.
    pub fn row_by_number(&mut self, row: usize) -> Result<Option<Vec<Vec<u8>>>, NippyJarError> {
        self.row = row as u64;
        self.next_row()
    }

    /// Advances cursor to next row and returns it.
    pub fn next_row(&mut self) -> Result<Option<Vec<Vec<u8>>>, NippyJarError> {
        if self.row as usize * self.jar.columns == self.jar.offsets.len() {
            // Has reached the end
            return Ok(None)
        }

        let mut row = Vec::with_capacity(self.jar.columns);
        let mut column_value = Vec::with_capacity(32);

        // Retrieve all column values from the row
        for column in 0..self.jar.columns {
            // Find out the offset of the column value
            let offset_pos = self.row as usize * self.jar.columns + column;
            let value_offset = self.jar.offsets.select(offset_pos).expect("should exist");

            // Seek to the offset
            self.data_handle.seek(SeekFrom::Start(value_offset as u64))?;

            // Copy value from offset. TODO: replace with mmap, no need to copy
            if self.jar.offsets.len() == (offset_pos + 1) {
                // It's the last column of the last row
                column_value.clear();
                self.data_handle.read_to_end(&mut column_value)?;
            } else {
                let len =
                    self.jar.offsets.select(offset_pos + 1).expect("should exist") - value_offset;
                column_value.resize(len, 0);
                self.data_handle.read_exact(&mut column_value[..len])?;
            }

            // Decompression
            if let Some(zstd_dict_decompressors) = &self.zstd_decompressors {
                // Uses zstd dictionaries
                let extra_capacity =
                    (column_value.len() * 2).saturating_sub(self.tmp_buf.capacity());
                self.tmp_buf.clear();
                self.tmp_buf.reserve(extra_capacity);

                zstd_dict_decompressors
                    .lock()
                    .unwrap()
                    .get_mut(column)
                    .unwrap()
                    .decompress_to_buffer(&column_value, &mut self.tmp_buf)?;

                row.push(self.tmp_buf.clone());
            } else if let Some(compression) = &self.jar.compressor {
                // Uses the chosen default decompressor
                row.push(compression.decompress(&column_value)?);
            } else {
                // Not compressed
                row.push(column_value.clone())
            }
        }

        if row.is_empty() {
            return Ok(None)
        }

        self.row += 1;

        Ok(Some(row))
    }
}
