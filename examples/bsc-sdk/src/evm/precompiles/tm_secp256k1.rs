//! Credits to <https://github.com/bnb-chain/revm/blob/d66170e712460ae766fc26a063f106658ce33e9d/crates/precompile/src/tm_secp256k1.rs>

use alloy_primitives::Bytes;
use revm::precompile::{
    u64_to_address, PrecompileError, PrecompileOutput, PrecompileResult, PrecompileWithAddress,
};
use secp256k1::{ecdsa, Message, PublicKey};
use tendermint::{account, public_key};

/// Tendermint SECP256K1 signature recover precompile for BSC.
pub(crate) const TM_SECP256K1_SIGNATURE_RECOVER: PrecompileWithAddress =
    PrecompileWithAddress(u64_to_address(105), tm_secp256k1_signature_recover_run);

const SECP256K1_PUBKEY_LENGTH: usize = 33;
const SECP256K1_SIGNATURE_LENGTH: usize = 64;
const SECP256K1_SIGNATURE_MSGHASH_LENGTH: usize = 32;

/// Runs the Tendermint SECP256K1 signature recover precompile.
///
/// input:
///
/// | PubKey   | Signature    |  SignatureMsgHash    |
///
/// | 33 bytes |  64 bytes    |       32 bytes       |
fn tm_secp256k1_signature_recover_run(input: &[u8], gas_limit: u64) -> PrecompileResult {
    const TM_SECP256K1_SIGNATURE_RECOVER_BASE: u64 = 3_000;

    if TM_SECP256K1_SIGNATURE_RECOVER_BASE > gas_limit {
        return Err(PrecompileError::OutOfGas);
    }

    let input_length = input.len();
    if input_length !=
        SECP256K1_PUBKEY_LENGTH + SECP256K1_SIGNATURE_LENGTH + SECP256K1_SIGNATURE_MSGHASH_LENGTH
    {
        return Err(PrecompileError::other("invalid input"));
    }

    let public_key = match PublicKey::from_slice(&input[..SECP256K1_PUBKEY_LENGTH]) {
        Ok(pk) => pk,
        Err(_) => return Err(PrecompileError::other("invalid pubkey")),
    };

    let message = Message::from_digest(
        input[SECP256K1_PUBKEY_LENGTH + SECP256K1_SIGNATURE_LENGTH..].try_into().unwrap(),
    );

    let sig = match ecdsa::Signature::from_compact(
        &input[SECP256K1_PUBKEY_LENGTH..SECP256K1_PUBKEY_LENGTH + SECP256K1_SIGNATURE_LENGTH],
    ) {
        Ok(s) => s,
        Err(_) => return Err(PrecompileError::other("invalid signature")),
    };

    let res = sig.verify(&message, &public_key).is_ok();

    if !res {
        return Err(PrecompileError::other("invalid signature"));
    }

    let tm_pub_key =
        match public_key::PublicKey::from_raw_secp256k1(&input[..SECP256K1_PUBKEY_LENGTH]) {
            Some(pk) => pk,
            None => return Err(PrecompileError::other("invalid pubkey")),
        };

    Ok(PrecompileOutput::new(
        TM_SECP256K1_SIGNATURE_RECOVER_BASE,
        Bytes::copy_from_slice(account::Id::from(tm_pub_key).as_bytes()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;

    #[test]
    fn test_tm_secp256k1_signature_recover_run_local_key() {
        let pub_key =
            hex::decode("0278caa4d6321aa856d6341dd3e8bcdfe0b55901548871c63c3f5cec43c2ae88a9")
                .unwrap();
        let sig = hex::decode("0cb78be0d8eaeab991907b06c61240c04f4ca83f54b7799ce77cf029b837988038c4b3b7f5df231695b0d14499b716e1fd6504860eb3c9244ecb4e569d44c062").unwrap();
        let msg_hash =
            hex::decode("b6ac827edff4bbbf23579720782dbef40b65780af292cc66849e7e5944f1230f")
                .unwrap();

        let expect_address = hex::decode("fa3B227adFf8EA1706098928715076D76959Ae6c").unwrap();

        let mut input = vec![];
        input.extend(pub_key);
        input.extend(sig);
        input.extend(msg_hash);

        let input = Bytes::copy_from_slice(&input);
        let res = tm_secp256k1_signature_recover_run(&input, 3_000u64).unwrap();

        let gas = res.gas_used;
        assert_eq!(gas, 3_000u64);

        let res = res.bytes;
        assert_eq!(res, Bytes::from(expect_address));
    }

    #[test]
    fn test_tm_secp256k1_signature_recover_run_ledger_key() {
        let pub_key =
            hex::decode("02d63ee39adb1779353b4393dd5ea9d6d2b6df63b71d168571803cc7b9a0a20e98")
                .unwrap();
        let sig = hex::decode("66bdb5d381b2773c0f569858c7ee143959522d7c1f46dc656c325cb7353ec40c28ec22dff3650b34c096c5b12e702d7237d409f1ebaaa6dd1128a8f2d401fd5b").unwrap();
        let msg_hash =
            hex::decode("c45e8f0dc7c054c31912beeffd6f10f1c585606d61e252e97968cd66661c2571")
                .unwrap();

        let expect_address = hex::decode("65a284146b84210a01add088954bb52d88b230af").unwrap();

        let mut input = vec![];
        input.extend(pub_key);
        input.extend(sig);
        input.extend(msg_hash);

        let input = Bytes::copy_from_slice(&input);
        let res = tm_secp256k1_signature_recover_run(&input, 3_000u64).unwrap();

        let gas = res.gas_used;
        assert_eq!(gas, 3_000u64);

        let res = res.bytes;
        assert_eq!(res, Bytes::from(expect_address));
    }
}
