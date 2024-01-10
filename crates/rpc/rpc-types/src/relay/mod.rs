//! Relay API bindings: <https://flashbots.github.io/relay-specs/>

#![allow(missing_docs)]

use crate::{
    beacon::{BlsPublicKey, BlsSignature},
    engine::{
        BlobsBundleV1, ExecutionPayload, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
    },
};
use alloy_primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};

pub mod error;

/// Represents an entry of the `/relay/v1/builder/validators` endpoint
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validator {
    #[serde_as(as = "DisplayFromStr")]
    pub slot: u64,
    #[serde_as(as = "DisplayFromStr")]
    pub validator_index: u64,
    pub entry: ValidatorRegistration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorRegistration {
    pub message: ValidatorRegistrationMessage,
    pub signature: BlsSignature,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorRegistrationMessage {
    #[serde(rename = "fee_recipient")]
    pub fee_recipient: Address,
    #[serde_as(as = "DisplayFromStr")]
    pub gas_limit: u64,
    #[serde_as(as = "DisplayFromStr")]
    pub timestamp: u64,
    pub pubkey: BlsPublicKey,
}

/// Represents public information about a block sent by a builder to the relay, or from the relay to
/// the proposer. Depending on the context, value might represent the claimed value by a builder
/// (not necessarily a value confirmed by the relay).
#[serde_as]
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ssz", derive(ssz_derive::Encode, ssz_derive::Decode))]
pub struct BidTrace {
    #[serde_as(as = "DisplayFromStr")]
    pub slot: u64,
    pub parent_hash: B256,
    pub block_hash: B256,
    #[serde(rename = "builder_pubkey")]
    pub builder_public_key: BlsPublicKey,
    #[serde(rename = "proposer_pubkey")]
    pub proposer_public_key: BlsPublicKey,
    pub proposer_fee_recipient: Address,
    #[serde_as(as = "DisplayFromStr")]
    pub gas_limit: u64,
    #[serde_as(as = "DisplayFromStr")]
    pub gas_used: u64,
    #[serde_as(as = "DisplayFromStr")]
    pub value: U256,
}

/// SignedBidTrace is a BidTrace with a signature
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ssz", derive(ssz_derive::Encode, ssz_derive::Decode))]
pub struct SignedBidTrace {
    pub message: BidTrace,
    pub signature: BlsSignature,
}

/// Submission for the `/relay/v1/builder/blocks` endpoint (Bellatrix).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "ssz", derive(ssz_derive::Decode, ssz_derive::Encode))]
pub struct SignedBidSubmissionV1 {
    pub message: BidTrace,
    #[serde(with = "crate::beacon::payload::beacon_payload_v1")]
    pub execution_payload: ExecutionPayloadV1,
    pub signature: BlsSignature,
}

/// Submission for the `/relay/v1/builder/blocks` endpoint (Capella).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "ssz", derive(ssz_derive::Decode, ssz_derive::Encode))]
pub struct SignedBidSubmissionV2 {
    pub message: BidTrace,
    #[serde(with = "crate::beacon::payload::beacon_payload_v2")]
    pub execution_payload: ExecutionPayloadV2,
    pub signature: BlsSignature,
}

/// Submission for the `/relay/v1/builder/blocks` endpoint (Deneb).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "ssz", derive(ssz_derive::Decode, ssz_derive::Encode))]
pub struct SignedBidSubmissionV3 {
    pub message: BidTrace,
    #[serde(with = "crate::beacon::payload::beacon_payload_v3")]
    pub execution_payload: ExecutionPayloadV3,
    /// The Deneb block bundle for this bid.
    pub blobs_bundle: BlobsBundleV1,
    pub signature: BlsSignature,
}

/// SubmitBlockRequest is the request from the builder to submit a block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitBlockRequest {
    pub message: BidTrace,
    #[serde(with = "crate::beacon::payload::beacon_payload")]
    pub execution_payload: ExecutionPayload,
    pub signature: BlsSignature,
}

/// A Request to validate a [SubmitBlockRequest] <https://github.com/flashbots/builder/blob/03ee71cf0a344397204f65ff6d3a917ee8e06724/eth/block-validation/api.go#L132-L136>
#[serde_as]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderBlockValidationRequest {
    #[serde(flatten)]
    pub request: SubmitBlockRequest,
    #[serde_as(as = "DisplayFromStr")]
    pub registered_gas_limit: u64,
}

