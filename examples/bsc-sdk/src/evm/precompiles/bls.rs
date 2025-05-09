//! Credits to <https://github.com/bnb-chain/revm/blob/d66170e712460ae766fc26a063f106658ce33e9d/crates/precompile/src/bls.rs>

use alloy_primitives::Bytes;
use bls_on_arkworks as bls;
use revm::precompile::{
    u64_to_address, PrecompileError, PrecompileOutput, PrecompileResult, PrecompileWithAddress,
};
use std::vec::Vec;

use super::error::BscPrecompileError;

pub(crate) const BLS_SIGNATURE_VALIDATION: PrecompileWithAddress =
    PrecompileWithAddress(u64_to_address(102), bls_signature_validation_run);

const BLS_MSG_HASH_LENGTH: u64 = 32;
const BLS_SIGNATURE_LENGTH: u64 = 96;
const BLS_SINGLE_PUBKEY_LENGTH: u64 = 48;
const BLS_DST: &[u8] = bls::DST_ETHEREUM.as_bytes();

/// Run bls signature validation precompile.
///
/// The input is encoded as follows:
/// | msg_hash |  signature  |  [{bls pubkey}]  |
/// |    32    |      96     |      [{48}]      |
fn bls_signature_validation_run(input: &[u8], gas_limit: u64) -> PrecompileResult {
    let cost = calc_gas_cost(input);
    if cost > gas_limit {
        return Err(PrecompileError::OutOfGas);
    }

    let msg_and_sig_length = BLS_MSG_HASH_LENGTH + BLS_SIGNATURE_LENGTH;
    let input_length = input.len() as u64;
    if (input_length <= msg_and_sig_length) ||
        ((input_length - msg_and_sig_length) % BLS_SINGLE_PUBKEY_LENGTH != 0)
    {
        return Err(BscPrecompileError::Reverted(cost).into());
    }

    let msg_hash: &Vec<u8> = &input[..BLS_MSG_HASH_LENGTH as usize].to_vec();
    let signature = &input[BLS_MSG_HASH_LENGTH as usize..msg_and_sig_length as usize].to_vec();
    let pub_keys_data = &input[msg_and_sig_length as usize..].to_vec();

    // check signature format
    if bls::signature_to_point(&signature.to_vec()).is_err() {
        return Err(BscPrecompileError::Reverted(cost).into());
    }

    let pub_key_count = (input_length - msg_and_sig_length) / BLS_SINGLE_PUBKEY_LENGTH;
    let mut pub_keys = Vec::with_capacity(pub_key_count as usize);
    let mut msg_hashes = Vec::with_capacity(pub_key_count as usize);

    // check pubkey format and push to pub_keys
    for i in 0..pub_key_count {
        let pub_key = &pub_keys_data[i as usize * BLS_SINGLE_PUBKEY_LENGTH as usize..
            (i + 1) as usize * BLS_SINGLE_PUBKEY_LENGTH as usize];
        if !bls::key_validate(&pub_key.to_vec()) {
            return Err(BscPrecompileError::Reverted(cost).into());
        }
        pub_keys.push(pub_key.to_vec());
        msg_hashes.push(msg_hash.clone().to_vec());
    }
    if pub_keys.is_empty() {
        return Err(BscPrecompileError::Reverted(cost).into());
    }

    // verify signature
    let mut output = Bytes::from(vec![1]);
    if (pub_keys.len() == 1 && !bls::verify(&pub_keys[0], msg_hash, signature, &BLS_DST.to_vec())) ||
        !bls::aggregate_verify(pub_keys, msg_hashes, signature, &BLS_DST.to_vec())
    {
        output = Bytes::from(vec![]);
    }

    Ok(PrecompileOutput::new(cost, output))
}

