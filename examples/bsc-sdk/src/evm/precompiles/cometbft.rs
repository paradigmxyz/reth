//! Credits to <https://github.com/bnb-chain/revm/blob/d66170e712460ae766fc26a063f106658ce33e9d/crates/precompile/src/cometbft.rs>
use crate::evm::precompiles::error::BscPrecompileError;
use alloy_primitives::Bytes;
use cometbft::{block::signed_header::SignedHeader, validator::Set, vote::Power, PublicKey};
use cometbft_light_client::{
    predicates::VerificationPredicates,
    types::{LightBlock, TrustThreshold},
};
use cometbft_light_client_verifier::{
    operations::voting_power::ProdVotingPowerCalculator,
    predicates::ProdPredicates,
    types::{Validator, ValidatorSet},
};
use cometbft_proto::types::v1::LightBlock as TmLightBlock;
use prost::Message;
use revm::precompile::{
    u64_to_address, PrecompileError, PrecompileOutput, PrecompileResult, PrecompileWithAddress,
};
use std::{borrow::ToOwned, string::String, vec::Vec};

pub(crate) const COMETBFT_LIGHT_BLOCK_VALIDATION: PrecompileWithAddress =
    PrecompileWithAddress(u64_to_address(103), cometbft_light_block_validation_run);

pub(crate) const COMETBFT_LIGHT_BLOCK_VALIDATION_BEFORE_HERTZ: PrecompileWithAddress =
    PrecompileWithAddress(u64_to_address(103), cometbft_light_block_validation_run_before_hertz);

const UINT64_TYPE_LENGTH: u64 = 8;
const CONSENSUS_STATE_LENGTH_BYTES_LENGTH: u64 = 32;
const VALIDATE_RESULT_METADATA_LENGTH: u64 = 32;

const CHAIN_ID_LENGTH: u64 = 32;
const HEIGHT_LENGTH: u64 = 8;
const VALIDATOR_SET_HASH_LENGTH: u64 = 32;
const VALIDATOR_PUBKEY_LENGTH: u64 = 32;
const VALIDATOR_VOTING_POWER_LENGTH: u64 = 8;
const RELAYER_ADDRESS_LENGTH: u64 = 20;
const RELAYER_BLS_KEY_LENGTH: u64 = 48;

const SINGLE_VALIDATOR_BYTES_LENGTH: u64 = VALIDATOR_PUBKEY_LENGTH +
    VALIDATOR_VOTING_POWER_LENGTH +
    RELAYER_ADDRESS_LENGTH +
    RELAYER_BLS_KEY_LENGTH;

const MAX_CONSENSUS_STATE_LENGTH: u64 = CHAIN_ID_LENGTH +
    HEIGHT_LENGTH +
    VALIDATOR_SET_HASH_LENGTH +
    99 * SINGLE_VALIDATOR_BYTES_LENGTH;

fn cometbft_light_block_validation_run(input: &[u8], gas_limit: u64) -> PrecompileResult {
    cometbft_light_block_validation_run_inner(input, gas_limit, true)
}

fn cometbft_light_block_validation_run_before_hertz(
    input: &[u8],
    gas_limit: u64,
) -> PrecompileResult {
    cometbft_light_block_validation_run_inner(input, gas_limit, false)
}

fn cometbft_light_block_validation_run_inner(
    input: &[u8],
    gas_limit: u64,
    is_hertz: bool,
) -> PrecompileResult {
    const COMETBFT_LIGHT_BLOCK_VALIDATION_BASE: u64 = 3_000;

    if COMETBFT_LIGHT_BLOCK_VALIDATION_BASE > gas_limit {
        return Err(PrecompileError::OutOfGas);
    }

    let (mut consensus_state, tm_light_block) = decode_light_block_validation_input(input)?;

    let light_block = convert_light_block_from_proto(&tm_light_block)?;

    let mut validator_set_changed = consensus_state.apply_light_block(&light_block)?;
    if !is_hertz {
        validator_set_changed = false;
    }

    let consensus_state_bytes = consensus_state.encode()?;

    Ok(PrecompileOutput::new(
        COMETBFT_LIGHT_BLOCK_VALIDATION_BASE,
        encode_light_block_validation_result(validator_set_changed, consensus_state_bytes),
    ))
}

type ConvertLightBlockResult = Result<LightBlock, PrecompileError>;
fn convert_light_block_from_proto(light_block_proto: &TmLightBlock) -> ConvertLightBlockResult {
    let signed_header =
        match SignedHeader::try_from(light_block_proto.signed_header.as_ref().unwrap().clone()) {
            Ok(sh) => sh.clone(),
            Err(_) => return Err(BscPrecompileError::CometBftInvalidInput.into()),
        };

    let validator_set =
        match Set::try_from(light_block_proto.validator_set.as_ref().unwrap().clone()) {
            Ok(vs) => vs.clone(),
            Err(_) => return Err(BscPrecompileError::CometBftInvalidInput.into()),
        };

    let next_validator_set = validator_set.clone();
    let peer_id = cometbft::node::Id::new([0u8; 20]);
    Ok(LightBlock::new(signed_header, validator_set, next_validator_set, peer_id))
}

type DecodeLightBlockResult = Result<(ConsensusState, TmLightBlock), PrecompileError>;
fn decode_light_block_validation_input(input: &[u8]) -> DecodeLightBlockResult {
    let input_length = input.len() as u64;
    if input_length < CONSENSUS_STATE_LENGTH_BYTES_LENGTH {
        return Err(BscPrecompileError::CometBftInvalidInput.into());
    }

    let cs_length = u64::from_be_bytes(
        input[CONSENSUS_STATE_LENGTH_BYTES_LENGTH as usize - UINT64_TYPE_LENGTH as usize..
            CONSENSUS_STATE_LENGTH_BYTES_LENGTH as usize]
            .try_into()
            .unwrap(),
    );
    let input_length_checked = CONSENSUS_STATE_LENGTH_BYTES_LENGTH.checked_add(cs_length);
    if input_length_checked.is_none() {
        // overflow
        return Err(BscPrecompileError::CometBftInvalidInput.into());
    }

    if input_length < input_length_checked.unwrap() {
        return Err(BscPrecompileError::CometBftInvalidInput.into());
    }

    let decode_input = Bytes::from(
        input[CONSENSUS_STATE_LENGTH_BYTES_LENGTH as usize..
            (CONSENSUS_STATE_LENGTH_BYTES_LENGTH + cs_length) as usize]
            .to_vec(),
    );
    let consensus_state = decode_consensus_state(&decode_input)?;

    let mut light_block_pb: TmLightBlock = TmLightBlock::default();
    match light_block_pb
        .merge(&input[CONSENSUS_STATE_LENGTH_BYTES_LENGTH as usize + cs_length as usize..])
    {
        Ok(pb) => pb,
        Err(_) => return Err(BscPrecompileError::CometBftInvalidInput.into()),
    };

    Ok((consensus_state, light_block_pb))
}

struct ConsensusState {
    chain_id: String,
    height: u64,
    next_validator_set_hash: Bytes,
    validators: ValidatorSet,
}

impl ConsensusState {
    fn new(
        chain_id: String,
        height: u64,
        next_validator_set_hash: Bytes,
        validators: ValidatorSet,
    ) -> Self {
        Self { chain_id, height, next_validator_set_hash, validators }
    }

    fn apply_light_block(&mut self, light_block: &LightBlock) -> Result<bool, PrecompileError> {
        if light_block.height().value() <= self.height {
            return Err(BscPrecompileError::CometBftInvalidInput.into());
        }
        if light_block.signed_header.header().chain_id.as_str() != self.chain_id {
            return Err(BscPrecompileError::CometBftInvalidInput.into());
        }

        let vp = ProdPredicates;
        let voting_power_calculator = ProdVotingPowerCalculator::default();
        let trust_threshold_two_third = TrustThreshold::TWO_THIRDS;
        let trust_threshold_one_third = TrustThreshold::ONE_THIRD;
        if self.height + 1 == light_block.height().value() {
            if self.next_validator_set_hash.ne(light_block
                .signed_header
                .header()
                .validators_hash
                .as_bytes())
            {
                return Err(BscPrecompileError::CometBftApplyBlockFailed.into());
            }
            // Verify Commit Light Trusted
            let result = vp.has_sufficient_validators_overlap(
                &light_block.signed_header,
                &light_block.validators,
                &trust_threshold_two_third,
                &voting_power_calculator,
            );
            if result.is_err() {
                return Err(BscPrecompileError::CometBftApplyBlockFailed.into());
            }
        } else {
            // Verify Commit Light Trusting
            let result = vp.has_sufficient_validators_overlap(
                &light_block.signed_header,
                &self.validators,
                &trust_threshold_one_third,
                &voting_power_calculator,
            );

            if result.is_err() {
                return Err(BscPrecompileError::CometBftApplyBlockFailed.into());
            }

            // Verify Commit Light
            let result = vp.has_sufficient_validators_overlap(
                &light_block.signed_header,
                &light_block.validators,
                &trust_threshold_two_third,
                &voting_power_calculator,
            );
            if result.is_err() {
                return Err(BscPrecompileError::CometBftApplyBlockFailed.into());
            }
        }

        let validator_set_changed = self.validators.hash().as_bytes().ne(light_block
            .signed_header
            .header()
            .validators_hash
            .as_bytes());
        self.height = light_block.height().value();
        self.next_validator_set_hash = Bytes::from(
            light_block.signed_header.header().next_validators_hash.as_bytes().to_vec(),
        );
        self.validators = light_block.validators.clone();

        Ok(validator_set_changed)
    }

    fn encode(&self) -> Result<Bytes, PrecompileError> {
        let validator_set_length = self.validators.validators().len();
        let serialize_length = (CHAIN_ID_LENGTH +
            HEIGHT_LENGTH +
            VALIDATOR_SET_HASH_LENGTH +
            validator_set_length as u64 * SINGLE_VALIDATOR_BYTES_LENGTH)
            as usize;
        if serialize_length > MAX_CONSENSUS_STATE_LENGTH as usize {
            return Err(BscPrecompileError::CometBftEncodeConsensusStateFailed.into());
        }
        if self.chain_id.len() > CHAIN_ID_LENGTH as usize {
            return Err(BscPrecompileError::CometBftEncodeConsensusStateFailed.into());
        }

        let mut output = vec![0; serialize_length];
        let mut pos: usize = 0;
        let chain_id_bytes = self.chain_id.as_bytes();
        if chain_id_bytes.len() > CHAIN_ID_LENGTH as usize {
            return Err(BscPrecompileError::CometBftEncodeConsensusStateFailed.into());
        }
        let mut filled_chain_id = [0u8; 32];
        filled_chain_id[..chain_id_bytes.len()].copy_from_slice(chain_id_bytes);
        output[pos..pos + CHAIN_ID_LENGTH as usize]
            .copy_from_slice(filled_chain_id.to_vec().as_slice());
        pos += CHAIN_ID_LENGTH as usize;

        output[pos..pos + HEIGHT_LENGTH as usize].copy_from_slice(&self.height.to_be_bytes());
        pos += HEIGHT_LENGTH as usize;

        output[pos..pos + VALIDATOR_SET_HASH_LENGTH as usize]
            .copy_from_slice(self.next_validator_set_hash.as_ref());
        pos += VALIDATOR_SET_HASH_LENGTH as usize;

        for i in 0..validator_set_length {
            let validator = &self.validators.validators()[i];
            let voting_power = validator.power();

            output[pos..pos + VALIDATOR_PUBKEY_LENGTH as usize]
                .copy_from_slice(&validator.pub_key.to_bytes());
            pos += VALIDATOR_PUBKEY_LENGTH as usize;

            output[pos..pos + VALIDATOR_VOTING_POWER_LENGTH as usize]
                .copy_from_slice(&voting_power.to_be_bytes());
            pos += VALIDATOR_VOTING_POWER_LENGTH as usize;

            output[pos..pos + RELAYER_ADDRESS_LENGTH as usize]
                .copy_from_slice(validator.relayer_address.as_ref());
            pos += RELAYER_ADDRESS_LENGTH as usize;

            output[pos..pos + RELAYER_BLS_KEY_LENGTH as usize]
                .copy_from_slice(validator.bls_key.as_ref());
            pos += RELAYER_BLS_KEY_LENGTH as usize;
        }

        Ok(Bytes::from(output))
    }
}

