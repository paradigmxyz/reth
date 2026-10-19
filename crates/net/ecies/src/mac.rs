//! # Ethereum MAC Module
//!
//! This module provides the implementation of the Ethereum MAC (Message Authentication Code)
//! construction, as specified in the Ethereum `RLPx` protocol.
//!
//! The Ethereum MAC is a nonstandard MAC construction that utilizes AES-256 (as a block cipher)
//! and Keccak-256. It is specifically designed for messages of 128 bits in length and is not
//! intended for general MAC use.
//!
//! For more information, refer to the [Ethereum MAC specification](https://github.com/ethereum/devp2p/blob/master/rlpx.md#mac).

use aes::{Aes256Enc, Block};
use alloy_primitives::{Keccak256, B128, B256};
use cipher::BlockEncrypt;
use digest::KeyInit;

/// [`Ethereum MAC`](https://github.com/ethereum/devp2p/blob/master/rlpx.md#mac) state.
///
/// The ethereum MAC is a cursed MAC construction.
///
/// The ethereum MAC is a nonstandard MAC construction that uses AES-256 (without a mode, as a
/// block cipher) and Keccak-256. However, it only ever encrypts messages that are 128 bits long,
/// and is not defined as a general MAC.
#[derive(Debug)]
pub struct MAC {
    /// AES-256 block cipher keyed with the MAC secret.
    ///
    /// The secret is fixed for the lifetime of the connection, so the key schedule is expanded
    /// once here instead of on every header/body update.
    aes: Aes256Enc,
    hasher: Keccak256,
}

impl MAC {
    /// Initialize the MAC with the given secret
    pub fn new(secret: B256) -> Self {
        Self {
            aes: Aes256Enc::new_from_slice(secret.as_ref())
                .expect("32 bytes is a valid AES-256 key"),
            hasher: Keccak256::new(),
        }
    }

    /// Update the internal keccak256 hasher with the given data
    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data)
    }

    /// Accumulate the given header bytes into the MAC's internal state.
    pub fn update_header(&mut self, data: &[u8; 16]) {
        let mut encrypted = self.digest();

        self.aes.encrypt_block(Block::from_mut_slice(encrypted.as_mut_slice()));
        self.hasher.update(encrypted ^ B128::from(data));
    }

    /// Accumulate the given message body into the MAC's internal state.
    pub fn update_body(&mut self, data: &[u8]) {
        self.hasher.update(data);
        let prev = self.digest();
        let mut encrypted = prev;

        self.aes.encrypt_block(Block::from_mut_slice(encrypted.as_mut_slice()));
        self.hasher.update(encrypted ^ prev);
    }

    /// Produce a digest by finalizing the internal keccak256 hasher and returning the first 128
    /// bits.
    pub fn digest(&self) -> B128 {
        B128::from_slice(&self.hasher.clone().finalize()[..16])
    }
}
