use crate::{BlockRange, EthVersion};
use alloy_chains::{Chain, NamedChain};
use alloy_hardforks::{EthereumHardfork, ForkId, Head};
use alloy_primitives::{hex, B256, U256};
use alloy_rlp::{BufMut, Encodable, RlpDecodable, RlpEncodable};
use core::fmt::{Debug, Display};
use reth_chainspec::{EthChainSpec, Hardforks, MAINNET};
use reth_codecs_derive::add_arbitrary_tests;

/// `UnifiedStatus` is an internal superset of all ETH status fields for all `eth/` versions.
///
/// This type can be converted into [`Status`], [`StatusEth69`], or [`StatusEth70`] depending on
/// the negotiated version and unsupported fields are stripped out.
#[derive(Clone, Debug, PartialEq, Eq, Copy)]
pub struct UnifiedStatus {
    /// The eth protocol version (e.g. eth/66 to eth/70).
    pub version: EthVersion,
    /// The chain ID identifying the peer’s network.
    pub chain: Chain,
    /// The genesis block hash of the peer’s chain.
    pub genesis: B256,
    /// The fork ID as defined by EIP-2124.
    pub forkid: ForkId,
    /// The latest block hash known to the peer.
    pub blockhash: B256,
    /// The total difficulty of the peer’s best chain (eth/66–68 only).
    pub total_difficulty: Option<U256>,
    /// The earliest block this node can serve (eth/69 only).
    pub earliest_block: Option<u64>,
    /// The latest block number this node has (eth/69 only).
    pub latest_block: Option<u64>,
    /// The block range this node can serve (eth/70).
    pub block_range: Option<BlockRange>,
}

impl Default for UnifiedStatus {
    fn default() -> Self {
        let mainnet_genesis = MAINNET.genesis_hash();
        Self {
            version: EthVersion::Eth68,
            chain: Chain::from_named(NamedChain::Mainnet),
            genesis: mainnet_genesis,
            forkid: MAINNET
                .hardfork_fork_id(EthereumHardfork::Frontier)
                .expect("Frontier must exist"),
            blockhash: mainnet_genesis,
            total_difficulty: Some(U256::from(17_179_869_184u64)),
            earliest_block: Some(0),
            latest_block: Some(0),
            block_range: None,
        }
    }
}

impl UnifiedStatus {
    /// Helper for creating the `UnifiedStatus` builder
    pub fn builder() -> StatusBuilder {
        Default::default()
    }

    /// Build from chain‑spec + head.  Earliest/latest default to full history.
    pub fn spec_builder<Spec>(spec: &Spec, head: &Head) -> Self
    where
        Spec: EthChainSpec + Hardforks,
    {
        Self::builder()
            .chain(spec.chain())
            .genesis(spec.genesis_hash())
            .forkid(spec.fork_id(head))
            .blockhash(head.hash)
            .total_difficulty(Some(head.total_difficulty))
            .earliest_block(Some(0))
            .latest_block(Some(head.number))
            .block_range(Some(BlockRange { start_block: 0, end_block: head.number }))
            .build()
    }

    /// Override the `(earliest, latest)` history range we’ll advertise to
    /// eth/69 peers.
    pub const fn set_history_range(&mut self, earliest: u64, latest: u64) {
        self.earliest_block = Some(earliest);
        self.latest_block = Some(latest);
        self.block_range = Some(BlockRange { start_block: earliest, end_block: latest });
    }

    /// Sets the [`EthVersion`] for the status.
    pub const fn set_eth_version(&mut self, v: EthVersion) {
        self.version = v;
    }

    /// Sets the block range for eth/70 peers.
    pub const fn set_block_range(&mut self, block_range: BlockRange) {
        self.block_range = Some(block_range);
        self.earliest_block = Some(block_range.start_block);
        self.latest_block = Some(block_range.end_block);
    }

    /// Consume this `UnifiedStatus` and produce the legacy [`Status`] message used by all
    /// `eth/66`–`eth/68`.
    pub fn into_legacy(self) -> Status {
        Status {
            version: self.version,
            chain: self.chain,
            genesis: self.genesis,
            forkid: self.forkid,
            blockhash: self.blockhash,
            total_difficulty: self.total_difficulty.unwrap_or(U256::ZERO),
        }
    }

    /// Consume this `UnifiedStatus` and produce the [`StatusEth69`] message used by `eth/69`.
    pub fn into_eth69(self) -> StatusEth69 {
        StatusEth69 {
            version: self.version,
            chain: self.chain,
            genesis: self.genesis,
            forkid: self.forkid,
            earliest: self.earliest_block.unwrap_or(0),
            latest: self.latest_block.unwrap_or(0),
            blockhash: self.blockhash,
        }
    }

