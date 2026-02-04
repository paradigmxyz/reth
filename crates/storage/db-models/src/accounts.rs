use alloy_primitives::B256;
use reth_primitives_traits::{Account, ValueWithSubKey};

/// Account as it is saved in the database.
///
/// [`B256`] (hashed address) is the subkey.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub struct AccountBeforeTx {
    /// Hashed address for the account. Acts as `DupSort::SubKey`.
    pub hashed_address: B256,
    /// Account state before the transaction.
    pub info: Option<Account>,
}

impl ValueWithSubKey for AccountBeforeTx {
    type SubKey = B256;

    fn get_subkey(&self) -> Self::SubKey {
        self.hashed_address
    }
}

// NOTE: Removing reth_codec and manually encode subkey
// and compress second part of the value. If we have compression
// over whole value (Even SubKey) that would mess up fetching of values with seek_by_key_subkey
#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for AccountBeforeTx {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        // for now put full bytes and later compress it.
        buf.put_slice(self.hashed_address.as_slice());

        let acc_len = if let Some(account) = self.info { account.to_compact(buf) } else { 0 };
        acc_len + 32
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        use bytes::Buf;
        let hashed_address = B256::from_slice(&buf[..32]);
        buf.advance(32);

        let info = (len - 32 > 0).then(|| {
            let (acc, advanced_buf) = Account::from_compact(buf, len - 32);
            buf = advanced_buf;
            acc
        });

        (Self { hashed_address, info }, buf)
    }
}
