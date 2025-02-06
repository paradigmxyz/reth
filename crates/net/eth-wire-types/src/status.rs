use crate::EthVersion;
use alloy_chains::{Chain, NamedChain};
use alloy_primitives::{hex, B256, U256};
use alloy_rlp::{
    BufMut, Decodable, Encodable, Error as RlpError, Header, RlpDecodable, RlpEncodable,
};
use core::fmt::{Debug, Display};
use reth_chainspec::{EthChainSpec, Hardforks, MAINNET};
use reth_codecs_derive::add_arbitrary_tests;
use reth_ethereum_forks::{EthereumHardfork, ForkId, Head};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub enum Status {
    Eth66_68(Status66_68),
}

impl Status {
    /// Helper for returning a builder for the status message.
    pub fn builder() -> StatusBuilder {
        Default::default()
    }

    /// Sets the [`EthVersion`] for the status.
    pub const fn set_eth_version(&mut self, version: EthVersion) {
        use EthVersion::*;
        match (self, version) {
            (Status::Eth66_68(status), Eth66 | Eth67 | Eth68) => status.version = version,
            _ => {}
        }
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

    pub fn version(&self) -> EthVersion {
        match self {
            Self::Eth66_68(status) => status.version,
        }
    }

    pub fn chain(&self) -> &Chain {
        match self {
            Self::Eth66_68(status) => &status.chain,
        }
    }

    pub fn total_difficulty(&self) -> Option<&U256> {
        match self {
            Self::Eth66_68(status) => Some(&status.total_difficulty),
        }
    }

    pub fn total_difficulty_mut(&mut self) -> Option<&mut U256> {
        match self {
            Self::Eth66_68(status) => Some(&mut status.total_difficulty),
        }
    }

    pub fn blockhash(&self) -> &B256 {
        match self {
            Self::Eth66_68(status) => &status.blockhash,
        }
    }

    pub fn blockhash_mut(&mut self) -> &mut B256 {
        match self {
            Self::Eth66_68(status) => &mut status.blockhash,
        }
    }

    pub fn genesis(&self) -> &B256 {
        match self {
            Self::Eth66_68(status) => &status.genesis,
        }
    }

    pub fn forkid(&self) -> &ForkId {
        match self {
            Self::Eth66_68(status) => &status.forkid,
        }
    }

    pub fn forkid_mut(&mut self) -> &mut ForkId {
        match self {
            Self::Eth66_68(status) => &mut status.forkid,
        }
    }
}

// <https://etherscan.io/block/0>
impl Default for Status {
    fn default() -> Self {
        Self::Eth66_68(Status66_68::default())
    }
}

impl Display for Status {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Status::Eth66_68(status) => Display::fmt(&status, f),
        }
    }
}

impl Encodable for Status {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            Self::Eth66_68(status) => status.encode(out),
        }
    }
}

impl Decodable for Status {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let b = &mut &**buf;
        let rlp_head = Header::decode(b)?;
        if !rlp_head.list {
            return Err(RlpError::UnexpectedString);
        }
        let version = EthVersion::decode(b)?;

        use EthVersion::*;
        match version {
            Eth66 | Eth67 | Eth68 | Eth69 => Ok(Status::Eth66_68(Status66_68::decode(buf)?)),
        }
    }
}

/// The status message is used in the eth protocol handshake to ensure that peers are on the same
/// network and are following the same fork.
///
/// When performing a handshake, the total difficulty is not guaranteed to correspond to the block
/// hash. This information should be treated as untrusted.
#[derive(Copy, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct Status66_68 {
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

// <https://etherscan.io/block/0>
impl Default for Status66_68 {
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

impl Display for Status66_68 {
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

impl Debug for Status66_68 {
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

impl Into<Status> for Status66_68 {
    fn into(self) -> Status {
        Status::Eth66_68(self)
    }
}

/// Builder for [`Status`] messages.
///
/// # Example
/// ```
/// use alloy_consensus::constants::MAINNET_GENESIS_HASH;
/// use alloy_primitives::{B256, U256};
/// use reth_chainspec::{Chain, EthereumHardfork, MAINNET};
/// use reth_eth_wire_types::{EthVersion, Status, Status66_68};
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
///     Status66_68 {
///         version: EthVersion::Eth66,
///         chain: Chain::mainnet(),
///         total_difficulty: U256::from(100),
///         blockhash: B256::from(MAINNET_GENESIS_HASH),
///         genesis: B256::from(MAINNET_GENESIS_HASH),
///         forkid: MAINNET.hardfork_fork_id(EthereumHardfork::Paris).unwrap(),
///     }.into()
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
        self.status.set_eth_version(version);
        self
    }

