use crate::{compression::Compression, Filter, KeySet, NippyJar, NippyJarError};
use std::{
    clone::Clone,
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::Mutex,
};
use sucds::int_vectors::Access;
use zstd::bulk::Decompressor;

pub struct NippyJarCursor<'a> {
    config: &'a NippyJar,
    zstd_decompressors: Option<Mutex<Vec<Decompressor<'a>>>>,
    data_handle: File,
    tmp_buf: Vec<u8>,
    row: u64,
    col: u64,
}

impl<'a> std::fmt::Debug for NippyJarCursor<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NippyJarCursor {{ config: {:?} }}", self.config)
    }
}

impl<'a> NippyJarCursor<'a> {
    pub fn new(
        config: &'a NippyJar,
        zstd_decompressors: Option<Mutex<Vec<Decompressor<'a>>>>,
    ) -> Result<Self, NippyJarError> {
        Ok(NippyJarCursor {
            config,
            zstd_decompressors,
            data_handle: File::open(config.data_path())?,
            tmp_buf: vec![],
            row: 0,
            col: 0,
        })
    }

    pub fn reset(&mut self) {
        self.row = 0;
        self.col = 0;
    }

    // eg. transaction_by_hash
    // may have false positives
    pub fn row_by_filter(&mut self, value: &[u8]) -> Result<Option<Vec<Vec<u8>>>, NippyJarError> {
        if let (Some(filter), Some(phf)) = (&self.config.filter, &self.config.phf) {
            // May have false positives
            if filter.contains(value)? {
                // May have false positives
                let row_index = phf.get_index(value)?.unwrap();

                self.row = self.config.offsets_index.access(row_index as usize).unwrap() as u64;
                return self.next_row()
            }
        } else {
            panic!("requires it");
        }

        panic!("didnt find it");
    }

    // eg. transaction_by_nnumber
    pub fn row_by_number(&mut self, row: usize) -> Result<Option<Vec<Vec<u8>>>, NippyJarError> {
        self.row = row as u64;
        self.next_row()
    }

    pub fn next_row(&mut self) -> Result<Option<Vec<Vec<u8>>>, NippyJarError> {
        if self.row as usize * self.config.columns == self.config.offsets.len() {
            // Has reached the end
            return Ok(None)
        }

        let mut row = Vec::with_capacity(self.config.columns);
        let mut column_value = Vec::with_capacity(32);

        for column in 0..self.config.columns {
            let index_offset = self.row as usize * self.config.columns + column;
            let value_offset = self.config.offsets.select(index_offset).unwrap();

            self.data_handle.seek(SeekFrom::Start(value_offset as u64))?;

            // It's the last column of the last row
            if self.config.offsets.len() == (index_offset + 1) {
                column_value.clear();
                self.data_handle.read_to_end(&mut column_value)?;
            } else {
                let len = self.config.offsets.select(index_offset + 1).unwrap() - value_offset;
                column_value.resize(len, 0);
                self.data_handle.read_exact(&mut column_value[..len])?;
            }

            if let Some(zstd_dict_decompressors) = &self.zstd_decompressors {
                // Decompress using zstd dictionaries
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
            } else if let Some(compression) = &self.config.compressor {
                // Decompress using the vanilla
                row.push(compression.decompress(&column_value)?);
            } else {
                // It's not compressed
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