    /// Consume this `UnifiedStatus` and produce the [`StatusEth70`] message used by `eth/70`.
    pub fn into_eth70(self) -> StatusEth70 {
        let block_range = self
            .block_range
            .or_else(|| {
                self.earliest_block
                    .zip(self.latest_block)
                    .map(|(start_block, end_block)| BlockRange { start_block, end_block })
            })
            .unwrap_or(BlockRange { start_block: 0, end_block: 0 });

        StatusEth70 {
            version: self.version,
            chain: self.chain,
            genesis: self.genesis,
            forkid: self.forkid,
            block_range,
            blockhash: self.blockhash,
        }
    }

    /// Convert this `UnifiedStatus` into the appropriate `StatusMessage` variant based on version.
    pub fn into_message(self) -> StatusMessage {
        match self.version {
            v if v >= EthVersion::Eth70 => StatusMessage::Eth70(self.into_eth70()),
            v if v >= EthVersion::Eth69 => StatusMessage::Eth69(self.into_eth69()),
            _ => StatusMessage::Legacy(self.into_legacy()),
        }
    }

    /// Build a `UnifiedStatus` from a received `StatusMessage`.
    pub const fn from_message(msg: StatusMessage) -> Self {
        match msg {
            StatusMessage::Legacy(s) => Self {
                version: s.version,
                chain: s.chain,
                genesis: s.genesis,
                forkid: s.forkid,
                blockhash: s.blockhash,
                total_difficulty: Some(s.total_difficulty),
                earliest_block: None,
                latest_block: None,
                block_range: None,
            },
            StatusMessage::Eth69(e) => Self {
                version: e.version,
                chain: e.chain,
                genesis: e.genesis,
                forkid: e.forkid,
                blockhash: e.blockhash,
                total_difficulty: None,
                earliest_block: Some(e.earliest),
                latest_block: Some(e.latest),
                block_range: Some(BlockRange { start_block: e.earliest, end_block: e.latest }),
            },
            StatusMessage::Eth70(e) => Self {
                version: e.version,
                chain: e.chain,
                genesis: e.genesis,
                forkid: e.forkid,
                blockhash: e.blockhash,
                total_difficulty: None,
                earliest_block: Some(e.block_range.start_block),
                latest_block: Some(e.block_range.end_block),
                block_range: Some(e.block_range),
            },
        }
    }
}

/// Builder type for constructing a [`UnifiedStatus`] message.
#[derive(Debug, Default)]
pub struct StatusBuilder {
    status: UnifiedStatus,
}

impl StatusBuilder {
    /// Consumes the builder and returns the constructed [`UnifiedStatus`].
    pub const fn build(mut self) -> UnifiedStatus {
        // If earliest/latest were set (eth/69 path), also populate block_range so eth/70 can reuse
        // the same values without requiring a separate setter. eth/69 conversion ignores
        // block_range, so this is safe for all versions.
        if let (Some(start), Some(end)) = (self.status.earliest_block, self.status.latest_block) {
            self.status.block_range = Some(BlockRange { start_block: start, end_block: end });
        }
        self.status
    }

    /// Sets the eth protocol version (e.g., eth/66, eth/70).
    pub const fn version(mut self, version: EthVersion) -> Self {
        self.status.version = version;
        self
    }

    /// Sets the chain ID
    pub const fn chain(mut self, chain: Chain) -> Self {
        self.status.chain = chain;
        self
    }

    /// Sets the genesis block hash of the chain.
    pub const fn genesis(mut self, genesis: B256) -> Self {
        self.status.genesis = genesis;
        self
    }

    /// Sets the fork ID, used for fork compatibility checks.
    pub const fn forkid(mut self, forkid: ForkId) -> Self {
        self.status.forkid = forkid;
        self
    }

    /// Sets the block hash of the current head.
    pub const fn blockhash(mut self, blockhash: B256) -> Self {
        self.status.blockhash = blockhash;
        self
    }

    /// Sets the total difficulty, if relevant (Some for eth/66–68).
    pub const fn total_difficulty(mut self, td: Option<U256>) -> Self {
        self.status.total_difficulty = td;
        self
    }

    /// Sets the earliest available block, if known (Some for eth/69).
    pub const fn earliest_block(mut self, earliest: Option<u64>) -> Self {
        self.status.earliest_block = earliest;
        self
    }

    /// Sets the latest known block, if known (Some for eth/69).
    pub const fn latest_block(mut self, latest: Option<u64>) -> Self {
        self.status.latest_block = latest;
        self
    }

