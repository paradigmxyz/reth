//! Implementation of consensus layer messages[ClayerConsensusMessage]
use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use reth_codecs::derive_arbitrary;

use reth_primitives::{
    bytes::{Buf, BufMut},
    keccak256, public_key_to_address, Bytes, B256,
};
use secp256k1::{PublicKey, SecretKey};

use crate::{prepare::Prepare, preprepare::PrePrepare, signature::VerifySignatureError};

use super::signature::ClayerSignature;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Consensus layer message body
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ClayerConsensusMessageBody {
    /// consensus type
    pub ctype: u8,
    /// consensus body
    pub body: Bytes,
}

impl ClayerConsensusMessageBody {
    /// Compute the hash of the message
    pub fn hash_slow(&self) -> B256 {
        let mut out = bytes::BytesMut::new();
        self.encode(&mut out);
        keccak256(&out)
    }
}

/// Consensus layer message
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ClayerConsensusMessage {
    /// consensus body
    pub body: ClayerConsensusMessageBody,
    /// consensus signature
    pub signature: ClayerSignature,
    /// public key serialize
    pub author: Bytes,
}

// /// Consensus layer message
// #[derive_arbitrary(rlp)]
// #[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
// #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
// pub struct ClayerConsensusMessage {
//     /// consensus type
//     pub ctype: u8,
//     /// consensus body
//     pub body: Bytes,
//     /// consensus signature
//     pub signature: ClayerSignature,
//     /// public key serialize
//     pub author: Bytes,
// }

impl ClayerConsensusMessage {
    /// Create new message from [PrePrepare]
    pub fn from_pre_prepare(
        sk: SecretKey,
        msg: &PrePrepare,
    ) -> std::result::Result<ClayerConsensusMessage, VerifySignatureError> {
        let body = ClayerConsensusMessageBody {
            ctype: ClayerMessageID::PrePrepare as u8,
            body: msg.as_bytes(),
        };

        let signature_hash = body.hash_slow();
        let signature = ClayerSignature::sign_hash(sk, signature_hash)?;
        let pk = sk.public_key(secp256k1::SECP256K1);
        let author = Bytes::copy_from_slice(&pk.serialize());
        Ok(Self { body, signature, author })
    }

    /// Create new message from [Prepare]
    pub fn from_prepare(
        sk: SecretKey,
        msg: &Prepare,
    ) -> std::result::Result<ClayerConsensusMessage, VerifySignatureError> {
        let body = ClayerConsensusMessageBody {
            ctype: ClayerMessageID::Prepare as u8,
            body: msg.as_bytes(),
        };
        let signature_hash = body.hash_slow();
        let signature = ClayerSignature::sign_hash(sk, signature_hash)?;
        let pk = sk.public_key(secp256k1::SECP256K1);
        let author = Bytes::copy_from_slice(&pk.serialize());
        Ok(Self { body, signature, author })
    }

    /// Create new message from [Commit]
    pub fn from_commit(
        sk: SecretKey,
        msg: &Prepare,
    ) -> std::result::Result<ClayerConsensusMessage, VerifySignatureError> {
        let body = ClayerConsensusMessageBody {
            ctype: ClayerMessageID::Commit as u8,
            body: msg.as_bytes(),
        };
        let signature_hash = body.hash_slow();
        let signature = ClayerSignature::sign_hash(sk, signature_hash)?;
        let pk = sk.public_key(secp256k1::SECP256K1);
        let author = Bytes::copy_from_slice(&pk.serialize());
        Ok(Self { body, signature, author })
    }

    // pub fn ctype(&self) -> ClayerMessageID {
    //     ClayerMessageID::try_from(self.body.ctype)
    // }

    /// Verify message
    pub fn verify(&self) -> std::result::Result<bool, VerifySignatureError> {
        let signature_hash = self.body.hash_slow();
        let recovered = match self.signature.recover_signer(signature_hash) {
            Some(addr) => addr,
            None => return Err(VerifySignatureError::VerifyError),
        };

        let pk = PublicKey::from_slice(&self.author);
        let pk = pk.map_err(|_| VerifySignatureError::VerifyError)?;
        let expected = public_key_to_address(pk);
        Ok(recovered == expected)
    }
}

/// Represents message IDs for eth protocol messages.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum ClayerMessageID {
    /// Pre-prepare message
    PrePrepare = 0x00,
    /// Prepare message
    Prepare = 0x01,
    /// Commit message
    Commit = 0x02,
}

impl Encodable for ClayerMessageID {
    fn encode(&self, out: &mut dyn BufMut) {
        out.put_u8(*self as u8);
    }
    fn length(&self) -> usize {
        1
    }
}

impl Decodable for ClayerMessageID {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let id = buf.first().ok_or(alloy_rlp::Error::InputTooShort)?;
        let id = match id {
            0x00 => ClayerMessageID::PrePrepare,
            0x01 => ClayerMessageID::Prepare,
            0x02 => ClayerMessageID::Commit,
            _ => return Err(alloy_rlp::Error::Custom("Invalid message ID")),
        };
        buf.advance(1);
        Ok(id)
    }
}

impl TryFrom<usize> for ClayerMessageID {
    type Error = &'static str;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(ClayerMessageID::PrePrepare),
            0x01 => Ok(ClayerMessageID::Prepare),
            0x02 => Ok(ClayerMessageID::Commit),
            _ => Err("Invalid message ID"),
        }
    }
}
