use alloy_primitives::{B256, U256};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables::PlainStorageState,
    DatabaseError,
};

/// Iterator over wiped storage entries that reads directly from the database cursor.
/// This avoids the need to collect all wiped entries into a Vec first.
#[derive(Debug)]
pub struct WipedStorageIter<'a, C> {
    cursor: &'a mut C,
    address: alloy_primitives::Address,
    is_first: bool,
    has_more: bool,
}

impl<'a, C> WipedStorageIter<'a, C>
where
    C: DbCursorRO<PlainStorageState> + DbDupCursorRO<PlainStorageState>,
{
    /// Create a new iterator for wiped storage entries at the given address.
    pub fn new(
        cursor: &'a mut C,
        address: alloy_primitives::Address,
    ) -> Result<Self, DatabaseError> {
        // Position the cursor at the first entry for this address
        let has_more = cursor.seek_exact(address)?.is_some();

        Ok(Self { cursor, address, is_first: true, has_more })
    }
}

impl<'a, C> Iterator for WipedStorageIter<'a, C>
where
    C: DbCursorRO<PlainStorageState> + DbDupCursorRO<PlainStorageState>,
{
    type Item = (B256, U256);

    fn next(&mut self) -> Option<Self::Item> {
        if !self.has_more {
            return None;
        }

        if self.is_first {
            self.is_first = false;
            // For the first item, get the current position
            if let Ok(Some((current_address, entry))) = self.cursor.current() {
                if current_address == self.address {
                    Some((entry.key, entry.value))
                } else {
                    self.has_more = false;
                    None
                }
            } else {
                self.has_more = false;
                None
            }
        } else {
            // Move to next duplicate value
            match self.cursor.next_dup_val() {
                Ok(Some(entry)) => {
                    // For duplicate values, the address remains the same, so we don't need to check
                    Some((entry.key, entry.value))
                }
                Ok(None) | Err(_) => {
                    self.has_more = false;
                    None
                }
            }
        }
    }
}
