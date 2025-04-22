use crate::EthVersion;
use alloy_chains::{Chain, NamedChain};
use alloy_hardforks::{EthereumHardfork, ForkId, Head};
use alloy_primitives::{hex, B256, U256};
use alloy_rlp::{BufMut, Encodable, RlpDecodable, RlpEncodable};
use core::fmt::{Debug, Display};
use reth_chainspec::{EthChainSpec, Hardforks, MAINNET};
use reth_codecs_derive::add_arbitrary_tests;

/// The status message is used in the eth protocol handshake to ensure that peers are on the same
/// network and are following the same fork.
///
/// When performing a handshake, the total difficulty is not guaranteed to correspond to the block
/// hash. This information should be treated as untrusted.
#[derive(Copy, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct Status {
    /// The current protocol version. For example, peers running `eth/66` would have a version of
    /// 66.
    pub version: EthVersion,

    /// The chain id, as introduced in
    /// [EIP155](https://eips.ethereum.org/EIPS/eip-155#list-of-chain-ids).
    pub chain: Chain,

    /// Total difficulty of the best chain.
    pub total_difficulty: U256,

    /// The highest difficulty block hash the peer has seen
    pub blockhash: B256,

    /// The genesis hash of the peer's chain.
    pub genesis: B256,

    /// The fork identifier, a [CRC32
    /// checksum](https://en.wikipedia.org/wiki/Cyclic_redundancy_check#CRC-32_algorithm) for
    /// identifying the peer's fork as defined by
    /// [EIP-2124](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-2124.md).
    /// This was added in [`eth/64`](https://eips.ethereum.org/EIPS/eip-2364)
    pub forkid: ForkId,
}

impl Status {
    /// Helper for returning a builder for the status message.
    pub fn builder() -> StatusBuilder {
        Default::default()
    }

    /// Sets the [`EthVersion`] for the status.
    pub const fn set_eth_version(&mut self, version: EthVersion) {
        self.version = version;
    }

    /// Create a [`StatusBuilder`] from the given [`EthChainSpec`] and head block.
    ///
    /// Sets the `chain` and `genesis`, `blockhash`, and `forkid` fields based on the
    /// [`EthChainSpec`] and head.
    pub fn spec_builder<Spec>(spec: Spec, head: &Head) -> StatusBuilder
    where
        Spec: EthChainSpec + Hardforks,
    {
        Self::builder()
            .chain(spec.chain())
            .genesis(spec.genesis_hash())
            .blockhash(head.hash)
            .total_difficulty(head.total_difficulty)
            .forkid(spec.fork_id(head))
    }

    /// Converts this [`Status`] into the [Eth69](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-7642.md) variant that excludes the total difficulty field.
    pub const fn into_eth69(self) -> StatusEth69 {
        StatusEth69 {
            version: EthVersion::Eth69,
            chain: self.chain,
            blockhash: self.blockhash,
            genesis: self.genesis,
            forkid: self.forkid,
        }
    }
}

impl Display for Status {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let hexed_blockhash = hex::encode(self.blockhash);
        let hexed_genesis = hex::encode(self.genesis);
        write!(
            f,
            "Status {{ version: {}, chain: {}, total_difficulty: {}, blockhash: {}, genesis: {}, forkid: {:X?} }}",
            self.version,
            self.chain,
            self.total_difficulty,
            hexed_blockhash,
            hexed_genesis,
            self.forkid
        )
    }
}

impl Debug for Status {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let hexed_blockhash = hex::encode(self.blockhash);
        let hexed_genesis = hex::encode(self.genesis);
        if f.alternate() {
            write!(
                f,
                "Status {{\n\tversion: {:?},\n\tchain: {:?},\n\ttotal_difficulty: {:?},\n\tblockhash: {},\n\tgenesis: {},\n\tforkid: {:X?}\n}}",
                self.version,
                self.chain,
                self.total_difficulty,
                hexed_blockhash,
                hexed_genesis,
                self.forkid
            )
        } else {
            write!(
                f,
                "Status {{ version: {:?}, chain: {:?}, total_difficulty: {:?}, blockhash: {}, genesis: {}, forkid: {:X?} }}",
                self.version,
                self.chain,
                self.total_difficulty,
                hexed_blockhash,
                hexed_genesis,
                self.forkid
            )
        }
    }
}