type DecodeConsensusStateResult = Result<ConsensusState, PrecompileError>;
/// input:
/// | chainID   | height   | nextValidatorSetHash | [{validator pubkey, voting power, relayer address, relayer bls pubkey}] |
/// | 32 bytes  | 8 bytes  | 32 bytes             | [{32 bytes, 8 bytes, 20 bytes, 48 bytes}]
fn decode_consensus_state(input: &Bytes) -> DecodeConsensusStateResult {
    let minimum_length = CHAIN_ID_LENGTH + HEIGHT_LENGTH + VALIDATOR_SET_HASH_LENGTH;
    let input_length = input.len() as u64;
    if input_length <= minimum_length ||
        (input_length - minimum_length) % SINGLE_VALIDATOR_BYTES_LENGTH != 0
    {
        return Err(BscPrecompileError::CometBftInvalidInput.into());
    }

    let mut pos = 0_u64;
    let chain_id = &input[..CHAIN_ID_LENGTH as usize];
    let chain_id = String::from_utf8_lossy(chain_id);
    let chain_id = chain_id.trim_end_matches('\0').to_owned();
    pos += CHAIN_ID_LENGTH;

    let height =
        u64::from_be_bytes(input[pos as usize..(pos + HEIGHT_LENGTH) as usize].try_into().unwrap());
    pos += HEIGHT_LENGTH;

    let next_validator_set_hash =
        Bytes::from(input[pos as usize..(pos + VALIDATOR_SET_HASH_LENGTH) as usize].to_vec());
    pos += VALIDATOR_SET_HASH_LENGTH;

    let validator_set_length = (input_length - minimum_length) / SINGLE_VALIDATOR_BYTES_LENGTH;
    let validator_set_bytes = input[pos as usize..].to_vec();
    let mut validator_set = Vec::with_capacity(validator_set_length as usize);
    for i in 0..validator_set_length {
        let validator = &validator_set_bytes[i as usize * SINGLE_VALIDATOR_BYTES_LENGTH as usize..
            (i + 1) as usize * SINGLE_VALIDATOR_BYTES_LENGTH as usize];

        let voting_power = u64::from_be_bytes(
            validator[VALIDATOR_PUBKEY_LENGTH as usize..
                (VALIDATOR_PUBKEY_LENGTH + VALIDATOR_VOTING_POWER_LENGTH) as usize]
                .try_into()
                .unwrap(),
        );
        let relayer_address = Bytes::from(
            validator[(VALIDATOR_PUBKEY_LENGTH + VALIDATOR_VOTING_POWER_LENGTH) as usize..
                (VALIDATOR_PUBKEY_LENGTH + VALIDATOR_VOTING_POWER_LENGTH + RELAYER_ADDRESS_LENGTH)
                    as usize]
                .to_vec(),
        );
        let relayer_bls_key = Bytes::from(
            validator[(VALIDATOR_PUBKEY_LENGTH +
                VALIDATOR_VOTING_POWER_LENGTH +
                RELAYER_ADDRESS_LENGTH) as usize..]
                .to_vec(),
        );
        let pk = match PublicKey::from_raw_ed25519(&validator[..VALIDATOR_PUBKEY_LENGTH as usize]) {
            Some(pk) => pk,
            None => return Err(BscPrecompileError::CometBftInvalidInput.into()),
        };
        let vp = Power::from(voting_power as u32);
        let validator_info = Validator::new_with_bls_and_relayer(
            pk,
            vp,
            relayer_bls_key.to_vec(),
            relayer_address.to_vec(),
        );
        validator_set.push(validator_info);
    }

    Ok(ConsensusState::new(
        chain_id,
        height,
        next_validator_set_hash,
        ValidatorSet::without_proposer(validator_set),
    ))
}

