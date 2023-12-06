use super::mask::{ColumnSelectorOne, ColumnSelectorThree, ColumnSelectorTwo};
use crate::table::Decompress;
use derive_more::{Deref, DerefMut};
use reth_interfaces::provider::ProviderResult;
use reth_nippy_jar::{MmapHandle, NippyJar, NippyJarCursor};
use reth_primitives::{snapshot::SegmentHeader, B256};

/// Cursor of a snapshot segment.
#[derive(Debug, Deref, DerefMut)]
pub struct SnapshotCursor<'a>(NippyJarCursor<'a, SegmentHeader>);

impl<'a> SnapshotCursor<'a> {
    /// Returns a new [`SnapshotCursor`].
    pub fn new(jar: &'a NippyJar<SegmentHeader>, mmap_handle: MmapHandle) -> ProviderResult<Self> {
        Ok(Self(NippyJarCursor::with_handle(jar, mmap_handle)?))
    }

    /// Returns the current `BlockNumber` or `TxNumber` of the cursor depending on the kind of
    /// snapshot segment.
    pub fn number(&self) -> u64 {
        self.row_index() + self.jar().user_header().start()
    }

    /// Gets a row of values.
    pub fn get(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
        mask: usize,
    ) -> ProviderResult<Option<Vec<&'_ [u8]>>> {
        let row = match key_or_num {
            KeyOrNumber::Key(k) => self.row_by_key_with_cols(k, mask),
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
    pub fn get_one<M: ColumnSelectorOne>(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
    ) -> ProviderResult<Option<M::FIRST>> {
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
    ) -> ProviderResult<Option<(M::FIRST, M::SECOND)>> {
        let row = self.get(key_or_num, M::MASK)?;

        match row {
            Some(row) => Ok(Some((M::FIRST::decompress(row[0])?, M::SECOND::decompress(row[1])?))),
            None => Ok(None),
        }
    }

    /// Gets three column values from a row.
    #[allow(clippy::type_complexity)]
    pub fn get_three<M: ColumnSelectorThree>(
        &mut self,
        key_or_num: KeyOrNumber<'_>,
    ) -> ProviderResult<Option<(M::FIRST, M::SECOND, M::THIRD)>> {
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

impl<'a> From<u64> for KeyOrNumber<'a> {
    fn from(value: u64) -> Self {
        KeyOrNumber::Number(value)
    }
}
