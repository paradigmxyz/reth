use crate::{
    compression::{Compression, Compressors, Zstd},
    DataReader, InclusionFilter, NippyJar, NippyJarError, NippyJarHeader, PerfectHashingFunction,
    RefRow,
};
use std::{ops::Range, sync::Arc};
use sucds::int_vectors::Access;
use zstd::bulk::Decompressor;

/// Simple cursor implementation to retrieve data from [`NippyJar`].
#[derive(Clone)]
pub struct NippyJarCursor<'a, H = ()> {
    /// [`NippyJar`] which holds most of the required configuration to read from the file.
    jar: &'a NippyJar<H>,
    /// Data and offset reader.
    reader: Arc<DataReader>,
    /// Internal buffer to unload data to without reallocating memory on each retrieval.
    internal_buffer: Vec<u8>,
    /// Cursor row position.
    row: u64,
}

impl<'a, H: NippyJarHeader> std::fmt::Debug for NippyJarCursor<'a, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NippyJarCursor").field("config", &self.jar).finish_non_exhaustive()
    }
}

impl<'a, H: NippyJarHeader> NippyJarCursor<'a, H> {
    pub fn new(jar: &'a NippyJar<H>) -> Result<Self, NippyJarError> {
        let max_row_size = jar.max_row_size;
        Ok(NippyJarCursor {
            jar,
            reader: Arc::new(jar.open_data_reader()?),
            // Makes sure that we have enough buffer capacity to decompress any row of data.
            internal_buffer: Vec::with_capacity(max_row_size),
            row: 0,
        })
    }

    pub fn with_reader(
        jar: &'a NippyJar<H>,
        reader: Arc<DataReader>,
    ) -> Result<Self, NippyJarError> {
        let max_row_size = jar.max_row_size;
        Ok(NippyJarCursor {
            jar,
            reader,
            // Makes sure that we have enough buffer capacity to decompress any row of data.
            internal_buffer: Vec::with_capacity(max_row_size),
            row: 0,
        })
    }

    /// Returns a reference to the related [`NippyJar`]
    pub fn jar(&self) -> &NippyJar<H> {
        self.jar
    }

    /// Returns current row index of the cursor
    pub fn row_index(&self) -> u64 {
        self.row
    }

    /// Resets cursor to the beginning.
    pub fn reset(&mut self) {
        self.row = 0;
    }

    /// Returns a row, searching it by a key used during [`NippyJar::prepare_index`].
    ///
    /// **May return false positives.**
    ///
    /// Example usage would be querying a transactions file with a transaction hash which is **NOT**
    /// stored in file.
    pub fn row_by_key(&mut self, key: &[u8]) -> Result<Option<RefRow<'_>>, NippyJarError> {
        if let (Some(filter), Some(phf)) = (&self.jar.filter, &self.jar.phf) {
            // TODO: is it worth to parallize both?

            // May have false positives
            if filter.contains(key)? {
                // May have false positives
                if let Some(row_index) = phf.get_index(key)? {
                    self.row = self
                        .jar
                        .offsets_index
                        .access(row_index as usize)
                        .expect("built from same set") as u64;
                    return self.next_row()
                }
            }
        } else {
            return Err(NippyJarError::UnsupportedFilterQuery)
        }

        Ok(None)
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

        let mut row = Vec::with_capacity(self.jar.columns);

        // Retrieve all column values from the row
        for column in 0..self.jar.columns {
            self.read_value(column, &mut row)?;
        }

        self.row += 1;

        Ok(Some(
            row.into_iter()
                .map(|v| match v {
                    ValueRange::Mmap(range) => self.reader.data(range),
                    ValueRange::Internal(range) => &self.internal_buffer[range],
                })
                .collect(),
        ))
    }

    /// Returns a row, searching it by a key used during [`NippyJar::prepare_index`]  by using a
    /// `mask` to only read certain columns from the row.
    ///
    /// **May return false positives.**
    ///
    /// Example usage would be querying a transactions file with a transaction hash which is **NOT**
    /// stored in file.
    pub fn row_by_key_with_cols(
        &mut self,
        key: &[u8],
        mask: usize,
    ) -> Result<Option<RefRow<'_>>, NippyJarError> {
        if let (Some(filter), Some(phf)) = (&self.jar.filter, &self.jar.phf) {
            // TODO: is it worth to parallize both?

            // May have false positives
            if filter.contains(key)? {
                // May have false positives
                if let Some(row_index) = phf.get_index(key)? {
                    self.row = self
                        .jar
                        .offsets_index
                        .access(row_index as usize)
                        .expect("built from same set") as u64;
                    return self.next_row_with_cols(mask)
                }
            }
        } else {
            return Err(NippyJarError::UnsupportedFilterQuery)
        }

        Ok(None)
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
        let mut row = Vec::with_capacity(columns);

        for column in 0..columns {
            if mask & (1 << column) != 0 {
                self.read_value(column, &mut row)?
            }
        }
        self.row += 1;

        Ok(Some(
            row.into_iter()
                .map(|v| match v {
                    ValueRange::Mmap(range) => self.reader.data(range),
                    ValueRange::Internal(range) => &self.internal_buffer[range],
                })
                .collect(),
        ))
    }

    /// Takes the column index and reads the range value for the corresponding column.
    fn read_value(
        &mut self,
        column: usize,
        row: &mut Vec<ValueRange>,
    ) -> Result<(), NippyJarError> {
        // Find out the offset of the column value
        let offset_pos = self.row as usize * self.jar.columns + column;
        let value_offset = self.reader.offset(offset_pos) as usize;

        let column_offset_range = if self.jar.rows * self.jar.columns == offset_pos + 1 {
            // It's the last column of the last row
            value_offset..self.reader.size()
        } else {
            let next_value_offset = self.reader.offset(offset_pos + 1) as usize;
            value_offset..next_value_offset
        };

        if let Some(compression) = self.jar.compressor() {
            let from = self.internal_buffer.len();
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
                        self.reader.data(column_offset_range),
                        &mut self.internal_buffer,
                        &mut decompressor,
                    )?;
                }
                _ => {
                    // Uses the chosen default decompressor
                    compression.decompress_to(
                        self.reader.data(column_offset_range),
                        &mut self.internal_buffer,
                    )?;
                }
            }
            let to = self.internal_buffer.len();

            row.push(ValueRange::Internal(from..to));
        } else {
            // Not compressed
            row.push(ValueRange::Mmap(column_offset_range));
        }

        Ok(())
    }
}

/// Helper type that stores the range of the decompressed column value either on a `mmap` slice or
/// on the internal buffer.
enum ValueRange {
    Mmap(Range<usize>),
    Internal(Range<usize>),
}