    /// Sets the block range, if known (eth/70).
    pub const fn block_range(mut self, block_range: Option<BlockRange>) -> Self {
        if let Some(block_range) = block_range {
            self.status.block_range = Some(block_range);
            self.status.earliest_block = Some(block_range.start_block);
            self.status.latest_block = Some(block_range.end_block);
        } else {
            self.status.block_range = None;
        }
        self
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

    /// The genesis hash of the peer's chain.
    pub genesis: B256,

    /// The fork identifier, a [CRC32
    /// checksum](https://en.wikipedia.org/wiki/Cyclic_redundancy_check#CRC-32_algorithm) for
    /// identifying the peer's fork as defined by
    /// [EIP-2124](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-2124.md).
    /// This was added in [`eth/64`](https://eips.ethereum.org/EIPS/eip-2364)
    pub forkid: ForkId,

    /// Earliest block number this node can serve
    pub earliest: u64,

    /// Latest block number this node has (current head)
    pub latest: u64,

    /// Hash of the latest block this node has (current head)
    pub blockhash: B256,
}

impl Display for StatusEth69 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let hexed_blockhash = hex::encode(self.blockhash);
        let hexed_genesis = hex::encode(self.genesis);
        write!(
            f,
            "StatusEth69 {{ version: {}, chain: {}, genesis: {}, forkid: {:X?}, earliest: {}, latest: {}, blockhash: {} }}",
            self.version,
            self.chain,
            hexed_genesis,
            self.forkid,
            self.earliest,
            self.latest,
            hexed_blockhash,
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
                "StatusEth69 {{\n\tversion: {:?},\n\tchain: {:?},\n\tgenesis: {},\n\tforkid: {:X?},\n\tearliest: {},\n\tlatest: {},\n\tblockhash: {}\n}}",
                self.version, self.chain, hexed_genesis, self.forkid, self.earliest, self.latest, hexed_blockhash
            )
        } else {
            write!(
                f,
                "StatusEth69 {{ version: {:?}, chain: {:?}, genesis: {}, forkid: {:X?}, earliest: {}, latest: {}, blockhash: {} }}",
                self.version, self.chain, hexed_genesis, self.forkid, self.earliest, self.latest, hexed_blockhash
            )
        }
    }
}

/// Status message for `eth/70` including the block range as per EIP-7975.
#[derive(Copy, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct StatusEth70 {
    /// The current protocol version (eth/70).
    pub version: EthVersion,

    /// The chain id, as introduced in EIP-155.
    pub chain: Chain,

    /// The genesis hash of the peer's chain.
    pub genesis: B256,

    /// Fork identifier as defined by EIP-2124.
    pub forkid: ForkId,

    /// The block range the peer can serve.
    pub block_range: BlockRange,

    /// Hash of the latest block this node has (current head).
    pub blockhash: B256,
}

impl Display for StatusEth70 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let hexed_blockhash = hex::encode(self.blockhash);
        let hexed_genesis = hex::encode(self.genesis);
        write!(
            f,
            "StatusEth70 {{ version: {}, chain: {}, genesis: {}, forkid: {:X?}, block_range: [{}, {}], blockhash: {} }}",
            self.version,
            self.chain,
            hexed_genesis,
            self.forkid,
            self.block_range.start_block,
            self.block_range.end_block,
            hexed_blockhash,
        )
    }
}

impl Debug for StatusEth70 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let hexed_blockhash = hex::encode(self.blockhash);
        let hexed_genesis = hex::encode(self.genesis);
        if f.alternate() {
            write!(
                f,
                "StatusEth70 {{\n\tversion: {:?},\n\tchain: {:?},\n\tgenesis: {},\n\tforkid: {:X?},\n\tblock_range: [{}, {}],\n\tblockhash: {}\n}}",
                self.version,
                self.chain,
                hexed_genesis,
                self.forkid,
                self.block_range.start_block,
                self.block_range.end_block,
                hexed_blockhash
            )
        } else {
            write!(
                f,
                "StatusEth70 {{ version: {:?}, chain: {:?}, genesis: {}, forkid: {:X?}, block_range: [{}, {}], blockhash: {} }}",
                self.version,
                self.chain,
                hexed_genesis,
                self.forkid,
                self.block_range.start_block,
                self.block_range.end_block,
                hexed_blockhash
            )
        }
    }
}

