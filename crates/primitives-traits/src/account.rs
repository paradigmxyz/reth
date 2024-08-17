use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_genesis::GenesisAccount;
use alloy_primitives::{keccak256, Bytes, B256, U256};
use byteorder::{BigEndian, ReadBytesExt};
use bytes::Buf;
use derive_more::Deref;
use reth_codecs::{add_arbitrary_tests, Compact};
use revm_primitives::{AccountInfo, Bytecode as RevmBytecode, JumpTable};
use serde::{Deserialize, Serialize};

/// An Ethereum account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize, Compact)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub struct Account {
    /// Account nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
    /// Hash of the account's bytecode.
    pub bytecode_hash: Option<B256>,
}

impl Account {
    /// Whether the account has bytecode.
    pub const fn has_bytecode(&self) -> bool {
        self.bytecode_hash.is_some()
    }

    /// After `SpuriousDragon` empty account is defined as account with nonce == 0 && balance == 0
    /// && bytecode = None (or hash is [`KECCAK_EMPTY`]).
    pub fn is_empty(&self) -> bool {
        self.nonce == 0 &&
            self.balance.is_zero() &&
            self.bytecode_hash.map_or(true, |hash| hash == KECCAK_EMPTY)
    }

    /// Makes an [Account] from [`GenesisAccount`] type
    pub fn from_genesis_account(value: &GenesisAccount) -> Self {
        Self {
            // nonce must exist, so we default to zero when converting a genesis account
            nonce: value.nonce.unwrap_or_default(),
            balance: value.balance,
            bytecode_hash: value.code.as_ref().map(keccak256),
        }
    }

    /// Returns an account bytecode's hash.
    /// In case of no bytecode, returns [`KECCAK_EMPTY`].
    pub fn get_bytecode_hash(&self) -> B256 {
        self.bytecode_hash.unwrap_or(KECCAK_EMPTY)
    }
}

/// Bytecode for an account.
///
/// A wrapper around [`revm::primitives::Bytecode`][RevmBytecode] with encoding/decoding support.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Deref)]
pub struct Bytecode(pub RevmBytecode);

impl Bytecode {
    /// Create new bytecode from raw bytes.
    ///
    /// No analysis will be performed.
    ///
    /// # Panics
    ///
    /// Panics if bytecode is EOF and has incorrect format.
    pub fn new_raw(bytes: Bytes) -> Self {
        Self(RevmBytecode::new_raw(bytes))
    }
}

impl Compact for Bytecode {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        // TODO: this is a hack, ideally we should store the bytecode type first
        let bytecode =
            if self.0.is_eof() { self.0.original_byte_slice() } else { &self.0.bytecode()[..] };
        buf.put_u32(bytecode.len() as u32);
        buf.put_slice(bytecode);
        let len = match &self.0 {
            RevmBytecode::LegacyRaw(_) => {
                buf.put_u8(0);
                1
            }
            // `1` has been removed.
            RevmBytecode::LegacyAnalyzed(analyzed) => {
                buf.put_u8(2);
                buf.put_u64(analyzed.original_len() as u64);
                let map = analyzed.jump_table().as_slice();
                buf.put_slice(map);
                1 + 8 + map.len()
            }
            RevmBytecode::Eof(eof) => {
                buf.put_u8(3);
                buf.put_slice(eof.raw().as_ref());
                1 + eof.raw().as_ref().len()
            }
        };
        len + bytecode.len() + 4
    }

    // # Panics
    //
    // A panic will be triggered if a bytecode variant of 1 or greater than 2 is passed from the
    // database.
    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let len = buf.read_u32::<BigEndian>().expect("could not read bytecode length");
        let bytes = Bytes::from(buf.copy_to_bytes(len as usize));
        let variant = buf.read_u8().expect("could not read bytecode variant");
        let decoded = match variant {
            0 => Self(RevmBytecode::new_raw(bytes)),
            1 => unreachable!("Junk data in database: checked Bytecode variant was removed"),
            2 => Self(unsafe {
                RevmBytecode::new_analyzed(
                    bytes,
                    buf.read_u64::<BigEndian>().unwrap() as usize,
                    JumpTable::from_slice(buf),
                )
            }),
            3 => {
                // EOF bytecode object will be decoded from the raw bytecode
                Self(RevmBytecode::new_raw(bytes))
            }
            _ => unreachable!("Junk data in database: unknown Bytecode variant"),
        };
        (decoded, &[])
    }
}

