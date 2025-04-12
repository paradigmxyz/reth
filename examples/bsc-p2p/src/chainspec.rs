//! Chain specification for BSC, credits to: <https://github.com/bnb-chain/reth/blob/main/crates/bsc/chainspec/src/bsc.rs>

use alloy_primitives::{BlockHash, U256};
use reth_chainspec::{
    hardfork, make_genesis_header, BaseFeeParams, BaseFeeParamsKind, Chain, ChainHardforks,
    ChainSpec, EthereumHardfork, ForkCondition, Hardfork, Head, NamedChain,
};
use reth_network_peers::NodeRecord;
use reth_primitives::SealedHeader;
use std::{str::FromStr, sync::Arc};

hardfork!(
    /// The name of a bsc hardfork.
    ///
    /// When building a list of hardforks for a chain, it's still expected to mix with [`EthereumHardfork`].
    BscHardfork {
        /// BSC `Ramanujan` hardfork
        Ramanujan,
        /// BSC `Niels` hardfork
        Niels,
        /// BSC `MirrorSync` hardfork
        MirrorSync,
        /// BSC `Bruno` hardfork
        Bruno,
        /// BSC `Euler` hardfork
        Euler,
        /// BSC `Nano` hardfork
        Nano,
        /// BSC `Moran` hardfork
        Moran,
        /// BSC `Gibbs` hardfork
        Gibbs,
        /// BSC `Planck` hardfork
        Planck,
        /// BSC `Luban` hardfork
        Luban,
        /// BSC `Plato` hardfork
        Plato,
        /// BSC `Hertz` hardfork
        Hertz,
        /// BSC `HertzFix` hardfork
        HertzFix,
        /// BSC `Kepler` hardfork
        Kepler,
        /// BSC `Feynman` hardfork
        Feynman,
        /// BSC `FeynmanFix` hardfork
        FeynmanFix,
        /// BSC `Haber` hardfork
        Haber,
        /// BSC `HaberFix` hardfork
        HaberFix,
        /// BSC `Bohr` hardfork
        Bohr,
        /// BSC `Pascal` hardfork
        Pascal,
        /// BSC `Prague` hardfork
        Prague,
    }
);

impl BscHardfork {
    /// Bsc mainnet list of hardforks.
    fn bsc_mainnet() -> ChainHardforks {
        ChainHardforks::new(vec![
            (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
            (EthereumHardfork::MuirGlacier.boxed(), ForkCondition::Block(0)),
            (Self::Ramanujan.boxed(), ForkCondition::Block(0)),
            (Self::Niels.boxed(), ForkCondition::Block(0)),
            (Self::MirrorSync.boxed(), ForkCondition::Block(5184000)),
            (Self::Bruno.boxed(), ForkCondition::Block(13082000)),
            (Self::Euler.boxed(), ForkCondition::Block(18907621)),
            (Self::Nano.boxed(), ForkCondition::Block(21962149)),
            (Self::Moran.boxed(), ForkCondition::Block(22107423)),
            (Self::Gibbs.boxed(), ForkCondition::Block(23846001)),
            (Self::Planck.boxed(), ForkCondition::Block(27281024)),
            (Self::Luban.boxed(), ForkCondition::Block(29020050)),
            (Self::Plato.boxed(), ForkCondition::Block(30720096)),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(31302048)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(31302048)),
            (Self::Hertz.boxed(), ForkCondition::Block(31302048)),
            (Self::HertzFix.boxed(), ForkCondition::Block(34140700)),
            (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(1705996800)),
            (Self::Kepler.boxed(), ForkCondition::Timestamp(1705996800)),
            (Self::Feynman.boxed(), ForkCondition::Timestamp(1713419340)),
            (Self::FeynmanFix.boxed(), ForkCondition::Timestamp(1713419340)),
            (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(1718863500)),
            (Self::Haber.boxed(), ForkCondition::Timestamp(1718863500)),
            (Self::HaberFix.boxed(), ForkCondition::Timestamp(1727316120)),
            (Self::Bohr.boxed(), ForkCondition::Timestamp(1727317200)),
            (Self::Pascal.boxed(), ForkCondition::Timestamp(1742436600)),
            (Self::Prague.boxed(), ForkCondition::Timestamp(1742436600)),
        ])
    }
}