/// A Request to validate a [SubmitBlockRequest] <https://github.com/flashbots/builder/blob/03ee71cf0a344397204f65ff6d3a917ee8e06724/eth/block-validation/api.go#L204-L204>
#[serde_as]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderBlockValidationRequestV2 {
    #[serde(flatten)]
    pub request: SubmitBlockRequest,
    #[serde_as(as = "DisplayFromStr")]
    pub registered_gas_limit: u64,
    pub withdrawals_root: B256,
}

/// Query for the GET `/relay/v1/data/bidtraces/proposer_payload_delivered`
///
/// Provides [BidTrace]s for payloads that were delivered to proposers.
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposerPayloadsDeliveredQuery {
    /// A specific slot
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<u64>,
    /// Maximum number of entries (200 max)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// Search for a specific blockhash
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_hash: Option<B256>,
    /// Search for a specific EL block number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_number: Option<u64>,
    /// filter results by a proposer public key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposer_key: Option<BlsPublicKey>,
    /// How to order results
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_by: Option<OrderBy>,
}

impl ProposerPayloadsDeliveredQuery {
    /// Sets the specific slot
    pub fn slot(mut self, slot: u64) -> Self {
        self.slot = Some(slot);
        self
    }

    /// Sets the maximum number of entries (200 max)
    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Sets the specific blockhash
    pub fn block_hash(mut self, block_hash: B256) -> Self {
        self.block_hash = Some(block_hash);
        self
    }

    /// Sets the specific EL block number
    pub fn block_number(mut self, block_number: u64) -> Self {
        self.block_number = Some(block_number);
        self
    }

    /// Sets the proposer public key
    pub fn proposer_key(mut self, proposer_key: BlsPublicKey) -> Self {
        self.proposer_key = Some(proposer_key);
        self
    }

    /// Configures how to order results
    pub fn order_by(mut self, order_by: OrderBy) -> Self {
        self.order_by = Some(order_by);
        self
    }

    /// Order results by descending value (highest value first)
    pub fn order_by_desc(self) -> Self {
        self.order_by(OrderBy::Desc)
    }

    /// Order results by ascending value (lowest value first)
    pub fn order_by_asc(self) -> Self {
        self.order_by(OrderBy::Asc)
    }
}

/// OrderBy : Sort results in either ascending or descending values.  * `-value` - descending value
/// (highest value first)  * `value` - ascending value (lowest value first) Sort results in either
/// ascending or descending values.  * `-value` - descending value (highest value first)  * `value`
/// - ascending value (lowest value first)
#[derive(
    Default, Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
)]
pub enum OrderBy {
    /// Sort result by descending value (highest value first)
    #[default]
    #[serde(rename = "-value")]
    Desc,
    /// Sort result by ascending value (lowest value first)
    #[serde(rename = "value")]
    Asc,
}

/// Query for the GET `/relay/v1/data/bidtraces/builder_blocks_received` endpoint.
/// This endpoint provides BidTraces for the builder block submission for a given slot (that were
/// verified successfully).
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderBlocksReceivedQuery {
    /// A specific slot
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<u64>,
    /// Maximum number of entries (200 max)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// Search for a specific blockhash
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_hash: Option<B256>,
    /// Search for a specific EL block number
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_number: Option<u64>,
}

impl BuilderBlocksReceivedQuery {
    /// Sets the specific slot
    pub fn slot(mut self, slot: u64) -> Self {
        self.slot = Some(slot);
        self
    }

    /// Sets the maximum number of entries (200 max)
    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Sets the specific blockhash
    pub fn block_hash(mut self, block_hash: B256) -> Self {
        self.block_hash = Some(block_hash);
        self
    }

