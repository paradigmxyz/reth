use core::ops::{Range, RangeInclusive};
use serde::{Deserialize, Serialize};

use alloy_primitives::{Address, BlockNumber, StorageKey};
use reth_primitives_traits::Account;

/// Account as it is saved in the database.
///
/// [`Address`] is the subkey.
#[derive(Debug, Default, Clone, Eq, PartialEq, Serialize)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary, serde::Deserialize))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub struct AccountBeforeTx {
    /// Address for the account. Acts as `DupSort::SubKey`.
    pub address: Address,
    /// Account state before the transaction.
    pub info: Option<Account>,
}

// NOTE: Removing reth_codec and manually encode subkey
// and compress second part of the value. If we have compression
// over whole value (Even SubKey) that would mess up fetching of values with seek_by_key_subkey
#[cfg(feature = "reth-codec")]
impl reth_codecs::Compact for AccountBeforeTx {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        // for now put full bytes and later compress it.
        buf.put_slice(self.address.as_slice());

        let mut acc_len = 0;
        if let Some(account) = self.info {
            acc_len = account.to_compact(buf);
        }
        acc_len + 20
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        use bytes::Buf;
        let address = Address::from_slice(&buf[..20]);
        buf.advance(20);

        let info = (len - 20 > 0).then(|| {
            let (acc, advanced_buf) = Account::from_compact(buf, len - 20);
            buf = advanced_buf;
            acc
        });

        (Self { address, info }, buf)
    }
}

/// [`BlockNumber`] concatenated with [`Address`].
///
/// Since it's used as a key, it isn't compressed when encoding it.
#[derive(
    Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd, Hash,
)]
pub struct BlockNumberAddress(pub (BlockNumber, Address));

impl BlockNumberAddress {
    /// Create a new Range from `start` to `end`
    ///
    /// Note: End is inclusive
    pub fn range(range: RangeInclusive<BlockNumber>) -> Range<Self> {
        (*range.start(), Address::ZERO).into()..(*range.end() + 1, Address::ZERO).into()
    }

    /// Return the block number
    pub const fn block_number(&self) -> BlockNumber {
        self.0 .0
    }

    /// Return the address
    pub const fn address(&self) -> Address {
        self.0 .1
    }

    /// Consumes `Self` and returns [`BlockNumber`], [`Address`]
    pub const fn take(self) -> (BlockNumber, Address) {
        (self.0 .0, self.0 .1)
    }
}

impl From<(BlockNumber, Address)> for BlockNumberAddress {
    fn from(tpl: (u64, Address)) -> Self {
        Self(tpl)
    }
}

/// [`Address`] concatenated with [`StorageKey`]. Used by `reth_etl` and history stages.
///
/// Since it's used as a key, it isn't compressed when encoding it.
#[derive(
    Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd, Hash,
)]
pub struct AddressStorageKey(pub (Address, StorageKey));
