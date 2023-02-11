#![allow(missing_docs)]
use aes::Aes256Enc;
use block_padding::NoPadding;
use cipher::BlockEncrypt;
use digest::KeyInit;
use generic_array::GenericArray;
use reth_primitives::{H128, H256};
use sha3::{Digest, Keccak256};
use typenum::U16;

pub type HeaderBytes = GenericArray<u8, U16>;

/// [`Ethereum MAC`](https://github.com/ethereum/devp2p/blob/master/rlpx.md#mac) state.
///
/// The ethereum MAC is a cursed MAC construction.
///
/// The ethereum MAC is a nonstandard MAC construction that uses AES-256 (without a mode, as a
/// block cipher) and Keccak-256. However, it only ever encrypts messages that are 128 bits long,
/// and is not defined as a general MAC.
#[derive(Debug)]
pub struct MAC {
    secret: H256,
    hasher: Keccak256,
}

impl MAC {
    /// Initialize the MAC with the given secret
    pub fn new(secret: H256) -> Self {
        Self { secret, hasher: Keccak256::new() }
    }

    /// Update the internal keccak256 hasher with the given data
    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data)
    }

    /// Accumulate the given [`HeaderBytes`] into the MAC's internal state.
    pub fn update_header(&mut self, data: &HeaderBytes) {
        let aes = Aes256Enc::new_from_slice(self.secret.as_ref()).unwrap();
        let mut encrypted = self.digest().to_fixed_bytes();

        aes.encrypt_padded::<NoPadding>(&mut encrypted, H128::len_bytes()).unwrap();
        for i in 0..data.len() {
            encrypted[i] ^= data[i];
        }
        self.hasher.update(encrypted);
    }

    /// Accumulate the given message body into the MAC's internal state.
    pub fn update_body(&mut self, data: &[u8]) {
        self.hasher.update(data);
        let prev = self.digest();
        let aes = Aes256Enc::new_from_slice(self.secret.as_ref()).unwrap();
        let mut encrypted = self.digest().to_fixed_bytes();

        aes.encrypt_padded::<NoPadding>(&mut encrypted, H128::len_bytes()).unwrap();
        for i in 0..16 {
            encrypted[i] ^= prev[i];
        }
        self.hasher.update(encrypted);
    }

    /// Produce a digest by finalizing the internal keccak256 hasher and returning the first 128
    /// bits.
    pub fn digest(&self) -> H128 {
        H128::from_slice(&self.hasher.clone().finalize()[..16])
    }
}
