use super::mask::{ColumnSelectorOne, ColumnSelectorThree, ColumnSelectorTwo};
use alloy_primitives::B256;
use derive_more::{Deref, DerefMut};
use reth_db_api::table::Decompress;
use reth_nippy_jar::{DataReader, NippyJar, NippyJarCursor};
use reth_primitives::static_file::SegmentHeader;
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use std::sync::Arc;

/// Cursor of a static file segment.
#[derive(Debug, Deref, DerefMut)]
pub struct StaticFileCursor<'a>(NippyJarCursor<'a, SegmentHeader>);

/// Type alias for column results with optional values.
type ColumnResult<T> = ProviderResult<Option<T>>;

impl<'a> StaticFileCursor<'a> {
    /// Returns a new [`StaticFileCursor`].
    pub fn new(jar: &'a NippyJar<SegmentHeader>, reader: Arc<DataReader>) -> ProviderResult<Self> {
        Ok(Self(
            NippyJarCursor::with_reader(jar, reader)
                .map_err(|err| ProviderError::NippyJar(err.to_string()))?,
        ))
    }

    /// Returns the current `BlockNumber` or `TxNumber` of the cursor depending on the kind of
    /// static file segment.
    pub fn number(&self) -> Option<u64> {
        self.jar().user_header().start().map(|start| self.row_index() + start)
    }

    /// Gets a row of values.
    pub fn get(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
        mask: usize,
    ) -> ProviderResult<Option<Vec<&'_ [u8]>>> {
        if self.jar().rows() == 0 {
            return Ok(None)
        }

        let row = match key_or_num {
            KeyOrNumber::Key(_) => unimplemented!(),
            KeyOrNumber::Number(n) => match self.jar().user_header().start() {
                Some(offset) => {
                    if offset > n {
                        return Ok(None)
                    }
                    self.row_by_number_with_cols((n - offset) as usize, mask)
                }
                None => Ok(None),
            },
        }
        .map_or(None, |v| v);

        Ok(row)
    }

    /// Gets one column value from a row.
    pub fn get_one<M: ColumnSelectorOne>(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
    ) -> ColumnResult<M::FIRST> {
        let row = self.get(key_or_num, M::MASK)?;

        match row {
            Some(row) => Ok(Some(M::FIRST::decompress(row[0])?)),
            None => Ok(None),
        }
    }

    /// Gets two column values from a row.
    pub fn get_two<M: ColumnSelectorTwo>(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
    ) -> ColumnResult<(M::FIRST, M::SECOND)> {
        let row = self.get(key_or_num, M::MASK)?;

        match row {
            Some(row) => Ok(Some((M::FIRST::decompress(row[0])?, M::SECOND::decompress(row[1])?))),
            None => Ok(None),
        }
    }

    /// Gets three column values from a row.
    pub fn get_three<M: ColumnSelectorThree>(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
    ) -> ColumnResult<(M::FIRST, M::SECOND, M::THIRD)> {
        let row = self.get(key_or_num, M::MASK)?;

        match row {
            Some(row) => Ok(Some((
                M::FIRST::decompress(row[0])?,
                M::SECOND::decompress(row[1])?,
                M::THIRD::decompress(row[2])?,
            ))),
            None => Ok(None),
        }
    }
}

/// Either a key _or_ a block/tx number
#[derive(Debug)]
pub enum KeyOrNumber<'a> {
    /// A slice used as a key. Usually a block/tx hash
    Key(&'a [u8]),
    /// A block/tx number
    Number(u64),
}

impl<'a> From<&'a B256> for KeyOrNumber<'a> {
    fn from(value: &'a B256) -> Self {
        KeyOrNumber::Key(value.as_slice())
    }
}

impl From<u64> for KeyOrNumber<'_> {
    fn from(value: u64) -> Self {
        KeyOrNumber::Number(value)
    }
}
