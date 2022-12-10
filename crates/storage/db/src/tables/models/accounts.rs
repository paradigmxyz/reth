//! Account related models and types.

use crate::{
    impl_fixed_arbitrary,
    table::{Decode, Encode},
    Error,
};
use bytes::Bytes;
use reth_codecs::{main_codec, Compact};
use reth_primitives::{Account, Address, TxNumber};
use serde::{Deserialize, Serialize};

/// Account as it is saved inside [`AccountChangeSet`]. [`Address`] is the subkey.
/// TODO there should be `not_existing` boolean or Account be made as `Option` to
/// handle scenario where account was not present before transaction.
#[main_codec]
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct AccountBeforeTx {
    /// Address for the account. Acts as `DupSort::SubKey`.
    pub address: Address,
    /// Account state before the transaction.
    pub info: Option<Account>,
}

/// [`TxNumber`] concatenated with [`Address`]. Used as a key for [`StorageChangeSet`]
///
/// Since it's used as a key, it isn't compressed when encoding it.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxNumberAddress(pub (TxNumber, Address));

impl TxNumberAddress {
    /// Consumes `Self` and returns [`TxNumber`], [`Address`]
    pub fn take(self) -> (TxNumber, Address) {
        (self.0 .0, self.0 .1)
    }
}

impl From<(u64, Address)> for TxNumberAddress {
    fn from(tpl: (u64, Address)) -> Self {
        TxNumberAddress(tpl)
    }
}

impl Encode for TxNumberAddress {
    type Encoded = [u8; 28];

    fn encode(self) -> Self::Encoded {
        let tx = self.0 .0;
        let address = self.0 .1;

        let mut buf = [0u8; 28];

        buf[..8].copy_from_slice(&tx.to_be_bytes());
        buf[8..].copy_from_slice(address.as_bytes());
        buf
    }
}

impl Decode for TxNumberAddress {
    fn decode<B: Into<Bytes>>(value: B) -> Result<Self, Error> {
        let value: bytes::Bytes = value.into();

        let num =
            u64::from_be_bytes(value.as_ref()[..8].try_into().map_err(|_| Error::DecodeError)?);
        let hash = Address::from_slice(&value.slice(8..));

        Ok(TxNumberAddress((num, hash)))
    }
}

impl_fixed_arbitrary!(TxNumberAddress, 28);

#[cfg(test)]
mod test {
    use super::*;
    use rand::{thread_rng, Rng};
    use std::str::FromStr;

    #[test]
    fn test_tx_number_address() {
        let num = 1u64;
        let hash = Address::from_str("ba5e000000000000000000000000000000000000").unwrap();
        let key = TxNumberAddress((num, hash));

        let mut bytes = [0u8; 28];
        bytes[..8].copy_from_slice(&num.to_be_bytes());
        bytes[8..].copy_from_slice(&hash.0);

        let encoded = Encode::encode(key.clone());
        assert_eq!(encoded, bytes);

        let decoded: TxNumberAddress = Decode::decode(encoded.to_vec()).unwrap();
        assert_eq!(decoded, key);
    }

    #[test]
    fn test_tx_number_address_rand() {
        let mut bytes = [0u8; 28];
        thread_rng().fill(bytes.as_mut_slice());
        let key = TxNumberAddress::arbitrary(&mut Unstructured::new(&bytes)).unwrap();
        assert_eq!(bytes, Encode::encode(key));
    }
}