    /// Sets the specific EL block number
    pub fn block_number(mut self, block_number: u64) -> Self {
        self.block_number = Some(block_number);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_validator() {
        let s = r#"[{"slot":"7689441","validator_index":"748813","entry":{"message":{"fee_recipient":"0xbe87be8ac54fb2a4ecb8d7935d0fc80f72c28f9f","gas_limit":"30000000","timestamp":"1688333351","pubkey":"0xb56ff6826cfa6b82fc6c2974988b1576fe5c34bd6c672f911e1d3eec1134822581d6d68f68992ad1f945b0c80468d941"},"signature":"0x8b42028d248f5a2fd41ab425408470ffde1d941ee83db3d9bde583feb22413608673dc27930383893410ef05e52ed8cf0e0291d8ed111189a065f9598176d1c51cabeaba8f628b2f92626bb58d2068292eb7682673a31473d0cdbe278e67c723"}},{"slot":"7689443","validator_index":"503252","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1680328764","pubkey":"0xa8ac80cd889110f407fd6e42d08c71faf158aa917c4c5f5d65bfcea7c4ae5231df9e14721c1980e18233fb1e79316cf6"},"signature":"0xa3e0e3190acc0be2c3a5f9c75cea5d79bebbb955f3f477794e8d7c0cbf73cd61fa0b0c3bfeb5cd9ba53a60c7bf9958640a63ecbd355a8ddb717f2ac8f413dbe4865cbae46291cb590410fa51471df9eaccb3602718df4c17f8eee750df8b5491"}},{"slot":"7689445","validator_index":"792322","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1691185175","pubkey":"0xa25455216e254c96d7ddfc029da033f6de715e91ab09b2fb6a97613b13a8e9189c43f0eaabd671b3c9aab051f6ff0630"},"signature":"0xb63ea72aac017cdfaa7b0fb9cbaffbe3c72b8ce2dd986b6e21834fc2f0accf9184e301de6c5f066bb7075d3f5d020a9e169dee80998685e20553c197ab27ef4d7b0a19f062796a228308ef33a01d0a08dbe35b02e7dca762e247c84e5ea9d170"}},{"slot":"7689446","validator_index":"888141","entry":{"message":{"fee_recipient":"0x73b9067aeeeab2e6263951ad9637971145566ba6","gas_limit":"30000000","timestamp":"1692606575","pubkey":"0x921b0309fffd798599f9564103f9ab34ebe2a3ea17ab39439e5e033ec5affb925a5ad65c0863a1d0f84365e4c1eec952"},"signature":"0x8a88068c7926d81348b773977050d093450b673af1762c0f0b416e4fcc76f277f2f117138780e62e49f5ac02d13ba5ed0e2d76df363003c4ff7ad40713ed3ef6aa3eb57580c8a3e1e6fe7245db700e541d1f2a7e88ec426c3fba82fa91b647a4"}},{"slot":"7689451","validator_index":"336979","entry":{"message":{"fee_recipient":"0xebec795c9c8bbd61ffc14a6662944748f299cacf","gas_limit":"30000000","timestamp":"1680621366","pubkey":"0x8b46eb0f36a51fcfab66910380be4d68c6323291eada9f68ad628a798a9c21ed858d62f1b15c08484b13c9cdcd0fc657"},"signature":"0x910afc415aed14a0c49cc1c2d29743018a69e517de84cee9e4ff2ea21a0d3b95c25b0cd59b8a2539fbe8e73d5e53963a0afbcf9f86db4de4ba03aa1f80d9dc1ecca18e597829e5e9269ce08b99ff347eba43c0d9c87c174f3a30422d4de800c8"}},{"slot":"7689452","validator_index":"390650","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1678460723","pubkey":"0x854464f0b798d1510a0f76a77190f15e9e67d5ac348647f5fe906539cf4ff7101fb1463f4c408b72e6fae9cfbd21ffd3"},"signature":"0xb5c3aa515cdf723f03fafd092150d6fc5453f6bcec873194927f64f65aa96241f4c5ed417e0676163c5a07af0d63f83811268096e520af3b6a5d5031e619609a0999efc03cc94a30a1175e5e5a52c66d868ebb527669be27a7b81920e44c511a"}},{"slot":"7689453","validator_index":"316626","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1680791443","pubkey":"0xa7a4ecdc53283698af917eaf0ba68e350f5cd136c2b12fa10e59f1e1fd15130da1889a28975777887d84638c85e9036c"},"signature":"0xa3f7604dafd63d9a22d66c228d7d90e83a69d9fa4b920dceeb2e7d43ef49307645725f7c4890fefce18ba41d36b4d23c09cef5109827c7fb3bc78f8526ba0bfeceb950a6f0b5d5d8ad1c8dc740267f053b4f9113141c27e1528c8557b9175e3f"}},{"slot":"7689455","validator_index":"733684","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1688895576","pubkey":"0xb2731b8efe43d723c4b621bd23e48dd00e1fe7816414e708f3607da6f17f5ba5c172ceac005591cb3e28320ae6aa81cf"},"signature":"0x85c8fd5201705413e004b0b4ef773c28d95864deab2e7698da6ea8b27c46867d03e50dae4ad523cebc1ea6205b7925810347e14f8db5396d11200c0cd099faefe254bc2844b29bf15d4d62e15f876f08ee53e2cd33ceee698d69f4f70e8eac82"}},{"slot":"7689457","validator_index":"950865","entry":{"message":{"fee_recipient":"0x8b733fe59d1190b3efa5be3f21a574ef2ab0c62b","gas_limit":"30000000","timestamp":"1696575191","pubkey":"0xa7c8fbb503de34fb92d43490533c35f0418a52ff5834462213950217b592f975caa3ac1daa3f1cdd89362d6f48ef46c1"},"signature":"0x9671727de015aa343db8a8068e27b555b59b6dcfc9b7f8b47bce820fe60cd1179dfdfc91270918edf40a918b513e5a16069e23aaf1cdd37fce0d63e3ade7ca9270ed4a64f70eb64915791e47074bf76aa3225ebd727336444b44826f2cf2a002"}},{"slot":"7689459","validator_index":"322181","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1677755315","pubkey":"0x9381507f58bd51e0ce662e6bb27796e416988bd1f64f219f8cffd2137e933a5fb4c8196ca80c653fd5f69ef378f038aa"},"signature":"0xaeb27e1b729dded42d28efcfadfc6691851749ccb19779f488a37a31e12a56cfd856e81d251fe0e7868aa539f66116b11216a9599bd51a06f386d2255381fcd9d7c698df980964a7e54428cee458e28e3ca5078db92e6837cba72276e7af3334"}},{"slot":"7689461","validator_index":"482285","entry":{"message":{"fee_recipient":"0x6d2e03b7effeae98bd302a9f836d0d6ab0002766","gas_limit":"30000000","timestamp":"1680010474","pubkey":"0x940c6e411db07f151c7c647cb09842f5979db8d89c11c3c1a08894588f1d561e59bc15bd085dc9a025aac52b1cf83d73"},"signature":"0xb4b9fab65a2c01208fd17117b83d59b1e0bb92ede9f7ac3f48f55957551b36add2c3d880e76118fecf1e01496bc3b065194ae6bcb317f1c583f70e2a67cf2a28f4f68d80fc3697941d3700468ac29aafd0118778112d253eb3c31d6bcbdc0c13"}},{"slot":"7689466","validator_index":"98883","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1680479471","pubkey":"0x8d50dad3c465c2c5cd327fd725e239915c0ba43adfdc106909665222c43b2705e9db77f8102308445ae5301131d2a843"},"signature":"0x8a597b9c3160f12bed715a0311134ce04875d05050eb6a349fcc470a610f039ce5a07eebf5332db4f2126b77ebdd1beb0df83784e01061489333aba009ecdb9767e61933f78d2fd96520769afffa3be4455d8dfc6de3fb1b2cee2133f8dd15cf"}},{"slot":"7689470","validator_index":"533204","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1695122498","pubkey":"0x88b31798f15c2857200c60ac923c5a310c204edbce0d49c755f3f0c65e688ab065870385a5b2b18972917c566ecc02a4"},"signature":"0x96033c3ac9d7083d889a723ecd99c79cb2ab3caebeac5435d2319fd300b796ca4f6261eca211b0dbb6df98ce23eba75b04203e026c5aee373d2ba579904ea06284ff58df7bd45ea9a4d9cc9b24ef15ee57e8894193d1c6c8883dace63feb77b7"}},{"slot":"7689471","validator_index":"129205","entry":{"message":{"fee_recipient":"0xed33259a056f4fb449ffb7b7e2ecb43a9b5685bf","gas_limit":"30000000","timestamp":"1695122497","pubkey":"0xb83dc440882e55185ef74631329f954e0f2d547e4f8882bd4470698d1883754eb9b5ee17a091de9730c80d5d1982f6e7"},"signature":"0xabc4d7cc48c2b4608ba49275620837a543e8a6a79d65395c9fca9794442acacf6df2fb1aca71f85472c521c4cf5797f702bd8adef32d7cd38d98334a0a11f8d197a702fa70f48d8512676e64e55a98914a0acc89b60a37046efb646f684a3917"}},{"slot":"7689472","validator_index":"225708","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1680528109","pubkey":"0xa35ae9c944764f4abb247471ad4744b5acec3156a5ec0e72f67a4417268c7c0a8c2775a9639e70ef345176c41b3cd1ba"},"signature":"0x96dd275d5eadd7648e0b8ef44070da65bd2e8d597b51e292d1f775af36595300dcd69e466b51e422f938e695c9cbacd71249d7dfadc9fdf358170082645f63a2ddc6fd82e6a68100460b6beac7603af09012ef06705900410204e8adb6c08d21"}},{"slot":"7689473","validator_index":"959108","entry":{"message":{"fee_recipient":"0xf197c6f2ac14d25ee2789a73e4847732c7f16bc9","gas_limit":"30000000","timestamp":"1696666607","pubkey":"0xaa4c7bc4848f7ea9939112c726e710db5603bc562ef006b6bf5f4644f31b9ab4daf8e3ff72f0140c077f756073dbe3bd"},"signature":"0x8de420ab9db85a658c2ba5feb59020d1e5c0c5e385f55cb877304a348609ad64ec3f3d7be1c6e847df8687bf9c045f2c06e56b4302a04df07801fbcccaf32dbeaeb854680536e13c7c2dc9372272fbf65651298bfb12bdedb58ddda3de5b26c2"}},{"slot":"7689477","validator_index":"258637","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1693935108","pubkey":"0x944bad9bc9ad0fa7b287b1e85e3bf0a5e246c871f2ce62c974d46b2968f853bdc06c22933e2be0549967343d4b01310b"},"signature":"0x906badf5ea9b63a210e7e5baa2f9b4871c9176a26b55282b148fb6eb3f433a41cabe61be612b02c1d6c071f13a374ee118b1fe71d0461c12a9595e0ed458123b0a1bbfef389aec8954803af60eca8ae982f767aa2f7f4c051f38ef630eaef8bf"}},{"slot":"7689479","validator_index":"641748","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1685364312","pubkey":"0xaf548fad3364238b9a909dc56735249b91e0fd5e330f65aa9072fe7f73024b4d8febc7cc2bd401ad8ace9a1043440b22"},"signature":"0xb36cb46aeb188463e9fec2f89b6dcb2e52489af7c852915011ff625fb24d82ded781ae864ccbd354cbbed1f02037a67a152fecc2735b598ab31c4620e7151dd20eb761c620c49dcb31704f43b631979fc545be4292bc0f193a1919255db7a5b8"}},{"slot":"7689481","validator_index":"779837","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1687790504","pubkey":"0xa39f287b3195ecaeb1c18c4f957d881a86a31f809078fe1d7525acfa751b7b0c43f369813d63d72fdd35d0e58f73bea9"},"signature":"0xb0bd69f16df95d09f02768c0625eb1d99dd6385a7db8aa72606a4b0b05a36add5671b02c554914e087f302bf9e17698f0b551a5b517ebdad68118f672bf80ea8de118d9e06808a39cf107bbc13a0cdfbfd0d5e1daf39ad4d66364a0047609dea"}},{"slot":"7689484","validator_index":"903816","entry":{"message":{"fee_recipient":"0xebec795c9c8bbd61ffc14a6662944748f299cacf","gas_limit":"30000000","timestamp":"1695066839","pubkey":"0xa1401efd4b681f3123da1c5de0f786e8c7879ceebc399227db994debf416e198ec25afecd1ee443808affd93143d183e"},"signature":"0x90234ccb98ca78ba35ae5925af7eb985c3cf6fd5f94291f881f315cf1072ab45a7dd812af52d8aede2f08d8179a5a7eb02b2b01fc8a2a792ef7077010df9f444866a03b8ec4798834dc9af8ff55fcd52f399b41d9dd9b0959d456f24aa38ac3c"}},{"slot":"7689485","validator_index":"364451","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1678669663","pubkey":"0x879658976e272aafab49be7105b27f9dea07e79f386dc4a165e17b082059c4b5628d8677203cd809a9332580d9cc28fe"},"signature":"0x8a3013425fd933630521a88a96dcc51e82f8f23cc5c243102415f7782799d39a3450bc5dc6b8a59331f78199e7729896186e700b166841195057ed6efbbd36a5a352cbe6a69ecbec27d74f9b2336f290e19940623a9712b70b59724f879f4e77"}},{"slot":"7689487","validator_index":"641598","entry":{"message":{"fee_recipient":"0x2370d6d6a4e6de417393d54abb144e740f662e01","gas_limit":"30000000","timestamp":"1683216978","pubkey":"0xa226a5c9121aee6a0e20759d877386bb50cb07de978eb89cb0a09dd9d7159117e4d610b3b80233c48bd84a1b9df5f1b2"},"signature":"0x983a0e5721dc39ae3bc35a83999f964ff7840e25e1033f033d51f40408acd07b4f8bda2bbd27f9fe793dd26e2dfe150c03577d0e2ff16d88cef0fb3bb849602d7287aac89199a4b39b1903a8dd9cd9e206ff68c391732fc6e6ef5ff2c89cb439"}},{"slot":"7689490","validator_index":"954682","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1696695974","pubkey":"0xadbe2aeecfc01016938dc0a90986d36975acdd1e3cbb3461bb917a7eaf260c1a387867e47f3d9a1dd56778f552a0ed6a"},"signature":"0x85dea1b1b01ecaf3240571ecddcfc4eaa06b4e23b1c2cc6db646164716f96b8ad46bf0687f5bb840a7468514ac18439205bfb16941cdafc6853c4c65271cd113be72f9428469d0815a7169c70ae37709a19ad669e709d6a9cfd90bc471309bc6"}},{"slot":"7689496","validator_index":"833362","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1690132607","pubkey":"0xa4e9ee836facfaf67dab10c59c10ed6d3a0f007799129f13b009be56ed87ad6b08b30b315f38b6cc38f2fdb747dac587"},"signature":"0xb73b40c9766004d6471b3355fc6ffa765a442c0687852687ed89120cdcebf03c37ed2c087fd254492b2b7f11c2446ec5116e40f442367196af967e6905ca5fb333a2b3a9705c0d302817038199b43c1dd36124fe6085d610c491176d1d5d0cff"}},{"slot":"7689498","validator_index":"202233","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1678891455","pubkey":"0x8cb0401c024bb74a481b9253ce61af57ad263e7ab542166922c13e46d76c2593b4735a85e5fbaba9d1cd63d8996981e1"},"signature":"0xb9364c714be2c11651b8c6854b0fc5872d8b109fa1c8b67e4e1bf71555a364e3003a88d98292a09487d20b3e89a0716c1030a559ce70aeef582fab3d6820fde678249d3952c809c53e56940cc74ba6fcc112bb94adf72d41451e5e69788f98da"}},{"slot":"7689500","validator_index":"380674","entry":{"message":{"fee_recipient":"0x388c818ca8b9251b393131c08a736a67ccb19297","gas_limit":"30000000","timestamp":"1693299181","pubkey":"0x9801236e1e78426d7b4be81f828419bd1aac3b2041ebed2af9683eca259e9915c6f5d45d9e3021a8218f8b6a73550ee4"},"signature":"0x98961cb2f920898e656ddaf66d11bcfd808179cf8f313687260eb760bd065f1f5ae79a37587575a1c6c46671845c63e409cc01bca97823adc0e6dbbc280327df886df4fb8aa7e1d310311bc80e29fed90a6ae3346017d1b5d20b32beed8fd477"}},{"slot":"7689502","validator_index":"486859","entry":{"message":{"fee_recipient":"0x0b91a6bafdae6ae32d864ed9a0e883a5ca9a02dd","gas_limit":"30000000","timestamp":"1680003847","pubkey":"0x99225f70bb2c9310835a367e95b34c0d6021b5ec9bf350f4d8f0fc4dce34c9c16f4299b788ad0d27001b0efd2712d494"},"signature":"0x86ab16d4b4e686b20d1bb9d532975961acd797e02d9c9e48d13805ec2ba71df9e69a63c3b254b8e640fcc26f651ad243155430095caa6c5770b52039f1d6a9312e0d8f9dd2fb4fe2d35d372075a93b14e745be91e7eb1f28f0f5bf2c62f7584e"}},{"slot":"7689503","validator_index":"70348","entry":{"message":{"fee_recipient":"0x6d2e03b7effeae98bd302a9f836d0d6ab0002766","gas_limit":"30000000","timestamp":"1680011532","pubkey":"0x820dd8b5396377da3f2d4972d4c94fbad401bbf4b3a56e570a532f77c1802f2cc310bf969bb6aa96d90ea2708c562ed6"},"signature":"0xb042608d02c4ca053c6b9e22804af99421c47eda96ce585d1df6f37cbf59cfd6830a3592f6de543232135c42bb3da9cd13ecf5aea2e3b802114dc08877e9023a7cf6d75e28ca30d1df3f3c98bddd36b4f521e63895179a7e8c3752f5cbc681ea"}}]"#;

        let validators: Vec<Validator> = serde_json::from_str(s).unwrap();
        let json: serde_json::Value = serde_json::from_str(s).unwrap();
        assert_eq!(json, serde_json::to_value(validators).unwrap());
    }

    #[test]
    fn bellatrix_bid_submission() {
        let s = r#"{"message":{"slot":"1","parent_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","block_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","builder_pubkey":"0x93247f2209abcacf57b75a51dafae777f9dd38bc7053d1af526f220a7489a6d3a2753e5f3e8b1cfe39b56f43611df74a", "proposer_pubkey": "0x93247f2209abcacf57b75a51dafae777f9dd38bc7053d1af526f220a7489a6d3a2753e5f3e8b1cfe39b56f43611df74a","proposer_fee_recipient":"0xabcf8e0d4e9587369b2301d0790347320302cc09","gas_limit":"1","gas_used":"1","value":"1"},"execution_payload":{"parent_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","fee_recipient":"0xabcf8e0d4e9587369b2301d0790347320302cc09","state_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","receipts_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","logs_bloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prev_randao":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","block_number":"1","gas_limit":"1","gas_used":"1","timestamp":"1","extra_data":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","base_fee_per_gas":"1","block_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","transactions":["0x02f878831469668303f51d843b9ac9f9843b9aca0082520894c93269b73096998db66be0441e836d873535cb9c8894a19041886f000080c001a031cc29234036afbf9a1fb9476b463367cb1f957ac0b919b69bbc798436e604aaa018c4e9c3914eb27aadd0b91e10b18655739fcf8c1fc398763a9f1beecb8ddc86"]},"signature":"0x1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505cc411d61252fb6cb3fa0017b679f8bb2305b26a285fa2737f175668d0dff91cc1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505"}"#;

        let bid = serde_json::from_str::<SignedBidSubmissionV1>(s).unwrap();
        let json: serde_json::Value = serde_json::from_str(s).unwrap();
        assert_eq!(json, serde_json::to_value(bid).unwrap());
    }

    #[test]
    fn capella_bid_submission() {
        let s = r#"{"message":{"slot":"1","parent_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","block_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","builder_pubkey":"0x93247f2209abcacf57b75a51dafae777f9dd38bc7053d1af526f220a7489a6d3a2753e5f3e8b1cfe39b56f43611df74a", "proposer_pubkey": "0x93247f2209abcacf57b75a51dafae777f9dd38bc7053d1af526f220a7489a6d3a2753e5f3e8b1cfe39b56f43611df74a","proposer_fee_recipient":"0xabcf8e0d4e9587369b2301d0790347320302cc09","gas_limit":"1","gas_used":"1","value":"1"},"execution_payload":{"parent_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","fee_recipient":"0xabcf8e0d4e9587369b2301d0790347320302cc09","state_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","receipts_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","logs_bloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prev_randao":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","block_number":"1","gas_limit":"1","gas_used":"1","timestamp":"1","extra_data":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","base_fee_per_gas":"1","block_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","transactions":["0x02f878831469668303f51d843b9ac9f9843b9aca0082520894c93269b73096998db66be0441e836d873535cb9c8894a19041886f000080c001a031cc29234036afbf9a1fb9476b463367cb1f957ac0b919b69bbc798436e604aaa018c4e9c3914eb27aadd0b91e10b18655739fcf8c1fc398763a9f1beecb8ddc86"],"withdrawals":[{"index":"1","validator_index":"1","address":"0xabcf8e0d4e9587369b2301d0790347320302cc09","amount":"32000000000"}]},"signature":"0x1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505cc411d61252fb6cb3fa0017b679f8bb2305b26a285fa2737f175668d0dff91cc1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505"}"#;

        let bid = serde_json::from_str::<SignedBidSubmissionV2>(s).unwrap();
        let json: serde_json::Value = serde_json::from_str(s).unwrap();
        assert_eq!(json, serde_json::to_value(bid).unwrap());
    }

    #[test]
    fn deneb_bid_submission() {
        let s = r#"{"message":{"slot":"1","parent_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","block_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","builder_pubkey":"0x93247f2209abcacf57b75a51dafae777f9dd38bc7053d1af526f220a7489a6d3a2753e5f3e8b1cfe39b56f43611df74a", "proposer_pubkey": "0x93247f2209abcacf57b75a51dafae777f9dd38bc7053d1af526f220a7489a6d3a2753e5f3e8b1cfe39b56f43611df74a","proposer_fee_recipient":"0xabcf8e0d4e9587369b2301d0790347320302cc09","gas_limit":"1","gas_used":"1","value":"1"},"execution_payload":{"parent_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","fee_recipient":"0xabcf8e0d4e9587369b2301d0790347320302cc09","state_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","receipts_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","logs_bloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prev_randao":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","block_number":"1","gas_limit":"1","gas_used":"1","timestamp":"1","extra_data":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","base_fee_per_gas":"1","block_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","transactions":["0x02f878831469668303f51d843b9ac9f9843b9aca0082520894c93269b73096998db66be0441e836d873535cb9c8894a19041886f000080c001a031cc29234036afbf9a1fb9476b463367cb1f957ac0b919b69bbc798436e604aaa018c4e9c3914eb27aadd0b91e10b18655739fcf8c1fc398763a9f1beecb8ddc86"],"withdrawals":[{"index":"1","validator_index":"1","address":"0xabcf8e0d4e9587369b2301d0790347320302cc09","amount":"32000000000"}], "blob_gas_used":"1","excess_blob_gas":"1"},"blobs_bundle":{"commitments":[],"proofs":[],"blobs":[]},"signature":"0x1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505cc411d61252fb6cb3fa0017b679f8bb2305b26a285fa2737f175668d0dff91cc1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505"}"#;

        let bid = serde_json::from_str::<SignedBidSubmissionV3>(s).unwrap();
        let json: serde_json::Value = serde_json::from_str(s).unwrap();
        assert_eq!(json, serde_json::to_value(bid).unwrap());
    }

    #[cfg(feature = "ssz")]
    #[test]
    fn capella_bid_submission_ssz() {
        use ssz::{Decode, Encode};

        let bytes =
            include_bytes!("../../test_data/relay/signed_bid_submission_capella.ssz").to_vec();
        let bid = SignedBidSubmissionV2::from_ssz_bytes(&bytes).unwrap();
        assert_eq!(bytes, bid.as_ssz_bytes());
    }

    #[test]
    fn test_can_parse_validation_request_body() {
        const VALIDATION_REQUEST_BODY: &str =
            include_str!("../../test_data/relay/single_payload.json");

        let _validation_request_body: BuilderBlockValidationRequest =
            serde_json::from_str(VALIDATION_REQUEST_BODY).unwrap();
    }
}
