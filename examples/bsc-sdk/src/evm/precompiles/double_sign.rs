//! Credits to <https://github.com/bnb-chain/revm/blob/d66170e712460ae766fc26a063f106658ce33e9d/crates/precompile/src/double_sign.rs>

use crate::evm::precompiles::error::BscPrecompileError;
use alloy_primitives::{keccak256, BlockNumber, Bytes, ChainId, B256, B512, U256};
use alloy_rlp::{Decodable, RlpDecodable, RlpEncodable};
use core::cmp::Ordering;
use revm::precompile::{
    secp256k1, u64_to_address, PrecompileError, PrecompileOutput, PrecompileResult,
    PrecompileWithAddress,
};

/// Double sign evidence validation precompile for BSC.
pub(crate) const DOUBLE_SIGN_EVIDENCE_VALIDATION: PrecompileWithAddress =
    PrecompileWithAddress(u64_to_address(104), double_sign_evidence_validation_run);

const EXTRA_SEAL_LENGTH: usize = 65;

/// Double sign evidence with two different headers.
#[derive(Debug, RlpDecodable, PartialEq)]
pub(crate) struct DoubleSignEvidence {
    pub(crate) chain_id: ChainId,
    pub(crate) header_bytes1: Bytes,
    pub(crate) header_bytes2: Bytes,
}

/// Header of a block.
#[derive(Debug, RlpDecodable, PartialEq)]
pub(crate) struct Header {
    pub(crate) parent_hash: [u8; 32],
    pub(crate) uncle_hash: [u8; 32],
    pub(crate) coinbase: [u8; 20],
    pub(crate) root: [u8; 32],
    pub(crate) tx_hash: [u8; 32],
    pub(crate) receipt_hash: [u8; 32],
    pub(crate) bloom: [u8; 256],
    pub(crate) difficulty: U256,
    pub(crate) number: BlockNumber,
    pub(crate) gas_limit: u64,
    pub(crate) gas_used: u64,
    pub(crate) time: u64,
    pub(crate) extra: Bytes,
    pub(crate) mix_digest: [u8; 32],
    pub(crate) nonce: [u8; 8],
}

/// The fields to generate the seal hash.
#[derive(Debug, RlpEncodable, RlpDecodable, PartialEq)]
pub(crate) struct SealContent {
    pub(crate) chain_id: ChainId,
    pub(crate) parent_hash: [u8; 32],
    pub(crate) uncle_hash: [u8; 32],
    pub(crate) coinbase: [u8; 20],
    pub(crate) root: [u8; 32],
    pub(crate) tx_hash: [u8; 32],
    pub(crate) receipt_hash: [u8; 32],
    pub(crate) bloom: [u8; 256],
    pub(crate) difficulty: U256,
    pub(crate) number: BlockNumber,
    pub(crate) gas_limit: u64,
    pub(crate) gas_used: u64,
    pub(crate) time: u64,
    pub(crate) extra: Bytes,
    pub(crate) mix_digest: [u8; 32],
    pub(crate) nonce: [u8; 8],
}

/// Run the double sign evidence validation precompile.
///
/// input: rlp encoded DoubleSignEvidence
///
/// return:
///
/// signer address| evidence height|
///
/// 20 bytes      | 32 bytes       |
fn double_sign_evidence_validation_run(input: &Bytes, gas_limit: u64) -> PrecompileResult {
    const DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE: u64 = 10_000;

    if DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE > gas_limit {
        return Err(PrecompileError::OutOfGas);
    }

    let evidence = match DoubleSignEvidence::decode(&mut input.iter().as_ref()) {
        Ok(e) => e,
        Err(_) => {
            return Err(BscPrecompileError::Reverted(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE).into())
        }
    };

    let header1 = match Header::decode(&mut evidence.header_bytes1.as_ref()) {
        Ok(e) => e,
        Err(_) => {
            return Err(BscPrecompileError::Reverted(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE).into())
        }
    };
    let header2 = match Header::decode(&mut evidence.header_bytes2.as_ref()) {
        Ok(e) => e,
        Err(_) => {
            return Err(BscPrecompileError::Reverted(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE).into())
        }
    };

    // basic check
    if header1.number.to_be_bytes().len() > 32 || header2.number.to_be_bytes().len() > 32 {
        return Err(BscPrecompileError::Reverted(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE).into());
    }
    if header1.number != header2.number {
        return Err(BscPrecompileError::Reverted(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE).into());
    }
    if header1.parent_hash.cmp(&header2.parent_hash) != Ordering::Equal {
        return Err(BscPrecompileError::Reverted(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE).into());
    }

    if header1.extra.len() < EXTRA_SEAL_LENGTH || header1.extra.len() < EXTRA_SEAL_LENGTH {
        return Err(BscPrecompileError::Reverted(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE).into());
    }
    let sig1 = &header1.extra[header1.extra.len() - EXTRA_SEAL_LENGTH..];
    let sig2 = &header2.extra[header2.extra.len() - EXTRA_SEAL_LENGTH..];
    if sig1.eq(sig2) {
        return Err(BscPrecompileError::Reverted(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE).into());
    }

    // check signature
    let msg_hash1 = seal_hash(&header1, evidence.chain_id);
    let msg_hash2 = seal_hash(&header2, evidence.chain_id);

    if msg_hash1.eq(&msg_hash2) {
        return Err(BscPrecompileError::Reverted(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE).into());
    }

    let recid1 = sig1[64];
    let sig1 = <&B512>::try_from(&sig1[..64]).unwrap();
    let addr1 = match secp256k1::ecrecover(sig1, recid1, &msg_hash1) {
        Ok(pk) => pk,
        Err(_) => {
            return Err(BscPrecompileError::Reverted(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE).into())
        }
    };

    let recid2 = sig2[64];
    let sig2 = <&B512>::try_from(&sig2[..64]).unwrap();
    let addr2 = match secp256k1::ecrecover(sig2, recid2, &msg_hash2) {
        Ok(pk) => pk,
        Err(_) => {
            return Err(BscPrecompileError::Reverted(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE).into())
        }
    };

    if !addr1.eq(&addr2) {
        return Err(BscPrecompileError::Reverted(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE).into());
    }

    let mut res = [0; 52];
    let signer = &addr1[12..];
    res[..20].clone_from_slice(signer);
    res[52 - header1.number.to_be_bytes().len()..].clone_from_slice(&header1.number.to_be_bytes());

    Ok(PrecompileOutput::new(DOUBLE_SIGN_EVIDENCE_VALIDATION_BASE, Bytes::copy_from_slice(&res)))
}

