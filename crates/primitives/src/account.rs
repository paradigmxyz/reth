use crate::{H256, KECCAK_EMPTY, U256};
use bytes::{Buf, BufMut, Bytes};
use fixed_hash::byteorder::{BigEndian, ReadBytesExt};
use reth_codecs::{main_codec, Compact};
use revm_primitives::{Bytecode as RevmBytecode, BytecodeState, JumpMap};
use serde::{Deserialize, Serialize};
use std::ops::Deref;

/// An Ethereum account.
#[main_codec]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Account {
    /// Account nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
    /// Hash of the account's bytecode.
    pub bytecode_hash: Option<H256>,
}

impl Account {
    /// Whether the account has bytecode.
    pub fn has_bytecode(&self) -> bool {
        self.bytecode_hash.is_some()
    }

    /// After SpuriousDragon empty account is defined as account with nonce == 0 && balance == 0 &&
    /// bytecode = None.
    pub fn is_empty(&self) -> bool {
        let is_bytecode_empty = match self.bytecode_hash {
            None => true,
            Some(hash) => hash == KECCAK_EMPTY,
        };

        self.nonce == 0 && self.balance == U256::ZERO && is_bytecode_empty
    }

    /// Returns an account bytecode's hash.
    /// In case of no bytecode, returns [`KECCAK_EMPTY`].
    pub fn get_bytecode_hash(&self) -> H256 {
        match self.bytecode_hash {
            Some(hash) => hash,
            None => KECCAK_EMPTY,
        }
    }
}

/// Bytecode for an account.
///
/// A wrapper around [`revm::primitives::Bytecode`][RevmBytecode] with encoding/decoding support.
///
/// Note: Upon decoding bytecode from the database, you *should* set the code hash using
/// [`Self::with_code_hash`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bytecode(pub RevmBytecode);

impl Bytecode {
    /// Create new bytecode from raw bytes.
    ///
    /// No analysis will be performed.
    pub fn new_raw(bytes: Bytes) -> Self {
        Self(RevmBytecode::new_raw(bytes))
    }

    /// Set the hash of the inner bytecode.
    pub fn with_code_hash(mut self, code_hash: H256) -> Self {
        self.0.hash = code_hash;
        self
    }
}

impl Deref for Bytecode {
    type Target = RevmBytecode;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Compact for Bytecode {
    fn to_compact(self, buf: &mut impl BufMut) -> usize {
        buf.put_u32(self.0.bytecode.len() as u32);
        buf.put_slice(self.0.bytecode.as_ref());
        let len = match self.0.state() {
            BytecodeState::Raw => {
                buf.put_u8(0);
                1
            }
            BytecodeState::Checked { len } => {
                buf.put_u8(1);
                buf.put_u64(*len as u64);
                9
            }
            BytecodeState::Analysed { len, jump_map } => {
                buf.put_u8(2);
                buf.put_u64(*len as u64);
                let map = jump_map.as_slice();
                buf.put_slice(map);
                9 + map.len()
            }
        };
        len + self.0.bytecode.len() + 4
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8])
    where
        Self: Sized,
    {
        let len = buf.read_u32::<BigEndian>().expect("could not read bytecode length");
        let bytes = buf.copy_to_bytes(len as usize);
        let variant = buf.read_u8().expect("could not read bytecode variant");
        let decoded = match variant {
            0 => Bytecode(RevmBytecode::new_raw(bytes)),
            1 => Bytecode(unsafe {
                RevmBytecode::new_checked(
                    bytes,
                    buf.read_u64::<BigEndian>().unwrap() as usize,
                    None,
                )
            }),
            2 => Bytecode(RevmBytecode {
                bytecode: bytes,
                hash: KECCAK_EMPTY,
                state: BytecodeState::Analysed {
                    len: buf.read_u64::<BigEndian>().unwrap() as usize,
                    jump_map: JumpMap::from_slice(buf),
                },
            }),
            _ => unreachable!(),
        };
        (decoded, &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_literal::hex;

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
    fn test_bytecode() {
        let mut buf = vec![];
        let mut bytecode = Bytecode(RevmBytecode::new_raw(Bytes::default()));
        let len = bytecode.clone().to_compact(&mut buf);
        assert_eq!(len, 5);

        let mut buf = vec![];
        bytecode.0.bytecode = Bytes::from(hex!("ffff").as_ref());
        let len = bytecode.clone().to_compact(&mut buf);
        assert_eq!(len, 7);

        let mut buf = vec![];
        bytecode.0.state = BytecodeState::Analysed { len: 2, jump_map: JumpMap::from_slice(&[0]) };
        let len = bytecode.clone().to_compact(&mut buf);
        assert_eq!(len, 16);

        let (decoded, remainder) = Bytecode::from_compact(&buf, len);
        assert_eq!(decoded, bytecode);
        assert!(remainder.is_empty());
    }
}