    /// Sets the chain id.
    pub const fn chain(mut self, chain: Chain) -> Self {
        match self.status {
            Status::Eth66_68(ref mut status) => {
                status.chain = chain;
            }
        }
        self
    }

    /// Sets the total difficulty.
    pub const fn total_difficulty(mut self, total_difficulty: U256) -> Self {
        match self.status {
            Status::Eth66_68(ref mut status) => status.total_difficulty = total_difficulty,
        }
        self
    }

    /// Sets the block hash.
    pub const fn blockhash(mut self, blockhash: B256) -> Self {
        match self.status {
            Status::Eth66_68(ref mut status) => status.blockhash = blockhash,
        }
        self
    }

    /// Sets the genesis hash.
    pub const fn genesis(mut self, genesis: B256) -> Self {
        match self.status {
            Status::Eth66_68(ref mut status) => status.genesis = genesis,
        }
        self
    }

    /// Sets the fork id.
    pub const fn forkid(mut self, forkid: ForkId) -> Self {
        match self.status {
            Status::Eth66_68(ref mut status) => status.forkid = forkid,
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use crate::{status::Status66_68, EthVersion, Status};
    use alloy_consensus::constants::MAINNET_GENESIS_HASH;
    use alloy_genesis::Genesis;
    use alloy_primitives::{hex, B256, U256};
    use alloy_rlp::{Decodable, Encodable};
    use rand::Rng;
    use reth_chainspec::{Chain, ChainSpec, ForkCondition, NamedChain};
    use reth_ethereum_forks::{EthereumHardfork, ForkHash, ForkId, Head};
    use std::str::FromStr;

    #[test]
    fn encode_eth_status_message() {
        let expected = hex!("f85643018a07aac59dabcdd74bc567a0feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13da0d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3c684b715077d80");
        let status: Status = Status66_68 {
            version: EthVersion::Eth67,
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

        let mut rlp_status = vec![];
        status.encode(&mut rlp_status);
        assert_eq!(rlp_status, expected);
    }

    #[test]
    fn decode_eth_status_message() {
        let data = hex!("f85643018a07aac59dabcdd74bc567a0feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13da0d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3c684b715077d80");
        let expected = Status66_68 {
            version: EthVersion::Eth67,
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
        let status = Status::decode(&mut &data[..]).unwrap();
        assert_eq!(status, expected);
    }

    #[test]
    fn encode_network_status_message() {
        let expected = hex!("f850423884024190faa0f8514c4680ef27700751b08f37645309ce65a449616a3ea966bf39dd935bb27ba00d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5bc6845d43d2fd80");
        let status: Status = Status66_68 {
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
        }
        .into();

        let mut rlp_status = vec![];
        status.encode(&mut rlp_status);
        assert_eq!(rlp_status, expected);
    }

    #[test]
    fn decode_network_status_message() {
        let data = hex!("f850423884024190faa0f8514c4680ef27700751b08f37645309ce65a449616a3ea966bf39dd935bb27ba00d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5bc6845d43d2fd80");
        let expected = Status66_68 {
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
        }
        .into();
        let status = Status::decode(&mut &data[..]).unwrap();
        assert_eq!(status, expected);
    }

    #[test]
    fn decode_another_network_status_message() {
        let data = hex!("f86142820834936d68fcffffffffffffffffffffffffdeab81b8a0523e8163a6d620a4cc152c547a05f28a03fec91a2a615194cb86df9731372c0ca06499dccdc7c7def3ebb1ce4c6ee27ec6bd02aee570625ca391919faf77ef27bdc6841a67ccd880");
        let expected = Status66_68 {
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
        }
        .into();
        let status = Status::decode(&mut &data[..]).unwrap();
        assert_eq!(status, expected);
    }

    #[test]
    fn init_custom_status_fields() {
        let mut rng = rand::thread_rng();
        let head_hash = rng.gen();
        let total_difficulty = U256::from(rng.gen::<u64>());

        // create a genesis that has a random part, so we can check that the hash is preserved
        let genesis = Genesis { nonce: rng.gen(), ..Default::default() };

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

        assert_eq!(status.chain(), &Chain::from_id(1337));
        assert_eq!(status.forkid(), &forkid);
        assert_eq!(status.total_difficulty(), Some(&total_difficulty));
        assert_eq!(status.blockhash(), &head_hash);
        assert_eq!(status.genesis(), &genesis_hash);
    }
}
