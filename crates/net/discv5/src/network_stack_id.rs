//! Keys of ENR [`ForkId`](reth_ethereum_forks::ForkId) kv-pair. Identifies which network stack a
//! node belongs to.

use reth_chainspec::EthChainSpec;

/// Identifies which Ethereum network stack a node belongs to, on the discovery network.
#[derive(Debug)]
pub struct NetworkStackId;

impl NetworkStackId {
    /// ENR fork ID kv-pair key, for an Ethereum L1 EL node.
    pub const ETH: &'static [u8] = b"eth";

    /// ENR fork ID kv-pair key, for an Ethereum L1 CL node.
    pub const ETH2: &'static [u8] = b"eth2";

    /// ENR fork ID kv-pair key, for an Optimism EL node.
    pub const OPEL: &'static [u8] = b"opel";

    /// ENR fork ID kv-pair key, for an Optimism CL node.
    pub const OPSTACK: &'static [u8] = b"opstack";

    #[allow(clippy::missing_const_for_fn)]
    /// Returns the [`NetworkStackId`] that matches the given chain spec.
    pub fn id(chain: impl EthChainSpec) -> Option<&'static [u8]> {
        if chain.chain().is_optimism() {
            return Some(Self::OPEL)
        } else if chain.chain().is_ethereum() {
            return Some(Self::ETH)
        }

        None
    }
}
