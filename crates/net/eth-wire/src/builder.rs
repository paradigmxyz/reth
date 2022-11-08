//! Builder structs for [`Status`](crate::types::Status) and [`Hello`](crate::types::Hello) messages.

use reth_primitives::{Chain, U256, H256, MAINNET_GENESIS};
use crate::{Status, forkid::ForkId, EthVersion, hardfork::Hardfork};

/// Builder for [`Status`](crate::types::Status) messages.
pub struct StatusBuilder {
    /// The current protocol version. For example, peers running `eth/66` would have a version of
    /// 66.
    pub version: Option<u8>,

    /// The chain id, as introduced in
    /// [EIP155](https://eips.ethereum.org/EIPS/eip-155#list-of-chain-ids).
    pub chain: Option<Chain>,

    /// Total difficulty of the best chain.
    pub total_difficulty: Option<U256>,

    /// The highest difficulty block hash the peer has seen
    pub blockhash: Option<H256>,

    /// The genesis hash of the peer's chain.
    pub genesis: Option<H256>,

    /// The fork identifier as defined by
    /// [EIP-2124](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-2124.md).
    pub forkid: Option<ForkId>,
}

impl StatusBuilder {
    /// Consumes the type and creates the actual [`Status`](crate::types::Status) message.
    pub fn build(self) -> Status {
        Status {
            version: self.version.unwrap_or(EthVersion::Eth67 as u8),
            chain: self.chain.unwrap_or(Chain::Named(ethers_core::types::Chain::Mainnet)),
            total_difficulty: self.total_difficulty.unwrap_or(U256::zero()),
            blockhash: self.blockhash.unwrap_or(MAINNET_GENESIS),
            genesis: self.genesis.unwrap_or(MAINNET_GENESIS),
            forkid: self.forkid.unwrap_or_else(|| Hardfork::Homestead.fork_id()),
        }
    }
}
