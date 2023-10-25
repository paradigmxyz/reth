use super::mask::{ColumnMaskOne, ColumnMaskThree, ColumnMaskTwo};
use crate::table::Decompress;
use derive_more::{Deref, DerefMut};
use reth_interfaces::{RethError, RethResult};
use reth_nippy_jar::{MmapHandle, NippyJar, NippyJarCursor};
use reth_primitives::{snapshot::SegmentHeader, B256};

/// Cursor of a snapshot segment.
#[derive(Debug, Deref, DerefMut)]
pub struct SnapshotCursor<'a>(NippyJarCursor<'a, SegmentHeader>);

impl<'a> SnapshotCursor<'a> {
    /// Returns a new [`SnapshotCursor`].
    pub fn new(
        jar: &'a NippyJar<SegmentHeader>,
        mmap_handle: MmapHandle,
    ) -> Result<Self, RethError> {
        Ok(Self(NippyJarCursor::with_handle(jar, mmap_handle)?))
    }

    /// Gets a row of values.
    pub fn get(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
        mask: usize,
    ) -> RethResult<Option<Vec<&'_ [u8]>>> {
        let row = match key_or_num {
            KeyOrNumber::Hash(k) => self.row_by_key_with_cols(k, mask),
            KeyOrNumber::Number(n) => {
                let offset = self.jar().user_header().start();
                if offset > n {
                    return Ok(None)
                }
                self.row_by_number_with_cols((n - offset) as usize, mask)
            }
        }?;

        Ok(row)
    }

    /// Gets one column value from a row.
    pub fn get_one<M: ColumnMaskOne>(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
    ) -> RethResult<Option<M::T>> {
        let row = self.get(key_or_num, M::MASK)?;

        match row {
            Some(row) => Ok(Some(M::T::decompress(row[0])?)),
            None => Ok(None),
        }
    }

    /// Gets two column values from a row.
    pub fn get_two<M: ColumnMaskTwo>(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
    ) -> RethResult<Option<(M::T, M::J)>> {
        let row = self.get(key_or_num, M::MASK)?;

        match row {
            Some(row) => Ok(Some((M::T::decompress(row[0])?, M::J::decompress(row[1])?))),
            None => Ok(None),
        }
    }

    /// Gets three column values from a row.
    #[allow(clippy::type_complexity)]
    pub fn get_three<M: ColumnMaskThree>(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
    ) -> RethResult<Option<(M::T, M::J, M::K)>> {
        let row = self.get(key_or_num, M::MASK)?;

        match row {
            Some(row) => Ok(Some((
                M::T::decompress(row[0])?,
                M::J::decompress(row[1])?,
                M::K::decompress(row[2])?,
            ))),
            None => Ok(None),
        }
    }
}

/// Either a key _or_ a block/tx number
#[derive(Debug)]
pub enum KeyOrNumber<'a> {
    /// A slice used as a key. Usually a block/tx hash
    Hash(&'a [u8]),
    /// A block/tx number
    Number(u64),
}

impl<'a> From<&'a B256> for KeyOrNumber<'a> {
    fn from(value: &'a B256) -> Self {
        KeyOrNumber::Hash(value.as_slice())
    }
}

impl<'a> From<u64> for KeyOrNumber<'a> {
    fn from(value: u64) -> Self {
        KeyOrNumber::Number(value)
    }
}