// <https://etherscan.io/block/0>
impl Default for Status {
    fn default() -> Self {
        let mainnet_genesis = MAINNET.genesis_hash();
        Self {
            version: EthVersion::Eth68,
            chain: Chain::from_named(NamedChain::Mainnet),
            total_difficulty: U256::from(17_179_869_184u64),
            blockhash: mainnet_genesis,
            genesis: mainnet_genesis,
            forkid: MAINNET
                .hardfork_fork_id(EthereumHardfork::Frontier)
                .expect("The Frontier hardfork should always exist"),
        }
    }
}

/// Builder for [`Status`] messages.
///
/// # Example
/// ```
/// use alloy_consensus::constants::MAINNET_GENESIS_HASH;
/// use alloy_primitives::{B256, U256};
/// use reth_chainspec::{Chain, EthereumHardfork, MAINNET};
/// use reth_eth_wire_types::{EthVersion, Status};
///
/// // this is just an example status message!
/// let status = Status::builder()
///     .version(EthVersion::Eth66)
///     .chain(Chain::mainnet())
///     .total_difficulty(U256::from(100))
///     .blockhash(B256::from(MAINNET_GENESIS_HASH))
///     .genesis(B256::from(MAINNET_GENESIS_HASH))
///     .forkid(MAINNET.hardfork_fork_id(EthereumHardfork::Paris).unwrap())
///     .build();
///
/// assert_eq!(
///     status,
///     Status {
///         version: EthVersion::Eth66,
///         chain: Chain::mainnet(),
///         total_difficulty: U256::from(100),
///         blockhash: B256::from(MAINNET_GENESIS_HASH),
///         genesis: B256::from(MAINNET_GENESIS_HASH),
///         forkid: MAINNET.hardfork_fork_id(EthereumHardfork::Paris).unwrap(),
///     }
/// );
/// ```
#[derive(Debug, Default)]
pub struct StatusBuilder {
    status: Status,
}

impl StatusBuilder {
    /// Consumes the type and creates the actual [`Status`] message.
    pub const fn build(self) -> Status {
        self.status
    }

    /// Sets the protocol version.
    pub const fn version(mut self, version: EthVersion) -> Self {
        self.status.version = version;
        self
    }

    /// Sets the chain id.
    pub const fn chain(mut self, chain: Chain) -> Self {
        self.status.chain = chain;
        self
    }

    /// Sets the total difficulty.
    pub const fn total_difficulty(mut self, total_difficulty: U256) -> Self {
        self.status.total_difficulty = total_difficulty;
        self
    }

    /// Sets the block hash.
    pub const fn blockhash(mut self, blockhash: B256) -> Self {
        self.status.blockhash = blockhash;
        self
    }

    /// Sets the genesis hash.
    pub const fn genesis(mut self, genesis: B256) -> Self {
        self.status.genesis = genesis;
        self
    }

    /// Sets the fork id.
    pub const fn forkid(mut self, forkid: ForkId) -> Self {
        self.status.forkid = forkid;
        self
    }
}

/// Similar to [`Status`], but for `eth/69` version, which does not contain
/// the `total_difficulty` field.
#[derive(Copy, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct StatusEth69 {
    /// The current protocol version.
    /// Here, version is `eth/69`.
    pub version: EthVersion,

    /// The chain id, as introduced in
    /// [EIP155](https://eips.ethereum.org/EIPS/eip-155#list-of-chain-ids).
    pub chain: Chain,

    /// The highest difficulty block hash the peer has seen
    pub blockhash: B256,

    /// The genesis hash of the peer's chain.
    pub genesis: B256,

    /// The fork identifier, a [CRC32
    /// checksum](https://en.wikipedia.org/wiki/Cyclic_redundancy_check#CRC-32_algorithm) for
    /// identifying the peer's fork as defined by
    /// [EIP-2124](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-2124.md).
    /// This was added in [`eth/64`](https://eips.ethereum.org/EIPS/eip-2364)
    pub forkid: ForkId,
}

impl Display for StatusEth69 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let hexed_blockhash = hex::encode(self.blockhash);
        let hexed_genesis = hex::encode(self.genesis);
        write!(
            f,
            "Status {{ version: {}, chain: {}, blockhash: {}, genesis: {}, forkid: {:X?} }}",
            self.version, self.chain, hexed_blockhash, hexed_genesis, self.forkid
        )
    }
}