fn seal_hash(header: &Header, chain_id: ChainId) -> B256 {
    let seal_content = SealContent {
        chain_id,
        parent_hash: header.parent_hash,
        uncle_hash: header.uncle_hash,
        coinbase: header.coinbase,
        root: header.root,
        tx_hash: header.tx_hash,
        receipt_hash: header.receipt_hash,
        bloom: header.bloom,
        difficulty: header.difficulty,
        number: header.number,
        gas_limit: header.gas_limit,
        gas_used: header.gas_used,
        time: header.time,
        extra: header.extra.slice(..header.extra.len() - EXTRA_SEAL_LENGTH),
        mix_digest: header.mix_digest,
        nonce: header.nonce,
    };
    let encoded = alloy_rlp::encode(seal_content);

    keccak256(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;

    #[test]
    fn test_double_sign_evidence_validation_run() {
        let input = hex::decode("f906278202cab9030ff9030ca01062d3d5015b9242bc193a9b0769f3d3780ecb55f97f40a752ae26d0b68cd0d8a0fae1a05fcb14bfd9b8a9f2b65007a9b6c2000de0627a73be644dd993d32342c494976ea74026e726554db657fa54763abd0c3a0aa9a0f385cc58ed297ff0d66eb5580b02853d3478ba418b1819ac659ee05df49b9794a0bf88464af369ed6b8cf02db00f0b9556ffa8d49cd491b00952a7f83431446638a00a6d0870e586a76278fbfdcedf76ef6679af18fc1f9137cfad495f434974ea81b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001820cdf830f4240830f4240846555fa64b90111d983010301846765746888676f312e32302e378664617277696e00007abd731ef8ae07b86091cb8836d58f5444b883422a18825d899035d3e6ea39ad1a50069bf0b86da8b5573dde1cb4a0a34f19ce94e0ef78ff7518c80265b8a3ca56e3c60167523590d4e8dcc324900559465fc0fa403774096614e135de280949b58a45cc96f2ba9e17f848820d41a08429d0d8b33ee72a84f750fefea846cbca54e487129c7961c680bb72309ca888820d42a08c9db14d938b19f9e2261bbeca2679945462be2b58103dfff73665d0d150fb8a804ae755e0fe64b59753f4db6308a1f679747bce186aa2c62b95fa6eeff3fbd08f3b0667e45428a54ade15bad19f49641c499b431b36f65803ea71b379e6b61de501a0232c9ba2d41b40d36ed794c306747bcbc49bf61a0f37409c18bfe2b5bef26a2d880000000000000000b9030ff9030ca01062d3d5015b9242bc193a9b0769f3d3780ecb55f97f40a752ae26d0b68cd0d8a0b2789a5357827ed838335283e15c4dcc42b9bebcbf2919a18613246787e2f96094976ea74026e726554db657fa54763abd0c3a0aa9a071ce4c09ee275206013f0063761bc19c93c13990582f918cc57333634c94ce89a00e095703e5c9b149f253fe89697230029e32484a410b4b1f2c61442d73c3095aa0d317ae19ede7c8a2d3ac9ef98735b049bcb7278d12f48c42b924538b60a25e12b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001820cdf830f4240830f4240846555fa64b90111d983010301846765746888676f312e32302e378664617277696e00007abd731ef8ae07b86091cb8836d58f5444b883422a18825d899035d3e6ea39ad1a50069bf0b86da8b5573dde1cb4a0a34f19ce94e0ef78ff7518c80265b8a3ca56e3c60167523590d4e8dcc324900559465fc0fa403774096614e135de280949b58a45cc96f2ba9e17f848820d41a08429d0d8b33ee72a84f750fefea846cbca54e487129c7961c680bb72309ca888820d42a08c9db14d938b19f9e2261bbeca2679945462be2b58103dfff73665d0d150fb8a80c0b17bfe88534296ff064cb7156548f6deba2d6310d5044ed6485f087dc6ef232e051c28e1909c2b50a3b4f29345d66681c319bef653e52e5d746480d5a3983b00a0b56228685be711834d0f154292d07826dea42a0fad3e4f56c31470b7fbfbea26880000000000000000").unwrap();

        let res = double_sign_evidence_validation_run(&Bytes::from(input), 10_000).unwrap();

        let gas = res.gas_used;
        assert_eq!(gas, 10_000u64);

        let res = hex::encode(res.bytes);
        assert_eq!(res, "15d34aaf54267db7d7c367839aaf71a00a2c6a650000000000000000000000000000000000000000000000000000000000000cdf")
    }

    #[test]
    fn test_double_sign_evidence_validation_run_invalid_evidence() {
        let input = hex::decode("f9066b38b90332f9032fa01062d3d5015b9242bc193a9b0769f3d3780ecb55f97f40a752ae26d0b68cd0d8a0fae1a05fcb14bfd9b8a9f2b65007a9b6c2000de0627a73be644dd993d32342c494df87f0e2b8519ea2dd4abd8b639cdd628497ed25a0f385cc58ed297ff0d66eb5580b02853d3478ba418b1819ac659ee05df49b9794a0bf88464af369ed6b8cf02db00f0b9556ffa8d49cd491b00952a7f83431446638a00a6d0870e586a76278fbfdcedf76ef6679af18fc1f9137cfad495f434974ea81b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001a1010000000000000000000000000000000000000000000000000000000000000000830f4240830f42408465bc6996b90115d983010306846765746889676f312e32302e3131856c696e7578000053474aa9f8b25fb860b0844a5082bfaa2299d2a23f076e2f6b17b15f839cc3e7d5a875656f6733fd4b87ba3401f906d15f3dea263cd9a6076107c7db620a4630dd3832c4a4b57eb8f497e28a3d69e5c03b30205c4b45675747d513e1accd66329770f3c35b18c9d023f84c84023a5ad6a086a28d985d9a6c8e7f9a4feadd5ace0adba9818e1e1727edca755fcc0bd8344684023a5ad7a0bc3492196b2e68b8e6ceea87cfa7588b4d590089eb885c4f2c1e9d9fb450f7b980988e1b9d0beb91dab063e04879a24c43d33baae3759dee41fd62ffa83c77fd202bea27a829b49e8025bdd198393526dd12b223ab16052fd26a43f3aabf63e76901a0232c9ba2d41b40d36ed794c306747bcbc49bf61a0f37409c18bfe2b5bef26a2d880000000000000000b90332f9032fa01062d3d5015b9242bc193a9b0769f3d3780ecb55f97f40a752ae26d0b68cd0d8a0b2789a5357827ed838335283e15c4dcc42b9bebcbf2919a18613246787e2f96094df87f0e2b8519ea2dd4abd8b639cdd628497ed25a071ce4c09ee275206013f0063761bc19c93c13990582f918cc57333634c94ce89a00e095703e5c9b149f253fe89697230029e32484a410b4b1f2c61442d73c3095aa0d317ae19ede7c8a2d3ac9ef98735b049bcb7278d12f48c42b924538b60a25e12b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001a1010000000000000000000000000000000000000000000000000000000000000000830f4240830f42408465bc6996b90115d983010306846765746889676f312e32302e3131856c696e7578000053474aa9f8b25fb860b0844a5082bfaa2299d2a23f076e2f6b17b15f839cc3e7d5a875656f6733fd4b87ba3401f906d15f3dea263cd9a6076107c7db620a4630dd3832c4a4b57eb8f497e28a3d69e5c03b30205c4b45675747d513e1accd66329770f3c35b18c9d023f84c84023a5ad6a086a28d985d9a6c8e7f9a4feadd5ace0adba9818e1e1727edca755fcc0bd8344684023a5ad7a0bc3492196b2e68b8e6ceea87cfa7588b4d590089eb885c4f2c1e9d9fb450f7b9804c71ed015dd0c5c2d7393b68c2927f83f0a5da4c66f761f09e2f950cc610832c7876144599368404096ddef0eadacfde57717e2c7d23982b927285b797d41bfa00a0b56228685be711834d0f154292d07826dea42a0fad3e4f56c31470b7fbfbea26880000000000000000").unwrap();

        let res = double_sign_evidence_validation_run(&Bytes::from(input), 10_000);
        assert_eq!(res.err(), Some(PrecompileError::Other("Reverted(10000)".to_string())));
    }
}