fn calc_gas_cost(input: &[u8]) -> u64 {
    const BLS_SIGNATURE_VALIDATION_BASE: u64 = 1_000;
    const BLS_SIGNATURE_VALIDATION_PER_KER: u64 = 3_500;

    let msg_length = BLS_MSG_HASH_LENGTH + BLS_SIGNATURE_LENGTH;
    let single_pubkey_length = BLS_SINGLE_PUBKEY_LENGTH;
    let input_length = input.len() as u64;

    if (input_length <= msg_length) || ((input_length - msg_length) % single_pubkey_length != 0) {
        return BLS_SIGNATURE_VALIDATION_BASE;
    }

    let pub_key_number = (input_length - msg_length) / single_pubkey_length;

    BLS_SIGNATURE_VALIDATION_BASE + BLS_SIGNATURE_VALIDATION_PER_KER * pub_key_number
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;

    #[test]
    fn test_bls_signature_validation_with_single_key() {
        let msg_hash = hex!("6377c7e66081cb65e473c1b95db5195a27d04a7108b468890224bedbe1a8a6eb");
        let signature = hex!("8325fccd4ff01e6e0e73de4955d3cb2c6678c6a6abfc465c2991e375c5cf68841ac7847ac51c32a26bd99828bc99f2f6082c41986097e0f6e6711e57c5bd5b18fa6f8f44bf416617cf192a2ff6d4edf0890315d87e3c04f04f0d1611b64bbe0a");
        let pub_key = hex!("a842801f14464ce36470737dc159cb13191e3ad8a49f4f3a38e6a94ea5594ff65753f74661fb7ec944b98fc673bb8230");
        let mut input = Vec::<u8>::new();
        input.extend_from_slice(&msg_hash);
        input.extend_from_slice(&signature);
        input.extend_from_slice(&pub_key);

        let excepted_output = Bytes::from(vec![1]);
        let result = match bls_signature_validation_run(&Bytes::from(input.clone()), 100_000_000) {
            Ok(o) => o.bytes,
            Err(e) => panic!("BLS signature validation failed, {e:?}"),
        };
        assert_eq!(result, excepted_output);

        // wrong msg hash
        let msg_hash = hex!("1377c7e66081cb65e473c1b95db5195a27d04a7108b468890224bedbe1a8a6eb");
        let signature = hex!("8325fccd4ff01e6e0e73de4955d3cb2c6678c6a6abfc465c2991e375c5cf68841ac7847ac51c32a26bd99828bc99f2f6082c41986097e0f6e6711e57c5bd5b18fa6f8f44bf416617cf192a2ff6d4edf0890315d87e3c04f04f0d1611b64bbe0a");
        let pub_key = hex!("a842801f14464ce36470737dc159cb13191e3ad8a49f4f3a38e6a94ea5594ff65753f74661fb7ec944b98fc673bb8230");
        let mut input = Vec::<u8>::new();
        input.extend_from_slice(&msg_hash);
        input.extend_from_slice(&signature);
        input.extend_from_slice(&pub_key);

        let excepted_output = Bytes::from(vec![]);
        let result = match bls_signature_validation_run(&Bytes::from(input.clone()), 100_000_000) {
            Ok(o) => o.bytes,
            Err(e) => panic!("BLS signature validation failed, {e:?}"),
        };
        assert_eq!(result, excepted_output);

        // wrong signature
        let msg_hash = hex!("6377c7e66081cb65e473c1b95db5195a27d04a7108b468890224bedbe1a8a6eb");
        let signature = hex!("1325fccd4ff01e6e0e73de4955d3cb2c6678c6a6abfc465c2991e375c5cf68841ac7847ac51c32a26bd99828bc99f2f6082c41986097e0f6e6711e57c5bd5b18fa6f8f44bf416617cf192a2ff6d4edf0890315d87e3c04f04f0d1611b64bbe0a");
        let pub_key = hex!("a842801f14464ce36470737dc159cb13191e3ad8a49f4f3a38e6a94ea5594ff65753f74661fb7ec944b98fc673bb8230");
        let mut input = Vec::<u8>::new();
        input.extend_from_slice(&msg_hash);
        input.extend_from_slice(&signature);
        input.extend_from_slice(&pub_key);

        match bls_signature_validation_run(&Bytes::from(input.clone()), 100_000_000) {
            Ok(_) => panic!("BLS signature validation failed, expect error"),
            Err(e) => assert_eq!(e, BscPrecompileError::Reverted(4500).into()),
        }

        // wrong pubkey
        let msg_hash = hex!("6377c7e66081cb65e473c1b95db5195a27d04a7108b468890224bedbe1a8a6eb");
        let signature = hex!("1325fccd4ff01e6e0e73de4955d3cb2c6678c6a6abfc465c2991e375c5cf68841ac7847ac51c32a26bd99828bc99f2f6082c41986097e0f6e6711e57c5bd5b18fa6f8f44bf416617cf192a2ff6d4edf0890315d87e3c04f04f0d1611b64bbe0a");
        let pub_key = hex!("1842801f14464ce36470737dc159cb13191e3ad8a49f4f3a38e6a94ea5594ff65753f74661fb7ec944b98fc673bb8230");
        let mut input = Vec::<u8>::new();
        input.extend_from_slice(&msg_hash);
        input.extend_from_slice(&signature);
        input.extend_from_slice(&pub_key);

        match bls_signature_validation_run(&Bytes::from(input.clone()), 100_000_000) {
            Ok(_) => panic!("BLS signature validation failed, expect error"),
            Err(e) => assert_eq!(e, BscPrecompileError::Reverted(4500).into()),
        }
    }

    #[test]
    fn test_bls_signature_validation_with_multiple_keys() {
        let msg_hash = hex!("6377c7e66081cb65e473c1b95db5195a27d04a7108b468890224bedbe1a8a6eb");
        let signature = hex!("876ac46847e82a2f2887bbeb855916e05bd259086fda5553e4e5e5ee0dcda6869c10e2aa539e265b492015d1bd1b553815a42ea9daf6713b6a0002c6f1aacfa51e55b931745638c0552d7fab4a499bbd6ba71f9be36d35ffa527f77b2a6cebda");
        let pub_key1 = hex!("80223255a26d81a8e1cd94df746f45e87a91d28f408a037804062910b7db68a724cfd204b7f9337bcecac25de86d5515");
        let pub_key2 = hex!("8bec004e938668c67aa0fab6f555282efdf213817e455d9eaa6a0897211eae5f79db5cdc626d1cd5759a0c1c10cf7aa0");
        let pub_key3 = hex!("af952757f442d7240a4cec62de638973a24fde8eb0ad5217be61eea53211c19859c03a299125ea8520f015f6f8865076");
        let mut input = Vec::<u8>::new();
        input.extend_from_slice(&msg_hash);
        input.extend_from_slice(&signature);
        input.extend_from_slice(&pub_key1);
        input.extend_from_slice(&pub_key2);
        input.extend_from_slice(&pub_key3);

        let excepted_output = Bytes::from(vec![1]);
        let result = match bls_signature_validation_run(&Bytes::from(input.clone()), 100_000_000) {
            Ok(o) => o.bytes,
            Err(e) => panic!("BLS signature validation failed, {e:?}"),
        };
        assert_eq!(result, excepted_output);

        // wrong msg hash
        let msg_hash = hex!("1377c7e66081cb65e473c1b95db5195a27d04a7108b468890224bedbe1a8a6eb");
        let signature = hex!("876ac46847e82a2f2887bbeb855916e05bd259086fda5553e4e5e5ee0dcda6869c10e2aa539e265b492015d1bd1b553815a42ea9daf6713b6a0002c6f1aacfa51e55b931745638c0552d7fab4a499bbd6ba71f9be36d35ffa527f77b2a6cebda");
        let pub_key1 = hex!("80223255a26d81a8e1cd94df746f45e87a91d28f408a037804062910b7db68a724cfd204b7f9337bcecac25de86d5515");
        let pub_key2 = hex!("8bec004e938668c67aa0fab6f555282efdf213817e455d9eaa6a0897211eae5f79db5cdc626d1cd5759a0c1c10cf7aa0");
        let pub_key3 = hex!("af952757f442d7240a4cec62de638973a24fde8eb0ad5217be61eea53211c19859c03a299125ea8520f015f6f8865076");
        let mut input = Vec::<u8>::new();
        input.extend_from_slice(&msg_hash);
        input.extend_from_slice(&signature);
        input.extend_from_slice(&pub_key1);
        input.extend_from_slice(&pub_key2);
        input.extend_from_slice(&pub_key3);
        let excepted_output = Bytes::from(vec![]);
        let result = match bls_signature_validation_run(&Bytes::from(input.clone()), 100_000_000) {
            Ok(o) => o.bytes,
            Err(e) => panic!("BLS signature validation failed, {e:?}"),
        };
        assert_eq!(result, excepted_output);

        // wrong signature
        let msg_hash = hex!("1377c7e66081cb65e473c1b95db5195a27d04a7108b468890224bedbe1a8a6eb");
        let signature = hex!("276ac46847e82a2f2887bbeb855916e05bd259086fda5553e4e5e5ee0dcda6869c10e2aa539e265b492015d1bd1b553815a42ea9daf6713b6a0002c6f1aacfa51e55b931745638c0552d7fab4a499bbd6ba71f9be36d35ffa527f77b2a6cebda");
        let pub_key1 = hex!("80223255a26d81a8e1cd94df746f45e87a91d28f408a037804062910b7db68a724cfd204b7f9337bcecac25de86d5515");
        let pub_key2 = hex!("8bec004e938668c67aa0fab6f555282efdf213817e455d9eaa6a0897211eae5f79db5cdc626d1cd5759a0c1c10cf7aa0");
        let pub_key3 = hex!("af952757f442d7240a4cec62de638973a24fde8eb0ad5217be61eea53211c19859c03a299125ea8520f015f6f8865076");
        let mut input = Vec::<u8>::new();
        input.extend_from_slice(&msg_hash);
        input.extend_from_slice(&signature);
        input.extend_from_slice(&pub_key1);
        input.extend_from_slice(&pub_key2);
        input.extend_from_slice(&pub_key3);

        match bls_signature_validation_run(&Bytes::from(input.clone()), 100_000_000) {
            Ok(_) => panic!("BLS signature validation failed, expect error"),
            Err(e) => assert_eq!(e, BscPrecompileError::Reverted(11500).into()),
        }

        // invalid pubkey
        let msg_hash = hex!("1377c7e66081cb65e473c1b95db5195a27d04a7108b468890224bedbe1a8a6eb");
        let signature = hex!("276ac46847e82a2f2887bbeb855916e05bd259086fda5553e4e5e5ee0dcda6869c10e2aa539e265b492015d1bd1b553815a42ea9daf6713b6a0002c6f1aacfa51e55b931745638c0552d7fab4a499bbd6ba71f9be36d35ffa527f77b2a6cebda");
        let pub_key1 = hex!("10223255a26d81a8e1cd94df746f45e87a91d28f408a037804062910b7db68a724cfd204b7f9337bcecac25de86d5515");
        let pub_key2 = hex!("8bec004e938668c67aa0fab6f555282efdf213817e455d9eaa6a0897211eae5f79db5cdc626d1cd5759a0c1c10cf7aa0");
        let pub_key3 = hex!("af952757f442d7240a4cec62de638973a24fde8eb0ad5217be61eea53211c19859c03a299125ea8520f015f6f8865076");
        let mut input = Vec::<u8>::new();
        input.extend_from_slice(&msg_hash);
        input.extend_from_slice(&signature);
        input.extend_from_slice(&pub_key1);
        input.extend_from_slice(&pub_key2);
        input.extend_from_slice(&pub_key3);

        match bls_signature_validation_run(&Bytes::from(input.clone()), 100_000_000) {
            Ok(_) => panic!("BLS signature validation failed, expect error"),
            Err(e) => assert_eq!(e, BscPrecompileError::Reverted(11500).into()),
        }

        // duplicate pubkey
        let msg_hash = hex!("6377c7e66081cb65e473c1b95db5195a27d04a7108b468890224bedbe1a8a6eb");
        let signature = hex!("876ac46847e82a2f2887bbeb855916e05bd259086fda5553e4e5e5ee0dcda6869c10e2aa539e265b492015d1bd1b553815a42ea9daf6713b6a0002c6f1aacfa51e55b931745638c0552d7fab4a499bbd6ba71f9be36d35ffa527f77b2a6cebda");
        let pub_key1 = hex!("8bec004e938668c67aa0fab6f555282efdf213817e455d9eaa6a0897211eae5f79db5cdc626d1cd5759a0c1c10cf7aa0");
        let pub_key2 = hex!("8bec004e938668c67aa0fab6f555282efdf213817e455d9eaa6a0897211eae5f79db5cdc626d1cd5759a0c1c10cf7aa0");
        let pub_key3 = hex!("af952757f442d7240a4cec62de638973a24fde8eb0ad5217be61eea53211c19859c03a299125ea8520f015f6f8865076");
        let mut input = Vec::<u8>::new();
        input.extend_from_slice(&msg_hash);
        input.extend_from_slice(&signature);
        input.extend_from_slice(&pub_key1);
        input.extend_from_slice(&pub_key2);
        input.extend_from_slice(&pub_key3);
        let excepted_output = Bytes::from(vec![]);
        let result = match bls_signature_validation_run(&Bytes::from(input.clone()), 100_000_000) {
            Ok(o) => o.bytes,
            Err(e) => panic!("BLS signature validation failed, {e:?}"),
        };
        assert_eq!(result, excepted_output);
    }
}