impl Debug for StatusEth69 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let hexed_blockhash = hex::encode(self.blockhash);
        let hexed_genesis = hex::encode(self.genesis);
        if f.alternate() {
            write!(
                f,
                "Status {{\n\tversion: {:?},\n\tchain: {:?},\n\tblockhash: {},\n\tgenesis: {},\n\tforkid: {:X?}\n}}",
                self.version,
                self.chain,
                hexed_blockhash,
                hexed_genesis,
                self.forkid
            )
        } else {
            write!(
                f,
                "Status {{ version: {:?}, chain: {:?}, blockhash: {}, genesis: {}, forkid: {:X?} }}",
                self.version,
                self.chain,
                hexed_blockhash,
                hexed_genesis,
                self.forkid
            )
        }
    }
}

// <https://etherscan.io/block/0>
impl Default for StatusEth69 {
    fn default() -> Self {
        Status::default().into()
    }
}

impl From<Status> for StatusEth69 {
    fn from(status: Status) -> Self {
        status.into_eth69()
    }
}

/// `StatusMessage` can store either the Legacy version (with TD) or the
/// eth/69 version (omits TD).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusMessage {
    /// The legacy status (`eth/66` through `eth/68`) with `total_difficulty`.
    Legacy(Status),
    /// The new `eth/69` status with no `total_difficulty`.
    Eth69(StatusEth69),
}

impl StatusMessage {
    /// Returns the genesis hash from the status message.
    pub const fn genesis(&self) -> B256 {
        match self {
            Self::Legacy(legacy_status) => legacy_status.genesis,
            Self::Eth69(status_69) => status_69.genesis,
        }
    }

    /// Returns the protocol version.
    pub const fn version(&self) -> EthVersion {
        match self {
            Self::Legacy(legacy_status) => legacy_status.version,
            Self::Eth69(status_69) => status_69.version,
        }
    }

    /// Returns the chain identifier.
    pub const fn chain(&self) -> &Chain {
        match self {
            Self::Legacy(legacy_status) => &legacy_status.chain,
            Self::Eth69(status_69) => &status_69.chain,
        }
    }

    /// Returns the fork identifier.
    pub const fn forkid(&self) -> ForkId {
        match self {
            Self::Legacy(legacy_status) => legacy_status.forkid,
            Self::Eth69(status_69) => status_69.forkid,
        }
    }

    /// Converts to legacy Status since full support for EIP-7642
    /// is not fully implemented
    /// `<https://github.com/ethereum/EIPs/blob/master/EIPS/eip-7642.md>`
    pub fn to_legacy(self) -> Status {
        match self {
            Self::Legacy(legacy_status) => legacy_status,
            Self::Eth69(status_69) => Status {
                version: status_69.version,
                chain: status_69.chain,
                // total_difficulty is omitted in Eth69.
                total_difficulty: U256::default(),
                blockhash: status_69.blockhash,
                genesis: status_69.genesis,
                forkid: status_69.forkid,
            },
        }
    }
}

