use alloy_primitives::Address;
use reth_primitives_traits::{Account, ValueWithSubKey};

/// Account as it is saved in the database.
///
/// [`Address`] is the subkey.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub struct AccountBeforeTx {
    /// Address for the account. Acts as `DupSort::SubKey`.
    pub address: Address,
    /// Account state before the transaction.
    pub info: Option<Account>,
}

impl ValueWithSubKey for AccountBeforeTx {
    type SubKey = Address;

    fn get_subkey(&self) -> Self::SubKey {
        self.address
    }
}

// NOTE: Removing reth_codec and manually encode subkey
// and compress second part of the value. If we have compression
// over whole value (Even SubKey) that would mess up fetching of values with seek_by_key_subkey
#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for AccountBeforeTx {
    #[inline]
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        // for now put full bytes and later compress it.
        buf.put_slice(self.address.as_slice());

        let acc_len = if let Some(account) = self.info { account.to_compact(buf) } else { 0 };
        acc_len + 20
    }

    #[inline]
    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let mut address = [0u8; 20];
        address.copy_from_slice(&buf[..20]);
        let address = Address::new(address);
        let mut rest = &buf[20..];

        let info = (len > 20).then(|| {
            let (acc, advanced_buf) = Account::from_compact(rest, len - 20);
            rest = advanced_buf;
            acc
        });

        (Self { address, info }, rest)
    }
}

#[cfg(any(test, feature = "reth-codec"))]
reth_codecs::impl_compression_for_compact!(AccountBeforeTx);