impl From<AccountInfo> for Account {
    fn from(revm_acc: AccountInfo) -> Self {
        let code_hash = revm_acc.code_hash;
        Self {
            balance: revm_acc.balance,
            nonce: revm_acc.nonce,
            bytecode_hash: (code_hash != KECCAK_EMPTY).then_some(code_hash),
        }
    }
}

impl From<Account> for AccountInfo {
    fn from(reth_acc: Account) -> Self {
        Self {
            balance: reth_acc.balance,
            nonce: reth_acc.nonce,
            code_hash: reth_acc.bytecode_hash.unwrap_or(KECCAK_EMPTY),
            code: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{hex_literal::hex, Address, B256, U256};
    use revm_primitives::LegacyAnalyzedBytecode;

    #[test]
    fn test_account() {
        let mut buf = vec![];
        let mut acc = Account::default();
        let len = acc.to_compact(&mut buf);
        assert_eq!(len, 2);

        acc.balance = U256::from(2);
        let len = acc.to_compact(&mut buf);
        assert_eq!(len, 3);

        acc.nonce = 2;
        let len = acc.to_compact(&mut buf);
        assert_eq!(len, 4);
    }

    #[test]
    fn test_empty_account() {
        let mut acc = Account { nonce: 0, balance: U256::ZERO, bytecode_hash: None };
        // Nonce 0, balance 0, and bytecode hash set to None is considered empty.
        assert!(acc.is_empty());

        acc.bytecode_hash = Some(KECCAK_EMPTY);
        // Nonce 0, balance 0, and bytecode hash set to KECCAK_EMPTY is considered empty.
        assert!(acc.is_empty());

        acc.balance = U256::from(2);
        // Non-zero balance makes it non-empty.
        assert!(!acc.is_empty());

        acc.balance = U256::ZERO;
        acc.nonce = 10;
        // Non-zero nonce makes it non-empty.
        assert!(!acc.is_empty());

        acc.nonce = 0;
        acc.bytecode_hash = Some(B256::from(U256::ZERO));
        // Non-empty bytecode hash makes it non-empty.
        assert!(!acc.is_empty());
    }

    #[test]
    fn test_bytecode() {
        let mut buf = vec![];
        let bytecode = Bytecode::new_raw(Bytes::default());
        let len = bytecode.to_compact(&mut buf);
        assert_eq!(len, 5);

        let mut buf = vec![];
        let bytecode = Bytecode::new_raw(Bytes::from(&hex!("ffff")));
        let len = bytecode.to_compact(&mut buf);
        assert_eq!(len, 7);

        let mut buf = vec![];
        let bytecode = Bytecode(RevmBytecode::LegacyAnalyzed(LegacyAnalyzedBytecode::new(
            Bytes::from(&hex!("ffff")),
            2,
            JumpTable::from_slice(&[0]),
        )));
        let len = bytecode.to_compact(&mut buf);
        assert_eq!(len, 16);

        let (decoded, remainder) = Bytecode::from_compact(&buf, len);
        assert_eq!(decoded, bytecode);
        assert!(remainder.is_empty());

        let mut buf = vec![];
        let bytecode = Bytecode(
            RevmBytecode::new_raw(
                Bytes::from(
                    &hex!("ef0001010004020001007c04006800008000056080806040526004361015e100035f80fd5f3560e01c8063964efccb14e1002e63f6b4dfb41415e1ffe434e1001d5f600319360112e1001060209060018060a01b035f54168152f35f80fd5f80fd34e100275f600319360112e1001a306bffffffffffffffffffffffff60a01b5f5416175f555f80f35f80fd5f80fda3646970667358221220d3bad1e010bf2bc1b82399d0fc56a03009ef435628e0d1ef547138fcfcb470d86c6578706572696d656e74616cf564736f6c637827302e382e32372d646576656c6f702e323032342e382e352b636f6d6d69742e38386366363036300066")
                )
            )
        );
        let len = bytecode.to_compact(&mut buf);
        assert_eq!(len, 499);

        let (decoded, remainder) = Bytecode::from_compact(&buf, len);
        assert_eq!(decoded, bytecode);
        assert!(remainder.is_empty());

        let mut buf = vec![];
        let bytecode = Bytecode(RevmBytecode::new_eip7702(Address::with_last_byte(42)));
        let len = bytecode.to_compact(&mut buf);
        assert_eq!(len, 51);

        let (decoded, remainder) = Bytecode::from_compact(&buf, len);
        assert_eq!(decoded, bytecode);
        assert!(remainder.is_empty());
    }
}
