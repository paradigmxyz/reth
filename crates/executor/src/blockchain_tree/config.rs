//! Blockchain tree configuration

/// The configuration for the blockchain tree.
#[derive(Clone, Debug)]
pub struct BlockchainTreeConfig {
    /// Finalization windows. Number of blocks that can be reorged
    max_reorg_depth: u64,
    /// Number of block after finalized block that we are storing. It should be more then
    /// finalization window
    max_blocks_in_chain: u64,
    /// For EVM's "BLOCKHASH" opcode we require last 256 block hashes. So we need to specify
    /// at least `additional_canonical_block_hashes`+`max_reorg_depth`, for eth that would be
    /// 256+64.
    num_of_additional_canonical_block_hashes: u64,
}

impl Default for BlockchainTreeConfig {
    fn default() -> Self {
        // The defaults for Ethereum mainnet
        Self {
            // Gasper allows reorgs of any length from 1 to 64.
            max_reorg_depth: 64,
            // This default is just an assumption. Has to be greater than the `max_reorg_depth`.
            max_blocks_in_chain: 65,
            // EVM requires that last 256 block hashes are available.
            num_of_additional_canonical_block_hashes: 256,
        }
    }
}

impl BlockchainTreeConfig {
    /// Create tree configuration.
    pub fn new(
        max_reorg_depth: u64,
        max_blocks_in_chain: u64,
        num_of_additional_canonical_block_hashes: u64,
    ) -> Self {
        if max_reorg_depth > max_blocks_in_chain {
            panic!("Side chain size should be more then finalization window");
        }
        Self { max_blocks_in_chain, max_reorg_depth, num_of_additional_canonical_block_hashes }
    }

    /// Return the maximum reorg depth.
    pub fn max_reorg_depth(&self) -> u64 {
        self.max_reorg_depth
    }

    /// Return the maximum number of blocks in one chain.
    pub fn max_blocks_in_chain(&self) -> u64 {
        self.max_blocks_in_chain
    }

    /// Return number of additional canonical block hashes that we need to retain
    /// in order to have enough information for EVM execution.
    pub fn num_of_additional_canonical_block_hashes(&self) -> u64 {
        self.num_of_additional_canonical_block_hashes
    }
}