/// `StatusMessage` can store either the Legacy version (with TD), the
/// eth/69 version (omits TD), or the eth/70 version (with block range).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusMessage {
    /// The legacy status (`eth/66` through `eth/68`) with `total_difficulty`.
    Legacy(Status),
    /// The new `eth/69` status with no `total_difficulty`.
    Eth69(StatusEth69),
    /// The `eth/70` status which includes the `block_range`.
    Eth70(StatusEth70),
}

impl StatusMessage {
    /// Returns the genesis hash from the status message.
    pub const fn genesis(&self) -> B256 {
        match self {
            Self::Legacy(legacy_status) => legacy_status.genesis,
            Self::Eth69(status_69) => status_69.genesis,
            Self::Eth70(status_70) => status_70.genesis,
        }
    }

    /// Returns the protocol version.
    pub const fn version(&self) -> EthVersion {
        match self {
            Self::Legacy(legacy_status) => legacy_status.version,
            Self::Eth69(status_69) => status_69.version,
            Self::Eth70(status_70) => status_70.version,
        }
    }

    /// Returns the chain identifier.
    pub const fn chain(&self) -> &Chain {
        match self {
            Self::Legacy(legacy_status) => &legacy_status.chain,
            Self::Eth69(status_69) => &status_69.chain,
            Self::Eth70(status_70) => &status_70.chain,
        }
    }

    /// Returns the fork identifier.
    pub const fn forkid(&self) -> ForkId {
        match self {
            Self::Legacy(legacy_status) => legacy_status.forkid,
            Self::Eth69(status_69) => status_69.forkid,
            Self::Eth70(status_70) => status_70.forkid,
        }
    }

    /// Returns the latest block hash
    pub const fn blockhash(&self) -> B256 {
        match self {
            Self::Legacy(legacy_status) => legacy_status.blockhash,
            Self::Eth69(status_69) => status_69.blockhash,
            Self::Eth70(status_70) => status_70.blockhash,
        }
    }
}

impl Encodable for StatusMessage {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(s) => s.encode(out),
            Self::Eth69(s) => s.encode(out),
            Self::Eth70(s) => s.encode(out),
        }
    }

    fn length(&self) -> usize {
        match self {
            Self::Legacy(s) => s.length(),
            Self::Eth69(s) => s.length(),
            Self::Eth70(s) => s.length(),
        }
    }
}

impl Display for StatusMessage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Legacy(s) => Display::fmt(s, f),
            Self::Eth69(s69) => Display::fmt(s69, f),
            Self::Eth70(s70) => Display::fmt(s70, f),
        }
    }
}
#[cfg(test)]
mod tests {
    use crate::{BlockRange, EthVersion, Status, StatusEth69, StatusMessage, UnifiedStatus};
    use alloy_consensus::constants::MAINNET_GENESIS_HASH;
    use alloy_genesis::Genesis;
    use alloy_hardforks::{EthereumHardfork, ForkHash, ForkId, Head};
    use alloy_primitives::{b256, hex, B256, U256};
    use alloy_rlp::{Decodable, Encodable};
    use rand::Rng;
    use reth_chainspec::{Chain, ChainSpec, ForkCondition, NamedChain};
    use std::str::FromStr;

    #[test]
    fn encode_eth_status_message() {
        let expected = hex!(
            "f85643018a07aac59dabcdd74bc567a0feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13da0d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3c684b715077d80"
        );
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
        let data = hex!(
            "f85643018a07aac59dabcdd74bc567a0feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13da0d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3c684b715077d80"
        );
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
    fn roundtrip_eth69() {
        let unified_status = UnifiedStatus::builder()
            .version(EthVersion::Eth69)
            .chain(Chain::mainnet())
            .genesis(MAINNET_GENESIS_HASH)
            .forkid(ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 })
            .blockhash(b256!("0xfeb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d"))
            .earliest_block(Some(1))
            .latest_block(Some(2))
            .total_difficulty(None)
            .build();

        let status_message = unified_status.into_message();
        let roundtripped_unified_status = UnifiedStatus::from_message(status_message);

        assert_eq!(unified_status, roundtripped_unified_status);
    }

    #[test]
    fn roundtrip_legacy() {
        let unified_status = UnifiedStatus::builder()
            .version(EthVersion::Eth68)
            .chain(Chain::sepolia())
            .genesis(MAINNET_GENESIS_HASH)
            .forkid(ForkId { hash: ForkHash([0xaa, 0xbb, 0xcc, 0xdd]), next: 0 })
            .blockhash(b256!("0xfeb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d"))
            .total_difficulty(Some(U256::from(42u64)))
            .earliest_block(None)
            .latest_block(None)
            .build();

        let status_message = unified_status.into_message();
        let roundtripped_unified_status = UnifiedStatus::from_message(status_message);
        assert_eq!(unified_status, roundtripped_unified_status);
    }