pub fn bsc_chain_spec() -> Arc<ChainSpec> {
    let genesis = serde_json::from_str(include_str!("genesis.json"))
        .expect("Can't deserialize BSC Mainnet genesis json");
    let hardforks = BscHardfork::bsc_mainnet();
    ChainSpec {
        chain: Chain::from_named(NamedChain::BinanceSmartChain),
        genesis: serde_json::from_str(include_str!("genesis.json"))
            .expect("Can't deserialize BSC Mainnet genesis json"),
        paris_block_and_final_difficulty: Some((0, U256::from(0))),
        hardforks: BscHardfork::bsc_mainnet(),
        deposit_contract: None,
        base_fee_params: BaseFeeParamsKind::Constant(BaseFeeParams::new(1, 1)),
        prune_delete_limit: 3500,
        genesis_header: SealedHeader::new(
            make_genesis_header(&genesis, &hardforks),
            BlockHash::from_str(
                "0x0d21840abff46b96c84b2ac9e10e4f5cdaeb5693cb665db62a2f3b02d2d57b5b",
            )
            .unwrap(),
        ),
        ..Default::default()
    }
    .into()
}

/// BSC mainnet bootnodes <https://github.com/bnb-chain/bsc/blob/master/params/bootnodes.go#L23>
static BOOTNODES : [&str; 6] = [
    "enode://433c8bfdf53a3e2268ccb1b829e47f629793291cbddf0c76ae626da802f90532251fc558e2e0d10d6725e759088439bf1cd4714716b03a259a35d4b2e4acfa7f@52.69.102.73:30311",
	"enode://571bee8fb902a625942f10a770ccf727ae2ba1bab2a2b64e121594a99c9437317f6166a395670a00b7d93647eacafe598b6bbcef15b40b6d1a10243865a3e80f@35.73.84.120:30311",
	"enode://fac42fb0ba082b7d1eebded216db42161163d42e4f52c9e47716946d64468a62da4ba0b1cac0df5e8bf1e5284861d757339751c33d51dfef318be5168803d0b5@18.203.152.54:30311",
	"enode://3063d1c9e1b824cfbb7c7b6abafa34faec6bb4e7e06941d218d760acdd7963b274278c5c3e63914bd6d1b58504c59ec5522c56f883baceb8538674b92da48a96@34.250.32.100:30311",
	"enode://ad78c64a4ade83692488aa42e4c94084516e555d3f340d9802c2bf106a3df8868bc46eae083d2de4018f40e8d9a9952c32a0943cd68855a9bc9fd07aac982a6d@34.204.214.24:30311",
	"enode://5db798deb67df75d073f8e2953dad283148133acb520625ea804c9c4ad09a35f13592a762d8f89056248f3889f6dcc33490c145774ea4ff2966982294909b37a@107.20.191.97:30311",

];

pub fn boot_nodes() -> Vec<NodeRecord> {
    BOOTNODES[..].iter().map(|s| s.parse().unwrap()).collect()
}

pub fn head() -> Head {
    Head { number: 40_000_000, timestamp: 1742436600, ..Default::default() }
}
#[cfg(test)]
mod tests {
    use crate::chainspec::{bsc_chain_spec, head};
    use alloy_primitives::hex;
    use reth_chainspec::{ForkHash, ForkId};

    #[test]
    fn can_create_forkid() {
        let b = hex::decode("ce18f5d3").unwrap();
        let expected = [b[0], b[1], b[2], b[3]];
        let expected_f_id = ForkId { hash: ForkHash(expected), next: 0 };

        let fork_id = bsc_chain_spec().fork_id(&head());
        assert_eq!(fork_id, expected_f_id);
    }
}
