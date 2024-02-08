use crate::{EthVersion, StatusBuilder};
use alloy_chains::{Chain, NamedChain};
use alloy_rlp::{RlpDecodable, RlpEncodable};
use reth_codecs::derive_arbitrary;
use reth_primitives::{hex, ChainSpec, ForkId, Genesis, Hardfork, Head, B256, MAINNET, U256};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display};

/// The status message is used in the eth protocol handshake to ensure that peers are on the same
/// network and are following the same fork.
///
/// When performing a handshake, the total difficulty is not guaranteed to correspond to the block
/// hash. This information should be treated as untrusted.
#[derive_arbitrary(rlp)]
#[derive(Copy, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Status {
    /// The current protocol version. For example, peers running `eth/66` would have a version of
    /// 66.
    pub version: u8,

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

impl From<Genesis> for Status {
    fn from(genesis: Genesis) -> Status {
        let chain = genesis.config.chain_id;
        let total_difficulty = genesis.difficulty;
        let chainspec = ChainSpec::from(genesis);

        Status {
            version: EthVersion::Eth68 as u8,
            chain: Chain::from_id(chain),
            total_difficulty,
            blockhash: chainspec.genesis_hash(),
            genesis: chainspec.genesis_hash(),
            forkid: chainspec.fork_id(&Head::default()),
        }
    }
}

impl Status {
    /// Helper for returning a builder for the status message.
    pub fn builder() -> StatusBuilder {
        Default::default()
    }

    /// Sets the [EthVersion] for the status.
    pub fn set_eth_version(&mut self, version: EthVersion) {
        self.version = version as u8;
    }

    /// Create a [`StatusBuilder`] from the given [`ChainSpec`] and head block.
    ///
    /// Sets the `chain` and `genesis`, `blockhash`, and `forkid` fields based on the [`ChainSpec`]
    /// and head.
    pub fn spec_builder(spec: &ChainSpec, head: &Head) -> StatusBuilder {
        Self::builder()
            .chain(spec.chain)
            .genesis(spec.genesis_hash())
            .blockhash(head.hash)
            .total_difficulty(head.total_difficulty)
            .forkid(spec.fork_id(head))
    }
}

impl Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        Status {
            version: EthVersion::Eth68 as u8,
            chain: Chain::from_named(NamedChain::Mainnet),
            total_difficulty: U256::from(17_179_869_184u64),
            blockhash: mainnet_genesis,
            genesis: mainnet_genesis,
            forkid: MAINNET
                .hardfork_fork_id(Hardfork::Frontier)
                .expect("The Frontier hardfork should always exist"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::types::{EthVersion, Status};
    use alloy_chains::{Chain, NamedChain};
    use alloy_rlp::{Decodable, Encodable};
    use rand::Rng;
    use reth_primitives::{
        hex, ChainSpec, ForkCondition, ForkHash, ForkId, Genesis, Hardfork, Head, B256, U256,
    };
    use std::str::FromStr;

    #[test]
    fn encode_eth_status_message() {
        let expected = hex!("f85643018a07aac59dabcdd74bc567a0feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13da0d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3c684b715077d80");
        let status = Status {
            version: EthVersion::Eth67 as u8,
            chain: Chain::from_named(NamedChain::Mainnet),
            total_difficulty: U256::from(36206751599115524359527u128),
            blockhash: B256::from_str(
                "feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d",
            )
            .unwrap(),
            genesis: B256::from_str(
                "d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3",
            )
            .unwrap(),
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
            version: EthVersion::Eth67 as u8,
            chain: Chain::from_named(NamedChain::Mainnet),
            total_difficulty: U256::from(36206751599115524359527u128),
            blockhash: B256::from_str(
                "feb27336ca7923f8fab3bd617fcb6e75841538f71c1bcfc267d7838489d9e13d",
            )
            .unwrap(),
            genesis: B256::from_str(
                "d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3",
            )
            .unwrap(),
            forkid: ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 0 },
        };
        let status = Status::decode(&mut &data[..]).unwrap();
        assert_eq!(status, expected);
    }

    #[test]
    fn encode_network_status_message() {
        let expected = hex!("f850423884024190faa0f8514c4680ef27700751b08f37645309ce65a449616a3ea966bf39dd935bb27ba00d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5bc6845d43d2fd80");
        let status = Status {
            version: EthVersion::Eth66 as u8,
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
            version: EthVersion::Eth66 as u8,
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
            version: EthVersion::Eth66 as u8,
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
        let mut rng = rand::thread_rng();
        let head_hash = rng.gen();
        let total_difficulty = U256::from(rng.gen::<u64>());

        // create a genesis that has a random part, so we can check that the hash is preserved
        let genesis = Genesis { nonce: rng.gen::<u64>(), ..Default::default() };

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
            (Hardfork::Tangerine, ForkCondition::Block(1)),
            (Hardfork::SpuriousDragon, ForkCondition::Block(2)),
            (Hardfork::Byzantium, ForkCondition::Block(3)),
            (Hardfork::MuirGlacier, ForkCondition::Block(5)),
            (Hardfork::London, ForkCondition::Block(8)),
            (Hardfork::Shanghai, ForkCondition::Timestamp(13)),
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
                ForkCondition::Block(n) => n,
                ForkCondition::Timestamp(n) => n,
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