/// output:
/// | validatorSetChanged | empty      | consensusStateBytesLength |  new consensusState |
/// | 1 byte              | 23 bytes   | 8 bytes                   |                     |
fn encode_light_block_validation_result(
    validator_set_changed: bool,
    consensus_state_bytes: Bytes,
) -> Bytes {
    let mut output =
        vec![0; (VALIDATE_RESULT_METADATA_LENGTH + consensus_state_bytes.len() as u64) as usize];
    output[0] = if validator_set_changed { 1 } else { 0 };
    output[24..32].copy_from_slice(consensus_state_bytes.len().to_be_bytes().as_ref());
    output[32..].copy_from_slice(consensus_state_bytes.as_ref());
    Bytes::from(output)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::hex;

    use super::*;

    #[test]
    fn test_cometbft_light_block_validate() {
        {
            let input = Bytes::from(hex!(
                "000000000000000000000000000000000000000000000000000000000000018c677265656e6669656c645f393030302d3132310000000000000000000000000000000000000000013c350cd55b99dc6c2b7da9bef5410fbfb869fede858e7b95bf7ca294e228bb40e33f6e876d63791ebd05ff617a1b4f4ad1aa2ce65e3c3a9cdfb33e0ffa7e8423000000000098968015154514f68ce65a0d9eecc578c0ab12da0a2a28a0805521b5b7ae56eb3fb24555efbfe59e1622bfe9f7be8c9022e9b3f2442739c1ce870b9adee169afe60f674edd7c86451c5363d89052fde8351895eeea166ce5373c36e31b518ed191d0c599aa0f5b0000000000989680432f6c4908a9aa5f3444421f466b11645235c99b831b2a2de9e504d7ea299e52a202ce529808618eb3bfc0addf13d8c5f2df821d81e18f9bc61583510b322d067d46323b0a572635c06a049c0a2a929e3c8184a50cf6a8b95708c25834ade456f399015a0000000000989680864cb9828254d712f8e59b164fc6a9402dc4e6c59065e38cff24f5323c8c5da888a0f97e5ee4ba1e11b0674b0a0d06204c1dfa247c370cd4be3e799fc4f6f48d977ac7ca0aeb060adb030a02080b1213677265656e6669656c645f393030302d3132311802220c08b2d7f3a10610e8d2adb3032a480a20ec6ecb5db4ffb17fabe40c60ca7b8441e9c5d77585d0831186f3c37aa16e9c15122408011220a2ab9e1eb9ea52812f413526e424b326aff2f258a56e00d690db9f805b60fe7e32200f40aeff672e8309b7b0aefbb9a1ae3d4299b5c445b7d54e8ff398488467f0053a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85542203c350cd55b99dc6c2b7da9bef5410fbfb869fede858e7b95bf7ca294e228bb404a203c350cd55b99dc6c2b7da9bef5410fbfb869fede858e7b95bf7ca294e228bb405220294d8fbd0b94b767a7eba9840f299a3586da7fe6b5dead3b7eecba193c400f935a20bc50557c12d7392b0d07d75df0b61232d48f86a74fdea6d1485d9be6317d268c6220e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b8556a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85572146699336aa109d1beab3946198c8e59f3b2cbd92f7a4065e3cd89e315ca39d87dee92835b98f8b8ec0861d6d9bb2c60156df5d375b3ceb1fbe71af6a244907d62548a694165caa660fec7a9b4e7b9198191361c71be0b128a0308021a480a20726abd0fdbfb6f779b0483e6e4b4b6f12241f6ea2bf374233ab1a316692b6415122408011220159f10ff15a8b58fc67a92ffd7f33c8cd407d4ce81b04ca79177dfd00ca19a67226808021214050cff76cc632760ba9db796c046004c900967361a0c08b3d7f3a10610808cadba03224080713027ffb776a702d78fd0406205c629ba473e1f8d6af646190f6eb9262cd67d69be90d10e597b91e06d7298eb6fa4b8f1eb7752ebf352a1f51560294548042268080212146699336aa109d1beab3946198c8e59f3b2cbd92f1a0c08b3d7f3a10610b087c1c00322405e2ddb70acfe4904438be3d9f4206c0ace905ac4fc306a42cfc9e86268950a0fbfd6ec5f526d3e41a3ef52bf9f9f358e3cb4c3feac76c762fa3651c1244fe004226808021214c55765fd2d0570e869f6ac22e7f2916a35ea300d1a0c08b3d7f3a10610f0b3d492032240ca17898bd22232fc9374e1188636ee321a396444a5b1a79f7628e4a11f265734b2ab50caf21e8092c55d701248e82b2f011426cb35ba22043b497a6b4661930612a0050aa8010a14050cff76cc632760ba9db796c046004c9009673612220a20e33f6e876d63791ebd05ff617a1b4f4ad1aa2ce65e3c3a9cdfb33e0ffa7e84231880ade2042080a6bbf6ffffffffff012a30a0805521b5b7ae56eb3fb24555efbfe59e1622bfe9f7be8c9022e9b3f2442739c1ce870b9adee169afe60f674edd7c86321415154514f68ce65a0d9eecc578c0ab12da0a2a283a14ee7a2a6a44d427f6949eeb8f12ea9fbb2501da880aa2010a146699336aa109d1beab3946198c8e59f3b2cbd92f12220a20451c5363d89052fde8351895eeea166ce5373c36e31b518ed191d0c599aa0f5b1880ade2042080ade2042a30831b2a2de9e504d7ea299e52a202ce529808618eb3bfc0addf13d8c5f2df821d81e18f9bc61583510b322d067d46323b3214432f6c4908a9aa5f3444421f466b11645235c99b3a14a0a7769429468054e19059af4867da0a495567e50aa2010a14c55765fd2d0570e869f6ac22e7f2916a35ea300d12220a200a572635c06a049c0a2a929e3c8184a50cf6a8b95708c25834ade456f399015a1880ade2042080ade2042a309065e38cff24f5323c8c5da888a0f97e5ee4ba1e11b0674b0a0d06204c1dfa247c370cd4be3e799fc4f6f48d977ac7ca3214864cb9828254d712f8e59b164fc6a9402dc4e6c53a143139916d97df0c589312b89950b6ab9795f34d1a12a8010a14050cff76cc632760ba9db796c046004c9009673612220a20e33f6e876d63791ebd05ff617a1b4f4ad1aa2ce65e3c3a9cdfb33e0ffa7e84231880ade2042080a6bbf6ffffffffff012a30a0805521b5b7ae56eb3fb24555efbfe59e1622bfe9f7be8c9022e9b3f2442739c1ce870b9adee169afe60f674edd7c86321415154514f68ce65a0d9eecc578c0ab12da0a2a283a14ee7a2a6a44d427f6949eeb8f12ea9fbb2501da88"));
            let except_output = Bytes::from(hex!(
                "000000000000000000000000000000000000000000000000000000000000018c677265656e6669656c645f393030302d3132310000000000000000000000000000000000000000023c350cd55b99dc6c2b7da9bef5410fbfb869fede858e7b95bf7ca294e228bb40e33f6e876d63791ebd05ff617a1b4f4ad1aa2ce65e3c3a9cdfb33e0ffa7e8423000000000098968015154514f68ce65a0d9eecc578c0ab12da0a2a28a0805521b5b7ae56eb3fb24555efbfe59e1622bfe9f7be8c9022e9b3f2442739c1ce870b9adee169afe60f674edd7c86451c5363d89052fde8351895eeea166ce5373c36e31b518ed191d0c599aa0f5b0000000000989680432f6c4908a9aa5f3444421f466b11645235c99b831b2a2de9e504d7ea299e52a202ce529808618eb3bfc0addf13d8c5f2df821d81e18f9bc61583510b322d067d46323b0a572635c06a049c0a2a929e3c8184a50cf6a8b95708c25834ade456f399015a0000000000989680864cb9828254d712f8e59b164fc6a9402dc4e6c59065e38cff24f5323c8c5da888a0f97e5ee4ba1e11b0674b0a0d06204c1dfa247c370cd4be3e799fc4f6f48d977ac7ca"
            ));

            let result = cometbft_light_block_validation_run(&input, 100_000);
            let PrecompileOutput { gas_used, bytes } = match result {
                Ok(output) => output,
                Err(_) => panic!("cometbft_light_block_validation_run failed"),
            };
            assert_eq!(gas_used, 3_000);
            assert_eq!(bytes, except_output);
        }
        // apply light block failed
        {
            let input = Bytes::from(hex!(
                "0000000000000000000000000000000000000000000000000000000000000264677265656e6669656c645f393030302d313734310000000000000000000000000000000000000001af6b801dda578dddfa4da1d5d67fd1b32510db24ec271346fc573e9242b01c9a112b51dda2d336246bdc0cc51407ba0cb0e5087be0db5f1cdc3285bbaa8e647500000000000003e84202722cf6a34d727be762b46825b0d26b6263a0a9355ebf3c24bedac5a357a56feeb2cd8b6fed9f14cca15c3091f523b9fb21183b4bb31eb482a0321885e3f57072156448e2b2f7d9a3e7b668757d9cc0bbd28cd674c34ed1c2ed75c5de3b6a8f8cad4600000000000003e8668a0acd8f6db5cae959a0e02132f4d6a672c4d7a4726b542012cc8023ee07b29ab3971cc999d8751bbd16f23413968afcdb070ed66ab47e6e1842bf875bef21dfc5b8af6813bfd82860d361e339bd1ae2f801b6d6ee46b8497a3d51c80b50b6160ea1cc00000000000003e80dfa99423d3084c596c5e3bd6bcb4f654516517b8d4786703c56b300b70f085c0d0482e5d6a3c7208883f0ec8abd2de893f71d18e8f919e7ab198499201d87f92c57ebce83ed2b763bb872e9bc148fb216fd5c93b18819670d9a946ae4b3075672d726b800000000000003e824aab6f85470ff73e3048c64083a09e980d4cb7f8146d231a7b2051c5f7a9c07ab6e6bfe277bd5f4a94f901fe6ee7a6b6bd8479e9e5e448de4b1b33d5ddd74194c86b3852cc140a3f08a9c4149efd45643202f8bef2ad7eecf53e58951c6df6fd932004b00000000000003e84998f6ef8d999a0f36a851bfa29dbcf0364dd65695c286deb3f1657664859d59876bf1ec5a288f6e66e18b37b8a2a1e6ee4a3ef8fa50784d8b758d0c3e70a7cdfe65ab5d0ad7080adf030a02080b1214677265656e6669656c645f393030302d3137343118f7fdbd01220c08dca092aa0610e79ba49c012a480a209cda416227712ec137b852d1de0fbb957045e6bf7e541bb595d7f5391b481360122408011220fbb48c3c7cbbe68becfd80e7dcdcee8b8737afdbdc2e484cdc8fd52a659215e932207ddc7495ef0d0c1229ae33348d7907d90459ecccf6dcb3415724b41c0b4d1b7c3a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855422003ebb95f12cb304c52484a0821ba13f4e73468f3139f45e168f8c11d9ba74ce04a2003ebb95f12cb304c52484a0821ba13f4e73468f3139f45e168f8c11d9ba74ce05220eceb4055624b9f678278a2706ca2f43e11f31a641580d5aacf2f4383d8edef7e5a206796260ff3744ac8f8a5007a23408c72abe018d65df767b71290aac5da68d5ba6220694a73ea799423215418f2390a3f56f23b31391fbbab0de7a8f5b3c4822257906a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85572144b7d2bc2f5c3381c9bbf55edface8747722bc9d07a4050a45d04cd3d64ee8bc00bd5842e8ee2458001e0dcb6fc78bb1f44271ebca04ea65c689c6776582962c27492a5850e80a919a98d889d21ecfa83c54f15c4de0812f20408f7fdbd011a480a202585ab3a1bc673b3c68cc77e08b8190667641f30edffa89ae5ddfc2e1f53d60b122408011220c55ae2ca75b9705b4a486067b088c275919a6c3fc2de51cf09973232457a16042268080212144b7d2bc2f5c3381c9bbf55edface8747722bc9d01a0c08dea092aa0610eca4d6a6012240243914b3cf5be06bc69a6c34f2fc41928c1043b5a7d1bff80fd567b27398c3ce890be932958ec0d2a2083c393afbd4c75776ce84c881ec58a73b1022fcb63a0a226808031214793cee4b478e537592c40ecfb2148ebe32b8f6051a0c08dea092aa0610dcdbcfd701224000c586fe47dc655b38e7f62c913f4dd217ce16fa143e9295e146918ad361c2ed777512de15ed65eefb479864a89eebe55e38ebb8644a3887f46970bda9472203226808031214d742fa5318dc3986e075e2b050529a22c6fa3b8b1a0c08dea092aa0610ec9dd2d701224007e0f3e5a3d209a75c5db60f1502880cf25c5fddad0717e4ecaa8bbd95ebf28e78ea3229e88d741a44d534f08ac283dc587a28ced5c25b9cc64657a5bc5ce50c226808031214f0f07dc2f5e159a35b9662553c6b4e51868502f71a0c08dea092aa0610b7c186d7012240546fbdcc448af86aa932a05d8db2e4bc99816b7035b00c494ffe514d562190a68c214e5468c16d7d3a8c2fa52a6165ce870f8fc2dd93fa7cee40f3fa74e118082268080312145fa8b3f3fcd4a3ea2495e11dd5dbd399b3d8d4f81a0c08dea092aa0610f2d4efd50122400fe7c2eb23f4e2b6e57722c79ead12b2180e87bc618fc26cceccb891107be796fe8242c700fb61eeac6936659503354dc1a66874725b80f316252820e308d90f220f08011a0b088092b8c398feffffff0112df070a91010a144b7d2bc2f5c3381c9bbf55edface8747722bc9d012220a20112b51dda2d336246bdc0cc51407ba0cb0e5087be0db5f1cdc3285bbaa8e647518d0ca1e209ed8ffffffffffffff012a30a9355ebf3c24bedac5a357a56feeb2cd8b6fed9f14cca15c3091f523b9fb21183b4bb31eb482a0321885e3f57072156432144202722cf6a34d727be762b46825b0d26b6263a00a88010a14793cee4b478e537592c40ecfb2148ebe32b8f60512220a206813bfd82860d361e339bd1ae2f801b6d6ee46b8497a3d51c80b50b6160ea1cc1889082089082a308d4786703c56b300b70f085c0d0482e5d6a3c7208883f0ec8abd2de893f71d18e8f919e7ab198499201d87f92c57ebce32140dfa99423d3084c596c5e3bd6bcb4f654516517b0a88010a14d742fa5318dc3986e075e2b050529a22c6fa3b8b12220a2083ed2b763bb872e9bc148fb216fd5c93b18819670d9a946ae4b3075672d726b818fe0720fe072a308146d231a7b2051c5f7a9c07ab6e6bfe277bd5f4a94f901fe6ee7a6b6bd8479e9e5e448de4b1b33d5ddd74194c86b385321424aab6f85470ff73e3048c64083a09e980d4cb7f0a88010a14f0f07dc2f5e159a35b9662553c6b4e51868502f712220a202cc140a3f08a9c4149efd45643202f8bef2ad7eecf53e58951c6df6fd932004b18fc0720fc072a3095c286deb3f1657664859d59876bf1ec5a288f6e66e18b37b8a2a1e6ee4a3ef8fa50784d8b758d0c3e70a7cdfe65ab5d32144998f6ef8d999a0f36a851bfa29dbcf0364dd6560a88010a145fa8b3f3fcd4a3ea2495e11dd5dbd399b3d8d4f812220a2048e2b2f7d9a3e7b668757d9cc0bbd28cd674c34ed1c2ed75c5de3b6a8f8cad4618f60720f6072a30a4726b542012cc8023ee07b29ab3971cc999d8751bbd16f23413968afcdb070ed66ab47e6e1842bf875bef21dfc5b8af3214668a0acd8f6db5cae959a0e02132f4d6a672c4d70a88010a1455b2b6281b02e991dd9cc790bc1e6cff9db1e2c612220a2057fef603948fe010f4410a27272fcf867287b3d8421eff0afd67a157b6facf3918e90720e9072a30acf60b4cfffda7d6b120cd513bfe39e0392b1a1c433f2bb6ec1fc9200ea8f4d0c44815d4d3872e2c685371f877454284321407e201acb9f7d331a37d52ab7ad246f5c8cd1ac11291010a144b7d2bc2f5c3381c9bbf55edface8747722bc9d012220a20112b51dda2d336246bdc0cc51407ba0cb0e5087be0db5f1cdc3285bbaa8e647518d0ca1e209ed8ffffffffffffff012a30a9355ebf3c24bedac5a357a56feeb2cd8b6fed9f14cca15c3091f523b9fb21183b4bb31eb482a0321885e3f57072156432144202722cf6a34d727be762b46825b0d26b6263a0"
            ));

            let result = cometbft_light_block_validation_run(&input, 100_000);
            let expected = Err(BscPrecompileError::CometBftApplyBlockFailed.into());
            assert_eq!(result, expected);
        }
        // consensus height >= light block height
        {
            let input = Bytes::from(hex!(
                "0000000000000000000000000000000000000000000000000000000000000264677265656e6669656c645f393030302d3137343100000000000000000000000000000000128d987caf6b801dda578dddfa4da1d5d67fd1b32510db24ec271346fc573e9242b01c9a112b51dda2d336246bdc0cc51407ba0cb0e5087be0db5f1cdc3285bbaa8e647500000000000003e84202722cf6a34d727be762b46825b0d26b6263a0a9355ebf3c24bedac5a357a56feeb2cd8b6fed9f14cca15c3091f523b9fb21183b4bb31eb482a0321885e3f57072156448e2b2f7d9a3e7b668757d9cc0bbd28cd674c34ed1c2ed75c5de3b6a8f8cad4600000000000003e8668a0acd8f6db5cae959a0e02132f4d6a672c4d7a4726b542012cc8023ee07b29ab3971cc999d8751bbd16f23413968afcdb070ed66ab47e6e1842bf875bef21dfc5b8af6813bfd82860d361e339bd1ae2f801b6d6ee46b8497a3d51c80b50b6160ea1cc00000000000003e80dfa99423d3084c596c5e3bd6bcb4f654516517b8d4786703c56b300b70f085c0d0482e5d6a3c7208883f0ec8abd2de893f71d18e8f919e7ab198499201d87f92c57ebce83ed2b763bb872e9bc148fb216fd5c93b18819670d9a946ae4b3075672d726b800000000000003e824aab6f85470ff73e3048c64083a09e980d4cb7f8146d231a7b2051c5f7a9c07ab6e6bfe277bd5f4a94f901fe6ee7a6b6bd8479e9e5e448de4b1b33d5ddd74194c86b3852cc140a3f08a9c4149efd45643202f8bef2ad7eecf53e58951c6df6fd932004b00000000000003e84998f6ef8d999a0f36a851bfa29dbcf0364dd65695c286deb3f1657664859d59876bf1ec5a288f6e66e18b37b8a2a1e6ee4a3ef8fa50784d8b758d0c3e70a7cdfe65ab5d0ad7080adf030a02080b1214677265656e6669656c645f393030302d3137343118f7fdbd01220c08dca092aa0610e79ba49c012a480a209cda416227712ec137b852d1de0fbb957045e6bf7e541bb595d7f5391b481360122408011220fbb48c3c7cbbe68becfd80e7dcdcee8b8737afdbdc2e484cdc8fd52a659215e932207ddc7495ef0d0c1229ae33348d7907d90459ecccf6dcb3415724b41c0b4d1b7c3a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855422003ebb95f12cb304c52484a0821ba13f4e73468f3139f45e168f8c11d9ba74ce04a2003ebb95f12cb304c52484a0821ba13f4e73468f3139f45e168f8c11d9ba74ce05220eceb4055624b9f678278a2706ca2f43e11f31a641580d5aacf2f4383d8edef7e5a206796260ff3744ac8f8a5007a23408c72abe018d65df767b71290aac5da68d5ba6220694a73ea799423215418f2390a3f56f23b31391fbbab0de7a8f5b3c4822257906a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85572144b7d2bc2f5c3381c9bbf55edface8747722bc9d07a4050a45d04cd3d64ee8bc00bd5842e8ee2458001e0dcb6fc78bb1f44271ebca04ea65c689c6776582962c27492a5850e80a919a98d889d21ecfa83c54f15c4de0812f20408f7fdbd011a480a202585ab3a1bc673b3c68cc77e08b8190667641f30edffa89ae5ddfc2e1f53d60b122408011220c55ae2ca75b9705b4a486067b088c275919a6c3fc2de51cf09973232457a16042268080212144b7d2bc2f5c3381c9bbf55edface8747722bc9d01a0c08dea092aa0610eca4d6a6012240243914b3cf5be06bc69a6c34f2fc41928c1043b5a7d1bff80fd567b27398c3ce890be932958ec0d2a2083c393afbd4c75776ce84c881ec58a73b1022fcb63a0a226808031214793cee4b478e537592c40ecfb2148ebe32b8f6051a0c08dea092aa0610dcdbcfd701224000c586fe47dc655b38e7f62c913f4dd217ce16fa143e9295e146918ad361c2ed777512de15ed65eefb479864a89eebe55e38ebb8644a3887f46970bda9472203226808031214d742fa5318dc3986e075e2b050529a22c6fa3b8b1a0c08dea092aa0610ec9dd2d701224007e0f3e5a3d209a75c5db60f1502880cf25c5fddad0717e4ecaa8bbd95ebf28e78ea3229e88d741a44d534f08ac283dc587a28ced5c25b9cc64657a5bc5ce50c226808031214f0f07dc2f5e159a35b9662553c6b4e51868502f71a0c08dea092aa0610b7c186d7012240546fbdcc448af86aa932a05d8db2e4bc99816b7035b00c494ffe514d562190a68c214e5468c16d7d3a8c2fa52a6165ce870f8fc2dd93fa7cee40f3fa74e118082268080312145fa8b3f3fcd4a3ea2495e11dd5dbd399b3d8d4f81a0c08dea092aa0610f2d4efd50122400fe7c2eb23f4e2b6e57722c79ead12b2180e87bc618fc26cceccb891107be796fe8242c700fb61eeac6936659503354dc1a66874725b80f316252820e308d90f220f08011a0b088092b8c398feffffff0112df070a91010a144b7d2bc2f5c3381c9bbf55edface8747722bc9d012220a20112b51dda2d336246bdc0cc51407ba0cb0e5087be0db5f1cdc3285bbaa8e647518d0ca1e209ed8ffffffffffffff012a30a9355ebf3c24bedac5a357a56feeb2cd8b6fed9f14cca15c3091f523b9fb21183b4bb31eb482a0321885e3f57072156432144202722cf6a34d727be762b46825b0d26b6263a00a88010a14793cee4b478e537592c40ecfb2148ebe32b8f60512220a206813bfd82860d361e339bd1ae2f801b6d6ee46b8497a3d51c80b50b6160ea1cc1889082089082a308d4786703c56b300b70f085c0d0482e5d6a3c7208883f0ec8abd2de893f71d18e8f919e7ab198499201d87f92c57ebce32140dfa99423d3084c596c5e3bd6bcb4f654516517b0a88010a14d742fa5318dc3986e075e2b050529a22c6fa3b8b12220a2083ed2b763bb872e9bc148fb216fd5c93b18819670d9a946ae4b3075672d726b818fe0720fe072a308146d231a7b2051c5f7a9c07ab6e6bfe277bd5f4a94f901fe6ee7a6b6bd8479e9e5e448de4b1b33d5ddd74194c86b385321424aab6f85470ff73e3048c64083a09e980d4cb7f0a88010a14f0f07dc2f5e159a35b9662553c6b4e51868502f712220a202cc140a3f08a9c4149efd45643202f8bef2ad7eecf53e58951c6df6fd932004b18fc0720fc072a3095c286deb3f1657664859d59876bf1ec5a288f6e66e18b37b8a2a1e6ee4a3ef8fa50784d8b758d0c3e70a7cdfe65ab5d32144998f6ef8d999a0f36a851bfa29dbcf0364dd6560a88010a145fa8b3f3fcd4a3ea2495e11dd5dbd399b3d8d4f812220a2048e2b2f7d9a3e7b668757d9cc0bbd28cd674c34ed1c2ed75c5de3b6a8f8cad4618f60720f6072a30a4726b542012cc8023ee07b29ab3971cc999d8751bbd16f23413968afcdb070ed66ab47e6e1842bf875bef21dfc5b8af3214668a0acd8f6db5cae959a0e02132f4d6a672c4d70a88010a1455b2b6281b02e991dd9cc790bc1e6cff9db1e2c612220a2057fef603948fe010f4410a27272fcf867287b3d8421eff0afd67a157b6facf3918e90720e9072a30acf60b4cfffda7d6b120cd513bfe39e0392b1a1c433f2bb6ec1fc9200ea8f4d0c44815d4d3872e2c685371f877454284321407e201acb9f7d331a37d52ab7ad246f5c8cd1ac11291010a144b7d2bc2f5c3381c9bbf55edface8747722bc9d012220a20112b51dda2d336246bdc0cc51407ba0cb0e5087be0db5f1cdc3285bbaa8e647518d0ca1e209ed8ffffffffffffff012a30a9355ebf3c24bedac5a357a56feeb2cd8b6fed9f14cca15c3091f523b9fb21183b4bb31eb482a0321885e3f57072156432144202722cf6a34d727be762b46825b0d26b6263a0"
            ));

            let result = cometbft_light_block_validation_run(&input, 100_000);
            let expected = Err(BscPrecompileError::CometBftInvalidInput.into());
            assert_eq!(result, expected);
        }
        // chain id mismatch
        {
            let input = Bytes::from(hex!(
                "00000000000000000000000000000000000000000000000000000000000004ec677265656e6669656c645f353630302d31000000000000000000000000000000000000000000000001c6ca63d82177ae8b4f8533bec2d4dc45a34ba3a49b720545ca48e8c6ea829a33e043df137dfc0ea134be6fae019c746c7e92bbac1641e0752557f1d30306881300000000000003e82fcbd94e02d0de3e58a4cd20dd545340b9ce990a8a9910093c8c5da624dc842eb31117b548609680bae2d1940743256c121e397ff8ead4f5582910bf329c9406ac2d83186a6ae749f288a4ce711186d2606bf4e18ae3c06fbd560d0bd2fd8ab532486e3300000000000003e8b6202c3a1620e48b9458ed75b533f37ace46398fac693b30853e3a5fa3ea18d1df5c41e94f0060cb4ecf5f3483528c525ed317d5f3441adb984431556f5d2202bf77586940affc7c57c8bf533c80569e4874de92cd406468c62b14812bbe99f3e0a14e6d00000000000003e81eb931cc754eefb3ebda912fb3bb67b32b64c1a89469c75de5a4dde7ecd8de85f1ba5acee07454cb900585354a2fda279dd115c8946dffddee2e48234e10dd785fdd7f182c6de7b46dbe53aecafcbda7c7439b6a851d9933ee8963a08335bbacd8ef710500000000000003e8bacea9f5f6f5521afc9a4d9245432f4074ce67c38de9dac5bccbfef36c3d78d743fa36c06d2ade58cfecaa8903770d7793a4bf8e9e705b4b6daf7a10b29d1190b0eef5c5ceeecd09ea4f481b37562e25264de176566582032d4c7912945fd43ae5c7f82e00000000000003e85379299a1a093c824c70629894620050ac18798e90242b34c53205728de8e81ef36b17d0e49b30f039d9c224f952410ad9a9fe1d07a001e60e92070f4082875070a4a0b688c92507b2ce3f073fe1169345544501ee11b5e495b4b2c230db9cfb25cad2de00000000000003e896931f9918e930d3e48c77aaa479349805dbcdcd85a874ce490ea46e3ed8883544496c8852de074094185ad5d881df95df71287c7a837da79b9e4e2aa5cee0d23ccff22fe17d86788d73a0d4af8a9b63d9b2b7c3bc8c3ba81871e4ba3f77be43aa8510b600000000000003e8cbe5cd96be693071413f313cc76bf085eed1806ab3f99d1a867d6710e5a9227009d45568b4a34e0355594c85d1f7330c10b706301127c39e130f02e53789e511147f550786125621c641b3344c7aa286673073cc72abac17790545ea2770f5e8be04b15500000000000003e830db54e9bd6c32580b3f01256e9ecbf97a2ea57191ef0170f22bbc666291be47e2b4ce933ceb3a4a547794ebe12097152c750068a32afeff9cfb32d89cc597e27dd6e0f20bcc4c8635134d01c17590af47838723d8246bf005ff6e74155b0a8671c1bdee00000000000003e8ebfc660e51646a77b37731c31dd46fa5ca92671eb0224a76a44eb32d778421d39726aebd29ceac9ab368aafdb51e7c19bdbf46b7e17a3f372c9b0ee842f5b37e683c47d697aa82c1183da679f69c741da4509f94490a364132958a1f0356967e6d73183300000000000003e80c2201ebc288e539dcbc8221652c719f3ee2edb38d3293a19d0e86b79ad6b5eb5bf50ca2361e5e68338db543de2551b4e1d3f48ba0e75586a5ab394b8d739ebdfd548f0a11fb80525fbe947284e9415615b58725e55876a00f0ee1d20b8b0e70dcde5bce00000000000003e8aeee47645498286d615b8850764f55985b8a643ea8c70c061865d6307e34bfd26e9b61bdcc0ae35f6cacc04eaa766f4b76ec354b99cd62fdb599e62a6cfe1e652ddf83500ad7080adf030a02080b1214677265656e6669656c645f393030302d3137343118f7fdbd01220c08dca092aa0610e79ba49c012a480a209cda416227712ec137b852d1de0fbb957045e6bf7e541bb595d7f5391b481360122408011220fbb48c3c7cbbe68becfd80e7dcdcee8b8737afdbdc2e484cdc8fd52a659215e932207ddc7495ef0d0c1229ae33348d7907d90459ecccf6dcb3415724b41c0b4d1b7c3a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855422003ebb95f12cb304c52484a0821ba13f4e73468f3139f45e168f8c11d9ba74ce04a2003ebb95f12cb304c52484a0821ba13f4e73468f3139f45e168f8c11d9ba74ce05220eceb4055624b9f678278a2706ca2f43e11f31a641580d5aacf2f4383d8edef7e5a206796260ff3744ac8f8a5007a23408c72abe018d65df767b71290aac5da68d5ba6220694a73ea799423215418f2390a3f56f23b31391fbbab0de7a8f5b3c4822257906a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85572144b7d2bc2f5c3381c9bbf55edface8747722bc9d07a4050a45d04cd3d64ee8bc00bd5842e8ee2458001e0dcb6fc78bb1f44271ebca04ea65c689c6776582962c27492a5850e80a919a98d889d21ecfa83c54f15c4de0812f20408f7fdbd011a480a202585ab3a1bc673b3c68cc77e08b8190667641f30edffa89ae5ddfc2e1f53d60b122408011220c55ae2ca75b9705b4a486067b088c275919a6c3fc2de51cf09973232457a16042268080212144b7d2bc2f5c3381c9bbf55edface8747722bc9d01a0c08dea092aa0610eca4d6a6012240243914b3cf5be06bc69a6c34f2fc41928c1043b5a7d1bff80fd567b27398c3ce890be932958ec0d2a2083c393afbd4c75776ce84c881ec58a73b1022fcb63a0a226808031214793cee4b478e537592c40ecfb2148ebe32b8f6051a0c08dea092aa0610dcdbcfd701224000c586fe47dc655b38e7f62c913f4dd217ce16fa143e9295e146918ad361c2ed777512de15ed65eefb479864a89eebe55e38ebb8644a3887f46970bda9472203226808031214d742fa5318dc3986e075e2b050529a22c6fa3b8b1a0c08dea092aa0610ec9dd2d701224007e0f3e5a3d209a75c5db60f1502880cf25c5fddad0717e4ecaa8bbd95ebf28e78ea3229e88d741a44d534f08ac283dc587a28ced5c25b9cc64657a5bc5ce50c226808031214f0f07dc2f5e159a35b9662553c6b4e51868502f71a0c08dea092aa0610b7c186d7012240546fbdcc448af86aa932a05d8db2e4bc99816b7035b00c494ffe514d562190a68c214e5468c16d7d3a8c2fa52a6165ce870f8fc2dd93fa7cee40f3fa74e118082268080312145fa8b3f3fcd4a3ea2495e11dd5dbd399b3d8d4f81a0c08dea092aa0610f2d4efd50122400fe7c2eb23f4e2b6e57722c79ead12b2180e87bc618fc26cceccb891107be796fe8242c700fb61eeac6936659503354dc1a66874725b80f316252820e308d90f220f08011a0b088092b8c398feffffff0112df070a91010a144b7d2bc2f5c3381c9bbf55edface8747722bc9d012220a20112b51dda2d336246bdc0cc51407ba0cb0e5087be0db5f1cdc3285bbaa8e647518d0ca1e209ed8ffffffffffffff012a30a9355ebf3c24bedac5a357a56feeb2cd8b6fed9f14cca15c3091f523b9fb21183b4bb31eb482a0321885e3f57072156432144202722cf6a34d727be762b46825b0d26b6263a00a88010a14793cee4b478e537592c40ecfb2148ebe32b8f60512220a206813bfd82860d361e339bd1ae2f801b6d6ee46b8497a3d51c80b50b6160ea1cc1889082089082a308d4786703c56b300b70f085c0d0482e5d6a3c7208883f0ec8abd2de893f71d18e8f919e7ab198499201d87f92c57ebce32140dfa99423d3084c596c5e3bd6bcb4f654516517b0a88010a14d742fa5318dc3986e075e2b050529a22c6fa3b8b12220a2083ed2b763bb872e9bc148fb216fd5c93b18819670d9a946ae4b3075672d726b818fe0720fe072a308146d231a7b2051c5f7a9c07ab6e6bfe277bd5f4a94f901fe6ee7a6b6bd8479e9e5e448de4b1b33d5ddd74194c86b385321424aab6f85470ff73e3048c64083a09e980d4cb7f0a88010a14f0f07dc2f5e159a35b9662553c6b4e51868502f712220a202cc140a3f08a9c4149efd45643202f8bef2ad7eecf53e58951c6df6fd932004b18fc0720fc072a3095c286deb3f1657664859d59876bf1ec5a288f6e66e18b37b8a2a1e6ee4a3ef8fa50784d8b758d0c3e70a7cdfe65ab5d32144998f6ef8d999a0f36a851bfa29dbcf0364dd6560a88010a145fa8b3f3fcd4a3ea2495e11dd5dbd399b3d8d4f812220a2048e2b2f7d9a3e7b668757d9cc0bbd28cd674c34ed1c2ed75c5de3b6a8f8cad4618f60720f6072a30a4726b542012cc8023ee07b29ab3971cc999d8751bbd16f23413968afcdb070ed66ab47e6e1842bf875bef21dfc5b8af3214668a0acd8f6db5cae959a0e02132f4d6a672c4d70a88010a1455b2b6281b02e991dd9cc790bc1e6cff9db1e2c612220a2057fef603948fe010f4410a27272fcf867287b3d8421eff0afd67a157b6facf3918e90720e9072a30acf60b4cfffda7d6b120cd513bfe39e0392b1a1c433f2bb6ec1fc9200ea8f4d0c44815d4d3872e2c685371f877454284321407e201acb9f7d331a37d52ab7ad246f5c8cd1ac11291010a144b7d2bc2f5c3381c9bbf55edface8747722bc9d012220a20112b51dda2d336246bdc0cc51407ba0cb0e5087be0db5f1cdc3285bbaa8e647518d0ca1e209ed8ffffffffffffff012a30a9355ebf3c24bedac5a357a56feeb2cd8b6fed9f14cca15c3091f523b9fb21183b4bb31eb482a0321885e3f57072156432144202722cf6a34d727be762b46825b0d26b6263a0"
            ));

            let result = cometbft_light_block_validation_run(&input, 100_000);
            let expected = Err(BscPrecompileError::CometBftInvalidInput.into());
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn test_encode_consensus_state() {
        {
            let chain_id = "chain_9000-121".to_string();
            let height = 1;
            let next_validator_set_hash = Bytes::from(hex!(
                "0CE856B1DC9CDCF3BF2478291CF02C62AEEB3679889E9866931BF1FB05A10EDA"
            ));
            let mut validators_info = Vec::new();
            let bls_pub_key = Bytes::from(hex!("a60afe627fd78b19e07e07e19d446009dd53a18c6c8744176a5d851a762bbb51198e7e006f2a6ea7225661a61ecd832d"));
            let relayer_address = Bytes::from(hex!("B32d0723583040F3A16D1380D1e6AA874cD1bdF7"));
            validators_info.push(cometbft::validator::Info::new_with_bls_and_relayer(
                PublicKey::from_raw_ed25519(&hex!(
                    "c3d9a1082f42ca161402f8668f8e39ec9e30092affd8d3262267ac7e248a959e"
                ))
                .unwrap(),
                cometbft::vote::Power::from(10000_u32),
                bls_pub_key.to_vec(),
                relayer_address.to_vec(),
            ));
            let validator_set = ValidatorSet::without_proposer(validators_info);

            let cs = ConsensusState::new(chain_id, height, next_validator_set_hash, validator_set);

            let expected_output = Bytes::from(hex!("636861696e5f393030302d31323100000000000000000000000000000000000000000000000000010ce856b1dc9cdcf3bf2478291cf02c62aeeb3679889e9866931bf1fb05a10edac3d9a1082f42ca161402f8668f8e39ec9e30092affd8d3262267ac7e248a959e0000000000002710b32d0723583040f3a16d1380d1e6aa874cd1bdf7a60afe627fd78b19e07e07e19d446009dd53a18c6c8744176a5d851a762bbb51198e7e006f2a6ea7225661a61ecd832d"));
            let cs_bytes = cs.encode().unwrap();
            assert_eq!(cs_bytes, expected_output);
        }
        {
            let chain_id = "chain_9000-121".to_string();
            let height = 1;
            let next_validator_set_hash = Bytes::from(hex!(
                "A5F1AF4874227F1CDBE5240259A365AD86484A4255BFD65E2A0222D733FCDBC3"
            ));
            let mut validators_info = Vec::new();
            validators_info.push(cometbft::validator::Info::new_with_bls_and_relayer(
                PublicKey::from_raw_ed25519(&hex!(
                    "20cc466ee9412ddd49e0fff04cdb41bade2b7622f08b6bdacac94d4de03bdb97"
                ))
                .unwrap(),
                cometbft::vote::Power::from(10000_u32),
                Bytes::from(hex!("aa2d28cbcd1ea3a63479f6fb260a3d755853e6a78cfa6252584fee97b2ec84a9d572ee4a5d3bc1558bb98a4b370fb861")).to_vec(),
                Bytes::from(hex!("d5e63aeee6e6fa122a6a23a6e0fca87701ba1541")).to_vec(),
            ));
            validators_info.push(cometbft::validator::Info::new_with_bls_and_relayer(
                PublicKey::from_raw_ed25519(&hex!(
                    "6b0b523ee91ad18a63d63f21e0c40a83ef15963f4260574ca5159fd90a1c5270"
                ))
                .unwrap(),
                cometbft::vote::Power::from(10000_u32),
                Bytes::from(hex!("b31e74a881fc78681e3dfa440978d2b8be0708a1cbbca2c660866216975fdaf0e9038d9b7ccbf9731f43956dba7f2451")).to_vec(),
                Bytes::from(hex!("6fd1ceb5a48579f322605220d4325bd9ff90d5fa")).to_vec(),
            ));
            validators_info.push(cometbft::validator::Info::new_with_bls_and_relayer(
                PublicKey::from_raw_ed25519(&hex!(
                    "919606ae20bf5d248ee353821754bcdb456fd3950618fda3e32d3d0fb990eeda"
                ))
                .unwrap(),
                cometbft::vote::Power::from(10000_u32),
                Bytes::from(hex!("b32979580ea04984a2be033599c20c7a0c9a8d121b57f94ee05f5eda5b36c38f6e354c89328b92cdd1de33b64d3a0867")).to_vec(),
                Bytes::from(hex!("97376a436bbf54e0f6949b57aa821a90a749920a")).to_vec(),
            ));
            let validator_set = ValidatorSet::without_proposer(validators_info);
            let cs = ConsensusState::new(chain_id, height, next_validator_set_hash, validator_set);

            let expected_output = Bytes::from(hex!("636861696e5f393030302d3132310000000000000000000000000000000000000000000000000001a5f1af4874227f1cdbe5240259a365ad86484a4255bfd65e2a0222d733fcdbc320cc466ee9412ddd49e0fff04cdb41bade2b7622f08b6bdacac94d4de03bdb970000000000002710d5e63aeee6e6fa122a6a23a6e0fca87701ba1541aa2d28cbcd1ea3a63479f6fb260a3d755853e6a78cfa6252584fee97b2ec84a9d572ee4a5d3bc1558bb98a4b370fb8616b0b523ee91ad18a63d63f21e0c40a83ef15963f4260574ca5159fd90a1c527000000000000027106fd1ceb5a48579f322605220d4325bd9ff90d5fab31e74a881fc78681e3dfa440978d2b8be0708a1cbbca2c660866216975fdaf0e9038d9b7ccbf9731f43956dba7f2451919606ae20bf5d248ee353821754bcdb456fd3950618fda3e32d3d0fb990eeda000000000000271097376a436bbf54e0f6949b57aa821a90a749920ab32979580ea04984a2be033599c20c7a0c9a8d121b57f94ee05f5eda5b36c38f6e354c89328b92cdd1de33b64d3a0867"));
            let cs_bytes = cs.encode().unwrap();
            assert_eq!(cs_bytes, expected_output);
        }
    }

    #[test]
    fn test_decode_consensus_state() {
        {
            let chain_id = "chain_9000-121".to_string();
            let height = 1;
            let next_validator_set_hash = Bytes::from(hex!(
                "0CE856B1DC9CDCF3BF2478291CF02C62AEEB3679889E9866931BF1FB05A10EDA"
            ));
            let mut validators_info = Vec::new();
            validators_info.push(Validator::new_with_bls_and_relayer(
                PublicKey::from_raw_ed25519(&hex!(
                    "c3d9a1082f42ca161402f8668f8e39ec9e30092affd8d3262267ac7e248a959e"
                ))
                .unwrap(),
                cometbft::vote::Power::from(10000_u32),
                Bytes::from(hex!("a60afe627fd78b19e07e07e19d446009dd53a18c6c8744176a5d851a762bbb51198e7e006f2a6ea7225661a61ecd832d")).to_vec(),
                Bytes::from(hex!("B32d0723583040F3A16D1380D1e6AA874cD1bdF7")).to_vec(),
            ));
            let validator_set = ValidatorSet::without_proposer(validators_info);
            let bls_pub_key = Bytes::from(hex!("a60afe627fd78b19e07e07e19d446009dd53a18c6c8744176a5d851a762bbb51198e7e006f2a6ea7225661a61ecd832d"));
            let relayer_address = Bytes::from(hex!("B32d0723583040F3A16D1380D1e6AA874cD1bdF7"));
            let cs_bytes = Bytes::from(hex!("636861696e5f393030302d31323100000000000000000000000000000000000000000000000000010ce856b1dc9cdcf3bf2478291cf02c62aeeb3679889e9866931bf1fb05a10edac3d9a1082f42ca161402f8668f8e39ec9e30092affd8d3262267ac7e248a959e0000000000002710b32d0723583040f3a16d1380d1e6aa874cd1bdf7a60afe627fd78b19e07e07e19d446009dd53a18c6c8744176a5d851a762bbb51198e7e006f2a6ea7225661a61ecd832d"));
            let cs = match decode_consensus_state(&cs_bytes) {
                Ok(cs) => cs,
                Err(_) => panic!("decode consensus state failed"),
            };
            assert_eq!(cs.chain_id, chain_id);
            assert_eq!(cs.height, height);
            assert_eq!(cs.next_validator_set_hash, next_validator_set_hash);
            assert_eq!(cs.validators, validator_set);
            assert_eq!(
                cs.validators.validators()[0].relayer_address.as_bytes(),
                relayer_address.to_vec()
            );
            assert_eq!(cs.validators.validators()[0].bls_key.as_bytes(), bls_pub_key.to_vec());
        }
        {
            let chain_id = "chain_9000-121".to_string();
            let height = 1;
            let next_validator_set_hash = Bytes::from(hex!(
                "A5F1AF4874227F1CDBE5240259A365AD86484A4255BFD65E2A0222D733FCDBC3"
            ));
            let mut validators_info = Vec::new();
            let mut bls_pub_keys = Vec::new();
            let mut relayer_addresses = Vec::new();
            validators_info.push(Validator::new_with_bls_and_relayer(
                PublicKey::from_raw_ed25519(&hex!(
                    "20cc466ee9412ddd49e0fff04cdb41bade2b7622f08b6bdacac94d4de03bdb97"
                ))
                .unwrap(),
                cometbft::vote::Power::from(10000_u32),
                Bytes::from(hex!("aa2d28cbcd1ea3a63479f6fb260a3d755853e6a78cfa6252584fee97b2ec84a9d572ee4a5d3bc1558bb98a4b370fb861")).to_vec(),
                Bytes::from(hex!("d5e63aeee6e6fa122a6a23a6e0fca87701ba1541")).to_vec(),
            ));
            bls_pub_keys.push(Bytes::from(hex!("aa2d28cbcd1ea3a63479f6fb260a3d755853e6a78cfa6252584fee97b2ec84a9d572ee4a5d3bc1558bb98a4b370fb861")));
            relayer_addresses.push(Bytes::from(hex!("d5e63aeee6e6fa122a6a23a6e0fca87701ba1541")));
            validators_info.push(cometbft::validator::Info::new_with_bls_and_relayer(
                PublicKey::from_raw_ed25519(&hex!(
                    "6b0b523ee91ad18a63d63f21e0c40a83ef15963f4260574ca5159fd90a1c5270"
                ))
                .unwrap(),
                cometbft::vote::Power::from(10000_u32),
                Bytes::from(hex!("b31e74a881fc78681e3dfa440978d2b8be0708a1cbbca2c660866216975fdaf0e9038d9b7ccbf9731f43956dba7f2451")).to_vec(),
                Bytes::from(hex!("6fd1ceb5a48579f322605220d4325bd9ff90d5fa")).to_vec(),
            ));
            bls_pub_keys.push(Bytes::from(hex!("b31e74a881fc78681e3dfa440978d2b8be0708a1cbbca2c660866216975fdaf0e9038d9b7ccbf9731f43956dba7f2451")));
            relayer_addresses.push(Bytes::from(hex!("6fd1ceb5a48579f322605220d4325bd9ff90d5fa")));
            validators_info.push(cometbft::validator::Info::new_with_bls_and_relayer(
                PublicKey::from_raw_ed25519(&hex!(
                    "919606ae20bf5d248ee353821754bcdb456fd3950618fda3e32d3d0fb990eeda"
                ))
                .unwrap(),
                cometbft::vote::Power::from(10000_u32),
                Bytes::from(hex!("b32979580ea04984a2be033599c20c7a0c9a8d121b57f94ee05f5eda5b36c38f6e354c89328b92cdd1de33b64d3a0867")).to_vec(),
                Bytes::from(hex!("97376a436bbf54e0f6949b57aa821a90a749920a")).to_vec(),
            ));
            bls_pub_keys.push(Bytes::from(hex!("b32979580ea04984a2be033599c20c7a0c9a8d121b57f94ee05f5eda5b36c38f6e354c89328b92cdd1de33b64d3a0867")));
            relayer_addresses.push(Bytes::from(hex!("97376a436bbf54e0f6949b57aa821a90a749920a")));
            let validator_set = ValidatorSet::without_proposer(validators_info);
            let cs_bytes = Bytes::from(hex!("636861696e5f393030302d3132310000000000000000000000000000000000000000000000000001a5f1af4874227f1cdbe5240259a365ad86484a4255bfd65e2a0222d733fcdbc320cc466ee9412ddd49e0fff04cdb41bade2b7622f08b6bdacac94d4de03bdb970000000000002710d5e63aeee6e6fa122a6a23a6e0fca87701ba1541aa2d28cbcd1ea3a63479f6fb260a3d755853e6a78cfa6252584fee97b2ec84a9d572ee4a5d3bc1558bb98a4b370fb8616b0b523ee91ad18a63d63f21e0c40a83ef15963f4260574ca5159fd90a1c527000000000000027106fd1ceb5a48579f322605220d4325bd9ff90d5fab31e74a881fc78681e3dfa440978d2b8be0708a1cbbca2c660866216975fdaf0e9038d9b7ccbf9731f43956dba7f2451919606ae20bf5d248ee353821754bcdb456fd3950618fda3e32d3d0fb990eeda000000000000271097376a436bbf54e0f6949b57aa821a90a749920ab32979580ea04984a2be033599c20c7a0c9a8d121b57f94ee05f5eda5b36c38f6e354c89328b92cdd1de33b64d3a0867"));
            let cs = match decode_consensus_state(&cs_bytes) {
                Ok(cs) => cs,
                Err(_) => panic!("decode consensus state failed"),
            };

            assert_eq!(cs.chain_id, chain_id);
            assert_eq!(cs.height, height);
            assert_eq!(cs.next_validator_set_hash, next_validator_set_hash);
            assert_eq!(cs.validators, validator_set);
            assert_eq!(
                cs.validators.validators()[0].relayer_address.as_bytes(),
                relayer_addresses[0].to_vec()
            );
            assert_eq!(cs.validators.validators()[0].bls_key.as_bytes(), bls_pub_keys[0].to_vec());
            assert_eq!(
                cs.validators.validators()[1].relayer_address.as_bytes(),
                relayer_addresses[1].to_vec()
            );
            assert_eq!(cs.validators.validators()[1].bls_key.as_bytes(), bls_pub_keys[1].to_vec());
            assert_eq!(
                cs.validators.validators()[2].relayer_address.as_bytes(),
                relayer_addresses[2].to_vec()
            );
            assert_eq!(cs.validators.validators()[2].bls_key.as_bytes(), bls_pub_keys[2].to_vec());
        }
    }

    #[test]
    fn test_apply_light_block() {
        {
            let cs_bytes = Bytes::from(hex!("677265656e6669656c645f393030302d3132310000000000000000000000000000000000000000013c350cd55b99dc6c2b7da9bef5410fbfb869fede858e7b95bf7ca294e228bb40e33f6e876d63791ebd05ff617a1b4f4ad1aa2ce65e3c3a9cdfb33e0ffa7e8423000000000098968015154514f68ce65a0d9eecc578c0ab12da0a2a28a0805521b5b7ae56eb3fb24555efbfe59e1622bfe9f7be8c9022e9b3f2442739c1ce870b9adee169afe60f674edd7c86451c5363d89052fde8351895eeea166ce5373c36e31b518ed191d0c599aa0f5b0000000000989680432f6c4908a9aa5f3444421f466b11645235c99b831b2a2de9e504d7ea299e52a202ce529808618eb3bfc0addf13d8c5f2df821d81e18f9bc61583510b322d067d46323b0a572635c06a049c0a2a929e3c8184a50cf6a8b95708c25834ade456f399015a0000000000989680864cb9828254d712f8e59b164fc6a9402dc4e6c59065e38cff24f5323c8c5da888a0f97e5ee4ba1e11b0674b0a0d06204c1dfa247c370cd4be3e799fc4f6f48d977ac7ca"));
            let mut cs = match decode_consensus_state(&cs_bytes) {
                Ok(cs) => cs,
                Err(_) => panic!("decode consensus state failed"),
            };
            let light_block_bytes = Bytes::from(hex!("0aeb060adb030a02080b1213677265656e6669656c645f393030302d3132311802220c08b2d7f3a10610e8d2adb3032a480a20ec6ecb5db4ffb17fabe40c60ca7b8441e9c5d77585d0831186f3c37aa16e9c15122408011220a2ab9e1eb9ea52812f413526e424b326aff2f258a56e00d690db9f805b60fe7e32200f40aeff672e8309b7b0aefbb9a1ae3d4299b5c445b7d54e8ff398488467f0053a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85542203c350cd55b99dc6c2b7da9bef5410fbfb869fede858e7b95bf7ca294e228bb404a203c350cd55b99dc6c2b7da9bef5410fbfb869fede858e7b95bf7ca294e228bb405220294d8fbd0b94b767a7eba9840f299a3586da7fe6b5dead3b7eecba193c400f935a20bc50557c12d7392b0d07d75df0b61232d48f86a74fdea6d1485d9be6317d268c6220e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b8556a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85572146699336aa109d1beab3946198c8e59f3b2cbd92f7a4065e3cd89e315ca39d87dee92835b98f8b8ec0861d6d9bb2c60156df5d375b3ceb1fbe71af6a244907d62548a694165caa660fec7a9b4e7b9198191361c71be0b128a0308021a480a20726abd0fdbfb6f779b0483e6e4b4b6f12241f6ea2bf374233ab1a316692b6415122408011220159f10ff15a8b58fc67a92ffd7f33c8cd407d4ce81b04ca79177dfd00ca19a67226808021214050cff76cc632760ba9db796c046004c900967361a0c08b3d7f3a10610808cadba03224080713027ffb776a702d78fd0406205c629ba473e1f8d6af646190f6eb9262cd67d69be90d10e597b91e06d7298eb6fa4b8f1eb7752ebf352a1f51560294548042268080212146699336aa109d1beab3946198c8e59f3b2cbd92f1a0c08b3d7f3a10610b087c1c00322405e2ddb70acfe4904438be3d9f4206c0ace905ac4fc306a42cfc9e86268950a0fbfd6ec5f526d3e41a3ef52bf9f9f358e3cb4c3feac76c762fa3651c1244fe004226808021214c55765fd2d0570e869f6ac22e7f2916a35ea300d1a0c08b3d7f3a10610f0b3d492032240ca17898bd22232fc9374e1188636ee321a396444a5b1a79f7628e4a11f265734b2ab50caf21e8092c55d701248e82b2f011426cb35ba22043b497a6b4661930612a0050aa8010a14050cff76cc632760ba9db796c046004c9009673612220a20e33f6e876d63791ebd05ff617a1b4f4ad1aa2ce65e3c3a9cdfb33e0ffa7e84231880ade2042080a6bbf6ffffffffff012a30a0805521b5b7ae56eb3fb24555efbfe59e1622bfe9f7be8c9022e9b3f2442739c1ce870b9adee169afe60f674edd7c86321415154514f68ce65a0d9eecc578c0ab12da0a2a283a14ee7a2a6a44d427f6949eeb8f12ea9fbb2501da880aa2010a146699336aa109d1beab3946198c8e59f3b2cbd92f12220a20451c5363d89052fde8351895eeea166ce5373c36e31b518ed191d0c599aa0f5b1880ade2042080ade2042a30831b2a2de9e504d7ea299e52a202ce529808618eb3bfc0addf13d8c5f2df821d81e18f9bc61583510b322d067d46323b3214432f6c4908a9aa5f3444421f466b11645235c99b3a14a0a7769429468054e19059af4867da0a495567e50aa2010a14c55765fd2d0570e869f6ac22e7f2916a35ea300d12220a200a572635c06a049c0a2a929e3c8184a50cf6a8b95708c25834ade456f399015a1880ade2042080ade2042a309065e38cff24f5323c8c5da888a0f97e5ee4ba1e11b0674b0a0d06204c1dfa247c370cd4be3e799fc4f6f48d977ac7ca3214864cb9828254d712f8e59b164fc6a9402dc4e6c53a143139916d97df0c589312b89950b6ab9795f34d1a12a8010a14050cff76cc632760ba9db796c046004c9009673612220a20e33f6e876d63791ebd05ff617a1b4f4ad1aa2ce65e3c3a9cdfb33e0ffa7e84231880ade2042080a6bbf6ffffffffff012a30a0805521b5b7ae56eb3fb24555efbfe59e1622bfe9f7be8c9022e9b3f2442739c1ce870b9adee169afe60f674edd7c86321415154514f68ce65a0d9eecc578c0ab12da0a2a283a14ee7a2a6a44d427f6949eeb8f12ea9fbb2501da88"));
            let mut light_block_pb: TmLightBlock = TmLightBlock::default();
            match light_block_pb.merge(light_block_bytes) {
                Ok(_) => (),
                Err(_) => panic!("merge light block failed"),
            };
            let light_block = match convert_light_block_from_proto(&light_block_pb) {
                Ok(light_block) => light_block,
                Err(_) => panic!("convert light block from proto failed"),
            };
            let expected_height = 2_u64;
            let expected_validator_set_changed = false;

            match cs.apply_light_block(&light_block) {
                Ok(validator_set_changed) => {
                    assert_eq!(validator_set_changed, expected_validator_set_changed);
                    assert_eq!(cs.height, expected_height);
                }
                Err(_) => panic!("apply light block failed"),
            }
        }
        {
            let cs_bytes = Bytes::from(hex!("677265656e6669656c645f393030302d313734310000000000000000000000000000000000000001af6b801dda578dddfa4da1d5d67fd1b32510db24ec271346fc573e9242b01c9a112b51dda2d336246bdc0cc51407ba0cb0e5087be0db5f1cdc3285bbaa8e647500000000000003e84202722cf6a34d727be762b46825b0d26b6263a0a9355ebf3c24bedac5a357a56feeb2cd8b6fed9f14cca15c3091f523b9fb21183b4bb31eb482a0321885e3f57072156448e2b2f7d9a3e7b668757d9cc0bbd28cd674c34ed1c2ed75c5de3b6a8f8cad4600000000000003e8668a0acd8f6db5cae959a0e02132f4d6a672c4d7a4726b542012cc8023ee07b29ab3971cc999d8751bbd16f23413968afcdb070ed66ab47e6e1842bf875bef21dfc5b8af6813bfd82860d361e339bd1ae2f801b6d6ee46b8497a3d51c80b50b6160ea1cc00000000000003e80dfa99423d3084c596c5e3bd6bcb4f654516517b8d4786703c56b300b70f085c0d0482e5d6a3c7208883f0ec8abd2de893f71d18e8f919e7ab198499201d87f92c57ebce83ed2b763bb872e9bc148fb216fd5c93b18819670d9a946ae4b3075672d726b800000000000003e824aab6f85470ff73e3048c64083a09e980d4cb7f8146d231a7b2051c5f7a9c07ab6e6bfe277bd5f4a94f901fe6ee7a6b6bd8479e9e5e448de4b1b33d5ddd74194c86b3852cc140a3f08a9c4149efd45643202f8bef2ad7eecf53e58951c6df6fd932004b00000000000003e84998f6ef8d999a0f36a851bfa29dbcf0364dd65695c286deb3f1657664859d59876bf1ec5a288f6e66e18b37b8a2a1e6ee4a3ef8fa50784d8b758d0c3e70a7cdfe65ab5d"));
            let mut cs = match decode_consensus_state(&cs_bytes) {
                Ok(cs) => cs,
                Err(_) => panic!("decode consensus state failed"),
            };
            let light_block_bytes = Bytes::from(hex!("0aeb070ade030a02080b1214677265656e6669656c645f393030302d3137343118e9d810220c08f2f2b6a30610af9fcc8e022a480a20315130cf3a10f78c5f7633e3941f605151a6901910713c84da0d7929898e9b9e122408011220f09b2290e56b59a7286c2144a811c780f0fd5f631614a9f7ec2dec43f14ac5d63220d15354fdbcc6c7d3e8c5ede34f4f71e896599ba67773605eb6579e10e09254773a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b8554220311b22582926e7833b72904605441ed602896e8aeb093bca5f2e8170cea5ed6a4a20311b22582926e7833b72904605441ed602896e8aeb093bca5f2e8170cea5ed6a5220048091bc7ddc283f77bfbf91d73c44da58c3df8a9cbc867405d8b7f3daada22f5a20ee2da802b95c55e551291d96fe6ee4fe8074ddfa2df110042d6809acb665628a6220e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b8556a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b8557214793cee4b478e537592c40ecfb2148ebe32b8f6057a4034248b04af30e0d302cf8cedff585d5e1c6ff8db526bcf298d665cf301ca938a874c76ba9a1fd9fae302b2ec49a335930cf0242762c92843d7f9f7963d60580a12870408e9d8101a480a20452e1984f64c79550ac23db0c408b3eb021675d678ad94f9206ad7a2dec83a181224080112205224c29260b6c220685b29f593bac728e522e3e3675ec7edd92c12251acfe4b4226808021214d742fa5318dc3986e075e2b050529a22c6fa3b8b1a0c08f4f2b6a306109898f6a70322409762b7abd4dd63bb8858673dffd5795b1a87532d3719458d12fbbd1fd2443ca76bd36c4c09fa8952a440de4904f1b6b9270037a147431892c8ace96ad43bf90b2268080212145fa8b3f3fcd4a3ea2495e11dd5dbd399b3d8d4f81a0c08f4f2b6a30610f8f2fd9e03224093f2fc21a41492a34ed3b31ff2eba571ca752ae989f2e47728740bb1eec0f20eb59f59d390ce3d67734ab49a72bc2e97e185d21a4b00f3288ea50b0f1383220a226808021214793cee4b478e537592c40ecfb2148ebe32b8f6051a0c08f4f2b6a306108e8ed7a7032240a4a3c047ca75aeb6e9a21fbc3742f4339c64ead15d117675a2757f7db965aae3e6901f81a3707a67d91c61d6c842b95009e132e7fab187965dc04861d7faa902226808021214f0f07dc2f5e159a35b9662553c6b4e51868502f71a0c08f4f2b6a30610bfed829f032240e23ddc98b0bf7cc6cd494fd8ec96d440d29193910a6eca3dc7e41cdb14efa32471feb1ea2d613bb5acdd8623e8372ed3a36e1838bc75646bdfe9d2ef96647400220f08011a0b088092b8c398feffffff0112d0060a90010a14d742fa5318dc3986e075e2b050529a22c6fa3b8b12220a2083ed2b763bb872e9bc148fb216fd5c93b18819670d9a946ae4b3075672d726b818880820abe8ffffffffffffff012a308146d231a7b2051c5f7a9c07ab6e6bfe277bd5f4a94f901fe6ee7a6b6bd8479e9e5e448de4b1b33d5ddd74194c86b385321424aab6f85470ff73e3048c64083a09e980d4cb7f0a88010a145fa8b3f3fcd4a3ea2495e11dd5dbd399b3d8d4f812220a2048e2b2f7d9a3e7b668757d9cc0bbd28cd674c34ed1c2ed75c5de3b6a8f8cad4618fc0720fc072a30a4726b542012cc8023ee07b29ab3971cc999d8751bbd16f23413968afcdb070ed66ab47e6e1842bf875bef21dfc5b8af3214668a0acd8f6db5cae959a0e02132f4d6a672c4d70a88010a14793cee4b478e537592c40ecfb2148ebe32b8f60512220a206813bfd82860d361e339bd1ae2f801b6d6ee46b8497a3d51c80b50b6160ea1cc18ec0720ec072a308d4786703c56b300b70f085c0d0482e5d6a3c7208883f0ec8abd2de893f71d18e8f919e7ab198499201d87f92c57ebce32140dfa99423d3084c596c5e3bd6bcb4f654516517b0a88010a14f0f07dc2f5e159a35b9662553c6b4e51868502f712220a202cc140a3f08a9c4149efd45643202f8bef2ad7eecf53e58951c6df6fd932004b18ec0720ec072a3095c286deb3f1657664859d59876bf1ec5a288f6e66e18b37b8a2a1e6ee4a3ef8fa50784d8b758d0c3e70a7cdfe65ab5d32144998f6ef8d999a0f36a851bfa29dbcf0364dd6560a86010a1468478c1a37bc01c3acb7470cc6a78f1009a14f7012220a20de83e10566b038855254800b5b0ebf7c21aede9883c11e5cf289979e233b3efe180120012a3089063607696a9e6dbddbe6c23b4634a7c02b80212afc7ec65fb0d379d55d2d0cb25df19c0252356ffa2e2252eedd8f57321400000000000000000000000000000000000000001290010a14d742fa5318dc3986e075e2b050529a22c6fa3b8b12220a2083ed2b763bb872e9bc148fb216fd5c93b18819670d9a946ae4b3075672d726b818880820abe8ffffffffffffff012a308146d231a7b2051c5f7a9c07ab6e6bfe277bd5f4a94f901fe6ee7a6b6bd8479e9e5e448de4b1b33d5ddd74194c86b385321424aab6f85470ff73e3048c64083a09e980d4cb7f"));
            let mut light_block_pb: TmLightBlock = TmLightBlock::default();
            match light_block_pb.merge(light_block_bytes) {
                Ok(_) => (),
                Err(_) => panic!("merge light block failed"),
            };
            let light_block = match convert_light_block_from_proto(&light_block_pb) {
                Ok(light_block) => light_block,
                Err(_) => panic!("convert light block from proto failed"),
            };
            let expected_height = 273513_u64;
            let expected_validator_set_changed = true;

            match cs.apply_light_block(&light_block) {
                Ok(validator_set_changed) => {
                    assert_eq!(validator_set_changed, expected_validator_set_changed);
                    assert_eq!(cs.height, expected_height);
                }
                Err(_) => panic!("apply light block failed"),
            }
        }
    }

    #[test]
    fn test_cometbft_light_block_validate_before_hertz() {
        let input = Bytes::from(hex!(
            "000000000000000000000000000000000000000000000000000000000000018c677265656e6669656c645f393030302d3132310000000000000000000000000000000000000000013c350cd55b99dc6c2b7da9bef5410fbfb869fede858e7b95bf7ca294e228bb40e33f6e876d63791ebd05ff617a1b4f4ad1aa2ce65e3c3a9cdfb33e0ffa7e8423000000000098968015154514f68ce65a0d9eecc578c0ab12da0a2a28a0805521b5b7ae56eb3fb24555efbfe59e1622bfe9f7be8c9022e9b3f2442739c1ce870b9adee169afe60f674edd7c86451c5363d89052fde8351895eeea166ce5373c36e31b518ed191d0c599aa0f5b0000000000989680432f6c4908a9aa5f3444421f466b11645235c99b831b2a2de9e504d7ea299e52a202ce529808618eb3bfc0addf13d8c5f2df821d81e18f9bc61583510b322d067d46323b0a572635c06a049c0a2a929e3c8184a50cf6a8b95708c25834ade456f399015a0000000000989680864cb9828254d712f8e59b164fc6a9402dc4e6c59065e38cff24f5323c8c5da888a0f97e5ee4ba1e11b0674b0a0d06204c1dfa247c370cd4be3e799fc4f6f48d977ac7ca0aeb060adb030a02080b1213677265656e6669656c645f393030302d3132311802220c08b2d7f3a10610e8d2adb3032a480a20ec6ecb5db4ffb17fabe40c60ca7b8441e9c5d77585d0831186f3c37aa16e9c15122408011220a2ab9e1eb9ea52812f413526e424b326aff2f258a56e00d690db9f805b60fe7e32200f40aeff672e8309b7b0aefbb9a1ae3d4299b5c445b7d54e8ff398488467f0053a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85542203c350cd55b99dc6c2b7da9bef5410fbfb869fede858e7b95bf7ca294e228bb404a203c350cd55b99dc6c2b7da9bef5410fbfb869fede858e7b95bf7ca294e228bb405220294d8fbd0b94b767a7eba9840f299a3586da7fe6b5dead3b7eecba193c400f935a20bc50557c12d7392b0d07d75df0b61232d48f86a74fdea6d1485d9be6317d268c6220e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b8556a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85572146699336aa109d1beab3946198c8e59f3b2cbd92f7a4065e3cd89e315ca39d87dee92835b98f8b8ec0861d6d9bb2c60156df5d375b3ceb1fbe71af6a244907d62548a694165caa660fec7a9b4e7b9198191361c71be0b128a0308021a480a20726abd0fdbfb6f779b0483e6e4b4b6f12241f6ea2bf374233ab1a316692b6415122408011220159f10ff15a8b58fc67a92ffd7f33c8cd407d4ce81b04ca79177dfd00ca19a67226808021214050cff76cc632760ba9db796c046004c900967361a0c08b3d7f3a10610808cadba03224080713027ffb776a702d78fd0406205c629ba473e1f8d6af646190f6eb9262cd67d69be90d10e597b91e06d7298eb6fa4b8f1eb7752ebf352a1f51560294548042268080212146699336aa109d1beab3946198c8e59f3b2cbd92f1a0c08b3d7f3a10610b087c1c00322405e2ddb70acfe4904438be3d9f4206c0ace905ac4fc306a42cfc9e86268950a0fbfd6ec5f526d3e41a3ef52bf9f9f358e3cb4c3feac76c762fa3651c1244fe004226808021214c55765fd2d0570e869f6ac22e7f2916a35ea300d1a0c08b3d7f3a10610f0b3d492032240ca17898bd22232fc9374e1188636ee321a396444a5b1a79f7628e4a11f265734b2ab50caf21e8092c55d701248e82b2f011426cb35ba22043b497a6b4661930612a0050aa8010a14050cff76cc632760ba9db796c046004c9009673612220a20e33f6e876d63791ebd05ff617a1b4f4ad1aa2ce65e3c3a9cdfb33e0ffa7e84231880ade2042080a6bbf6ffffffffff012a30a0805521b5b7ae56eb3fb24555efbfe59e1622bfe9f7be8c9022e9b3f2442739c1ce870b9adee169afe60f674edd7c86321415154514f68ce65a0d9eecc578c0ab12da0a2a283a14ee7a2a6a44d427f6949eeb8f12ea9fbb2501da880aa2010a146699336aa109d1beab3946198c8e59f3b2cbd92f12220a20451c5363d89052fde8351895eeea166ce5373c36e31b518ed191d0c599aa0f5b1880ade2042080ade2042a30831b2a2de9e504d7ea299e52a202ce529808618eb3bfc0addf13d8c5f2df821d81e18f9bc61583510b322d067d46323b3214432f6c4908a9aa5f3444421f466b11645235c99b3a14a0a7769429468054e19059af4867da0a495567e50aa2010a14c55765fd2d0570e869f6ac22e7f2916a35ea300d12220a200a572635c06a049c0a2a929e3c8184a50cf6a8b95708c25834ade456f399015a1880ade2042080ade2042a309065e38cff24f5323c8c5da888a0f97e5ee4ba1e11b0674b0a0d06204c1dfa247c370cd4be3e799fc4f6f48d977ac7ca3214864cb9828254d712f8e59b164fc6a9402dc4e6c53a143139916d97df0c589312b89950b6ab9795f34d1a12a8010a14050cff76cc632760ba9db796c046004c9009673612220a20e33f6e876d63791ebd05ff617a1b4f4ad1aa2ce65e3c3a9cdfb33e0ffa7e84231880ade2042080a6bbf6ffffffffff012a30a0805521b5b7ae56eb3fb24555efbfe59e1622bfe9f7be8c9022e9b3f2442739c1ce870b9adee169afe60f674edd7c86321415154514f68ce65a0d9eecc578c0ab12da0a2a283a14ee7a2a6a44d427f6949eeb8f12ea9fbb2501da88"
        ));
        let except_output_after_hertz = Bytes::from(hex!(
            "000000000000000000000000000000000000000000000000000000000000018c677265656e6669656c645f393030302d3132310000000000000000000000000000000000000000023c350cd55b99dc6c2b7da9bef5410fbfb869fede858e7b95bf7ca294e228bb40e33f6e876d63791ebd05ff617a1b4f4ad1aa2ce65e3c3a9cdfb33e0ffa7e8423000000000098968015154514f68ce65a0d9eecc578c0ab12da0a2a28a0805521b5b7ae56eb3fb24555efbfe59e1622bfe9f7be8c9022e9b3f2442739c1ce870b9adee169afe60f674edd7c86451c5363d89052fde8351895eeea166ce5373c36e31b518ed191d0c599aa0f5b0000000000989680432f6c4908a9aa5f3444421f466b11645235c99b831b2a2de9e504d7ea299e52a202ce529808618eb3bfc0addf13d8c5f2df821d81e18f9bc61583510b322d067d46323b0a572635c06a049c0a2a929e3c8184a50cf6a8b95708c25834ade456f399015a0000000000989680864cb9828254d712f8e59b164fc6a9402dc4e6c59065e38cff24f5323c8c5da888a0f97e5ee4ba1e11b0674b0a0d06204c1dfa247c370cd4be3e799fc4f6f48d977ac7ca"
        ));

        let result = cometbft_light_block_validation_run_before_hertz(&input, 100_000);
        let PrecompileOutput { gas_used, bytes } = match result {
            Ok(output) => output,
            Err(_) => panic!("cometbft_light_block_validation_run failed"),
        };
        assert_eq!(gas_used, 3_000);
        assert_eq!(bytes, except_output_after_hertz);
    }
}