impl Encodable for StatusMessage {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(s) => s.encode(out),
            Self::Eth69(s) => s.encode(out),
        }
    }

    fn length(&self) -> usize {
        match self {
            Self::Legacy(s) => s.length(),
            Self::Eth69(s) => s.length(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{EthVersion, Status, StatusEth69};
    use alloy_consensus::constants::MAINNET_GENESIS_HASH;
    use alloy_genesis::Genesis;
    use alloy_hardforks::{EthereumHardfork, ForkHash, ForkId, Head};
    use alloy_primitives::{hex, B256, U256};
    use alloy_rlp::{Decodable, Encodable};
    use rand::Rng;
    use reth_chainspec::{Chain, ChainSpec, ForkCondition, NamedChain};
    use std::str::FromStr;

    #[test]
    fn encode_eth_status_message() {
        let expected = hex!("f85643018a07aac59dabcdd74bc567a0feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13da0d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3c684b715077d80");
        let status = Status {
            version: EthVersion::Eth67,
            chain: Chain::from_named(NamedChain::Mainnet),
            total_difficulty: U256::from(36206751599115524359527u128),
            blockhash: B256::from_str(
                "feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d",
            )
            .unwrap(),
            genesis: MAINNET_GENESIS_HASH,
            forkid: ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 },
        };

        let mut rlp_status = vec![];
        status.encode(&mut rlp_status);
        assert_eq!(rlp_status, expected);
    }

    #[test]
    fn decode_eth_status_message() {
        let data = hex!("f85643018a07aac59dabcdd74bc567a0feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13da0d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3c684b715077d80");
        let expected = Status {
            version: EthVersion::Eth67,
            chain: Chain::from_named(NamedChain::Mainnet),
            total_difficulty: U256::from(36206751599115524359527u128),
            blockhash: B256::from_str(
                "feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d",
            )
            .unwrap(),
            genesis: MAINNET_GENESIS_HASH,
            forkid: ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 },
        };
        let status = Status::decode(&mut &data[..]).unwrap();
        assert_eq!(status, expected);
    }

    #[test]
    fn test_status_to_statuseth69_conversion() {
        let status = StatusEth69 {
            version: EthVersion::Eth69,
            chain: Chain::from_named(NamedChain::Mainnet),
            blockhash: B256::from_str(
                "feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d",
            )
            .unwrap(),
            genesis: MAINNET_GENESIS_HASH,
            forkid: ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 },
        };
        let status_converted: StatusEth69 = Status {
            version: EthVersion::Eth69,
            chain: Chain::from_named(NamedChain::Mainnet),
            total_difficulty: U256::from(36206751599115524359527u128),
            blockhash: B256::from_str(
                "feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d",
            )
            .unwrap(),
            genesis: MAINNET_GENESIS_HASH,
            forkid: ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 },
        }
        .into();
        assert_eq!(status, status_converted);
    }

    #[test]
    fn encode_eth69_status_message() {
        let expected = hex!("f84b4501a0feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13da0d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3c684b715077d80");
        let status = StatusEth69 {
            version: EthVersion::Eth69,
            chain: Chain::from_named(NamedChain::Mainnet),
            blockhash: B256::from_str(
                "feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d",
            )
            .unwrap(),
            genesis: MAINNET_GENESIS_HASH,
            forkid: ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 },
        };

        let mut rlp_status = vec![];
        status.encode(&mut rlp_status);
        assert_eq!(rlp_status, expected);

        let status: StatusEth69 = Status::builder()
            .chain(Chain::from_named(NamedChain::Mainnet))
            .blockhash(
                B256::from_str("feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d")
                    .unwrap(),
            )
            .genesis(MAINNET_GENESIS_HASH)
            .forkid(ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 })
            .build()
            .into();
        let mut rlp_status = vec![];
        status.encode(&mut rlp_status);
        assert_eq!(rlp_status, expected);
    }

    #[test]
    fn decode_eth69_status_message() {
        let data = hex!("0xf84b4501a0feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13da0d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3c684b715077d80");
        let expected = StatusEth69 {
            version: EthVersion::Eth69,
            chain: Chain::from_named(NamedChain::Mainnet),
            blockhash: B256::from_str(
                "feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d",
            )
            .unwrap(),
            genesis: MAINNET_GENESIS_HASH,
            forkid: ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 },
        };
        let status = StatusEth69::decode(&mut &data[..]).unwrap();
        assert_eq!(status, expected);
    }

    #[test]
    fn encode_network_status_message() {
        let expected = hex!("f850423884024190faa0f8514c4680ef27700751b08f37645309ce65a449616a3ea966bf39dd935bb27ba00d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5bc6845d43d2fd80");
        let status = Status {
            version: EthVersion::Eth66,
            chain: Chain::from_named(NamedChain::BinanceSmartChain),
            total_difficulty: U256::from(37851386u64),
            blockhash: B256::from_str(
                "f8514c4680ef27700751b08f37645309ce65a449616a3ea966bf39dd935bb27b",
            )
            .unwrap(),
            genesis: B256::from_str(
                "0d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5b",
            )
            .unwrap(),
            forkid: ForkId { hash: ForkHash([0x5d, 0x43, 0xd2, 0xfd]), next: 0 },
        };

        let mut rlp_status = vec![];
        status.encode(&mut rlp_status);
        assert_eq!(rlp_status, expected);
    }

    #[test]
    fn decode_network_status_message() {
        let data = hex!("f850423884024190faa0f8514c4680ef27700751b08f37645309ce65a449616a3ea966bf39dd935bb27ba00d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5bc6845d43d2fd80");
        let expected = Status {
            version: EthVersion::Eth66,
            chain: Chain::from_named(NamedChain::BinanceSmartChain),
            total_difficulty: U256::from(37851386u64),
            blockhash: B256::from_str(
                "f8514c4680ef27700751b08f37645309ce65a449616a3ea966bf39dd935bb27b",
            )
            .unwrap(),
            genesis: B256::from_str(
                "0d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5b",
            )
            .unwrap(),
            forkid: ForkId { hash: ForkHash([0x5d, 0x43, 0xd2, 0xfd]), next: 0 },
        };
        let status = Status::decode(&mut &data[..]).unwrap();
        assert_eq!(status, expected);
    }

    #[test]
    fn decode_another_network_status_message() {
        let data = hex!("f86142820834936d68fcffffffffffffffffffffffffdeab81b8a0523e8163a6d620a4cc152c547a05f28a03fec91a2a615194cb86df9731372c0ca06499dccdc7c7def3ebb1ce4c6ee27ec6bd02aee570625ca391919faf77ef27bdc6841a67ccd880");
        let expected = Status {
            version: EthVersion::Eth66,
            chain: Chain::from_id(2100),
            total_difficulty: U256::from_str(
                "0x000000000000000000000000006d68fcffffffffffffffffffffffffdeab81b8",
            )
            .unwrap(),
            blockhash: B256::from_str(
                "523e8163a6d620a4cc152c547a05f28a03fec91a2a615194cb86df9731372c0c",
            )
            .unwrap(),
            genesis: B256::from_str(
                "6499dccdc7c7def3ebb1ce4c6ee27ec6bd02aee570625ca391919faf77ef27bd",
            )
            .unwrap(),
            forkid: ForkId { hash: ForkHash([0x1a, 0x67, 0xcc, 0xd8]), next: 0 },
        };
        let status = Status::decode(&mut &data[..]).unwrap();
        assert_eq!(status, expected);
    }

    #[test]
    fn init_custom_status_fields() {
        let mut rng = rand::rng();
        let head_hash = rng.random();
        let total_difficulty = U256::from(rng.random::<u64>());

        // create a genesis that has a random part, so we can check that the hash is preserved
        let genesis = Genesis { nonce: rng.random(), ..Default::default() };

        // build head
        let head = Head {
            number: u64::MAX,
            hash: head_hash,
            difficulty: U256::from(13337),
            total_difficulty,
            timestamp: u64::MAX,
        };

        // add a few hardforks
        let hardforks = vec![
            (EthereumHardfork::Tangerine, ForkCondition::Block(1)),
            (EthereumHardfork::SpuriousDragon, ForkCondition::Block(2)),
            (EthereumHardfork::Byzantium, ForkCondition::Block(3)),
            (EthereumHardfork::MuirGlacier, ForkCondition::Block(5)),
            (EthereumHardfork::London, ForkCondition::Block(8)),
            (EthereumHardfork::Shanghai, ForkCondition::Timestamp(13)),
        ];

        let mut chainspec = ChainSpec::builder().genesis(genesis).chain(Chain::from_id(1337));

        for (fork, condition) in &hardforks {
            chainspec = chainspec.with_fork(*fork, *condition);
        }

        let spec = chainspec.build();

        // calculate proper forkid to check against
        let genesis_hash = spec.genesis_hash();
        let mut forkhash = ForkHash::from(genesis_hash);
        for (_, condition) in hardforks {
            forkhash += match condition {
                ForkCondition::Block(n) | ForkCondition::Timestamp(n) => n,
                _ => unreachable!("only block and timestamp forks are used in this test"),
            }
        }

        let forkid = ForkId { hash: forkhash, next: 0 };

        let status = Status::spec_builder(&spec, &head).build();

        assert_eq!(status.chain, Chain::from_id(1337));
        assert_eq!(status.forkid, forkid);
        assert_eq!(status.total_difficulty, total_difficulty);
        assert_eq!(status.blockhash, head_hash);
        assert_eq!(status.genesis, genesis_hash);
    }
}