    #[test]
    fn roundtrip_eth70() {
        let unified_status = UnifiedStatus::builder()
            .version(EthVersion::Eth70)
            .chain(Chain::mainnet())
            .genesis(MAINNET_GENESIS_HASH)
            .forkid(ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 })
            .blockhash(b256!("0xfeb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d"))
            .total_difficulty(None)
            .block_range(Some(BlockRange { start_block: 1, end_block: 2 }))
            .build();

        let status_message = unified_status.into_message();
        let roundtripped_unified_status = UnifiedStatus::from_message(status_message);
        assert_eq!(unified_status, roundtripped_unified_status);

        if let StatusMessage::Eth70(status70) = status_message {
            assert_eq!(status70.block_range.start_block, 1);
            assert_eq!(status70.block_range.end_block, 2);
        } else {
            panic!("expected StatusMessage::Eth70 variant");
        }
    }

    #[test]
    fn encode_eth69_status_message() {
        let expected = hex!("f8544501a0d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3c684b715077d8083ed14f2840112a880a0feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d");
        let status = StatusEth69 {
            version: EthVersion::Eth69,
            chain: Chain::from_named(NamedChain::Mainnet),

            genesis: MAINNET_GENESIS_HASH,
            forkid: ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 },
            earliest: 15_537_394,
            latest: 18_000_000,
            blockhash: B256::from_str(
                "feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d",
            )
            .unwrap(),
        };

        let mut rlp_status = vec![];
        status.encode(&mut rlp_status);
        assert_eq!(rlp_status, expected);

        let status = UnifiedStatus::builder()
            .version(EthVersion::Eth69)
            .chain(Chain::from_named(NamedChain::Mainnet))
            .genesis(MAINNET_GENESIS_HASH)
            .forkid(ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 })
            .blockhash(b256!("0xfeb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d"))
            .earliest_block(Some(15_537_394))
            .latest_block(Some(18_000_000))
            .build()
            .into_message();

        let mut rlp_status = vec![];
        status.encode(&mut rlp_status);
        assert_eq!(rlp_status, expected);
    }

    #[test]
    fn decode_eth69_status_message() {
        let data =  hex!("f8544501a0d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3c684b715077d8083ed14f2840112a880a0feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d");
        let expected = StatusEth69 {
            version: EthVersion::Eth69,
            chain: Chain::from_named(NamedChain::Mainnet),
            genesis: MAINNET_GENESIS_HASH,
            forkid: ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 },
            earliest: 15_537_394,
            latest: 18_000_000,
            blockhash: B256::from_str(
                "feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d",
            )
            .unwrap(),
        };
        let status = StatusEth69::decode(&mut &data[..]).unwrap();
        assert_eq!(status, expected);

        let expected_message = UnifiedStatus::builder()
            .version(EthVersion::Eth69)
            .chain(Chain::from_named(NamedChain::Mainnet))
            .genesis(MAINNET_GENESIS_HASH)
            .forkid(ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 })
            .earliest_block(Some(15_537_394))
            .latest_block(Some(18_000_000))
            .blockhash(b256!("0xfeb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d"))
            .build()
            .into_message();

        let expected_status = if let StatusMessage::Eth69(status69) = expected_message {
            status69
        } else {
            panic!("expected StatusMessage::Eth69 variant");
        };

        assert_eq!(status, expected_status);
    }

    #[test]
    fn encode_network_status_message() {
        let expected = hex!(
            "f850423884024190faa0f8514c4680ef27700751b08f37645309ce65a449616a3ea966bf39dd935bb27ba00d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5bc6845d43d2fd80"
        );
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
        let data = hex!(
            "f850423884024190faa0f8514c4680ef27700751b08f37645309ce65a449616a3ea966bf39dd935bb27ba00d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5bc6845d43d2fd80"
        );
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
        let data = hex!(
            "f86142820834936d68fcffffffffffffffffffffffffdeab81b8a0523e8163a6d620a4cc152c547a05f28a03fec91a2a615194cb86df9731372c0ca06499dccdc7c7def3ebb1ce4c6ee27ec6bd02aee570625ca391919faf77ef27bdc6841a67ccd880"
        );
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

        let status = UnifiedStatus::spec_builder(&spec, &head);

        assert_eq!(status.chain, Chain::from_id(1337));
        assert_eq!(status.forkid, forkid);
        assert_eq!(status.total_difficulty.unwrap(), total_difficulty);
        assert_eq!(status.blockhash, head_hash);
        assert_eq!(status.genesis, genesis_hash);
    }
}
